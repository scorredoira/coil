//! The rows a tab lists, and what the tabs that list files on disk or in git share: which
//! directories are open, how a directory or a set of changed paths becomes rows, and how
//! a row of a file or a directory is drawn.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::{Editor, Theme};
use tui::buffer::Buffer as Surface;

use super::git::{Change, ChangedFile};
use super::list::List;
use crate::ui::directory_entries;

/// One line of a tab.
pub enum Row {
    /// A file or a directory, on disk or as a commit touched it.
    Entry(Entry),
    /// A commit, in the history or standing above its own files.
    Commit(CommitRow),
}

pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub change: Option<Change>,
}

pub struct CommitRow {
    /// Its place in the tab's history, for the tab to find the commit by.
    pub index: usize,
    pub short: String,
    pub subject: String,
    pub time: i64,
    /// The opened commit's own row, over its files, which alone names its hash.
    pub head: bool,
}

impl Row {
    pub fn entry(&self) -> Option<&Entry> {
        match self {
            Row::Entry(entry) => Some(entry),
            Row::Commit(_) => None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.entry().map(|entry| entry.path.as_path())
    }

    /// The path of a directory row; a file's or a commit's is none.
    pub fn dir(&self) -> Option<&Path> {
        self.entry()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.as_path())
    }

    pub fn depth(&self) -> usize {
        self.entry().map_or(0, |entry| entry.depth)
    }
}

/// Which directories of a tab are open. A tree on disk opens closed and remembers what was
/// opened; one of changes opens open and remembers what was closed.
pub struct Folds {
    open_by_default: bool,
    toggled: HashSet<PathBuf>,
}

impl Folds {
    pub fn closed() -> Self {
        Self {
            open_by_default: false,
            toggled: HashSet::new(),
        }
    }

    pub fn opened() -> Self {
        Self {
            open_by_default: true,
            toggled: HashSet::new(),
        }
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.open_by_default != self.toggled.contains(path)
    }

    pub fn set(&mut self, path: PathBuf, open: bool) {
        if open == self.open_by_default {
            self.toggled.remove(&path);
        } else {
            self.toggled.insert(path);
        }
    }

    pub fn close_all(&mut self, dirs: impl Iterator<Item = PathBuf>) {
        if self.open_by_default {
            self.toggled.extend(dirs);
        } else {
            self.toggled.clear();
        }
    }

    /// Opens every directory between `root` and `path`, so `path` can be seen.
    pub fn open_ancestors(&mut self, root: &Path, path: &Path) {
        let mut dir = path.parent();
        while let Some(current) = dir {
            if current == root {
                break;
            }
            self.set(current.to_path_buf(), true);
            dir = current.parent();
        }
    }
}

/// Lists a directory on disk, recursing into the open ones; a directory that cannot be
/// read is said and left empty.
pub fn list_disk(
    dir: &Path,
    depth: usize,
    editor: &mut Editor,
    folds: &Folds,
    rows: &mut Vec<Row>,
) {
    let entries = match directory_entries(dir, editor, false) {
        Ok(entries) => entries,
        Err(err) => {
            editor.set_error(format!("{}: {}", dir.display(), err));
            return;
        }
    };
    for (path, is_dir) in entries {
        if path.ends_with("..") {
            continue;
        }
        let name = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let open = is_dir && folds.is_open(&path);
        rows.push(Row::Entry(Entry {
            path: path.clone(),
            name,
            is_dir,
            depth,
            change: None,
        }));
        if open {
            list_disk(&path, depth + 1, editor, folds, rows);
        }
    }
}

/// Lists changed files under the directories that hold them, below `root`: a chain of
/// directories with nothing else in them is one row, and a path outside the root is left
/// out.
pub fn list_changed(root: &Path, files: &[ChangedFile], folds: &Folds, rows: &mut Vec<Row>) {
    let mut top = ChangeDir::default();
    for file in files {
        let Ok(relative) = file.path.strip_prefix(root) else {
            continue;
        };
        let mut parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let Some(name) = parts.pop() else {
            continue;
        };
        let mut dir = &mut top;
        for part in parts {
            dir = dir.dirs.entry(part).or_default();
        }
        dir.files.insert(name, file.change);
    }
    list_change_dir(&top, root, 0, folds, rows);
}

#[derive(Default)]
struct ChangeDir {
    dirs: BTreeMap<String, ChangeDir>,
    files: BTreeMap<String, Change>,
}

fn list_change_dir(dir: &ChangeDir, path: &Path, depth: usize, folds: &Folds, rows: &mut Vec<Row>) {
    for (name, child) in &dir.dirs {
        let mut name = name.clone();
        let mut child = child;
        let mut child_path = path.join(&name);
        while child.files.is_empty() && child.dirs.len() == 1 {
            let Some((next_name, next)) = child.dirs.first_key_value() else {
                break;
            };
            name = format!("{name}/{next_name}");
            child_path = child_path.join(next_name);
            child = next;
        }
        let open = folds.is_open(&child_path);
        rows.push(Row::Entry(Entry {
            path: child_path.clone(),
            name,
            is_dir: true,
            depth,
            change: None,
        }));
        if open {
            list_change_dir(child, &child_path, depth + 1, folds, rows);
        }
    }
    for (name, change) in &dir.files {
        rows.push(Row::Entry(Entry {
            path: path.join(name),
            name: name.clone(),
            is_dir: false,
            depth,
            change: Some(*change),
        }));
    }
}

/// Puts the cursor back on `path` after the rows were laid out again.
pub fn reselect(rows: &[Row], list: &mut List, path: Option<&Path>) {
    list.set_len(rows.len());
    let Some(path) = path else {
        return;
    };
    if let Some(index) = rows.iter().position(|row| row.path() == Some(path)) {
        list.select(index);
    }
}

/// Where a row is drawn and how: the line it takes, whether it is the selected one and
/// whether it is the file being edited.
pub struct RowPaint {
    pub line: Rect,
    pub selected: Option<Style>,
    pub current: bool,
}

pub fn change_style(change: Change, theme: &Theme) -> Style {
    match change {
        Change::Added => theme.get("diff.plus"),
        Change::Deleted => theme.get("diff.minus"),
        Change::Modified | Change::Renamed => theme.get("diff.delta"),
    }
}

/// Draws a file or a directory: its fold marker and name, indented by depth, and a changed
/// file's letter in the last columns, clear of the name.
pub fn draw_entry(
    surface: &mut Surface,
    paint: &RowPaint,
    entry: &Entry,
    open: bool,
    theme: &Theme,
) {
    let text_style = theme.get("ui.text");
    let change_style = entry.change.map(|change| change_style(change, theme));
    let mut style = if entry.is_dir {
        theme.get("ui.text.directory")
    } else {
        change_style.unwrap_or(text_style)
    };
    if paint.current {
        style = style.add_modifier(Modifier::BOLD);
    }
    if let Some(selected) = paint.selected {
        style = style.patch(selected);
    }
    let marker = if !entry.is_dir {
        "  "
    } else if open {
        "▾ "
    } else {
        "▸ "
    };
    let indent = 1 + entry.depth * 2;
    let label = format!("{}{}", marker, entry.name);
    let x = paint.line.x + indent as u16;
    let letter_room = if entry.change.is_some() { 3 } else { 0 };
    let width = (paint.line.width as usize).saturating_sub(indent + letter_room);
    surface.set_string_truncated(x, paint.line.y, &label, width, |_| style, true, false);
    if let Some(change) = entry.change {
        let mut letter_style = change_style.unwrap_or(text_style);
        if let Some(selected) = paint.selected {
            letter_style = letter_style.patch(selected);
        }
        let letter_x = paint.line.right().saturating_sub(2).max(paint.line.x);
        surface.set_string(letter_x, paint.line.y, change.letter(), letter_style);
    }
}

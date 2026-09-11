use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use helix_view::editor::Action;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use std::borrow::Cow;

use crate::commands;
use crate::compositor::{self, EventResult};
use crate::job::Callback;
use crate::ui::{directory_entries, EditorView, Prompt, PromptEvent};

pub struct FileTree {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    cursor: usize,
    scroll: usize,
    /// Rows the last render had room for; what paging and scrolling are measured against.
    page: usize,
    pub open: bool,
    pub focused: bool,
    /// The document the tree last revealed, so a buffer switch is noticed at render time.
    revealed: Option<PathBuf>,
    area: Rect,
    tab: Tab,
    /// The Changes tab opens every directory, so what it remembers is the ones closed.
    collapsed: HashSet<PathBuf>,
    /// What the last `git status` answered: the changed paths, or why it could not say.
    changes: Option<ChangesAnswer>,
    pending: Option<Receiver<ChangesAnswer>>,
    queried_at: Option<Instant>,
    /// Where each tab's label was drawn on the header line, for a click to land on.
    tab_columns: [(u16, u16); 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Files,
    Changes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
}

type ChangesAnswer = Result<Vec<(PathBuf, Change)>, String>;

/// How often the Changes tab asks git again while it is on screen.
const CHANGES_REFRESH: Duration = Duration::from_secs(2);

struct Row {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
    change: Option<Change>,
}

#[derive(Default)]
struct ChangeDir {
    dirs: BTreeMap<String, ChangeDir>,
    files: BTreeMap<String, Change>,
}

impl FileTree {
    pub fn new(root: PathBuf, open: bool) -> Self {
        Self {
            root,
            expanded: HashSet::new(),
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            page: 1,
            open,
            focused: false,
            revealed: None,
            area: Rect::default(),
            tab: Tab::Files,
            collapsed: HashSet::new(),
            changes: None,
            pending: None,
            queried_at: None,
            tab_columns: [(0, 0); 2],
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.focused = false;
        }
    }

    pub fn focus(&mut self, editor: &mut Editor) {
        self.open = true;
        self.focused = true;
        self.rebuild(editor);
        self.reveal_current(editor);
    }

    fn rebuild(&mut self, editor: &mut Editor) {
        let selected = self.rows.get(self.cursor).map(|row| row.path.clone());
        let mut rows = Vec::new();
        match self.tab {
            Tab::Files => self.list(&self.root.clone(), 0, editor, &mut rows),
            Tab::Changes => self.list_changes(&mut rows),
        }
        self.rows = rows;
        if let Some(selected) = selected {
            self.select(&selected);
        }
        self.clamp();
    }

    fn list(&self, dir: &Path, depth: usize, editor: &mut Editor, rows: &mut Vec<Row>) {
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
            let expanded = is_dir && self.expanded.contains(&path);
            rows.push(Row {
                path: path.clone(),
                name,
                is_dir,
                depth,
                change: None,
            });
            if expanded {
                self.list(&path, depth + 1, editor, rows);
            }
        }
    }

    /// The rows of the Changes tab: only what `git status` names, and the directories that
    /// hold it, a chain of directories with nothing else in them folded into one row.
    fn list_changes(&self, rows: &mut Vec<Row>) {
        let Some(Ok(changes)) = &self.changes else {
            return;
        };
        let mut top = ChangeDir::default();
        for (path, change) in changes {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            let mut parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            let Some(file) = parts.pop() else {
                continue;
            };
            let mut dir = &mut top;
            for part in parts {
                dir = dir.dirs.entry(part).or_default();
            }
            dir.files.insert(file, *change);
        }
        self.list_change_dir(&top, &self.root, 0, rows);
    }

    fn list_change_dir(&self, dir: &ChangeDir, path: &Path, depth: usize, rows: &mut Vec<Row>) {
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
            let expanded = !self.collapsed.contains(&child_path);
            rows.push(Row {
                path: child_path.clone(),
                name,
                is_dir: true,
                depth,
                change: None,
            });
            if expanded {
                self.list_change_dir(child, &child_path, depth + 1, rows);
            }
        }
        for (name, change) in &dir.files {
            rows.push(Row {
                path: path.join(name),
                name: name.clone(),
                is_dir: false,
                depth,
                change: Some(*change),
            });
        }
    }

    fn is_expanded(&self, path: &Path) -> bool {
        match self.tab {
            Tab::Files => self.expanded.contains(path),
            Tab::Changes => !self.collapsed.contains(path),
        }
    }

    fn set_expanded(&mut self, path: PathBuf, expanded: bool) {
        match (self.tab, expanded) {
            (Tab::Files, true) => {
                self.expanded.insert(path);
            }
            (Tab::Files, false) => {
                self.expanded.remove(&path);
            }
            (Tab::Changes, true) => {
                self.collapsed.remove(&path);
            }
            (Tab::Changes, false) => {
                self.collapsed.insert(path);
            }
        }
    }

    fn switch_tab(&mut self, tab: Tab, editor: &mut Editor) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        self.rebuild(editor);
        self.revealed = None;
    }

    /// Takes the answer of a `git status` that finished, and starts the next one when the
    /// last is older than the refresh interval. Returns whether the changes moved.
    fn poll_changes(&mut self) -> bool {
        let mut moved = false;
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(answer) => {
                    moved = self.changes.as_ref() != Some(&answer);
                    self.changes = Some(answer);
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.changes = Some(Err("git status stopped without answering".into()));
                    self.pending = None;
                    moved = true;
                }
            }
        }
        let stale = self
            .queried_at
            .is_none_or(|at| at.elapsed() >= CHANGES_REFRESH);
        if self.pending.is_none() && stale {
            let (sender, receiver) = mpsc::channel();
            let root = self.root.clone();
            std::thread::spawn(move || {
                let answer = query_changes(&root);
                if sender.send(answer).is_err() {
                    return;
                }
                helix_event::request_redraw();
                // The redraw that follows is what asks again, for as long as the tab is shown.
                std::thread::sleep(CHANGES_REFRESH);
                helix_event::request_redraw();
            });
            self.pending = Some(receiver);
            self.queried_at = Some(Instant::now());
        }
        moved
    }

    /// Expands the ancestors of the focused document and moves the cursor onto it.
    fn reveal_current(&mut self, editor: &mut Editor) {
        let doc = doc!(editor);
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return;
        };
        self.revealed = Some(path.clone());
        if !path.starts_with(&self.root) {
            return;
        }
        if self.tab == Tab::Changes {
            self.select(&path);
            self.center_cursor();
            return;
        }
        let mut dir = path.parent();
        while let Some(current) = dir {
            if current == self.root {
                break;
            }
            self.expanded.insert(current.to_path_buf());
            dir = current.parent();
        }
        self.rebuild(editor);
        self.select(&path);
        self.center_cursor();
    }

    fn center_cursor(&mut self) {
        self.scroll = self.cursor.saturating_sub(self.page / 2);
        self.clamp();
    }

    fn select(&mut self, path: &Path) {
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.cursor = index;
        }
    }

    fn clamp(&mut self) {
        if self.rows.is_empty() {
            self.cursor = 0;
            self.scroll = 0;
            return;
        }
        self.cursor = self.cursor.min(self.rows.len() - 1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        if self.cursor >= self.scroll + self.page {
            self.scroll = self.cursor + 1 - self.page;
        }
        let max_scroll = self.rows.len().saturating_sub(self.page);
        self.scroll = self.scroll.min(max_scroll);
    }

    fn move_cursor(&mut self, delta: isize) {
        let target = self.cursor as isize + delta;
        self.cursor = target.max(0) as usize;
        self.clamp();
    }

    fn scroll_by(&mut self, delta: isize) {
        let max_scroll = self.rows.len().saturating_sub(self.page) as isize;
        let target = (self.scroll as isize + delta).clamp(0, max_scroll);
        self.scroll = target as usize;
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        }
        let last_visible = self.scroll + self.page - 1;
        if self.cursor > last_visible {
            self.cursor = last_visible;
        }
        self.clamp();
    }

    fn toggle_dir(&mut self, editor: &mut Editor) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        if !row.is_dir {
            return;
        }
        let path = row.path.clone();
        let expanded = self.is_expanded(&path);
        self.set_expanded(path, !expanded);
        self.rebuild(editor);
    }

    fn expand_dir(&mut self, editor: &mut Editor) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        if !row.is_dir || self.is_expanded(&row.path) {
            return;
        }
        self.set_expanded(row.path.clone(), true);
        self.rebuild(editor);
    }

    fn collapse_all(&mut self, editor: &mut Editor) {
        match self.tab {
            Tab::Files => self.expanded.clear(),
            Tab::Changes => {
                let dirs = self.rows.iter().filter(|row| row.is_dir);
                self.collapsed.extend(dirs.map(|row| row.path.clone()));
            }
        }
        self.rebuild(editor);
    }

    /// Collapses the directory under the cursor; on a file or a closed directory, jumps to
    /// the parent instead, so repeated presses walk up the tree.
    fn collapse_or_parent(&mut self, editor: &mut Editor) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        if row.is_dir && self.is_expanded(&row.path) {
            self.set_expanded(row.path.clone(), false);
            self.rebuild(editor);
            return;
        }
        let depth = row.depth;
        if depth == 0 {
            return;
        }
        let parent = self.rows[..self.cursor]
            .iter()
            .rposition(|candidate| candidate.depth < depth);
        if let Some(parent) = parent {
            self.cursor = parent;
            self.clamp();
        }
    }

    /// Opens the file under the cursor in the focused view; a directory is toggled instead.
    /// Returns whether a file was opened.
    fn open_row(&mut self, editor: &mut Editor) -> bool {
        let Some(row) = self.rows.get(self.cursor) else {
            return false;
        };
        if row.is_dir {
            self.toggle_dir(editor);
            return false;
        }
        let path = row.path.clone();
        if row.change == Some(Change::Deleted) {
            editor.set_status(format!("{} is deleted", self.relative(&path)));
            return false;
        }
        if let Err(err) = editor.open(&path, Action::Replace) {
            editor.set_error(format!("unable to open \"{}\": {}", path.display(), err));
            return false;
        }
        self.revealed = Some(path);
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent, cx: &mut commands::Context) -> EventResult {
        let editor = &mut cx.editor;
        let half_page = (self.page / 2).max(1) as isize;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.focused = false;
            }
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.open = false;
                self.focused = false;
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.move_cursor(1);
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.move_cursor(-1);
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.move_cursor(half_page);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.move_cursor(-half_page);
            }
            (KeyCode::PageDown, _) => {
                self.move_cursor(self.page as isize);
            }
            (KeyCode::PageUp, _) => {
                self.move_cursor(-(self.page as isize));
            }
            (KeyCode::Home, _) => {
                self.cursor = 0;
                self.clamp();
            }
            (KeyCode::End, _) => {
                self.cursor = self.rows.len().saturating_sub(1);
                self.clamp();
            }
            (KeyCode::Enter, _) => {
                if self.open_row(editor) {
                    self.focused = false;
                }
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                let is_dir = self.rows.get(self.cursor).is_some_and(|row| row.is_dir);
                if is_dir {
                    self.expand_dir(editor);
                } else if self.open_row(editor) {
                    self.focused = false;
                }
            }
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, _) => {
                self.collapse_or_parent(editor);
            }
            (KeyCode::Char('H'), KeyModifiers::NONE) => {
                self.collapse_all(editor);
            }
            (KeyCode::Char('R'), KeyModifiers::NONE) => {
                self.queried_at = None;
                self.rebuild(editor);
            }
            (KeyCode::Tab, _) => {
                let tab = match self.tab {
                    Tab::Files => Tab::Changes,
                    Tab::Changes => Tab::Files,
                };
                self.switch_tab(tab, editor);
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.prompt_new(cx);
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                self.prompt_rename(cx);
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                self.prompt_delete(cx);
            }
            _ => {}
        }
        EventResult::Consumed(None)
    }

    /// Reloads the tree after a change on disk and puts the cursor on `path`, expanding down
    /// to it; a path that is gone leaves the cursor where it was.
    pub fn refresh(&mut self, editor: &mut Editor, path: &Path) {
        let mut dir = path.parent();
        while let Some(current) = dir {
            if current == self.root {
                break;
            }
            self.expanded.insert(current.to_path_buf());
            dir = current.parent();
        }
        self.queried_at = None;
        self.rebuild(editor);
        self.select(path);
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    /// The directory a new entry goes in: the one under the cursor, else the cursor's parent.
    fn cursor_dir(&self) -> PathBuf {
        let Some(row) = self.rows.get(self.cursor) else {
            return self.root.clone();
        };
        if row.is_dir {
            return row.path.clone();
        }
        row.path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone())
    }

    fn prompt_new(&mut self, cx: &mut commands::Context) {
        let dir = self.cursor_dir();
        let mut line = self.relative(&dir);
        if !line.is_empty() {
            line.push('/');
        }
        let root = self.root.clone();
        let prompt = Prompt::new(
            "new (end with / for a directory): ".into(),
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != PromptEvent::Validate || input.trim().is_empty() {
                    return;
                }
                let target = root.join(input.trim());
                let result = if input.trim_end().ends_with('/') {
                    std::fs::create_dir_all(&target)
                } else {
                    create_file(&target)
                };
                if let Err(err) = result {
                    cx.editor
                        .set_error(format!("{}: {}", target.display(), err));
                    return;
                }
                if !target.is_dir() {
                    if let Err(err) = cx.editor.open(&target, Action::Replace) {
                        cx.editor
                            .set_error(format!("{}: {}", target.display(), err));
                    }
                }
                refresh_tree(cx, target);
            },
        )
        .with_line(line, cx.editor);
        cx.push_layer(Box::new(prompt));
    }

    fn prompt_rename(&mut self, cx: &mut commands::Context) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        let source = row.path.clone();
        let root = self.root.clone();
        let line = self.relative(&source);
        let prompt = Prompt::new(
            "rename: ".into(),
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != PromptEvent::Validate || input.trim().is_empty() {
                    return;
                }
                let target = root.join(input.trim().trim_end_matches('/'));
                if target == source {
                    return;
                }
                if target.exists() {
                    cx.editor
                        .set_error(format!("{} already exists", target.display()));
                    return;
                }
                let parent_made = target
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()));
                let renamed = parent_made.and_then(|_| std::fs::rename(&source, &target));
                if let Err(err) = renamed {
                    cx.editor
                        .set_error(format!("{}: {}", source.display(), err));
                    return;
                }
                retarget_documents(cx.editor, &source, &target);
                refresh_tree(cx, target);
            },
        )
        .with_line(line, cx.editor);
        cx.push_layer(Box::new(prompt));
    }

    fn prompt_delete(&mut self, cx: &mut commands::Context) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        let target = row.path.clone();
        let is_dir = row.is_dir;
        let question: Cow<'static, str> =
            format!("delete {}? (y/N): ", self.relative(&target)).into();
        let prompt = Prompt::new(
            question,
            None,
            |_editor, _input| Vec::new(),
            move |cx, input, event| {
                if event != PromptEvent::Validate || !input.trim().eq_ignore_ascii_case("y") {
                    return;
                }
                let removed = if is_dir {
                    std::fs::remove_dir_all(&target)
                } else {
                    std::fs::remove_file(&target)
                };
                if let Err(err) = removed {
                    cx.editor
                        .set_error(format!("{}: {}", target.display(), err));
                    return;
                }
                close_documents_under(cx.editor, &target);
                let parent = target
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| target.clone());
                refresh_tree(cx, parent);
            },
        );
        cx.push_layer(Box::new(prompt));
    }

    pub fn contains(&self, row: u16, column: u16) -> bool {
        self.open
            && row >= self.area.y
            && row < self.area.bottom()
            && column >= self.area.x
            && column < self.area.right()
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent, cx: &mut commands::Context) -> EventResult {
        let editor = &mut cx.editor;
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focused = true;
                // The first line holds the tabs, not a row.
                let line = event.row.saturating_sub(self.area.y) as usize;
                if line == 0 {
                    let tabs = [Tab::Files, Tab::Changes];
                    let hit = tabs
                        .into_iter()
                        .zip(self.tab_columns)
                        .find(|(_, (from, to))| event.column >= *from && event.column < *to);
                    if let Some((tab, _)) = hit {
                        self.switch_tab(tab, editor);
                    }
                    return EventResult::Consumed(None);
                }
                let index = self.scroll + line - 1;
                if index >= self.rows.len() {
                    return EventResult::Consumed(None);
                }
                self.cursor = index;
                if self.open_row(editor) {
                    self.focused = false;
                }
                self.clamp();
            }
            MouseEventKind::ScrollDown => {
                let lines = editor.config().scroll_lines;
                self.scroll_by(lines);
            }
            MouseEventKind::ScrollUp => {
                let lines = editor.config().scroll_lines;
                self.scroll_by(-lines);
            }
            _ => {}
        }
        EventResult::Consumed(None)
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, editor: &mut Editor) {
        self.area = area;
        self.page = area.height.saturating_sub(1).max(1) as usize;

        if self.tab == Tab::Changes {
            let first_answer = self.changes.is_none();
            if self.poll_changes() {
                self.rebuild(editor);
                // The tab opened empty, so the current file could not be marked until now.
                if first_answer {
                    self.revealed = None;
                }
            }
        }
        let current = doc!(editor).path().map(Path::to_path_buf);
        if current.is_some() && current != self.revealed {
            self.reveal_current(editor);
        } else if self.rows.is_empty() {
            self.rebuild(editor);
        }
        self.clamp();

        let theme = &editor.theme;
        let directory_style = theme.get("ui.text.directory");
        let text_style = theme.get("ui.text");
        // Reversed like a picker's row, so it reads on a light theme and a dark one alike;
        // without focus the bold current file is the only mark.
        let selected_style = theme.get("ui.menu.selected");
        let separator_style = theme.get("ui.window");

        let content_width = area.width.saturating_sub(1) as usize;
        for y in area.y..area.bottom() {
            surface.set_string(area.right() - 1, y, "│", separator_style);
        }

        let header_style = directory_style.add_modifier(Modifier::BOLD);
        let inactive_style = theme.get("ui.text.inactive");
        let changes_label = match &self.changes {
            Some(Ok(changes)) if !changes.is_empty() => format!("Changes {}", changes.len()),
            _ => "Changes".to_string(),
        };
        let labels = [
            (Tab::Files, "Files".to_string()),
            (Tab::Changes, changes_label),
        ];
        let mut x = area.x + 1;
        for (index, (tab, label)) in labels.iter().enumerate() {
            let style = if *tab == self.tab {
                header_style
            } else {
                inactive_style
            };
            let room = (area.right() - 1).saturating_sub(x) as usize;
            let (end, _) = surface.set_stringn(x, area.y, label, room, style);
            self.tab_columns[index] = (x, end);
            x = end + 2;
        }

        if self.tab == Tab::Changes && self.rows.is_empty() {
            let message = match &self.changes {
                None => "reading git status…".to_string(),
                Some(Ok(_)) => "no changes".to_string(),
                Some(Err(err)) => err.clone(),
            };
            let style = match &self.changes {
                Some(Err(_)) => theme.get("error"),
                _ => inactive_style,
            };
            let width = content_width.saturating_sub(1);
            surface.set_string_truncated(
                area.x + 1,
                area.y + 1,
                &message,
                width,
                |_| style,
                true,
                false,
            );
            return;
        }

        let rows = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.page);
        for (index, row) in rows {
            let y = area.y + 1 + (index - self.scroll) as u16;
            let change_style = row.change.map(|change| change_style(change, theme));
            let mut style = if row.is_dir {
                directory_style
            } else {
                change_style.unwrap_or(text_style)
            };
            if current.as_deref() == Some(row.path.as_path()) {
                style = style.add_modifier(Modifier::BOLD);
            }
            if index == self.cursor && self.focused {
                let line = Rect::new(area.x, y, area.width.saturating_sub(1), 1);
                surface.set_style(line, selected_style);
                style = style.patch(selected_style);
            }
            let marker = if !row.is_dir {
                "  "
            } else if self.is_expanded(&row.path) {
                "▾ "
            } else {
                "▸ "
            };
            let indent = 1 + row.depth * 2;
            let label = format!("{}{}", marker, row.name);
            let x = area.x + indent as u16;
            // A changed file keeps its letter in the last columns, clear of the name.
            let letter_room = if row.change.is_some() { 3 } else { 0 };
            let width = content_width.saturating_sub(indent + letter_room);
            surface.set_string_truncated(x, y, &label, width, |_| style, true, false);
            if let Some(change) = row.change {
                let mut letter_style = change_style.unwrap_or(text_style);
                if index == self.cursor && self.focused {
                    letter_style = letter_style.patch(selected_style);
                }
                let letter_x = area.right().saturating_sub(3).max(area.x);
                surface.set_string(letter_x, y, change.letter(), letter_style);
            }
        }
    }
}

impl Change {
    /// What one `git status` entry says of a file, both columns read as one: staged or not
    /// makes no difference to a tree that only shows what changed.
    fn from_status(index: u8, worktree: u8) -> Self {
        let either = |code: u8| index == code || worktree == code;
        if either(b'?') {
            Change::Added
        } else if either(b'D') {
            Change::Deleted
        } else if either(b'R') {
            Change::Renamed
        } else if either(b'A') || either(b'C') {
            Change::Added
        } else {
            Change::Modified
        }
    }

    fn letter(self) -> &'static str {
        match self {
            Change::Modified => "M",
            Change::Added => "A",
            Change::Deleted => "D",
            Change::Renamed => "R",
        }
    }
}

fn change_style(change: Change, theme: &helix_view::Theme) -> Style {
    match change {
        Change::Added => theme.get("diff.plus"),
        Change::Deleted => theme.get("diff.minus"),
        Change::Modified | Change::Renamed => theme.get("diff.delta"),
    }
}

/// Asks git itself, so the tab lists exactly what `git status` does, ignore rules included.
fn query_changes(root: &Path) -> ChangesAnswer {
    // Porcelain paths are relative to the repository's top, and the tree may sit below it.
    let prefix = run_git(root, &["rev-parse", "--show-prefix"])?;
    let prefix = String::from_utf8_lossy(&prefix).trim_end().to_string();
    let status = run_git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;

    let mut changes = Vec::new();
    let mut entries = status.split(|byte| *byte == 0);
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        if entry.len() < 4 {
            return Err(format!(
                "git status: unreadable entry {:?}",
                String::from_utf8_lossy(entry)
            ));
        }
        let change = Change::from_status(entry[0], entry[1]);
        // A rename or a copy carries the path it came from as the next entry.
        if matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C') {
            entries.next();
        }
        let path = String::from_utf8_lossy(&entry[3..]);
        let Some(inside) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        changes.push((root.join(inside), change));
    }
    Ok(changes)
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| format!("git: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("failed").to_string();
        return Err(format!("git {}: {reason}", args[0]));
    }
    Ok(output.stdout)
}

fn create_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        return Ok(());
    }
    std::fs::File::create(path).map(|_| ())
}

/// Points every open buffer under `source` at its new location after a rename.
fn retarget_documents(editor: &mut Editor, source: &Path, target: &Path) {
    let moved: Vec<(helix_view::DocumentId, PathBuf)> = editor
        .documents()
        .filter_map(|doc| {
            let path = doc.path()?;
            let rest = path.strip_prefix(source).ok()?;
            Some((doc.id(), target.join(rest)))
        })
        .collect();
    for (id, path) in moved {
        if let Some(doc) = editor.documents.get_mut(&id) {
            doc.set_path(Some(&path));
        }
    }
}

/// Closes the unmodified buffers of files that were just deleted; a modified one stays,
/// with its changes, as a buffer for a file that no longer exists.
fn close_documents_under(editor: &mut Editor, target: &Path) {
    let gone: Vec<helix_view::DocumentId> = editor
        .documents()
        .filter(|doc| {
            doc.path()
                .is_some_and(|path| path.starts_with(target) && !doc.is_modified())
        })
        .map(|doc| doc.id())
        .collect();
    for id in gone {
        match editor.close_document(id, false) {
            Ok(()) | Err(helix_view::editor::CloseError::DoesNotExist) => {}
            Err(helix_view::editor::CloseError::BufferModified(name)) => {
                editor.set_error(format!("{} is modified and stays open", name));
            }
            Err(helix_view::editor::CloseError::SaveError(err)) => {
                editor.set_error(err.to_string());
            }
        }
    }
}

fn refresh_tree(cx: &mut compositor::Context, path: PathBuf) {
    let callback = Box::pin(async move {
        let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
            if let Some(view) = compositor.find::<EditorView>() {
                view.file_tree.refresh(editor, &path);
            }
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

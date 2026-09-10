use std::collections::HashSet;
use std::path::{Path, PathBuf};

use helix_view::editor::Action;
use helix_view::graphics::{Modifier, Rect};
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
}

struct Row {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
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
        self.list(&self.root.clone(), 0, editor, &mut rows);
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
            });
            if expanded {
                self.list(&path, depth + 1, editor, rows);
            }
        }
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
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
        self.rebuild(editor);
    }

    fn expand_dir(&mut self, editor: &mut Editor) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        if !row.is_dir || self.expanded.contains(&row.path) {
            return;
        }
        self.expanded.insert(row.path.clone());
        self.rebuild(editor);
    }

    /// Collapses the directory under the cursor; on a file or a closed directory, jumps to
    /// the parent instead, so repeated presses walk up the tree.
    fn collapse_or_parent(&mut self, editor: &mut Editor) {
        let Some(row) = self.rows.get(self.cursor) else {
            return;
        };
        if row.is_dir && self.expanded.remove(&row.path) {
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
                self.expanded.clear();
                self.rebuild(editor);
            }
            (KeyCode::Char('R'), KeyModifiers::NONE) => {
                self.rebuild(editor);
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
                // The first line is the root header, not a row.
                let line = event.row.saturating_sub(self.area.y) as usize;
                if line == 0 {
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

        let root_name = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned());
        let header_style = directory_style.add_modifier(Modifier::BOLD);
        surface.set_stringn(
            area.x + 1,
            area.y,
            &root_name,
            content_width.saturating_sub(1),
            header_style,
        );

        let rows = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.page);
        for (index, row) in rows {
            let y = area.y + 1 + (index - self.scroll) as u16;
            let mut style = if row.is_dir {
                directory_style
            } else {
                text_style
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
            } else if self.expanded.contains(&row.path) {
                "▾ "
            } else {
                "▸ "
            };
            let indent = 1 + row.depth * 2;
            let label = format!("{}{}", marker, row.name);
            let x = area.x + indent as u16;
            let width = content_width.saturating_sub(indent);
            surface.set_string_truncated(x, y, &label, width, |_| style, true, false);
        }
    }
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

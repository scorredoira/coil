//! The sidebar: a column left of the editor with tabs on what a project is — the files on
//! disk, what git sees changed, the history. The sidebar owns what the tabs share: the
//! keys, the mouse, the cursor's movement, the folding of directories, the drawing, and
//! the buffer diffs are shown in. A tab is a [`TabView`]; adding one is a file and a
//! variant of [`TabKind`].
//!
//! Nothing is asked of git or the disk while drawing: a tab asks through [`background`]
//! and the answer lands back in the sidebar on the main thread, as a job of the editor's.

pub mod changes;
pub mod commits;
pub mod diff_view;
pub mod entries;
pub mod files;
pub mod git;
pub mod list;
pub mod tab;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use helix_view::graphics::{Modifier, Rect};
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use crate::commands;
use crate::compositor::EventResult;
use crate::ui::editor;

use changes::ChangesTab;
use commits::CommitsTab;
use diff_view::DiffView;
use entries::{Row, RowPaint};
use files::{FilesTab, PromptTarget};
use tab::{Activation, Outcome, TabContext, TabView};

pub use commits::format_age;
pub use git::Commit;

/// How long a tab that asks git waits after an answer before asking again, while it is on
/// screen.
pub const REFRESH: Duration = Duration::from_secs(2);

/// Two clicks on the same row closer than this are a double click: the terminal reports
/// each press on its own, so the sidebar tells them apart itself.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// The narrowest the separator can be dragged to.
const MIN_WIDTH: u16 = 12;

/// The columns the sidebar always leaves to the editor, however wide it is asked to be.
pub const EDITOR_ROOM: u16 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabKind {
    Files,
    Changes,
    Commits,
}

impl TabKind {
    /// The tabs in the order of the strip, which Tab walks.
    const ALL: [TabKind; 3] = [TabKind::Files, TabKind::Changes, TabKind::Commits];

    fn next(self) -> TabKind {
        let index = TabKind::ALL.iter().position(|kind| *kind == self);
        let next = index.map_or(0, |index| (index + 1) % TabKind::ALL.len());
        TabKind::ALL[next]
    }
}

pub struct Sidebar {
    root: PathBuf,
    tab: TabKind,
    files: FilesTab,
    changes: ChangesTab,
    commits: CommitsTab,
    diff: DiffView,
    pub open: bool,
    pub focused: bool,
    /// Whether the first render has laid the rows out; before it there is no editor to ask.
    built: bool,
    /// The document the sidebar last moved onto, so a buffer switch is noticed at render.
    revealed: Option<PathBuf>,
    area: Rect,
    /// Where each tab's label was drawn on the strip, for a click to land on.
    tab_columns: [(u16, u16); TabKind::ALL.len()],
    /// The width the separator was dragged to, kept between sessions over the configured.
    width: Option<u16>,
    /// Whether the separator is being dragged, so the mouse is the sidebar's wherever it goes.
    resizing: bool,
    /// Why the remembered width could not be read, said on the first render: at startup the
    /// editor's own messages would cover it.
    width_error: Option<String>,
    /// The row last clicked and when, so a second click on it soon after is a double click.
    last_click: Option<(usize, Instant)>,
}

/// Whether a key is a shortcut of the editor's rather than one of the sidebar's. A key
/// held with Ctrl, Alt or Cmd, and a function key, are shortcuts wherever the focus is;
/// a plain letter is not, or it would run a command on the file behind the sidebar.
fn is_editor_shortcut(key: KeyEvent) -> bool {
    if matches!(key.code, KeyCode::F(_)) {
        return true;
    }

    key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

impl Sidebar {
    pub fn new(root: PathBuf, open: bool) -> Self {
        // A broken file costs the remembered width, not the sidebar.
        let (width, width_error) = match load_width() {
            Ok(width) => (width, None),
            Err(err) => {
                log::error!("Could not read the sidebar's width: {err:#}");
                let message = format!("Could not read the sidebar's width: {err:#}");
                (None, Some(message))
            }
        };
        Self {
            files: FilesTab::new(root.clone()),
            changes: ChangesTab::new(root.clone()),
            commits: CommitsTab::new(root.clone()),
            diff: DiffView::new(root.clone()),
            root,
            tab: TabKind::Files,
            open,
            focused: false,
            built: false,
            revealed: None,
            area: Rect::default(),
            tab_columns: [(0, 0); TabKind::ALL.len()],
            width,
            resizing: false,
            width_error,
            last_click: None,
        }
    }

    /// The sidebar's width: the one the separator was dragged to, else the configured one.
    pub fn width(&self, configured: u16) -> u16 {
        self.width.unwrap_or(configured)
    }

    pub fn resizing(&self) -> bool {
        self.resizing
    }

    /// Whether `kind` is the tab on screen.
    pub fn showing(&self, kind: TabKind) -> bool {
        self.open && self.tab == kind
    }

    pub fn toggle(&mut self, editor: &mut Editor) {
        self.open = !self.open;
        if self.open {
            self.came_on_screen(editor);
        } else {
            self.focused = false;
        }
    }

    pub fn focus(&mut self, editor: &mut Editor) {
        let was_open = self.open;
        self.open = true;
        self.focused = true;
        if !was_open {
            self.came_on_screen(editor);
        }
        // The render moves onto the current file, once it knows how many rows fit.
        self.revealed = None;
    }

    /// Shows the history of one file in the Commits tab, focused.
    pub fn show_history(&mut self, editor: &mut Editor, path: PathBuf) {
        if !path.starts_with(&self.root) {
            editor.set_error(format!("{} is outside the workspace", path.display()));
            return;
        }
        self.open = true;
        self.focused = true;
        self.tab = TabKind::Commits;
        self.revealed = None;
        let mut cx = TabContext {
            editor,
            diff: &mut self.diff,
        };
        self.commits.show_history(&mut cx, path);
    }

    /// Opens `commit` into its files in the Commits tab, focused.
    pub fn open_commit(&mut self, commit: Commit) {
        self.open = true;
        self.focused = true;
        self.tab = TabKind::Commits;
        self.revealed = None;
        self.commits.open_commit(commit);
    }

    /// Something on disk changed under `path`, by one of the sidebar's own prompts.
    pub fn disk_changed(&mut self, editor: &mut Editor, path: &Path) {
        self.files.disk_changed(editor, path);
        self.changes.ask();
    }

    /// The tab on screen, and the diff buffer beside it, borrowed apart so a tab can act on
    /// both.
    fn parts(&mut self) -> (&mut dyn TabView, &mut DiffView) {
        let tab: &mut dyn TabView = match self.tab {
            TabKind::Files => &mut self.files,
            TabKind::Changes => &mut self.changes,
            TabKind::Commits => &mut self.commits,
        };
        (tab, &mut self.diff)
    }

    fn active(&self) -> &dyn TabView {
        match self.tab {
            TabKind::Files => &self.files,
            TabKind::Changes => &self.changes,
            TabKind::Commits => &self.commits,
        }
    }

    fn active_mut(&mut self) -> &mut dyn TabView {
        self.parts().0
    }

    /// The tab on screen was just put there: it lays itself out and asks what it asks.
    fn came_on_screen(&mut self, editor: &mut Editor) {
        self.built = true;
        let (tab, diff) = self.parts();
        tab.rebuild(editor);
        let mut cx = TabContext { editor, diff };
        tab.shown(&mut cx);
        self.revealed = None;
    }

    fn switch_tab(&mut self, kind: TabKind, editor: &mut Editor) {
        if self.tab == kind {
            // Asking for the tab again steps back, where the tab has somewhere to go.
            let (tab, diff) = self.parts();
            let mut cx = TabContext { editor, diff };
            tab.step_back(&mut cx);
            return;
        }
        self.tab = kind;
        self.came_on_screen(editor);
    }

    /// Moves the tab on screen onto the focused document, if it shows files. Called at
    /// render, where the rows that fit are known, so the file lands mid-screen.
    fn reveal_current(&mut self, editor: &mut Editor) {
        let current = doc!(editor).path().map(Path::to_path_buf);
        self.revealed = current.clone();
        if let Some(path) = current {
            self.active_mut().reveal(editor, &path);
        }
    }

    fn cursor_moved(&mut self, editor: &mut Editor) {
        let (tab, diff) = self.parts();
        let mut cx = TabContext { editor, diff };
        tab.cursor_moved(&mut cx);
    }

    /// Opens the row under the cursor: a directory is folded or unfolded, anything else is
    /// the tab's to open. Returns whether the keys go over to the editor.
    fn open_row(&mut self, editor: &mut Editor, how: Activation) -> bool {
        let tab = self.active();
        let row = tab.rows().get(tab.list().cursor);
        if row.is_some_and(|row| row.dir().is_some()) {
            self.toggle_dir(editor);
            return false;
        }
        let (tab, diff) = self.parts();
        let mut cx = TabContext { editor, diff };
        let outcome = tab.open(&mut cx, how);
        if outcome == Outcome::Leave {
            // The tab just opened what is now the focused document: nothing to move onto.
            self.revealed = doc!(editor).path().map(Path::to_path_buf);
        }
        outcome == Outcome::Leave
    }

    fn toggle_dir(&mut self, editor: &mut Editor) {
        let tab = self.active();
        let Some(dir) = tab.rows().get(tab.list().cursor).and_then(Row::dir) else {
            return;
        };
        let dir = dir.to_path_buf();
        let open = tab.folds().is_some_and(|folds| folds.is_open(&dir));
        self.set_dir_open(editor, dir, !open);
    }

    fn expand_dir(&mut self, editor: &mut Editor) {
        let tab = self.active();
        let Some(dir) = tab.rows().get(tab.list().cursor).and_then(Row::dir) else {
            return;
        };
        let dir = dir.to_path_buf();
        if tab.folds().is_some_and(|folds| folds.is_open(&dir)) {
            return;
        }
        self.set_dir_open(editor, dir, true);
    }

    fn set_dir_open(&mut self, editor: &mut Editor, dir: PathBuf, open: bool) {
        let tab = self.active_mut();
        let Some(folds) = tab.folds_mut() else {
            return;
        };
        folds.set(dir, open);
        tab.rebuild(editor);
    }

    fn collapse_all(&mut self, editor: &mut Editor) {
        let tab = self.active_mut();
        let dirs: Vec<PathBuf> = tab
            .rows()
            .iter()
            .filter_map(Row::dir)
            .map(Path::to_path_buf)
            .collect();
        let Some(folds) = tab.folds_mut() else {
            return;
        };
        folds.close_all(dirs.into_iter());
        tab.rebuild(editor);
    }

    /// Closes the directory under the cursor; on a file or a closed directory, jumps to
    /// the parent instead, so repeated presses walk up the tree.
    fn collapse_or_parent(&mut self, editor: &mut Editor) {
        let tab = self.active();
        let cursor = tab.list().cursor;
        let Some(row) = tab.rows().get(cursor) else {
            return;
        };
        if let Some(dir) = row.dir() {
            if tab.folds().is_some_and(|folds| folds.is_open(dir)) {
                let dir = dir.to_path_buf();
                self.set_dir_open(editor, dir, false);
                return;
            }
        }
        let depth = row.depth();
        if depth == 0 {
            return;
        }
        let parent = tab.rows()[..cursor]
            .iter()
            .rposition(|candidate| candidate.depth() < depth);
        if let Some(parent) = parent {
            self.active_mut().list_mut().select(parent);
        }
    }

    /// Where a prompt acts: the workspace, and the entry under the cursor.
    fn prompt_target(&self) -> PromptTarget {
        let tab = self.active();
        let entry = tab.rows().get(tab.list().cursor).and_then(Row::entry);
        PromptTarget {
            root: self.root.clone(),
            path: entry.map(|entry| entry.path.clone()),
            is_dir: entry.is_some_and(|entry| entry.is_dir),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, cx: &mut commands::Context) -> EventResult {
        let editor = &mut cx.editor;
        let before = self.active().list().cursor;
        let half_page = self.active().list().half_page();
        let page = self.active().list().page as isize;
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                let (tab, diff) = self.parts();
                let mut tab_cx = TabContext { editor, diff };
                if !tab.step_back(&mut tab_cx) {
                    self.focused = false;
                }
            }
            (KeyCode::Char('q'), KeyModifiers::NONE) => {
                self.open = false;
                self.focused = false;
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => {
                self.active_mut().list_mut().move_by(1);
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => {
                self.active_mut().list_mut().move_by(-1);
            }
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                self.active_mut().list_mut().move_by(half_page);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.active_mut().list_mut().move_by(-half_page);
            }
            (KeyCode::PageDown, _) => {
                self.active_mut().list_mut().move_by(page);
            }
            (KeyCode::PageUp, _) => {
                self.active_mut().list_mut().move_by(-page);
            }
            (KeyCode::Home, _) => {
                self.active_mut().list_mut().home();
            }
            (KeyCode::End, _) => {
                self.active_mut().list_mut().end();
            }
            (KeyCode::Enter, _) => {
                if self.open_row(editor, Activation::Enter) {
                    self.focused = false;
                }
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, _) => {
                let tab = self.active();
                let on_dir = tab
                    .rows()
                    .get(tab.list().cursor)
                    .is_some_and(|row| row.dir().is_some());
                if on_dir {
                    self.expand_dir(editor);
                } else if self.open_row(editor, Activation::Enter) {
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
                let (tab, diff) = self.parts();
                let mut tab_cx = TabContext { editor, diff };
                tab.refresh(&mut tab_cx);
            }
            (KeyCode::Tab, _) => {
                self.switch_tab(self.tab.next(), editor);
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) if self.active().edits_disk() => {
                let target = self.prompt_target();
                files::prompt_new(cx, target);
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) if self.active().edits_disk() => {
                let target = self.prompt_target();
                files::prompt_rename(cx, target);
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) if self.active().edits_disk() => {
                let target = self.prompt_target();
                files::prompt_delete(cx, target);
            }
            // Anything the sidebar does not use but the editor might: it goes through, so
            // Ctrl-q quits and Ctrl-s saves wherever the focus is.
            _ if is_editor_shortcut(key) => return EventResult::Ignored(None),
            _ => {}
        }
        if self.active().list().cursor != before {
            self.cursor_moved(cx.editor);
        }
        EventResult::Consumed(None)
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
        let separator = self.area.right().saturating_sub(1);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if event.column == separator => {
                self.resizing = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing => {
                // The separator is the sidebar's last column, so it lands where the mouse is.
                let total = self.area.width + editor.tree.area().width;
                let most = total.saturating_sub(EDITOR_ROOM).max(MIN_WIDTH);
                let wanted = event.column.saturating_sub(self.area.x) + 1;
                self.width = Some(wanted.clamp(MIN_WIDTH, most));
            }
            MouseEventKind::Up(MouseButton::Left) if self.resizing => {
                self.resizing = false;
                if let Some(width) = self.width {
                    if let Err(err) = save_width(width) {
                        log::error!("Could not remember the sidebar's width: {err:#}");
                        editor
                            .set_error(format!("Could not remember the sidebar's width: {err:#}"));
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.focused = true;
                // The first line holds the tabs, not a row.
                let line = event.row.saturating_sub(self.area.y) as usize;
                if line == 0 {
                    let hit = TabKind::ALL
                        .into_iter()
                        .zip(self.tab_columns)
                        .find(|(_, (from, to))| event.column >= *from && event.column < *to);
                    if let Some((kind, _)) = hit {
                        self.switch_tab(kind, editor);
                    }
                    return EventResult::Consumed(None);
                }
                let Some(index) = self.active().list().row_at(line - 1) else {
                    return EventResult::Consumed(None);
                };
                let now = Instant::now();
                let double = self
                    .last_click
                    .is_some_and(|(row, at)| row == index && now.duration_since(at) < DOUBLE_CLICK);
                self.last_click = if double { None } else { Some((index, now)) };
                let before = self.active().list().cursor;
                self.active_mut().list_mut().select(index);
                if index != before {
                    self.cursor_moved(editor);
                }
                let how = if double {
                    Activation::Enter
                } else {
                    Activation::Click
                };
                if self.open_row(editor, how) {
                    self.focused = false;
                }
            }
            MouseEventKind::ScrollDown => {
                let lines = editor.config().scroll_lines;
                self.scroll_by(editor, lines);
            }
            MouseEventKind::ScrollUp => {
                let lines = editor.config().scroll_lines;
                self.scroll_by(editor, -lines);
            }
            _ => {}
        }
        EventResult::Consumed(None)
    }

    fn scroll_by(&mut self, editor: &mut Editor, lines: isize) {
        let list = self.active_mut().list_mut();
        let before = list.cursor;
        list.scroll_by(lines);
        if list.cursor != before {
            self.cursor_moved(editor);
        }
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, editor: &mut Editor) {
        if let Some(err) = self.width_error.take() {
            editor.set_error(err);
        }
        self.area = area;
        let page = area.height.saturating_sub(1) as usize;
        self.active_mut().list_mut().set_page(page);
        if !self.built {
            self.came_on_screen(editor);
        }
        let current = doc!(editor).path().map(Path::to_path_buf);
        if current.is_some() && current != self.revealed {
            self.reveal_current(editor);
        }

        let theme = &editor.theme;
        let directory_style = theme.get("ui.text.directory");
        // The selected row reads as a menu's selected item does: a theme may give that item
        // only a background, one the sidebar's dimmed text barely shows on, so the menu's
        // own text colour comes along. Without focus the bold current file is the only mark.
        let selected_style = theme.get("ui.menu").patch(theme.get("ui.menu.selected"));
        let separator_style = theme.get("ui.window");
        let header_style = directory_style.add_modifier(Modifier::BOLD);
        let inactive_style = theme.get("ui.text.inactive");

        let content_width = area.width.saturating_sub(1) as usize;
        for y in area.y..area.bottom() {
            surface.set_string(area.right() - 1, y, "│", separator_style);
        }

        let labels = [
            self.files.label(),
            self.changes.label(),
            self.commits.label(),
        ];
        let mut x = area.x + 1;
        for (index, (kind, label)) in TabKind::ALL.iter().zip(labels).enumerate() {
            let style = if *kind == self.tab {
                header_style
            } else {
                inactive_style
            };
            let room = (area.right() - 1).saturating_sub(x) as usize;
            let (end, _) = surface.set_stringn(x, area.y, &label, room, style);
            self.tab_columns[index] = (x, end);
            x = end + 2;
        }

        let tab = self.active();
        if tab.rows().is_empty() {
            if let Some(message) = tab.empty_message() {
                let style = if message.is_error {
                    theme.get("error")
                } else {
                    inactive_style
                };
                let width = content_width.saturating_sub(1);
                let paint = |_: usize| style;
                surface.set_string_truncated(
                    area.x + 1,
                    area.y + 1,
                    &message.text,
                    width,
                    paint,
                    true,
                    false,
                );
            }
            return;
        }

        let list = tab.list();
        let rows = tab
            .rows()
            .iter()
            .enumerate()
            .skip(list.scroll)
            .take(list.page);
        for (index, row) in rows {
            let y = area.y + 1 + (index - list.scroll) as u16;
            let line = Rect::new(area.x, y, area.width.saturating_sub(1), 1);
            let selected = (index == list.cursor && self.focused).then_some(selected_style);
            if let Some(selected) = selected {
                surface.set_style(line, selected);
            }
            let current = current
                .as_deref()
                .is_some_and(|current| row.path() == Some(current));
            let paint = RowPaint {
                line,
                selected,
                current,
            };
            match row {
                Row::Entry(entry) => {
                    let open = tab.folds().is_some_and(|folds| folds.is_open(&entry.path));
                    entries::draw_entry(surface, &paint, entry, open, theme);
                }
                Row::Commit(commit) => commits::draw_commit(surface, &paint, commit, theme),
            }
        }
    }
}

/// Runs `work` off the main thread and hands what it made to the sidebar, on it.
pub(crate) fn background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    land: impl FnOnce(&mut Sidebar, &mut Editor, T) + Send + 'static,
) {
    editor::background(work, move |editor, view, answer| {
        land(&mut view.sidebar, editor, answer);
    });
}

/// Calls `then` on the sidebar after `delay`, for what asks git again while on screen.
pub(crate) fn later(
    delay: Duration,
    then: impl FnOnce(&mut Sidebar, &mut Editor) + Send + 'static,
) {
    editor::later(delay, move |editor, view| then(&mut view.sidebar, editor));
}

/// What the sidebar remembers between sessions.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedState {
    width: u16,
}

fn state_file() -> PathBuf {
    helix_loader::data_dir().join("sidebar.toml")
}

/// The width the separator was last dragged to; none before it ever was.
fn load_width() -> anyhow::Result<Option<u16>> {
    let path = state_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let state: SavedState =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(state.width))
}

/// Written aside and renamed over, so another helix reading it never sees half.
fn save_width(width: u16) -> anyhow::Result<()> {
    let path = state_file();
    let dir = helix_loader::data_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let text = toml::to_string(&SavedState { width })?;
    let temp = dir.join(format!(".sidebar.{}.toml", std::process::id()));
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> KeyEvent {
        name.parse().expect("a key of ours")
    }

    #[test]
    fn the_editors_shortcuts_pass_through_the_sidebar() {
        assert!(is_editor_shortcut(key("C-q")));
        assert!(is_editor_shortcut(key("C-s")));
        assert!(is_editor_shortcut(key("A-z")));
        assert!(is_editor_shortcut(key("Cmd-s")));
        assert!(is_editor_shortcut(key("F12")));

        // What the sidebar reads as its own: plain keys, which would otherwise run a
        // command on the file behind it.
        assert!(!is_editor_shortcut(key("j")));
        assert!(!is_editor_shortcut(key("i")));
        assert!(!is_editor_shortcut(key("ret")));
        assert!(!is_editor_shortcut(key("space")));
    }
}

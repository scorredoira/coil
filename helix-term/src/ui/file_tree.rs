use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use helix_core::{Selection, Transaction};
use helix_view::editor::Action;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::view::ViewPosition;
use helix_view::{DocumentId, Editor};
use tui::buffer::Buffer as Surface;

use std::borrow::Cow;

use anyhow::Context as _;

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
    /// The history the Commits tab lists, newest first, as far as it has been read.
    commits: Option<CommitsAnswer>,
    /// Whether the last page of history came back short, so there is nothing older to read.
    commits_complete: bool,
    commits_pending: Option<Receiver<CommitsPage>>,
    commits_queried_at: Option<Instant>,
    /// The file whose history the Commits tab lists in place of the whole repository's.
    history_of: Option<PathBuf>,
    /// The commit whose files the Commits tab lists in place of the history.
    opened: Option<OpenCommit>,
    /// The line blamed last, so blaming it again opens its commit.
    blamed: Option<Blamed>,
    /// The scratch buffer the diffs are shown in, reused for as long as it lives.
    diff_doc: Option<DocumentId>,
    /// What the diff buffer was last asked to show, so it is asked again only when that moves.
    previewed: Option<DiffTarget>,
    preview_pending: Option<Receiver<DiffAnswer>>,
    /// Where each tab's label was drawn on the header line, for a click to land on.
    tab_columns: [(u16, u16); 3],
    /// The width the separator was dragged to, kept between sessions over the configured one.
    width: Option<u16>,
    /// Whether the separator is being dragged, so the mouse is the tree's wherever it goes.
    resizing: bool,
    /// Why the remembered width could not be read, said on the first render: at startup the
    /// editor's own messages would cover it.
    width_error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Files,
    Changes,
    Commits,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
}

type ChangesAnswer = Result<Vec<(PathBuf, Change)>, String>;

type CommitsAnswer = Result<Vec<Commit>, String>;

type DiffAnswer = (DiffTarget, Result<String, String>);

/// How often the Changes and Commits tabs ask git again while they are on screen.
const CHANGES_REFRESH: Duration = Duration::from_secs(2);

/// How many commits one `git log` reads; the cursor nearing the end reads the next page.
const COMMITS_PAGE: usize = 200;

/// The narrowest the separator can be dragged to.
const MIN_WIDTH: u16 = 12;

/// The columns the tree always leaves to the editor, however wide it is asked to be.
pub const EDITOR_ROOM: u16 = 20;

struct Row {
    path: PathBuf,
    name: String,
    kind: RowKind,
    depth: usize,
    change: Option<Change>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    File,
    Dir,
    /// A commit of the history, by its index in it.
    Commit(usize),
    /// The opened commit, standing above its files for the whole of it.
    CommitHead,
}

#[derive(Default)]
struct ChangeDir {
    dirs: BTreeMap<String, ChangeDir>,
    files: BTreeMap<String, Change>,
}

#[derive(Clone)]
struct Commit {
    hash: String,
    short: String,
    time: i64,
    subject: String,
    /// The file the commit was reached by (a file's history, a blame), relative to the
    /// repository's top and named as it was in that commit.
    file: Option<String>,
}

/// A line to blame: the buffer's file, its line from 0, and the buffer's text, which is what
/// the line numbers count.
pub struct BlameRequest {
    pub path: PathBuf,
    pub line: usize,
    pub contents: String,
}

/// Who last changed a line, and in which commit; no commit when the change is not committed.
struct Blamed {
    path: PathBuf,
    line: usize,
    author: String,
    commit: Option<Commit>,
}

/// One page of history as `git log` answered it, and where in the history it starts.
struct CommitsPage {
    skip: usize,
    answer: CommitsAnswer,
}

struct OpenCommit {
    commit: Commit,
    /// Where the tree's root sits inside the repository, as git spells it: `""` or `"a/b/"`.
    prefix: String,
    files: Vec<CommitFile>,
    /// Its directories start open, like the Changes tab's, so what it remembers is the closed.
    collapsed: HashSet<PathBuf>,
    /// Where the history list stood, to put it back on the way out.
    list_cursor: usize,
    list_scroll: usize,
}

struct CommitFile {
    path: PathBuf,
    change: Change,
    /// Where a renamed file came from, relative to the repository's top: without it the diff
    /// would show the file as new.
    from: Option<String>,
}

/// What the diff buffer is asked to show: a commit, narrowed to the pathspecs given.
#[derive(Clone, PartialEq, Eq)]
struct DiffTarget {
    hash: String,
    pathspecs: Vec<String>,
    /// What the buffer goes by while it shows this diff: the commit, and the row's name.
    name: String,
}

impl Row {
    fn is_dir(&self) -> bool {
        self.kind == RowKind::Dir
    }
}

impl FileTree {
    pub fn new(root: PathBuf, open: bool) -> Self {
        // A broken file costs the remembered width, not the tree.
        let (width, width_error) = match load_width() {
            Ok(width) => (width, None),
            Err(err) => {
                log::error!("Could not read the file tree's width: {err:#}");
                let message = format!("Could not read the file tree's width: {err:#}");
                (None, Some(message))
            }
        };
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
            commits: None,
            commits_complete: false,
            commits_pending: None,
            commits_queried_at: None,
            history_of: None,
            opened: None,
            blamed: None,
            diff_doc: None,
            previewed: None,
            preview_pending: None,
            tab_columns: [(0, 0); 3],
            width,
            resizing: false,
            width_error,
        }
    }

    /// The tree's width: the one the separator was dragged to, else the configured one.
    pub fn width(&self, configured: u16) -> u16 {
        self.width.unwrap_or(configured)
    }

    pub fn resizing(&self) -> bool {
        self.resizing
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
        // A commit row has no path to be found by again; its index is what stays put.
        let selected = self
            .rows
            .get(self.cursor)
            .filter(|row| matches!(row.kind, RowKind::File | RowKind::Dir))
            .map(|row| row.path.clone());
        let mut rows = Vec::new();
        match self.tab {
            Tab::Files => self.list(&self.root.clone(), 0, editor, &mut rows),
            Tab::Changes => self.list_changes(&mut rows),
            Tab::Commits => self.list_commits(&mut rows),
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
            let kind = if is_dir { RowKind::Dir } else { RowKind::File };
            rows.push(Row {
                path: path.clone(),
                name,
                kind,
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
        let files = changes
            .iter()
            .map(|(path, change)| (path.as_path(), *change));
        let top = self.change_tree(files);
        self.list_change_dir(&top, &self.root, 0, rows);
    }

    /// The rows of the Commits tab: the history, or, inside a commit, the commit itself and
    /// the files it touched, laid out like the Changes tab's.
    fn list_commits(&self, rows: &mut Vec<Row>) {
        if let Some(opened) = &self.opened {
            rows.push(Row {
                path: PathBuf::new(),
                name: opened.commit.subject.clone(),
                kind: RowKind::CommitHead,
                depth: 0,
                change: None,
            });
            let files = opened
                .files
                .iter()
                .map(|file| (file.path.as_path(), file.change));
            let top = self.change_tree(files);
            self.list_change_dir(&top, &self.root, 0, rows);
            return;
        }
        let Some(Ok(commits)) = &self.commits else {
            return;
        };
        for (index, commit) in commits.iter().enumerate() {
            rows.push(Row {
                path: PathBuf::new(),
                name: commit.subject.clone(),
                kind: RowKind::Commit(index),
                depth: 0,
                change: None,
            });
        }
    }

    /// Groups each changed path under the directories that hold it, below the root; a path
    /// outside the root is left out.
    fn change_tree<'a>(&self, files: impl Iterator<Item = (&'a Path, Change)>) -> ChangeDir {
        let mut top = ChangeDir::default();
        for (path, change) in files {
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
            dir.files.insert(file, change);
        }
        top
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
            let expanded = self.is_expanded(&child_path);
            rows.push(Row {
                path: child_path.clone(),
                name,
                kind: RowKind::Dir,
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
                kind: RowKind::File,
                depth,
                change: Some(*change),
            });
        }
    }

    fn is_expanded(&self, path: &Path) -> bool {
        match self.tab {
            Tab::Files => self.expanded.contains(path),
            Tab::Changes => !self.collapsed.contains(path),
            Tab::Commits => self
                .opened
                .as_ref()
                .is_some_and(|opened| !opened.collapsed.contains(path)),
        }
    }

    fn set_expanded(&mut self, path: PathBuf, expanded: bool) {
        let set = match self.tab {
            Tab::Files => {
                if expanded {
                    self.expanded.insert(path);
                } else {
                    self.expanded.remove(&path);
                }
                return;
            }
            Tab::Changes => &mut self.collapsed,
            Tab::Commits => match &mut self.opened {
                Some(opened) => &mut opened.collapsed,
                None => return,
            },
        };
        if expanded {
            set.remove(&path);
        } else {
            set.insert(path);
        }
    }

    fn switch_tab(&mut self, tab: Tab, editor: &mut Editor) {
        if self.tab == tab {
            // Asking for the Commits tab again steps back: out of a commit, then out of a
            // file's history into the whole one.
            if tab == Tab::Commits && self.opened.is_some() {
                self.leave_commit(editor);
            } else if tab == Tab::Commits && self.history_of.is_some() {
                self.leave_history(editor);
            }
            return;
        }
        self.tab = tab;
        self.rebuild(editor);
        self.revealed = None;
    }

    /// Opens the commit under the cursor: the tab lists its files instead of the history, and
    /// the diff buffer follows the cursor over them.
    fn enter_commit(&mut self, editor: &mut Editor) {
        let Some(RowKind::Commit(index)) = self.rows.get(self.cursor).map(|row| row.kind) else {
            return;
        };
        let Some(Ok(commits)) = &self.commits else {
            return;
        };
        let Some(commit) = commits.get(index).cloned() else {
            return;
        };
        self.open_commit(editor, commit);
    }

    /// Lists the files of `commit` in the Commits tab, the cursor on the file the commit was
    /// reached by when it carries one. From anywhere but the history list, the list is left
    /// on the commit's own place in it.
    fn open_commit(&mut self, editor: &mut Editor, commit: Commit) {
        let (prefix, files) = match query_commit_files(&self.root, &commit.hash) {
            Ok(answer) => answer,
            Err(err) => {
                editor.set_error(err);
                return;
            }
        };
        let target = commit
            .file
            .as_deref()
            .and_then(|file| file.strip_prefix(prefix.as_str()))
            .map(|inside| self.root.join(inside));
        let from_list = self.tab == Tab::Commits && self.opened.is_none();
        let (list_cursor, list_scroll) = if from_list {
            (self.cursor, self.scroll)
        } else {
            (0, 0)
        };
        self.previewed = None;
        self.preview_pending = None;
        if self.tab != Tab::Commits {
            self.tab = Tab::Commits;
            self.revealed = None;
        }
        self.opened = Some(OpenCommit {
            commit,
            prefix,
            files,
            collapsed: HashSet::new(),
            list_cursor,
            list_scroll,
        });
        self.cursor = 0;
        self.scroll = 0;
        self.rebuild(editor);
        if let Some(target) = target {
            self.select(&target);
            self.center_cursor();
        }
    }

    /// Shows the history of one file in the Commits tab, focused, in place of the whole one.
    pub fn show_history(&mut self, editor: &mut Editor, path: PathBuf) {
        if !path.starts_with(&self.root) {
            editor.set_error(format!("{} is outside the workspace", path.display()));
            return;
        }
        self.opened = None;
        self.previewed = None;
        self.preview_pending = None;
        self.set_history(Some(path));
        self.tab = Tab::Commits;
        self.revealed = None;
        self.open = true;
        self.focused = true;
        self.rebuild(editor);
    }

    fn leave_history(&mut self, editor: &mut Editor) {
        self.set_history(None);
        self.rebuild(editor);
    }

    /// Sets whose history the Commits tab lists: a file's, or the whole repository's with
    /// none. What was read of the other is dropped, along with any read still under way.
    fn set_history(&mut self, history: Option<PathBuf>) {
        self.history_of = history;
        self.commits = None;
        self.commits_complete = false;
        self.commits_pending = None;
        self.commits_queried_at = None;
        self.cursor = 0;
        self.scroll = 0;
    }

    /// Says who last changed the requested line, in the status line. Blaming the line blamed
    /// last opens its commit instead, focused, the cursor on the file.
    pub fn blame(&mut self, cx: &mut compositor::Context, request: BlameRequest) {
        let again = self
            .blamed
            .as_ref()
            .filter(|blamed| blamed.path == request.path && blamed.line == request.line);
        if let Some(blamed) = again {
            let Some(commit) = blamed.commit.clone() else {
                cx.editor.set_status("Not committed yet");
                return;
            };
            self.open = true;
            self.focused = true;
            self.open_commit(cx.editor, commit);
            return;
        }
        let root = self.root.clone();
        let callback = Box::pin(async move {
            let answer = tokio::task::spawn_blocking(move || query_blame(&root, &request)).await?;
            let call: Callback = Callback::EditorCompositor(Box::new(move |editor, compositor| {
                let blamed = match answer {
                    Ok(blamed) => blamed,
                    Err(err) => {
                        editor.set_error(err);
                        return;
                    }
                };
                match &blamed.commit {
                    Some(commit) => editor.set_status(format!(
                        "{} · {} · {} ago · {} (blame again to open it)",
                        commit.short,
                        blamed.author,
                        format_age(commit.time),
                        commit.subject
                    )),
                    None => editor.set_status("Not committed yet"),
                }
                if let Some(view) = compositor.find::<EditorView>() {
                    view.file_tree.blamed = Some(blamed);
                }
            }));
            Ok(call)
        });
        cx.jobs.callback(callback);
    }

    fn leave_commit(&mut self, editor: &mut Editor) {
        let Some(opened) = self.opened.take() else {
            return;
        };
        self.previewed = None;
        self.preview_pending = None;
        // The history may have been read again meanwhile, so the commit is found by its hash.
        let index = match &self.commits {
            Some(Ok(commits)) => commits
                .iter()
                .position(|commit| commit.hash == opened.commit.hash),
            _ => None,
        };
        self.cursor = index.unwrap_or(opened.list_cursor);
        self.scroll = opened.list_scroll;
        self.rebuild(editor);
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

    /// Takes the page of history a `git log` answered, then asks for the next page when the
    /// cursor nears the end of what is read, or for the top again when the last look is older
    /// than the refresh interval. Returns whether the history moved.
    fn poll_commits(&mut self) -> bool {
        let mut moved = false;
        if let Some(pending) = &self.commits_pending {
            match pending.try_recv() {
                Ok(page) => {
                    self.commits_pending = None;
                    moved = self.take_commits(page);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.commits = Some(Err("git log stopped without answering".into()));
                    self.commits_pending = None;
                    moved = true;
                }
            }
        }
        if self.commits_pending.is_some() {
            return moved;
        }
        let next_page = match &self.commits {
            Some(Ok(commits))
                if self.opened.is_none()
                    && !self.commits_complete
                    && self.cursor + self.page >= commits.len() =>
            {
                Some(commits.len())
            }
            _ => None,
        };
        // A file's history is read whole, too much to read again every two seconds: R asks.
        let stale = self
            .commits_queried_at
            .is_none_or(|at| self.history_of.is_none() && at.elapsed() >= CHANGES_REFRESH);
        let skip = match next_page {
            Some(skip) => skip,
            None if stale => {
                self.commits_queried_at = Some(Instant::now());
                0
            }
            None => return moved,
        };
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        let history = self
            .history_of
            .as_ref()
            .and_then(|path| path.strip_prefix(&self.root).ok())
            .map(Path::to_path_buf);
        std::thread::spawn(move || {
            let answer = query_commits(&root, skip, history.as_deref());
            if sender.send(CommitsPage { skip, answer }).is_err() {
                return;
            }
            helix_event::request_redraw();
            // The redraw that follows is what asks again, for as long as the tab is shown.
            std::thread::sleep(CHANGES_REFRESH);
            helix_event::request_redraw();
        });
        self.commits_pending = Some(receiver);
        moved
    }

    /// Folds a page of history into the list. The top page replaces the list when the history
    /// moved under it (a commit, a rebase, another branch), keeping the cursor on its commit
    /// when that one is still there; a later page extends it. Returns whether the list moved.
    fn take_commits(&mut self, page: CommitsPage) -> bool {
        let commits = match page.answer {
            Ok(commits) => commits,
            Err(err) => {
                self.commits = Some(Err(err));
                return true;
            }
        };
        let complete = self.history_of.is_some() || commits.len() < COMMITS_PAGE;
        if page.skip > 0 {
            let Some(Ok(held)) = &mut self.commits else {
                return false;
            };
            // A page of a list that was read again from the top meanwhile.
            if page.skip != held.len() {
                return false;
            }
            held.extend(commits);
            self.commits_complete = complete;
            return true;
        }
        let held = match &self.commits {
            Some(Ok(held)) => Some(held),
            _ => None,
        };
        if let Some(held) = held {
            if held.first().map(|commit| &commit.hash) == commits.first().map(|commit| &commit.hash)
            {
                return false;
            }
        }
        let selected = match self.rows.get(self.cursor).map(|row| row.kind) {
            Some(RowKind::Commit(index)) => held
                .and_then(|held| held.get(index))
                .map(|commit| commit.hash.clone()),
            _ => None,
        };
        if let Some(index) =
            selected.and_then(|hash| commits.iter().position(|commit| commit.hash == hash))
        {
            self.cursor = index;
        }
        self.commits = Some(Ok(commits));
        self.commits_complete = complete;
        true
    }

    /// Keeps the diff buffer on what the cursor stands on inside an opened commit: shows the
    /// `git show` that finished, and asks for another when the cursor has moved.
    fn poll_preview(&mut self, editor: &mut Editor) {
        if let Some(pending) = &self.preview_pending {
            match pending.try_recv() {
                Ok((target, answer)) => {
                    self.preview_pending = None;
                    if self.previewed.as_ref() == Some(&target) {
                        match answer {
                            Ok(text) => self.show_diff(editor, target.name, text),
                            Err(err) => editor.set_error(err),
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.preview_pending = None;
                    editor.set_error("git show stopped without answering");
                }
            }
        }
        let Some(target) = self.diff_target() else {
            return;
        };
        if self.previewed.as_ref() == Some(&target) {
            return;
        }
        // A newer request replaces the one in flight, whose answer then has nowhere to land.
        let (sender, receiver) = mpsc::channel();
        let root = self.root.clone();
        let asked = target.clone();
        std::thread::spawn(move || {
            let answer = query_diff(&root, &asked);
            if sender.send((asked, answer)).is_err() {
                return;
            }
            helix_event::request_redraw();
        });
        self.previewed = Some(target);
        self.preview_pending = Some(receiver);
    }

    /// The diff for the row under the cursor inside an opened commit: the whole commit on its
    /// own row, a directory's files on the directory, one file on the file.
    fn diff_target(&self) -> Option<DiffTarget> {
        if self.tab != Tab::Commits {
            return None;
        }
        let opened = self.opened.as_ref()?;
        let row = self.rows.get(self.cursor)?;
        let top = |path: &Path| {
            let inside = path.strip_prefix(&self.root).unwrap_or(path);
            format!(
                ":(top,literal){}{}",
                opened.prefix,
                inside.to_string_lossy()
            )
        };
        let mut pathspecs = Vec::new();
        let mut name = opened.commit.short.clone();
        match row.kind {
            RowKind::Commit(_) => return None,
            RowKind::CommitHead => {
                if !opened.prefix.is_empty() {
                    pathspecs.push(format!(":(top,literal){}", opened.prefix));
                }
            }
            RowKind::Dir => {
                pathspecs.push(top(&row.path));
                name = format!("{name} {}/", row.name);
            }
            RowKind::File => {
                pathspecs.push(top(&row.path));
                name = format!("{name} {}", row.name);
                let file = opened.files.iter().find(|file| file.path == row.path);
                if let Some(from) = file.and_then(|file| file.from.as_ref()) {
                    pathspecs.push(format!(":(top,literal){from}"));
                }
            }
        }
        Some(DiffTarget {
            hash: opened.commit.hash.clone(),
            pathspecs,
            name,
        })
    }

    /// Puts `text` in the diff buffer under `name`, shown in the focused view from its first
    /// line. The buffer is made again when it is gone: helix drops an untouched scratch buffer
    /// as soon as a view leaves it.
    fn show_diff(&mut self, editor: &mut Editor, name: String, text: String) {
        let live = self.diff_doc.filter(|id| editor.documents.contains_key(id));
        let id = match live {
            Some(id) => id,
            None => {
                let id = editor.new_file(Action::Replace);
                let loader = editor.syn_loader.load();
                let result = doc_mut!(editor, &id).set_language_by_language_id("diff", &loader);
                if let Err(err) = result {
                    editor.set_error(format!("diff buffer: {err}"));
                }
                id
            }
        };
        self.diff_doc = Some(id);
        if view!(editor).doc != id {
            editor.switch(id, Action::Replace);
        }
        let (view, doc) = current!(editor);
        let length = doc.text().len_chars();
        let change = (0, length, Some(text.into()));
        let transaction = Transaction::change(doc.text(), std::iter::once(change))
            .with_selection(Selection::point(0));
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
        doc.reset_modified();
        doc.set_view_offset(view.id, ViewPosition::default());
        doc.scratch_name = Some(name);
        helix_event::request_redraw();
    }

    /// Expands the ancestors of the focused document and moves the cursor onto it.
    fn reveal_current(&mut self, editor: &mut Editor) {
        let doc = doc!(editor);
        let Some(path) = doc.path().map(Path::to_path_buf) else {
            return;
        };
        self.revealed = Some(path.clone());
        // The history is not the disk: the cursor there stays on the commit it was on.
        if !path.starts_with(&self.root) || self.tab == Tab::Commits {
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
        if !row.is_dir() {
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
        if !row.is_dir() || self.is_expanded(&row.path) {
            return;
        }
        self.set_expanded(row.path.clone(), true);
        self.rebuild(editor);
    }

    fn collapse_all(&mut self, editor: &mut Editor) {
        match self.tab {
            Tab::Files => self.expanded.clear(),
            Tab::Changes | Tab::Commits => {
                let dirs: Vec<PathBuf> = self
                    .rows
                    .iter()
                    .filter(|row| row.is_dir())
                    .map(|row| row.path.clone())
                    .collect();
                for dir in dirs {
                    self.set_expanded(dir, false);
                }
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
        if row.is_dir() && self.is_expanded(&row.path) {
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

    /// Opens the file under the cursor in the focused view; a directory is toggled instead, and
    /// a commit is entered. Inside a commit the diff already follows the cursor, so opening a
    /// row is going over to read it. Returns whether the keys should leave the tree.
    fn open_row(&mut self, editor: &mut Editor) -> bool {
        let Some(row) = self.rows.get(self.cursor) else {
            return false;
        };
        match row.kind {
            RowKind::Dir => {
                self.toggle_dir(editor);
                return false;
            }
            RowKind::Commit(_) => {
                self.enter_commit(editor);
                return false;
            }
            // Asked again, in case the diff buffer was left for another one meanwhile.
            RowKind::CommitHead => {
                self.previewed = None;
                return true;
            }
            RowKind::File if self.tab == Tab::Commits => {
                self.previewed = None;
                return true;
            }
            RowKind::File => {}
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
                if self.tab == Tab::Commits && self.opened.is_some() {
                    self.leave_commit(editor);
                } else if self.tab == Tab::Commits && self.history_of.is_some() {
                    self.leave_history(editor);
                } else {
                    self.focused = false;
                }
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
                let is_dir = self.rows.get(self.cursor).is_some_and(|row| row.is_dir());
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
                self.commits_queried_at = None;
                self.rebuild(editor);
            }
            (KeyCode::Tab, _) => {
                let tab = match self.tab {
                    Tab::Files => Tab::Changes,
                    Tab::Changes => Tab::Commits,
                    Tab::Commits => Tab::Files,
                };
                self.switch_tab(tab, editor);
            }
            // Creating, renaming and deleting act on the disk, which the history is not.
            (KeyCode::Char('a'), KeyModifiers::NONE) if self.tab != Tab::Commits => {
                self.prompt_new(cx);
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) if self.tab != Tab::Commits => {
                self.prompt_rename(cx);
            }
            (KeyCode::Char('d'), KeyModifiers::NONE) if self.tab != Tab::Commits => {
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
        if row.is_dir() {
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
        let is_dir = row.is_dir();
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
        let separator = self.area.right().saturating_sub(1);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if event.column == separator => {
                self.resizing = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.resizing => {
                // The separator is the tree's last column, so it lands where the mouse is.
                let total = self.area.width + editor.tree.area().width;
                let most = total.saturating_sub(EDITOR_ROOM).max(MIN_WIDTH);
                let wanted = event.column.saturating_sub(self.area.x) + 1;
                self.width = Some(wanted.clamp(MIN_WIDTH, most));
            }
            MouseEventKind::Up(MouseButton::Left) if self.resizing => {
                self.resizing = false;
                if let Some(width) = self.width {
                    if let Err(err) = save_width(width) {
                        log::error!("Could not remember the file tree's width: {err:#}");
                        editor.set_error(format!(
                            "Could not remember the file tree's width: {err:#}"
                        ));
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.focused = true;
                // The first line holds the tabs, not a row.
                let line = event.row.saturating_sub(self.area.y) as usize;
                if line == 0 {
                    let tabs = [Tab::Files, Tab::Changes, Tab::Commits];
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
                // Inside a commit a click on a file only moves the diff there, leaving the keys
                // with the tree to go on reading down the list.
                let selects_only =
                    self.tab == Tab::Commits && self.opened.is_some() && !self.rows[index].is_dir();
                if !selects_only && self.open_row(editor) {
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
        if let Some(err) = self.width_error.take() {
            editor.set_error(err);
        }
        self.area = area;
        self.page = area.height.saturating_sub(1).max(1) as usize;

        match self.tab {
            Tab::Files => {}
            Tab::Changes => {
                let first_answer = self.changes.is_none();
                if self.poll_changes() {
                    self.rebuild(editor);
                    // The tab opened empty, so the current file could not be marked until now.
                    if first_answer {
                        self.revealed = None;
                    }
                }
            }
            Tab::Commits => {
                if self.poll_commits() {
                    self.rebuild(editor);
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
        self.poll_preview(editor);

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
        let commits_label = match self.history_of.as_deref().and_then(Path::file_name) {
            Some(name) => format!("History {}", name.to_string_lossy()),
            None => "Commits".to_string(),
        };
        let labels = [
            (Tab::Files, "Files".to_string()),
            (Tab::Changes, changes_label),
            (Tab::Commits, commits_label),
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

        let empty_message = match self.tab {
            _ if !self.rows.is_empty() => None,
            Tab::Files => None,
            Tab::Changes => Some((
                "reading git status…",
                "no changes",
                answer_error(&self.changes),
            )),
            Tab::Commits => Some((
                "reading git log…",
                "no commits",
                answer_error(&self.commits),
            )),
        };
        if let Some((reading, nothing, answer)) = empty_message {
            let (message, style) = match answer {
                None => (reading, inactive_style),
                Some(None) => (nothing, inactive_style),
                Some(Some(err)) => (err, theme.get("error")),
            };
            let width = content_width.saturating_sub(1);
            surface.set_string_truncated(
                area.x + 1,
                area.y + 1,
                message,
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
            let selected = index == self.cursor && self.focused;
            if selected {
                let line = Rect::new(area.x, y, area.width.saturating_sub(1), 1);
                surface.set_style(line, selected_style);
            }
            let commit = match row.kind {
                RowKind::Commit(index) => match &self.commits {
                    Some(Ok(commits)) => commits.get(index),
                    _ => None,
                },
                RowKind::CommitHead => self.opened.as_ref().map(|opened| &opened.commit),
                RowKind::File | RowKind::Dir => None,
            };
            if let Some(commit) = commit {
                // The selection's background can be the dimmed colour itself, so a selected
                // row draws its hash and age in the text's colour.
                let mut hash_style = if selected { text_style } else { inactive_style };
                let mut subject_style = text_style;
                if row.kind == RowKind::CommitHead {
                    hash_style = hash_style.add_modifier(Modifier::BOLD);
                    subject_style = subject_style.add_modifier(Modifier::BOLD);
                }
                if selected {
                    hash_style = hash_style.patch(selected_style);
                    subject_style = subject_style.patch(selected_style);
                }
                let line = CommitLine {
                    x: area.x + 1,
                    y,
                    width: content_width.saturating_sub(1),
                    hash_style,
                    subject_style,
                };
                draw_commit(surface, &line, commit);
                continue;
            }
            let change_style = row.change.map(|change| change_style(change, theme));
            let mut style = if row.is_dir() {
                directory_style
            } else {
                change_style.unwrap_or(text_style)
            };
            if current.as_deref() == Some(row.path.as_path()) {
                style = style.add_modifier(Modifier::BOLD);
            }
            if selected {
                style = style.patch(selected_style);
            }
            let marker = if !row.is_dir() {
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
                if selected {
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

    /// What a `--name-status` letter says of a file in a commit.
    fn from_name_status(status: u8) -> Self {
        match status {
            b'A' | b'C' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            _ => Change::Modified,
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

/// Reads one page of history, newest first, starting `skip` commits down from HEAD.
/// With `history`, a path below the root, it is that file's history instead, followed across
/// renames, each commit carrying the name the file had in it.
fn query_commits(root: &Path, skip: usize, history: Option<&Path>) -> CommitsAnswer {
    let skip = format!("--skip={skip}");
    let count = format!("--max-count={COMMITS_PAGE}");
    let followed = history.map(|path| path.to_string_lossy().into_owned());
    // The committer's date, which a rebase renews, so the ages read in the list's order.
    let mut args = vec!["log", "-z", "--abbrev=7", "--format=%H%x1f%h%x1f%ct%x1f%s"];
    match &followed {
        // --follow miscounts --skip, so a file's history is read whole, in one page.
        Some(path) => args.extend(["--follow", "--name-status", "--", path.as_str()]),
        None => args.extend([skip.as_str(), count.as_str()]),
    }
    let log = run_git(root, &args)?;

    let mut commits: Vec<Commit> = Vec::new();
    // A commit's line of --name-status follows its header after a newline.
    let mut records = log
        .split(|byte| *byte == 0)
        .map(|record| record.strip_prefix(b"\n").unwrap_or(record));
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        // The followed file's status: its path next, or where it came from and then its path.
        if !record.contains(&0x1f) {
            if matches!(record[0], b'R' | b'C') {
                records.next();
            }
            let (Some(commit), Some(path)) = (commits.last_mut(), records.next()) else {
                return Err(format!(
                    "git log: unreadable entry {:?}",
                    String::from_utf8_lossy(record)
                ));
            };
            commit.file = Some(String::from_utf8_lossy(path).into_owned());
            continue;
        }
        let record = String::from_utf8_lossy(record);
        let mut fields = record.splitn(4, '\x1f');
        let (Some(hash), Some(short), Some(time), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("git log: unreadable entry {record:?}"));
        };
        let Ok(time) = time.parse() else {
            return Err(format!("git log: unreadable date {time:?}"));
        };
        commits.push(Commit {
            hash: hash.to_string(),
            short: short.to_string(),
            time,
            subject: subject.to_string(),
            file: None,
        });
    }
    Ok(commits)
}

/// Blames one line of the buffer's text as git would the file with that text in it: a line
/// changed since the last commit belongs to no commit.
fn query_blame(root: &Path, request: &BlameRequest) -> Result<Blamed, String> {
    let range = format!("{0},{0}", request.line + 1);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "blame",
            "--porcelain",
            "-L",
            &range,
            "--contents",
            "-",
            "--",
        ])
        .arg(&request.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("git: {err}"))?;
    // git reads the whole text before it answers, and the answer for one line is short, so
    // writing it all first cannot wait on a full pipe.
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(request.contents.as_bytes()),
        None => Ok(()),
    };
    let output = child
        .wait_with_output()
        .map_err(|err| format!("git blame: {err}"))?;
    // A git that gave up before reading says why; the broken pipe it leaves behind does not.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().next().unwrap_or("failed").to_string();
        return Err(format!("git blame: {reason}"));
    }
    written.map_err(|err| format!("git blame: {err}"))?;

    let answer = String::from_utf8_lossy(&output.stdout);
    let mut lines = answer.lines();
    let hash = lines
        .next()
        .and_then(|first| first.split(' ').next())
        .ok_or("git blame: no answer")?;
    let mut author = "";
    let mut time = "";
    let mut subject = "";
    let mut file = "";
    for line in lines.take_while(|line| !line.starts_with('\t')) {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => author = value,
            "committer-time" => time = value,
            "summary" => subject = value,
            "filename" => file = value,
            _ => {}
        }
    }
    let commit = if hash.bytes().all(|byte| byte == b'0') {
        None
    } else {
        let Ok(time) = time.parse() else {
            return Err(format!("git blame: unreadable date {time:?}"));
        };
        Some(Commit {
            hash: hash.to_string(),
            short: hash.chars().take(7).collect(),
            time,
            subject: subject.to_string(),
            file: Some(file.to_string()),
        })
    };
    Ok(Blamed {
        path: request.path.clone(),
        line: request.line,
        author: author.to_string(),
        commit,
    })
}

/// Lists what one commit changed below the root (a merge against its first parent), and
/// where the root sits inside the repository, which its diffs are then asked with.
fn query_commit_files(root: &Path, hash: &str) -> Result<(String, Vec<CommitFile>), String> {
    let prefix = run_git(root, &["rev-parse", "--show-prefix"])?;
    let prefix = String::from_utf8_lossy(&prefix).trim_end().to_string();
    let listing = run_git(
        root,
        &[
            "show",
            "--format=",
            "--name-status",
            "-z",
            "-M",
            "--diff-merges=first-parent",
            "--no-color",
            hash,
        ],
    )?;

    let mut files = Vec::new();
    let mut entries = listing.split(|byte| *byte == 0);
    while let Some(status) = entries.next() {
        let Some(letter) = status.first().copied() else {
            continue;
        };
        // A rename or a copy names the path it came from before the one it went to.
        let from = if matches!(letter, b'R' | b'C') {
            let Some(from) = entries.next() else {
                return Err("git show: a rename without its source".into());
            };
            Some(String::from_utf8_lossy(from).into_owned())
        } else {
            None
        };
        let Some(path) = entries.next() else {
            return Err(format!(
                "git show: {} names no file",
                String::from_utf8_lossy(status)
            ));
        };
        let path = String::from_utf8_lossy(path);
        let Some(inside) = path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        files.push(CommitFile {
            path: root.join(inside),
            change: Change::from_name_status(letter),
            from,
        });
    }
    Ok((prefix, files))
}

/// The patch of a commit narrowed to the target's pathspecs, under the commit's own header,
/// as `git show` prints it.
fn query_diff(root: &Path, target: &DiffTarget) -> Result<String, String> {
    let mut args = vec![
        "show",
        "--format=medium",
        "--no-color",
        "--no-ext-diff",
        "-M",
        "--diff-merges=first-parent",
        target.hash.as_str(),
        "--",
    ];
    args.extend(target.pathspecs.iter().map(String::as_str));
    let patch = run_git(root, &args)?;
    Ok(String::from_utf8_lossy(&patch).into_owned())
}

/// Where an answer from git stands: not in yet (`None`), in (`Some(None)`), or git's error.
fn answer_error<T>(answer: &Option<Result<T, String>>) -> Option<Option<&str>> {
    answer
        .as_ref()
        .map(|answer| answer.as_ref().err().map(String::as_str))
}

/// Where a commit row is drawn and how its parts look.
struct CommitLine {
    x: u16,
    y: u16,
    width: usize,
    hash_style: Style,
    subject_style: Style,
}

/// Draws a commit on one line: its short hash, its subject, and its age at the right edge.
fn draw_commit(surface: &mut Surface, line: &CommitLine, commit: &Commit) {
    let age = format_age(commit.time);
    let (after_hash, _) =
        surface.set_stringn(line.x, line.y, &commit.short, line.width, line.hash_style);
    let used = (after_hash - line.x) as usize + 1;
    let subject_width = line.width.saturating_sub(used + age.len() + 1);
    surface.set_string_truncated(
        after_hash + 1,
        line.y,
        &commit.subject,
        subject_width,
        |_| line.subject_style,
        true,
        false,
    );
    if line.width >= used + age.len() {
        let age_x = line.x + (line.width - age.len()) as u16;
        surface.set_string(age_x, line.y, &age, line.hash_style);
    }
}

/// How long ago a commit was made, in as few characters as still read: `5m`, `3h`, `2d`.
fn format_age(time: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(time, |since| since.as_secs() as i64);
    let seconds = (now - time).max(0);
    let (amount, unit) = match seconds {
        s if s < HOUR => (s / MINUTE, "m"),
        s if s < DAY => (s / HOUR, "h"),
        s if s < MONTH => (s / DAY, "d"),
        s if s < YEAR => (s / MONTH, "mo"),
        s => (s / YEAR, "y"),
    };
    format!("{amount}{unit}")
}

/// What the tree remembers between sessions.
#[derive(serde::Serialize, serde::Deserialize)]
struct TreeState {
    width: u16,
}

fn tree_state_file() -> PathBuf {
    helix_loader::data_dir().join("file-tree.toml")
}

/// The width the separator was last dragged to; none before it ever was.
fn load_width() -> anyhow::Result<Option<u16>> {
    let path = tree_state_file();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let state: TreeState =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(state.width))
}

/// Written aside and renamed over, so another helix reading it never sees half.
fn save_width(width: u16) -> anyhow::Result<()> {
    let path = tree_state_file();
    let dir = helix_loader::data_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let text = toml::to_string(&TreeState { width })?;
    let temp = dir.join(format!(".file-tree.{}.toml", std::process::id()));
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
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

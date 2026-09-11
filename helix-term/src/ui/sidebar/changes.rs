//! The Changes tab: what `git status` names, asked again every few seconds while the tab is
//! on screen.

use std::path::{Path, PathBuf};

use helix_view::editor::Action;
use helix_view::Editor;

use super::entries::{self, Folds, Row};
use super::git::{self, Change, ChangedFile};
use super::list::List;
use super::tab::{Activation, Message, Outcome, TabContext, TabView};
use super::{TabKind, REFRESH};

pub struct ChangesTab {
    root: PathBuf,
    folds: Folds,
    rows: Vec<Row>,
    list: List,
    /// What the last `git status` answered, or why it could not.
    answer: Option<git::Answer<Vec<ChangedFile>>>,
    asking: bool,
    /// Whether the next ask is already on its way, so coming on screen does not start a
    /// second chain of them.
    armed: bool,
}

impl ChangesTab {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            folds: Folds::opened(),
            rows: Vec::new(),
            list: List::default(),
            answer: None,
            asking: false,
            armed: false,
        }
    }

    /// Asks git status, off the main thread; the answer lands in `landed`.
    pub fn ask(&mut self) {
        if self.asking {
            return;
        }
        self.asking = true;
        let root = self.root.clone();
        super::background(
            move || git::status(&root),
            |sidebar, editor, answer| {
                let shown = sidebar.showing(TabKind::Changes);
                sidebar.changes.landed(shown, editor, answer);
            },
        );
    }

    fn landed(&mut self, shown: bool, editor: &mut Editor, answer: git::Answer<Vec<ChangedFile>>) {
        self.asking = false;
        let moved = self.answer.as_ref() != Some(&answer);
        self.answer = Some(answer);
        if moved {
            self.rebuild(editor);
            // The file being edited could not be marked before the first answer.
            if let Some(path) = doc!(editor).path().map(Path::to_path_buf) {
                self.reveal(editor, &path);
            }
        }
        // For as long as the tab is on screen, the next ask follows the last answer.
        if shown && !self.armed {
            self.armed = true;
            super::later(REFRESH, |sidebar, _editor| {
                sidebar.changes.armed = false;
                if sidebar.showing(TabKind::Changes) {
                    sidebar.changes.ask();
                }
            });
        }
    }

    fn count(&self) -> usize {
        match &self.answer {
            Some(Ok(files)) => files.len(),
            _ => 0,
        }
    }
}

impl TabView for ChangesTab {
    fn label(&self) -> String {
        match self.count() {
            0 => "Changes".to_string(),
            count => format!("Changes {count}"),
        }
    }

    fn rows(&self) -> &[Row] {
        &self.rows
    }

    fn list(&self) -> &List {
        &self.list
    }

    fn list_mut(&mut self) -> &mut List {
        &mut self.list
    }

    fn folds(&self) -> Option<&Folds> {
        Some(&self.folds)
    }

    fn folds_mut(&mut self) -> Option<&mut Folds> {
        Some(&mut self.folds)
    }

    fn empty_message(&self) -> Option<Message> {
        let (text, is_error) = match &self.answer {
            None => ("reading git status…".to_string(), false),
            Some(Ok(_)) => ("no changes".to_string(), false),
            Some(Err(err)) => (err.clone(), true),
        };
        Some(Message { text, is_error })
    }

    fn rebuild(&mut self, _editor: &mut Editor) {
        let selected = self
            .rows
            .get(self.list.cursor)
            .and_then(Row::path)
            .map(Path::to_path_buf);
        let mut rows = Vec::new();
        if let Some(Ok(files)) = &self.answer {
            entries::list_changed(&self.root, files, &self.folds, &mut rows);
        }
        self.rows = rows;
        entries::reselect(&self.rows, &mut self.list, selected.as_deref());
    }

    fn shown(&mut self, _cx: &mut TabContext) {
        if !self.armed {
            self.ask();
        }
    }

    fn refresh(&mut self, _cx: &mut TabContext) {
        self.ask();
    }

    fn open(&mut self, cx: &mut TabContext, _how: Activation) -> Outcome {
        let Some(entry) = self.rows.get(self.list.cursor).and_then(Row::entry) else {
            return Outcome::Stay;
        };
        if entry.change == Some(Change::Deleted) {
            let relative = entry.path.strip_prefix(&self.root).unwrap_or(&entry.path);
            cx.editor
                .set_status(format!("{} is deleted", relative.display()));
            return Outcome::Stay;
        }
        if let Err(err) = cx.editor.open(&entry.path, Action::Replace) {
            cx.editor.set_error(format!(
                "unable to open \"{}\": {}",
                entry.path.display(),
                err
            ));
            return Outcome::Stay;
        }
        Outcome::Leave
    }

    fn reveal(&mut self, _editor: &mut Editor, path: &Path) {
        entries::reselect(&self.rows, &mut self.list, Some(path));
        self.list.center();
    }

    fn edits_disk(&self) -> bool {
        true
    }
}

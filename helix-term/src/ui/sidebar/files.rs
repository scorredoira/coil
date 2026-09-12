//! The Files tab: the workspace as it is on disk, following the file being edited, with
//! the prompts that create, rename and delete.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use helix_view::editor::Action;
use helix_view::Editor;

use super::entries::{self, Folds, Row};
use super::list::List;
use super::tab::{Activation, Outcome, TabContext, TabView};
use crate::commands;
use crate::compositor;
use crate::job;
use crate::ui;
use crate::ui::{EditorView, Prompt, PromptEvent};

pub struct FilesTab {
    root: PathBuf,
    folds: Folds,
    rows: Vec<Row>,
    list: List,
}

impl FilesTab {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            folds: Folds::closed(),
            rows: Vec::new(),
            list: List::default(),
        }
    }

    /// Something on disk changed under `path`: the rows are laid out again down to it,
    /// with the cursor on it; a path that is gone leaves the cursor where it was.
    pub fn disk_changed(&mut self, editor: &mut Editor, path: &Path) {
        self.folds.open_ancestors(&self.root, path);
        self.rebuild(editor);
        entries::reselect(&self.rows, &mut self.list, Some(path));
    }
}

impl TabView for FilesTab {
    fn label(&self) -> String {
        "Files".to_string()
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

    fn rebuild(&mut self, editor: &mut Editor) {
        let selected = self
            .rows
            .get(self.list.cursor)
            .and_then(Row::path)
            .map(Path::to_path_buf);
        let mut rows = Vec::new();
        entries::list_disk(&self.root, 0, editor, &self.folds, &mut rows);
        self.rows = rows;
        entries::reselect(&self.rows, &mut self.list, selected.as_deref());
    }

    fn refresh(&mut self, cx: &mut TabContext) {
        self.rebuild(cx.editor);
    }

    fn open(&mut self, cx: &mut TabContext, _how: Activation) -> Outcome {
        let Some(path) = self.rows.get(self.list.cursor).and_then(Row::path) else {
            return Outcome::Stay;
        };
        if let Err(err) = cx.editor.open(path, Action::Replace) {
            cx.editor
                .set_error(format!("unable to open \"{}\": {}", path.display(), err));
            return Outcome::Stay;
        }
        Outcome::Leave
    }

    fn reveal(&mut self, editor: &mut Editor, path: &Path) {
        if !path.starts_with(&self.root) {
            return;
        }
        self.folds.open_ancestors(&self.root, path);
        self.rebuild(editor);
        entries::reselect(&self.rows, &mut self.list, Some(path));
        self.list.center();
    }

    fn edits_disk(&self) -> bool {
        true
    }
}

/// Where a prompt acts: the workspace, and the entry under the cursor when it needs one.
#[derive(Clone)]
pub struct PromptTarget {
    pub root: PathBuf,
    pub path: Option<PathBuf>,
    pub is_dir: bool,
}

impl PromptTarget {
    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    /// The directory a new entry goes in: the one under the cursor, else the cursor's
    /// parent, else the root.
    fn dir(&self) -> PathBuf {
        let Some(path) = &self.path else {
            return self.root.clone();
        };
        if self.is_dir {
            return path.clone();
        }
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone())
    }
}

/// `a`: a file, or a directory when the name ends in `/`, next to the cursor.
pub fn prompt_new(cx: &mut commands::Context, target: PromptTarget) {
    let dir = target.dir();
    let mut line = target.relative(&dir);
    if !line.is_empty() {
        line.push('/');
    }
    let root = target.root;
    let ask = ui::ask::Ask::new(
        "New file or folder",
        "A name ending in / makes a folder",
        "Create",
        Box::new(move |cx: &mut compositor::Context, input: String| {
            let made = root.join(input.trim());
            let result = if input.trim_end().ends_with('/') {
                std::fs::create_dir_all(&made)
            } else {
                create_file(&made)
            };
            if let Err(err) = result {
                cx.editor.set_error(format!("{}: {}", made.display(), err));
                return;
            }
            if !made.is_dir() {
                if let Err(err) = cx.editor.open(&made, Action::Replace) {
                    cx.editor.set_error(format!("{}: {}", made.display(), err));
                }
            }
            disk_changed(cx, made);
        }),
    )
    .with_line(&line);

    cx.push_layer(Box::new(ask));
}

/// `r`: the entry under the cursor gets a new name, its open buffers going along.
pub fn prompt_rename(cx: &mut commands::Context, target: PromptTarget) {
    let Some(source) = target.path.clone() else {
        return;
    };
    let line = target.relative(&source);
    let root = target.root;
    let ask = ui::ask::Ask::new(
        "Rename",
        "The new name, or a path to move it to",
        "Rename",
        Box::new(move |cx: &mut compositor::Context, input: String| {
            let renamed_to = root.join(input.trim().trim_end_matches('/'));
            if renamed_to == source {
                return;
            }
            if renamed_to.exists() {
                cx.editor
                    .set_error(format!("{} already exists", renamed_to.display()));
                return;
            }
            let parent_made = renamed_to
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()));
            let renamed = parent_made.and_then(|_| std::fs::rename(&source, &renamed_to));
            if let Err(err) = renamed {
                cx.editor
                    .set_error(format!("{}: {}", source.display(), err));
                return;
            }
            retarget_documents(cx.editor, &source, &renamed_to);
            disk_changed(cx, renamed_to);
        }),
    )
    .with_line(&line);

    cx.push_layer(Box::new(ask));
}

/// `d`: the entry under the cursor is deleted after a confirmation, its buffers closed.
pub fn prompt_delete(cx: &mut commands::Context, target: PromptTarget) {
    let Some(path) = target.path.clone() else {
        return;
    };
    let is_dir = target.is_dir;
    let question: Cow<'static, str> = format!("delete {}? (y/N): ", target.relative(&path)).into();
    let prompt = Prompt::new(
        question,
        None,
        |_editor, _input| Vec::new(),
        move |cx, input, event| {
            if event != PromptEvent::Validate || !input.trim().eq_ignore_ascii_case("y") {
                return;
            }
            let removed = if is_dir {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if let Err(err) = removed {
                cx.editor.set_error(format!("{}: {}", path.display(), err));
                return;
            }
            close_documents_under(cx.editor, &path);
            let parent = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| path.clone());
            disk_changed(cx, parent);
        },
    );
    cx.push_layer(Box::new(prompt));
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

/// Tells the sidebar the disk changed under `path`, once the prompt's own turn is over.
fn disk_changed(cx: &mut crate::compositor::Context, path: PathBuf) {
    let callback = Box::pin(async move {
        let call = job::Callback::EditorCompositor(Box::new(move |editor, compositor| {
            if let Some(view) = compositor.find::<EditorView>() {
                view.sidebar.disk_changed(editor, &path);
            }
        }));
        Ok(call)
    });
    cx.jobs.callback(callback);
}

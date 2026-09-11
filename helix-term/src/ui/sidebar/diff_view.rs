//! The buffer the sidebar shows a diff in: one scratch buffer, named for what it shows,
//! in the focused view.

use std::path::PathBuf;

use helix_core::{Selection, Transaction};
use helix_view::editor::Action;
use helix_view::view::ViewPosition;
use helix_view::{DocumentId, Editor};

use super::git;

/// What the buffer is asked to show: a commit's patch narrowed to some paths, and the name
/// the buffer goes by while it shows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffTarget {
    pub hash: String,
    pub pathspecs: Vec<String>,
    pub name: String,
}

pub struct DiffView {
    root: PathBuf,
    /// The buffer, reused for as long as it lives: helix drops an untouched scratch buffer
    /// as soon as a view leaves it, and then it is made again.
    doc: Option<DocumentId>,
    /// What was last asked for, so the same target is not asked twice, and an answer to
    /// something asked before the cursor moved on is dropped.
    asked: Option<DiffTarget>,
}

impl DiffView {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            doc: None,
            asked: None,
        }
    }

    /// Asks git for the target's patch, unless it is the one already asked for. The patch
    /// lands in the buffer when it comes, if the target is still the one wanted.
    pub fn ask(&mut self, target: DiffTarget) {
        if self.asked.as_ref() == Some(&target) {
            return;
        }
        self.asked = Some(target.clone());
        let root = self.root.clone();
        let asked = target.clone();
        super::background(
            move || git::show(&root, &asked.hash, &asked.pathspecs),
            move |sidebar, editor, answer| sidebar.diff.landed(editor, target, answer),
        );
    }

    /// Forgets what was asked, so the next ask goes to git even for the same target: the
    /// buffer may have been left for another one meanwhile.
    pub fn forget(&mut self) {
        self.asked = None;
    }

    fn landed(&mut self, editor: &mut Editor, target: DiffTarget, answer: git::Answer<String>) {
        if self.asked.as_ref() != Some(&target) {
            return;
        }
        match answer {
            Ok(text) => self.show(editor, target.name, text),
            Err(err) => editor.set_error(err),
        }
    }

    /// Puts `text` in the buffer under `name`, shown in the focused view from its first line.
    fn show(&mut self, editor: &mut Editor, name: String, text: String) {
        let live = self.doc.filter(|id| editor.documents.contains_key(id));
        let id = match live {
            Some(id) => id,
            None => {
                let id = editor.new_file(Action::Replace);
                let loader = editor.syn_loader.load();
                let doc = doc_mut!(editor, &id);
                if let Err(err) = doc.set_language_by_language_id("diff", &loader) {
                    editor.set_error(format!("diff buffer: {err}"));
                }
                id
            }
        };
        self.doc = Some(id);
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
    }
}

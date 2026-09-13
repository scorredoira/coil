//! The buffer the sidebar shows a diff in: one scratch buffer, named for what it shows,
//! in the focused view.

use helix_core::syntax::Loader;
use std::path::PathBuf;
use std::sync::Arc;

use helix_core::{Selection, Transaction};
use helix_view::editor::Action;
use helix_view::review::ReviewAnchor;
use helix_view::view::ViewPosition;
use helix_view::{DocumentId, Editor};

use super::{git, review};

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
    asked: Option<(DiffTarget, bool)>,
    full_context: bool,
}

impl DiffView {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            doc: None,
            asked: None,
            full_context: false,
        }
    }

    /// Asks git for the target's patch, unless it is the one already asked for. The patch
    /// lands in the buffer when it comes, if the target is still the one wanted.
    pub fn ask(&mut self, target: DiffTarget, loader: Arc<Loader>) {
        self.ask_at(target, loader, None);
    }

    fn ask_at(&mut self, target: DiffTarget, loader: Arc<Loader>, anchor: Option<ReviewAnchor>) {
        let request = (target.clone(), self.full_context);
        if self.asked.as_ref() == Some(&request) {
            return;
        }
        self.asked = Some(request.clone());
        let root = self.root.clone();
        let full_context = self.full_context;
        super::background(
            move || {
                let patch = git::show(&root, &target.hash, &target.pathspecs, full_context)?;
                let mut parsed = review::parse(&patch)?;
                parsed.review.prepare_syntax(&loader);
                Ok(parsed)
            },
            move |sidebar, editor, answer| sidebar.diff.landed(editor, request, anchor, answer),
        );
    }

    pub fn full_context(&self) -> bool {
        self.full_context
    }

    pub fn toggle_context(&mut self, editor: &mut Editor) {
        let (view, doc) = current_ref!(editor);
        let Some(review) = doc.review.as_ref().filter(|_| self.doc == Some(doc.id())) else {
            editor.set_error("Open a commit diff to change its context");
            return;
        };
        let row = doc
            .selection(view.id)
            .primary()
            .cursor_line(doc.text().slice(..));
        let anchor = review.anchor(row);
        let Some((target, _)) = self.asked.clone() else {
            return;
        };
        self.full_context = !self.full_context;
        let loader = editor.syn_loader.load_full();
        self.ask_at(target, loader, anchor);
    }

    /// Forgets what was asked, so the next ask goes to git even for the same target: the
    /// buffer may have been left for another one meanwhile.
    pub fn forget(&mut self) {
        self.asked = None;
    }

    fn landed(
        &mut self,
        editor: &mut Editor,
        request: (DiffTarget, bool),
        anchor: Option<ReviewAnchor>,
        answer: git::Answer<review::ParsedReview>,
    ) {
        if self.asked.as_ref() != Some(&request) {
            return;
        }
        match answer {
            Ok(text) => self.show(editor, request.0.name, text, anchor),
            Err(err) => editor.set_error(err),
        }
    }

    /// Puts `text` in the buffer under `name`, shown in the focused view from its first line.
    fn show(
        &mut self,
        editor: &mut Editor,
        name: String,
        parsed: review::ParsedReview,
        anchor: Option<ReviewAnchor>,
    ) {
        let line = anchor
            .as_ref()
            .and_then(|anchor| parsed.review.find_anchor(anchor));
        let live = self.doc.filter(|id| editor.documents.contains_key(id));
        let id = match live {
            Some(id) => id,
            None => editor.new_file(Action::Replace),
        };
        self.doc = Some(id);
        if view!(editor).doc != id {
            editor.switch(id, Action::Replace);
        }
        let (view, doc) = current!(editor);
        doc.review = None;
        let length = doc.text().len_chars();
        let change = (0, length, Some(parsed.text.into()));
        let transaction = Transaction::change(doc.text(), std::iter::once(change))
            .with_selection(Selection::point(0));
        doc.apply(&transaction, view.id);
        doc.append_changes_to_history(view);
        doc.reset_modified();
        if let Some(line) = line {
            let pos = doc.text().line_to_char(line);
            doc.set_selection(view.id, Selection::point(pos));
        }
        doc.set_view_offset(
            view.id,
            ViewPosition {
                anchor: line
                    .map(|line| doc.text().line_to_char(line.saturating_sub(3)))
                    .unwrap_or(0),
                ..ViewPosition::default()
            },
        );
        doc.scratch_name = Some(name);
        doc.readonly = true;
        doc.review = Some(parsed.review);
    }
}

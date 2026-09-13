//! The editor's own git commands: what a buffer's file has been through, and who changed
//! the line under the cursor. They answer in the status line and, for more, in the
//! sidebar.

use std::path::{Path, PathBuf};

use super::Context;
use crate::ui::sidebar::git::{self, Blame, BlameRequest};
use crate::ui::{self, EditorView};

/// The line blamed last, so blaming it again opens its commit.
pub struct LastBlame {
    path: PathBuf,
    line: usize,
    blame: Blame,
}

/// Shows the history of the current file in the sidebar.
pub fn file_history(cx: &mut Context) {
    let Some(path) = doc!(cx.editor).path().map(Path::to_path_buf) else {
        cx.editor
            .set_error("The buffer has no file to show the history of");
        return;
    };
    cx.callback.push(Box::new(move |compositor, cx| {
        let view = compositor.find::<EditorView>().unwrap();
        view.sidebar.show_history(cx.editor, path);
    }));
}

/// Says who last changed the line under the cursor, in which commit and when, counting the
/// buffer's unsaved text; asked again on the same line, opens that commit in the sidebar.
pub fn blame_line(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let path = doc.path().map(Path::to_path_buf);
    let text = doc.text().slice(..);
    let line = doc.selection(view.id).primary().cursor_line(text);
    let contents = text.to_string();
    let root = doc.workspace_root().to_path_buf();
    let Some(path) = path else {
        cx.editor.set_error("The buffer has no file to blame");
        return;
    };
    let request = BlameRequest {
        path,
        line,
        contents,
    };
    cx.callback.push(Box::new(move |compositor, cx| {
        let view = compositor.find::<EditorView>().unwrap();
        let same_line = view
            .last_blame
            .as_ref()
            .filter(|last| last.path == request.path && last.line == request.line);
        if let Some(last) = same_line {
            match last.blame.commit.clone() {
                Some(commit) => view.sidebar.open_commit(commit),
                None => cx.editor.set_status("Not committed yet"),
            }
            return;
        }
        ui::editor::background(
            move || {
                let blame = git::blame(&root, &request);
                (request, blame)
            },
            |editor, view, (request, blame)| {
                let blame = match blame {
                    Ok(blame) => blame,
                    Err(err) => {
                        editor.set_error(err);
                        return;
                    }
                };
                match &blame.commit {
                    Some(commit) => editor.set_status(format!(
                        "{} · {} · {} ago · {} (blame again to open it)",
                        commit.short,
                        blame.author,
                        ui::sidebar::format_age(commit.time),
                        commit.subject
                    )),
                    None => editor.set_status("Not committed yet"),
                }
                view.last_blame = Some(LastBlame {
                    path: request.path,
                    line: request.line,
                    blame,
                });
            },
        );
    }));
}

/// Review controls are commands so menus, configured keys and the palette share them.
pub fn review_commits_toggle(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let view = compositor.find::<EditorView>().unwrap();
        view.sidebar.toggle_commits(cx.editor);
    }));
}

pub fn review_code_toggle(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let view = compositor.find::<EditorView>().unwrap();
        view.sidebar.toggle_code(cx.editor);
    }));
}

pub fn review_context_toggle(cx: &mut Context) {
    cx.callback.push(Box::new(|compositor, cx| {
        let view = compositor.find::<EditorView>().unwrap();
        view.sidebar.toggle_context(cx.editor);
    }));
}

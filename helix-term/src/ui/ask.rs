use crate::compositor::{Component, Context, Event, EventResult};
use helix_core::Position;
use helix_view::{
    graphics::{CursorKind, Margin, Rect},
    input::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind},
    theme::Modifier,
    Editor,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// What is done with the line once it is taken.
pub type Taken = Box<dyn FnOnce(&mut Context, String)>;

/// The room around the text inside the border.
const PADDING: u16 = 2;

/// How wide the field is drawn, unless the title or the label needs more.
const FIELD_WIDTH: u16 = 48;

/// A question in the middle of the screen that wants a line of text: the box `Confirm`
/// draws, with a field in place of the message. The field has the focus from the moment
/// it opens — Enter takes what is in it, Escape answers nothing — and the buttons take a
/// click.
pub struct Ask {
    title: String,
    /// What the field is for, over it.
    label: String,
    line: String,
    /// Where the next character goes: a byte offset into `line`.
    cursor: usize,
    ok: String,
    taken: Option<Taken>,
    /// Where the field was drawn last, for the terminal's cursor to sit in.
    field: Rect,
    /// Where each button was drawn last, so a click can land on one.
    buttons: Vec<Rect>,
}

impl Ask {
    pub fn new(title: &str, label: &str, ok: &str, taken: Taken) -> Self {
        Self {
            title: title.to_string(),
            label: label.to_string(),
            line: String::new(),
            cursor: 0,
            ok: ok.to_string(),
            taken: Some(taken),
            field: Rect::default(),
            buttons: Vec::new(),
        }
    }

    /// The line the field holds starts as this, with the cursor after it.
    pub fn with_line(mut self, line: &str) -> Self {
        self.line = line.to_string();
        self.cursor = self.line.len();
        self
    }

    /// Closes the dialog and hands the line over. A blank field answers nothing: there is
    /// no name in it to take.
    fn take(&mut self) -> EventResult {
        let line = self.line.trim().to_string();
        if line.is_empty() {
            return EventResult::Consumed(None);
        }

        let Some(taken) = self.taken.take() else {
            return self.close();
        };

        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.pop();
            taken(cx, line);
        })))
    }

    fn close(&self) -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor, _| {
            compositor.pop();
        })))
    }

    fn insert(&mut self, text: &str) {
        self.line.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn delete_before(&mut self) {
        let Some(char) = self.line[..self.cursor].chars().next_back() else {
            return;
        };

        let from = self.cursor - char.len_utf8();
        self.line.replace_range(from..self.cursor, "");
        self.cursor = from;
    }

    fn delete_after(&mut self) {
        let Some(char) = self.line[self.cursor..].chars().next() else {
            return;
        };

        let to = self.cursor + char.len_utf8();
        self.line.replace_range(self.cursor..to, "");
    }

    fn move_left(&mut self) {
        self.cursor = self.line[..self.cursor]
            .chars()
            .next_back()
            .map_or(0, |char| self.cursor - char.len_utf8());
    }

    fn move_right(&mut self) {
        self.cursor = self.line[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |char| self.cursor + char.len_utf8());
    }

    /// What the field shows and where the cursor sits in it: the tail of the line when it
    /// has grown past the field's width, so the cursor is always on screen.
    fn shown(&self, width: usize) -> (&str, usize) {
        let before = self.line[..self.cursor].chars().count();
        if before < width {
            return (&self.line, before);
        }

        let skip = before - width + 1;
        let from = self
            .line
            .char_indices()
            .nth(skip)
            .map_or(self.line.len(), |(index, _)| index);

        (&self.line[from..], width - 1)
    }

    fn buttons_width(&self) -> usize {
        self.ok.chars().count() + 2 + 2 + "Cancel".len() + 2
    }
}

impl Component for Ask {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let text_width = [
            self.title.chars().count(),
            self.label.chars().count(),
            self.buttons_width(),
            FIELD_WIDTH as usize,
        ]
        .into_iter()
        .max()
        .unwrap_or(0);

        let width = (text_width as u16 + PADDING * 2 + 2).min(area.width);
        // The title, a blank row, the label, the field, the rule under it, a blank row
        // and the buttons, plus borders.
        let height = 9u16.min(area.height);
        let dialog = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );

        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        let text = theme.get("ui.text");
        let dim = theme.get("ui.text.inactive");
        let selected = theme.get("ui.menu").patch(theme.get("ui.menu.selected"));
        let rule = theme.get("ui.window");

        surface.clear_with(dialog, background);
        let block = Block::bordered().style(background);
        let inner = block.inner(dialog).inner(Margin::horizontal(PADDING - 1));
        block.render(dialog, surface);

        let bold = text.add_modifier(Modifier::BOLD);
        surface.set_stringn(inner.x, inner.y, &self.title, inner.width as usize, bold);

        // The label goes UNDER the rule, where it reads as what the field is for. Over it
        // the two run together and it reads as something already typed.
        surface.set_stringn(inner.x, inner.y + 4, &self.label, inner.width as usize, dim);

        // The field is a line to write on: a rule under it, the way a form has one, since
        // an empty row on the dialog's own colour is not visibly a place to type.
        self.field = Rect::new(inner.x, inner.y + 2, inner.width, 1);
        let (shown, _) = self.shown(self.field.width as usize);
        surface.set_stringn(
            self.field.x,
            self.field.y,
            shown,
            self.field.width as usize,
            text,
        );

        for x in self.field.x..self.field.right() {
            surface.set_string(x, self.field.y + 1, "─", rule);
        }

        // The buttons sit on the last row, together on the right.
        let y = inner.bottom().saturating_sub(1);
        let mut x = inner
            .right()
            .saturating_sub(self.buttons_width() as u16)
            .max(inner.x);

        self.buttons.clear();
        for (index, label) in ["Cancel", &self.ok].into_iter().enumerate() {
            let label = format!(" {label} ");
            let button_width = label.chars().count() as u16;
            let style = if index == 1 { selected } else { text };

            surface.set_stringn(x, y, &label, inner.width as usize, style);
            self.buttons.push(Rect::new(x, y, button_width, 1));
            x += button_width + 2;
        }
    }

    fn cursor(&self, _area: Rect, _editor: &Editor) -> (Option<Position>, CursorKind) {
        if self.field.width == 0 {
            return (None, CursorKind::Hidden);
        }

        let (_, at) = self.shown(self.field.width as usize);
        let position = Position::new(self.field.y as usize, self.field.x as usize + at);

        (Some(position), CursorKind::Bar)
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(*key),
            Event::Paste(text) => {
                self.insert(text);
                EventResult::Consumed(None)
            }
            Event::Mouse(event) => {
                if event.kind != MouseEventKind::Down(MouseButton::Left) {
                    return EventResult::Consumed(None);
                }

                let hit = self.buttons.iter().position(|button| {
                    event.row == button.y
                        && event.column >= button.x
                        && event.column < button.right()
                });
                match hit {
                    Some(1) => self.take(),
                    Some(_) => self.close(),
                    None => EventResult::Consumed(None),
                }
            }
            // A dialog is a question: nothing behind it hears anything until it is answered.
            _ => EventResult::Consumed(None),
        }
    }
}

impl Ask {
    fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('u') => {
                    self.line.clear();
                    self.cursor = 0;
                }
                KeyCode::Char('c') => return self.close(),
                _ => {}
            }

            return EventResult::Consumed(None);
        }

        match key.code {
            KeyCode::Esc => return self.close(),
            KeyCode::Enter => return self.take(),
            KeyCode::Char(char) => self.insert(&char.to_string()),
            KeyCode::Backspace => self.delete_before(),
            KeyCode::Delete => self.delete_after(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.line.len(),
            _ => {}
        }

        EventResult::Consumed(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(line: &str) -> Ask {
        Ask::new("Save", "Name", "Save", Box::new(|_, _| {})).with_line(line)
    }

    #[test]
    fn typing_lands_where_the_cursor_is() {
        let mut dialog = ask("main.rs");
        dialog.move_left();
        dialog.move_left();
        dialog.insert("X");

        assert_eq!(dialog.line, "main.Xrs");
    }

    #[test]
    fn deleting_walks_whole_characters() {
        let mut dialog = ask("añ");
        dialog.delete_before();
        assert_eq!(dialog.line, "a");

        dialog.move_left();
        dialog.delete_after();
        assert_eq!(dialog.line, "");

        // Nothing left to take: the dialog must not walk off either end.
        dialog.delete_before();
        dialog.delete_after();
        assert_eq!(dialog.line, "");
    }

    #[test]
    fn a_line_wider_than_the_field_shows_its_tail() {
        let dialog = ask("abcdefghij");

        assert_eq!(dialog.shown(20), ("abcdefghij", 10));
        // The cursor sits at the end, so the window ends with it.
        assert_eq!(dialog.shown(4), ("hij", 3));
    }
}

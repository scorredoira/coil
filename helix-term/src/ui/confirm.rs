use crate::compositor::{Component, Context, Event, EventResult};
use helix_view::{
    graphics::{Margin, Rect},
    input::{KeyCode, KeyEvent, MouseButton, MouseEventKind},
    theme::Modifier,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// What an answer does once it is chosen.
pub type Choice = Box<dyn FnOnce(&mut Context)>;

/// An answer: what it reads as, whether it is the destructive one, and what it does.
pub struct Answer {
    label: String,
    destructive: bool,
    choice: Choice,
}

impl Answer {
    pub fn new(label: &str, choice: Choice) -> Self {
        Self {
            label: label.to_string(),
            destructive: false,
            choice,
        }
    }

    /// An answer that throws work away wears the warning colour.
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

/// A question in the middle of the screen: what happened, and the answers to it. Left
/// and right (or Tab) walk them, Enter takes the one in focus, a click takes the one it
/// lands on, and Escape always answers nothing.
pub struct Confirm {
    title: String,
    lines: Vec<String>,
    answers: Vec<Answer>,
    focused: usize,
    /// Where each answer was drawn last, so a click can land on one.
    buttons: Vec<Rect>,
}

/// The room around the text inside the border.
const PADDING: u16 = 2;

impl Confirm {
    pub fn new(title: &str, lines: Vec<String>, answers: Vec<Answer>) -> Self {
        Self {
            title: title.to_string(),
            lines,
            answers,
            focused: 0,
            buttons: Vec::new(),
        }
    }

    /// The answer in focus, taken out of the dialog so it can be run.
    fn take(&mut self, index: usize) -> Option<Choice> {
        if index >= self.answers.len() {
            return None;
        }

        Some(self.answers.remove(index).choice)
    }

    /// Runs one answer and closes the dialog.
    fn answer(&mut self, index: usize) -> EventResult {
        let Some(choice) = self.take(index) else {
            return EventResult::Consumed(None);
        };

        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.pop();
            choice(cx);
        })))
    }

    fn close(&self) -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor, _| {
            compositor.pop();
        })))
    }

    fn walk(&mut self, forward: bool) {
        let answers = self.answers.len();
        if answers == 0 {
            return;
        }

        self.focused = if forward {
            (self.focused + 1) % answers
        } else {
            (self.focused + answers - 1) % answers
        };
    }

    /// How wide each answer is drawn, gaps included.
    fn buttons_width(&self) -> usize {
        let labels: usize = self
            .answers
            .iter()
            .map(|answer| answer.label.chars().count() + 2)
            .sum();

        labels + self.answers.len().saturating_sub(1) * 2
    }
}

impl Component for Confirm {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let text_width = self
            .lines
            .iter()
            .map(|line| line.chars().count())
            .chain([self.title.chars().count(), self.buttons_width()])
            .max()
            .unwrap_or(0);

        let width = (text_width as u16 + PADDING * 2 + 2).min(area.width);
        // The title, a blank row, the lines, a blank row and the answers, plus borders.
        let height = (self.lines.len() as u16 + 6).min(area.height);
        let dialog = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        );

        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        let text = theme.get("ui.text");
        let selected = theme.get("ui.menu").patch(theme.get("ui.menu.selected"));
        let warning = theme.get("warning");

        surface.clear_with(dialog, background);
        let block = Block::bordered().style(background);
        let inner = block.inner(dialog).inner(Margin::horizontal(PADDING - 1));
        block.render(dialog, surface);

        let bold = text.add_modifier(Modifier::BOLD);
        surface.set_stringn(inner.x, inner.y, &self.title, inner.width as usize, bold);

        for (index, line) in self.lines.iter().enumerate() {
            let y = inner.y + 2 + index as u16;
            if y >= inner.bottom() {
                break;
            }
            surface.set_stringn(inner.x, y, line, inner.width as usize, text);
        }

        // The answers sit on the last row, together on the right.
        let y = inner.bottom().saturating_sub(1);
        let mut x = inner
            .right()
            .saturating_sub(self.buttons_width() as u16)
            .max(inner.x);

        self.buttons.clear();
        for (index, answer) in self.answers.iter().enumerate() {
            let label = format!(" {} ", answer.label);
            let width = label.chars().count() as u16;
            let style = if index == self.focused {
                selected
            } else if answer.destructive {
                warning
            } else {
                text
            };

            surface.set_stringn(x, y, &label, inner.width as usize, style);
            self.buttons.push(Rect::new(x, y, width, 1));
            x += width + 2;
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(*key),
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
                    Some(index) => self.answer(index),
                    None => EventResult::Consumed(None),
                }
            }
            // A dialog is a question: nothing behind it hears anything until it is answered.
            _ => EventResult::Consumed(None),
        }
    }
}

impl Confirm {
    fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Esc => self.close(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let focused = self.focused;
                self.answer(focused)
            }
            KeyCode::Left => {
                self.walk(false);
                EventResult::Consumed(None)
            }
            KeyCode::Right | KeyCode::Tab => {
                self.walk(true);
                EventResult::Consumed(None)
            }
            _ => EventResult::Consumed(None),
        }
    }
}

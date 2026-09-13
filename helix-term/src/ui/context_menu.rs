use crate::compositor::{Component, Compositor, Context, Event, EventResult};
use helix_view::{
    graphics::{Margin, Rect},
    input::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// What an entry does when it is taken. It is handed the compositor as well, because
/// what a menu entry does is usually open something.
pub type Action = Box<dyn FnOnce(&mut Compositor, &mut Context)>;

/// Runs something that wants the editor's own context, and lets what it opened open: a
/// command that pushes a dialog leaves it on the context it was handed.
pub fn with_context(
    compositor: &mut Compositor,
    outer: &mut Context,
    run: impl FnOnce(&mut crate::commands::Context),
) {
    let mut cx = crate::commands::Context {
        editor: outer.editor,
        jobs: outer.jobs,
        count: None,
        register: None,
        callback: Vec::new(),
        on_next_key_callback: None,
    };

    run(&mut cx);

    let callbacks = std::mem::take(&mut cx.callback);
    drop(cx);

    for callback in callbacks {
        callback(compositor, outer);
    }
}

/// One line of the menu: what it reads as, the key that does the same thing, and what it
/// does. The key is only written down — the menu does not listen for it.
pub struct Entry {
    label: String,
    key: String,
    action: Action,
}

impl Entry {
    pub fn new(label: &str, key: &str, action: Action) -> Self {
        Self {
            label: label.to_string(),
            key: key.to_string(),
            action,
        }
    }
}

/// The menu the right button opens, where it was pressed: what can be done to the thing
/// under the pointer. Up and down walk it, Enter takes the one in focus, a click takes the
/// one it lands on, and Escape — or a click outside — answers nothing.
pub struct ContextMenu {
    entries: Vec<Entry>,
    /// The row and column the button was pressed on.
    at: (u16, u16),
    focused: usize,
    /// Where each entry was drawn last, so a click can land on one.
    rows: Vec<Rect>,
}

/// The room between the label and the key on its right.
const GAP: u16 = 3;

impl ContextMenu {
    pub fn new(at: (u16, u16), entries: Vec<Entry>) -> Self {
        Self {
            entries,
            at,
            focused: 0,
            rows: Vec::new(),
        }
    }

    /// Runs one entry and closes the menu.
    fn take(&mut self, index: usize) -> EventResult {
        if index >= self.entries.len() {
            return self.close();
        }

        let action = self.entries.remove(index).action;

        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.pop();
            action(compositor, cx);
        })))
    }

    fn close(&self) -> EventResult {
        EventResult::Consumed(Some(Box::new(|compositor, _| {
            compositor.pop();
        })))
    }

    fn walk(&mut self, forward: bool) {
        let entries = self.entries.len();
        if entries == 0 {
            return;
        }

        self.focused = if forward {
            (self.focused + 1) % entries
        } else {
            (self.focused + entries - 1) % entries
        };
    }

    /// How wide the widest line is, both columns and the gap between them.
    fn width(&self) -> u16 {
        let widest = self
            .entries
            .iter()
            .map(|entry| entry.label.chars().count() + entry.key.chars().count())
            .max()
            .unwrap_or(0);

        widest as u16 + GAP + 2
    }
}

impl Component for ContextMenu {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let width = (self.width() + 2).min(area.width);
        let height = (self.entries.len() as u16 + 2).min(area.height);

        // It opens at the pointer, and folds back onto the screen at its edges.
        let x = self.at.1.min(area.right().saturating_sub(width));
        let y = if self.at.0 + height <= area.bottom() {
            self.at.0
        } else {
            self.at.0.saturating_sub(height)
        };
        let menu = Rect::new(x, y, width, height);

        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        let text = theme.get("ui.text");
        let dim = theme.get("ui.text.inactive");
        let selected = theme.get("ui.menu").patch(theme.get("ui.menu.selected"));

        surface.clear_with(menu, background);
        let block = Block::bordered().style(background);
        let inner = block.inner(menu).inner(Margin::horizontal(1));
        block.render(menu, surface);

        self.rows.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            let y = inner.y + index as u16;
            if y >= inner.bottom() {
                break;
            }

            let row = Rect::new(inner.x, y, inner.width, 1);
            let (label, key) = if index == self.focused {
                surface.clear_with(row, selected);
                (selected, selected)
            } else {
                (text, dim)
            };

            surface.set_stringn(inner.x, y, &entry.label, inner.width as usize, label);

            let at = inner
                .right()
                .saturating_sub(entry.key.chars().count() as u16);
            surface.set_stringn(at, y, &entry.key, inner.width as usize, key);

            self.rows.push(row);
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) => self.handle_key(*key),
            Event::Mouse(event) => self.handle_mouse(event),
            // A menu is a question: nothing behind it hears anything until it is answered.
            _ => EventResult::Consumed(None),
        }
    }
}

impl ContextMenu {
    fn handle_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Esc => self.close(),
            KeyCode::Enter => {
                let focused = self.focused;
                self.take(focused)
            }
            KeyCode::Up => {
                self.walk(false);
                EventResult::Consumed(None)
            }
            KeyCode::Down | KeyCode::Tab => {
                self.walk(true);
                EventResult::Consumed(None)
            }
            _ => EventResult::Consumed(None),
        }
    }

    fn handle_mouse(&mut self, event: &MouseEvent) -> EventResult {
        let hit = self.rows.iter().position(|row| {
            event.row == row.y && event.column >= row.x && event.column < row.right()
        });

        match event.kind {
            // The pointer moving is only news when it lands on another entry: the
            // terminal reports every motion, and taking one repaints the screen.
            MouseEventKind::Moved => match hit {
                Some(index) if index != self.focused => {
                    self.focused = index;
                    EventResult::Consumed(None)
                }
                _ => EventResult::Ignored(None),
            },
            MouseEventKind::Down(MouseButton::Left | MouseButton::Right) => match hit {
                Some(index) => self.take(index),
                // Pressing outside a menu is how a menu is dismissed everywhere.
                None => self.close(),
            },
            _ => EventResult::Consumed(None),
        }
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::compositor::{Component, Context, Event, EventResult};
use crate::ui::Spinner;
use helix_view::{
    graphics::{Margin, Rect},
    input::KeyCode,
    theme::Modifier,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Widget},
};

/// How often the spinner moves on.
const FRAME: Duration = Duration::from_millis(80);

/// The room around the text inside the border.
const PADDING: u16 = 2;

/// Work under way, in the middle of the screen: a spinner beside what is being done, so a
/// wait on the network reads as work and not as nothing. It goes when the work lands and
/// its answer takes its place; Escape puts it away sooner, and the answer still comes.
pub struct Busy {
    id: &'static str,
    title: String,
    spinner: Spinner,
    /// Kept true while the dialog is up, so the redraws that turn the spinner stop with it.
    animating: Arc<AtomicBool>,
}

impl Busy {
    pub fn new(id: &'static str, title: impl Into<String>) -> Self {
        let mut spinner = Spinner::dots(FRAME.as_millis() as u64);
        spinner.start();
        let animating = Arc::new(AtomicBool::new(true));
        let running = animating.clone();
        tokio::spawn(async move {
            while running.load(Ordering::Relaxed) {
                helix_event::request_redraw();
                tokio::time::sleep(FRAME).await;
            }
        });
        Self {
            id,
            title: title.into(),
            spinner,
            animating,
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.animating.store(false, Ordering::Relaxed);
    }
}

impl Component for Busy {
    fn render(&mut self, area: Rect, surface: &mut Surface, cx: &mut Context) {
        let hint = "Esc hides this; the answer still comes";
        let title = format!("{} {}", self.spinner.frame().unwrap_or(" "), self.title);
        let text_width = title.chars().count().max(hint.chars().count());
        let width = (text_width as u16 + PADDING * 2 + 2).min(area.width);
        // The title, a blank row and the hint, plus borders.
        let height = 5.min(area.height);
        let dialog = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );

        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        let text = theme.get("ui.text").add_modifier(Modifier::BOLD);
        let dim = theme.get("ui.text.inactive");

        surface.clear_with(dialog, background);
        let block = Block::bordered().style(background);
        let inner = block.inner(dialog).inner(Margin::horizontal(PADDING - 1));
        block.render(dialog, surface);

        surface.set_stringn(inner.x, inner.y, &title, inner.width as usize, text);
        if inner.height > 2 {
            surface.set_stringn(inner.x, inner.y + 2, hint, inner.width as usize, dim);
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            Event::Key(key) if key.code == KeyCode::Esc => {
                let id = self.id;
                EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.remove(id);
                })))
            }
            // Nothing behind it is for typing into while the work it waits on is under way.
            Event::Key(_) | Event::Mouse(_) => EventResult::Consumed(None),
            _ => EventResult::Ignored(None),
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(self.id)
    }
}

//! Presentation data for a code review buffer. Its text contains only headings and code;
//! line identities and syntax belong to the two sides of each file, not to the patch.
use std::ops::Range;
use std::path::PathBuf;

use helix_core::syntax::{Highlight, HighlightEvent, Loader, OverlayHighlights, Syntax};
use helix_core::Rope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Header,
    Separator,
    Note,
    Context,
    Added,
    Removed,
}

#[derive(Debug)]
pub struct ReviewLine {
    pub kind: LineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    /// Source index and line within that side's code, before display interleaving.
    pub source: Option<(usize, usize)>,
}

pub struct ReviewSource {
    pub path: PathBuf,
    pub text: Rope,
    pub syntax: Option<Syntax>,
}

#[derive(Default)]
pub struct Review {
    pub lines: Vec<ReviewLine>,
    pub sources: Vec<ReviewSource>,
    pub digits: usize,
}

/// A code location survives switching between changed excerpts and complete files.
#[derive(Clone)]
pub struct ReviewAnchor {
    path: PathBuf,
    old: Option<usize>,
    new: Option<usize>,
    kind: LineKind,
}

impl Review {
    pub fn anchor(&self, row: usize) -> Option<ReviewAnchor> {
        let line = self.lines.get(row)?;
        let line = if line.source.is_some() {
            line
        } else {
            let following = self
                .lines
                .iter()
                .skip(row + 1)
                .take_while(|line| line.kind != LineKind::Header);
            following
                .clone()
                .find(|line| matches!(line.kind, LineKind::Added | LineKind::Removed))
                .or_else(|| following.clone().find(|line| line.source.is_some()))?
        };
        let (source, _) = line.source?;
        Some(ReviewAnchor {
            path: self.sources[source].path.clone(),
            old: line.old,
            new: line.new,
            kind: line.kind,
        })
    }

    pub fn find_anchor(&self, anchor: &ReviewAnchor) -> Option<usize> {
        let in_file = |line: &ReviewLine| {
            line.source
                .is_some_and(|(source, _)| self.sources[source].path == anchor.path)
        };
        let exact = self.lines.iter().position(|line| {
            line.kind == anchor.kind
                && line.old == anchor.old
                && line.new == anchor.new
                && in_file(line)
        });
        exact.or_else(|| {
            self.lines
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    in_file(line) && matches!(line.kind, LineKind::Added | LineKind::Removed)
                })
                .min_by_key(
                    |(_, line)| match (anchor.new, line.new, anchor.old, line.old) {
                        (Some(a), Some(b), _, _) | (_, _, Some(a), Some(b)) => a.abs_diff(b),
                        _ => usize::MAX,
                    },
                )
                .map(|(index, _)| index)
        })
    }

    pub fn prepare_syntax(&mut self, loader: &Loader) {
        for source in &mut self.sources {
            let Some(language) = loader.language_for_filename(&source.path) else {
                continue;
            };
            match Syntax::new(source.text.slice(..), language, loader) {
                Ok(syntax) => source.syntax = Some(syntax),
                Err(err) => log::warn!("Review syntax for {}: {err}", source.path.display()),
            }
        }
    }

    /// Highlight only visible code. Keep highlight layers separate so injected languages
    /// and nested captures retain the same style composition as ordinary editor buffers.
    pub fn highlights(
        &self,
        text: &Rope,
        lines: Range<usize>,
        loader: &Loader,
    ) -> Vec<OverlayHighlights> {
        let mut layers: Vec<Vec<(Highlight, Range<usize>)>> = Vec::new();
        for line in lines {
            let Some((source, source_line)) = self.lines.get(line).and_then(|line| line.source)
            else {
                continue;
            };
            let source = &self.sources[source];
            let Some(syntax) = &source.syntax else {
                continue;
            };
            let start = source.text.line_to_byte(source_line);
            let end = source.text.line_to_byte(source_line + 1);
            let source_char = source.text.line_to_char(source_line);
            let display_char = text.line_to_char(line);
            let slice = source.text.slice(..);
            let mut highlighter = syntax.highlighter(slice, loader, start as u32..end as u32);
            let mut active = Vec::new();
            let mut pos = start;
            while pos < end {
                while highlighter.next_event_offset() as usize <= pos {
                    let (event, highlights) = highlighter.advance();
                    if event == HighlightEvent::Refresh {
                        active.clear();
                    }
                    active.extend(highlights);
                }
                let next = (highlighter.next_event_offset() as usize).min(end);
                let from = display_char + source.text.byte_to_char(pos) - source_char;
                let to = display_char + source.text.byte_to_char(next) - source_char;
                for (depth, highlight) in active.iter().enumerate() {
                    if layers.len() <= depth {
                        layers.push(Vec::new());
                    }
                    layers[depth].push((*highlight, from..to));
                }
                pos = next;
            }
        }
        layers
            .into_iter()
            .map(|highlights| OverlayHighlights::Heterogenous { highlights })
            .collect()
    }
}

/// Review backgrounds use the theme's code colours with a light tint, leaving syntax
/// foregrounds free to describe the language. Themes can override each band explicitly.
pub fn line_style(kind: LineKind, theme: &crate::Theme) -> crate::graphics::Style {
    use crate::graphics::{Color, Modifier, Style};
    match kind {
        LineKind::Header => theme
            .try_get_exact("ui.diff.header")
            .unwrap_or_else(|| theme.get("ui.statusline"))
            .add_modifier(Modifier::BOLD),
        LineKind::Separator | LineKind::Note => theme.get("ui.text.inactive"),
        LineKind::Context => Style::default(),
        LineKind::Added | LineKind::Removed => {
            let (scope, fallback) = if kind == LineKind::Added {
                ("ui.diff.added", "diff.plus")
            } else {
                ("ui.diff.removed", "diff.minus")
            };
            if let Some(style) = theme.try_get_exact(scope) {
                return style;
            }
            let change = theme.get(fallback);
            if let Some(bg) = change.bg {
                return Style::default().bg(bg);
            }
            let base = theme.get("ui.background").bg;
            let bg = match (base, change.fg) {
                (Some(Color::Rgb(r, g, b)), Some(Color::Rgb(cr, cg, cb))) => {
                    let tint = |base: u8, accent: u8| ((base as u16 * 7 + accent as u16) / 8) as u8;
                    Some(Color::Rgb(tint(r, cr), tint(g, cg), tint(b, cb)))
                }
                // ANSI themes own their terminal palette. Honour its selection background
                // instead of guessing whether the terminal's default is light or dark.
                _ => theme.get("ui.selection").bg,
            };
            Style {
                bg,
                ..Style::default()
            }
        }
    }
}

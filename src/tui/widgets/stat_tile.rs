//! A single headline figure in a bordered box.
//!
//! The dashboard's top row is six of these. Making them a widget rather than
//! six hand-laid-out blocks buys the thing that matters in a grid: every tile
//! puts its label, its value and its footnote on the same rows, so the eye can
//! scan across the row without re-finding the baseline each time.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};

use crate::tui::theme::Theme;

/// A labelled metric with an optional second line of context.
pub struct StatTile<'a> {
    icon: &'a str,
    label: &'a str,
    value: String,
    footnote: Option<String>,
    accent: Color,
    /// Draws the border in the accent colour, for the tile that most wants
    /// attention.
    emphasised: bool,
}

impl<'a> StatTile<'a> {
    /// A tile showing `value` under `label`.
    #[must_use]
    pub fn new(icon: &'a str, label: &'a str, value: impl Into<String>) -> Self {
        Self {
            icon,
            label,
            value: value.into(),
            footnote: None,
            accent: Theme::CYAN,
            emphasised: false,
        }
    }

    /// Sets the colour of the value and, when emphasised, the border.
    #[must_use]
    pub const fn accent(mut self, accent: Color) -> Self {
        self.accent = accent;
        self
    }

    /// Adds a second, quieter line: a rate, a share, a comparison.
    #[must_use]
    pub fn footnote(mut self, footnote: impl Into<String>) -> Self {
        self.footnote = Some(footnote.into());
        self
    }

    /// Draws the border in the accent colour.
    #[must_use]
    pub const fn emphasised(mut self, emphasised: bool) -> Self {
        self.emphasised = emphasised;
        self
    }
}

impl Widget for StatTile<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 6 {
            // Too small to be honest in. Drawing a clipped tile would show a
            // number with its units cut off, which is worse than showing
            // nothing at all.
            return;
        }

        let border = if self.emphasised {
            self.accent
        } else {
            Theme::BORDER
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .style(Style::default().bg(Theme::SURFACE))
            .padding(Padding::horizontal(1))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", self.icon), Style::default().fg(self.accent)),
                Span::styled(self.label, Theme::label()),
                Span::raw(" "),
            ]));

        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = vec![Line::from(Span::styled(
            self.value,
            Style::default()
                .fg(self.accent)
                .add_modifier(Modifier::BOLD),
        ))];
        if let Some(footnote) = self.footnote {
            lines.push(Line::from(Span::styled(
                footnote,
                Style::default().fg(Theme::MUTED),
            )));
        }

        Paragraph::new(lines)
            .alignment(Alignment::Left)
            .render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(tile: StatTile<'_>, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        tile.render(area, &mut buf);
        buf
    }

    fn contains(buf: &Buffer, needle: &str) -> bool {
        (0..buf.area.height).any(|y| {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_owned())
                .collect();
            row.contains(needle)
        })
    }

    #[test]
    fn the_value_and_the_footnote_both_appear() {
        let buf = render(
            StatTile::new("$", "COST", "$12.34").footnote("$1.20/turn"),
            24,
            5,
        );
        assert!(contains(&buf, "$12.34"));
        assert!(contains(&buf, "$1.20/turn"));
    }

    #[test]
    fn a_tile_too_small_to_be_honest_in_draws_nothing() {
        // A clipped number reads as a different, wrong number.
        let buf = render(StatTile::new("$", "COST", "$12.34"), 4, 2);
        assert!(!contains(&buf, "$12"));
    }
}

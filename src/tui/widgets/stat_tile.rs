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

use crate::tui::format;
use crate::tui::palette::Palette;

/// A labelled metric with an optional second line of context.
pub struct StatTile<'a> {
    icon: &'a str,
    label: &'a str,
    value: String,
    footnote: Option<String>,
    /// The value's and, when emphasised, the border's colour. `None` falls
    /// back to `palette.accent_primary` at render time, so a tile that never
    /// calls [`StatTile::accent`] still gets a sensible colour without the
    /// builder needing a palette in hand before the theme is even known.
    accent: Option<Color>,
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
            accent: None,
            emphasised: false,
        }
    }

    /// Sets the colour of the value and, when emphasised, the border.
    #[must_use]
    pub const fn accent(mut self, accent: Color) -> Self {
        self.accent = Some(accent);
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

impl StatTile<'_> {
    /// Draws the tile, or nothing at all if `area` is too small to be honest
    /// in.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        if area.height < 3 || area.width < 6 {
            // Too small to be honest in. Drawing a clipped tile would show a
            // number with its units cut off, which is worse than showing
            // nothing at all.
            return;
        }

        let accent = self.accent.unwrap_or_else(|| palette.accent_primary.into());
        let border = if self.emphasised {
            accent
        } else {
            palette.border.into()
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .style(Style::default().bg(palette.surface.into()))
            .padding(Padding::horizontal(1))
            .title(Line::from(vec![
                Span::styled(format!(" {} ", self.icon), Style::default().fg(accent)),
                Span::styled(self.label, palette.label()),
                Span::raw(" "),
            ]));

        let inner = block.inner(area);
        block.render(area, buf);

        // `Paragraph` has no wrapping turned on here (a headline figure
        // wrapping onto a second row would collide with the footnote below
        // it), and without wrapping ratatui clips a line that overruns
        // `inner.width` by silently dropping whatever falls past the right
        // edge -- for a span of digits, that turns "$1234.56" into "$1" at
        // this tile's own registered minimum width, a wrong number with
        // nothing to say it is wrong. `format::fit` trims to the same width
        // itself, but leaves an ellipsis behind so a reader sees a figure
        // was cut rather than trusting a truncated one, the same convention
        // `TopProjects` already applies to a project name too long for its
        // column.
        let value = format::fit(&self.value, inner.width as usize, false);
        let mut lines = vec![Line::from(Span::styled(
            value,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))];
        if let Some(footnote) = self.footnote {
            let footnote = format::fit(&footnote, inner.width as usize, false);
            lines.push(Line::from(Span::styled(
                footnote,
                Style::default().fg(palette.muted.into()),
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
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    fn render(tile: StatTile<'_>, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        tile.render(area, &mut buf, &palette());
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

    #[test]
    fn a_value_too_wide_for_the_tile_is_marked_cut_rather_than_silently_shortened() {
        // Six columns is `StatTile`'s own registered floor (this exact width
        // is `panel.tile-row`'s registered minimum divided across six
        // tiles), which leaves only two columns for the value once the
        // border and horizontal padding are subtracted. Before this test
        // was written, an unwrapped `Paragraph` simply dropped whatever fell
        // past the right edge, so "$1234.56" rendered as "$1" here -- a
        // real number, just the wrong one, with nothing on screen to say it
        // had been cut. An ellipsis in its place is the honest version of
        // the same truncation.
        let buf = render(StatTile::new("¤", "COST", "$1234.56"), 6, 3);
        assert!(
            !contains(&buf, "$1 "),
            "a bare wrong number must not appear"
        );
        assert!(contains(&buf, "\u{2026}"), "an ellipsis marks the cut");
    }
}

//! The model-breakdown panel: what each model contributed to the total,
//! dearest first.
//!
//! [`crate::view::dashboard_view::ModelBreakdownView`] already holds the rows
//! sorted dearest-first, the same order
//! [`crate::domain::report::UsageRow::breakdown`] keeps for exactly the same
//! reason -- the model spending the most money is the one worth seeing
//! without scrolling.
//!
//! This is deliberately a bar *list*, not a pie chart. [`TokenMix`] can afford
//! a pie because it always has exactly four slices; the number of models a
//! session has touched is unbounded, and a pie with a dozen slivers around
//! its edge is unreadable in a way a dozen ranked rows is not. Each row's bar
//! is coloured from [`Palette::chart_series`] cycling by rank, so a model
//! keeps roughly the colour it had last time it was drawn even as the exact
//! set of models on screen changes.
//!
//! [`TokenMix`]: crate::tui::widgets::token_mix::TokenMix

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};

use crate::domain::money::Usd;
use crate::tui::format;
use crate::tui::icons::EIGHTHS;
use crate::tui::palette::Palette;

/// How many characters the cost column gets -- wide enough for `$1234.56`,
/// the same figure [`SpendPanel`](crate::tui::widgets::spend_panel::SpendPanel)
/// budgets for its own project rows.
const COST_COLUMN_WIDTH: usize = 8;

/// The most a model's name is ever given, even on a generously wide panel --
/// wide enough for a full model id such as `claude-sonnet-5` without
/// truncating it. Below this the name column shrinks with the panel instead
/// of staying fixed, which is what keeps the cost column from being pushed
/// out of a narrow panel entirely -- a fixed name column wide enough for the
/// panel's *widest* realistic name left no room at all for the bar or the
/// cost at this panel's own registered minimum width.
const NAME_COLUMN_MAX_WIDTH: usize = 20;

/// A ranked bar list of what each model cost.
pub struct ModelBreakdown<'a> {
    rows: &'a [(String, Usd)],
}

impl<'a> ModelBreakdown<'a> {
    /// A breakdown over `rows`, already sorted dearest first.
    #[must_use]
    pub const fn new(rows: &'a [(String, Usd)]) -> Self {
        Self { rows }
    }
}

impl ModelBreakdown<'_> {
    /// Draws the panel: one row per model, its bar proportional to its share
    /// of the total across every row shown, its cost right-aligned.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        if area.is_empty() {
            return;
        }
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border.into()))
            .style(Style::default().bg(palette.surface.into()))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " model breakdown ",
                palette.title(palette.accent_secondary.into()),
            ));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 || self.rows.is_empty() {
            return;
        }

        let total: f64 = self.rows.iter().map(|(_, cost)| cost.dollars()).sum();
        let total = total.max(f64::EPSILON);

        // The name and the bar share whatever is left once the cost column
        // and the two single-space gaps either side of the bar are paid for;
        // the name takes up to its own cap and the bar gets the remainder,
        // which is `0` rather than negative on a panel too narrow to spare
        // any -- see `bar`'s own doc for why a zero-width bar is still safe
        // to draw.
        let shared = (inner.width as usize).saturating_sub(COST_COLUMN_WIDTH + 2);
        let name_column = shared.min(NAME_COLUMN_MAX_WIDTH);
        let bar_width = shared.saturating_sub(name_column);

        let series = palette.chart_series();
        let lines: Vec<Line> = self
            .rows
            .iter()
            .take(inner.height as usize)
            .enumerate()
            .map(|(index, (name, cost))| {
                let colour = series[index % series.len()];
                let share = (cost.dollars() / total).clamp(0.0, 1.0);
                let name_text = format::fit(name, name_column, false);
                Line::from(vec![
                    Span::styled(
                        format!("{name_text:<name_column$} "),
                        Style::default().fg(palette.text.into()),
                    ),
                    Span::styled(bar(share, bar_width), Style::default().fg(colour)),
                    Span::styled(
                        format!(" {:>COST_COLUMN_WIDTH$}", format!("{cost}")),
                        Style::default().fg(colour),
                    ),
                ])
            })
            .collect();

        Paragraph::new(lines).render(inner, buf);
    }
}

/// A partial-block bar `width` cells wide, `share` of it filled -- the same
/// eighths-of-a-cell technique
/// [`usage_windows`](crate::tui::widgets::usage_windows)'s own `bar` helper
/// uses, reimplemented here rather than shared because that one is private to
/// its module and takes an `Option<f64>` this panel has no use for -- every
/// row here always has a share of a positive total to draw.
fn bar(share: f64, width: usize) -> String {
    let eighths = (share.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    (0..width)
        .map(|cell| {
            let here = eighths.saturating_sub(cell * 8).min(8);
            if here == 0 {
                ' '
            } else {
                EIGHTHS[here - 1].chars().next().expect("one character")
            }
        })
        .collect()
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

    fn render(rows: &[(String, Usd)], width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        ModelBreakdown::new(rows).render(area, &mut buf, &palette());
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn rows() -> Vec<(String, Usd)> {
        vec![
            ("claude-opus-5".to_owned(), Usd::new(9.0)),
            ("claude-sonnet-5".to_owned(), Usd::new(1.0)),
        ]
    }

    #[test]
    fn every_model_is_named_with_its_cost() {
        let out = render(&rows(), 40, 6);
        assert!(out.contains("claude-opus-5"), "{out}");
        assert!(out.contains("$9.00"), "{out}");
        assert!(out.contains("claude-sonnet-5"), "{out}");
        assert!(out.contains("$1.00"), "{out}");
    }

    #[test]
    fn the_dearest_model_keeps_the_first_row() {
        let out = render(&rows(), 40, 6);
        let opus_row = out.lines().find(|l| l.contains("claude-opus-5"));
        let sonnet_row = out.lines().find(|l| l.contains("claude-sonnet-5"));
        assert!(
            opus_row.map(|_| ()).is_some() && sonnet_row.is_some(),
            "both rows are drawn: {out}"
        );
        assert_eq!(
            out.lines().position(|l| l.contains("claude-opus-5")),
            Some(1),
            "the dearest model is not pushed below the cheaper one: {out}"
        );
    }

    #[test]
    fn a_models_bar_is_longer_the_bigger_its_share_of_the_total() {
        // Nine dollars against ten total is 90% of the bar; one dollar
        // against the same total is 10%.
        let out = render(&rows(), 60, 6);
        let opus_row = out
            .lines()
            .find(|l| l.contains("claude-opus-5"))
            .expect("a row");
        let sonnet_row = out
            .lines()
            .find(|l| l.contains("claude-sonnet-5"))
            .expect("a row");
        let opus_fill = opus_row.chars().filter(|&c| c != ' ').count();
        let sonnet_fill = sonnet_row.chars().filter(|&c| c != ' ').count();
        assert!(
            opus_fill > sonnet_fill,
            "opus row: {opus_row:?}, sonnet row: {sonnet_row:?}"
        );
    }

    #[test]
    fn an_empty_breakdown_draws_only_the_empty_frame() {
        let out = render(&[], 40, 6);
        assert!(!out.contains('$'));
    }

    #[test]
    fn drawing_into_a_tiny_or_empty_area_does_not_panic() {
        for (w, h) in [(0, 0), (1, 1), (4, 2), (24, 6)] {
            let _ = render(&rows(), w, h);
        }
    }
}

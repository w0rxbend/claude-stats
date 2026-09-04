//! The top-projects panel: the busiest projects inside the last seven days,
//! on their own.
//!
//! [`SpendPanel`] already prints this same list, but only after its own
//! today/block section -- useful when the reader wants the whole spend
//! picture in one panel, wasted space when a layout has already given today
//! and the active block their own panels elsewhere. This widget is the same
//! rows with none of that context: a name, a cost, nothing above them.
//!
//! [`SpendPanel`]: crate::tui::widgets::spend_panel::SpendPanel

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};

use crate::domain::money::Usd;
use crate::tui::format;
use crate::tui::palette::Palette;

/// How many characters the cost column gets -- wide enough for `$1234.56`,
/// the same figure [`SpendPanel`](crate::tui::widgets::spend_panel::SpendPanel)
/// budgets for these same rows.
const COST_COLUMN_WIDTH: usize = 8;

/// A plain, headerless list of the busiest projects.
pub struct TopProjects<'a> {
    rows: &'a [(String, Usd)],
}

impl<'a> TopProjects<'a> {
    /// A list over `rows`, already sorted dearest first and already capped
    /// at [`crate::view::dashboard_view::TopProjectsView`]'s own limit.
    #[must_use]
    pub const fn new(rows: &'a [(String, Usd)]) -> Self {
        Self { rows }
    }
}

impl TopProjects<'_> {
    /// Draws the panel: one row per project, name left, cost right-aligned.
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
                " top projects ",
                palette.title(palette.accent_primary.into()),
            ));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 || self.rows.is_empty() {
            return;
        }

        let cost_column = COST_COLUMN_WIDTH;
        let name_column = (inner.width as usize)
            .saturating_sub(cost_column + 1)
            .max(1);
        let lines: Vec<Line> = self
            .rows
            .iter()
            .take(inner.height as usize)
            .map(|(name, cost)| {
                let name_text = format::fit(name, name_column, false);
                Line::from(vec![
                    Span::styled(
                        format!("{name_text:<name_column$}"),
                        Style::default().fg(palette.text.into()),
                    ),
                    Span::styled(
                        format!("{:>cost_column$}", format!("{cost}")),
                        Style::default().fg(palette.accent_primary.into()),
                    ),
                ])
            })
            .collect();

        Paragraph::new(lines).render(inner, buf);
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

    fn render(rows: &[(String, Usd)], width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        TopProjects::new(rows).render(area, &mut buf, &palette());
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_project_is_named_with_its_cost_right_aligned() {
        let rows = vec![
            ("api".to_owned(), Usd::new(3.0)),
            ("web".to_owned(), Usd::new(1.5)),
        ];
        let out = render(&rows, 30, 6);
        assert!(out.contains("api"), "{out}");
        assert!(out.contains("$3.00"), "{out}");
        assert!(out.contains("web"), "{out}");
        assert!(out.contains("$1.50"), "{out}");
    }

    #[test]
    fn no_today_or_block_context_is_printed_here() {
        // This is what distinguishes the standalone panel from SpendPanel,
        // which prints the same project rows but only beneath a today/block
        // section this widget never draws.
        let rows = vec![("api".to_owned(), Usd::new(3.0))];
        let out = render(&rows, 30, 6);
        assert!(!out.contains("today"), "{out}");
        assert!(!out.contains("block"), "{out}");
    }

    #[test]
    fn a_long_project_name_is_truncated_rather_than_wrapped() {
        let rows = vec![(
            "a-very-long-working-directory-name-indeed".to_owned(),
            Usd::new(1.0),
        )];
        let out = render(&rows, 24, 6);
        assert!(
            out.contains('\u{2026}'),
            "the long name is cut with an ellipsis: {out}"
        );
    }

    #[test]
    fn an_empty_list_draws_only_the_empty_frame() {
        let out = render(&[], 24, 6);
        assert!(!out.contains('$'));
    }

    #[test]
    fn drawing_into_a_tiny_or_empty_area_does_not_panic() {
        let rows = vec![("api".to_owned(), Usd::new(3.0))];
        for (w, h) in [(0, 0), (1, 1), (4, 2), (24, 6)] {
            let _ = render(&rows, w, h);
        }
    }
}

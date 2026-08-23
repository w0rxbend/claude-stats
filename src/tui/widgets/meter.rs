//! A one-line labelled bar: `label ▓▓▓▓░░░░ value`.
//!
//! Used for the readings that are proportions rather than headline figures --
//! cache hit ratio, context efficiency, how far through the current segment
//! the session is. A number alone makes the reader do the comparison; a short
//! bar beside it does the comparison for them, and costs one line.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::icons::{EIGHTHS, Icon};
use crate::tui::theme::Theme;

/// Builds the `Line` for one labelled meter.
///
/// Returns a `Line` rather than being a `Widget`, because these are always
/// stacked inside a `Paragraph` alongside plain text rows. Making it a widget
/// would force the caller to lay out one `Rect` per line for no benefit.
#[must_use]
pub fn meter_line(
    label: &str,
    ratio: f64,
    value: String,
    accent: Color,
    bar_width: usize,
) -> Line<'_> {
    let ratio = ratio.clamp(0.0, 1.0);
    let eighths = (ratio * (bar_width * 8) as f64).round() as usize;

    let bar: String = (0..bar_width)
        .map(|cell| {
            let here = eighths.saturating_sub(cell * 8).min(8);
            if here == 0 {
                Icon::BAR_EMPTY
            } else {
                EIGHTHS[here - 1]
            }
        })
        .collect();

    Line::from(vec![
        // Eleven columns, because the longest label in use is "efficiency"
        // at ten and a bar that starts flush against its label reads as one
        // run-on word.
        Span::styled(format!("{label:<11}"), Theme::label()),
        Span::styled(bar, Style::default().fg(accent)),
        Span::raw(" "),
        Span::styled(
            value,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_column_is_wide_enough_for_the_longest_label_plus_a_gap() {
        let line = meter_line("efficiency", 1.0, "100%".to_owned(), Theme::MINT, 4);
        let label = line.spans[0].content.to_string();
        assert!(label.ends_with(' '), "{label:?} must not run into the bar");
    }

    fn bar_of(line: &Line<'_>) -> String {
        line.spans[1].content.to_string()
    }

    #[test]
    fn a_full_meter_is_solid_and_an_empty_one_is_all_track() {
        let full = meter_line("cache", 1.0, "100%".to_owned(), Theme::CYAN, 8);
        assert_eq!(bar_of(&full), "\u{2588}".repeat(8));

        let empty = meter_line("cache", 0.0, "0%".to_owned(), Theme::CYAN, 8);
        assert_eq!(bar_of(&empty), "\u{2591}".repeat(8));
    }

    #[test]
    fn a_ratio_outside_the_range_is_clamped_rather_than_overflowing_the_bar() {
        let over = meter_line("x", 3.0, "300%".to_owned(), Theme::CYAN, 4);
        assert_eq!(bar_of(&over).chars().count(), 4);
    }

    #[test]
    fn the_bar_uses_partial_blocks_so_it_moves_smoothly() {
        // Half of one cell should be a half block, not a jump to a full one.
        let half = meter_line("x", 0.5, "50%".to_owned(), Theme::CYAN, 1);
        assert_eq!(bar_of(&half), "\u{258c}");
    }
}

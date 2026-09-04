//! The daily-spend chart: what each of the last few calendar days cost, as a
//! bar per day.
//!
//! Every other spend figure on the dashboard is a total -- today's spend, a
//! window's spend, a project's spend -- and a total cannot say whether
//! yesterday was quiet or whether last Tuesday was the expensive one. A bar
//! per day answers that at a glance, the same way
//! [`OutputSparkline`](crate::tui::widgets::sparkline::OutputSparkline)
//! answers it for output size per response: height read against the tallest
//! bar in the window, not against an absolute scale nobody has memorised.
//!
//! Unlike the sparkline this chart is not decimated. A sparkline can show a
//! hundred responses in twenty columns because collapsing several turns into
//! one bucket loses nothing a reader would have used; collapsing two separate
//! days into one bar would erase the one thing this chart exists to show. So
//! when there is not room for every day, the chart shows only the most recent
//! ones that fit -- one column wide at minimum -- rather than squeezing every
//! day into a narrower column that stopped being readable.

use chrono::NaiveDate;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::domain::money::Usd;
use crate::tui::icons::SPARK_LEVELS;
use crate::tui::palette::Palette;

/// How many columns a weekday abbreviation needs -- `"Mon"`, `"Tue"`, and so
/// on are always three characters in the `%a` format `render` prints them in.
const LABEL_WIDTH: u16 = 3;

/// A bar chart of daily spend, one bar per calendar day.
pub struct DailySpendChart<'a> {
    days: &'a [(NaiveDate, Usd)],
}

impl<'a> DailySpendChart<'a> {
    /// A chart over `days`, oldest first -- the same order
    /// [`crate::view::dashboard_view::DailySpendView::days`] carries them in.
    #[must_use]
    pub const fn new(days: &'a [(NaiveDate, Usd)]) -> Self {
        Self { days }
    }
}

impl DailySpendChart<'_> {
    /// Draws the chart: the most recent days that fit, tallest bar reaching
    /// the top of the area, with the weekday printed beneath a bar wide
    /// enough to hold it.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        if area.is_empty() || self.days.is_empty() {
            return;
        }

        // The bottom row is given to the weekday label whenever there is a
        // second row to spare; a one-row area draws bars alone rather than
        // giving up the only row it has.
        let label_rows = u16::from(area.height > 1);
        let bar_rows = area.height - label_rows;
        if bar_rows == 0 {
            return;
        }

        let shown_count = self.days.len().min(area.width as usize);
        if shown_count == 0 {
            return;
        }
        // Right-aligned on the most recent days -- the same convention
        // `OutputSparkline` uses -- with any width beyond one column per day
        // spent widening every bar evenly rather than left as a gap.
        let shown = &self.days[self.days.len() - shown_count..];
        let col_width = (area.width / shown_count as u16).max(1);
        let used_width = col_width * shown_count as u16;
        let offset = area.width - used_width;

        let peak = shown
            .iter()
            .map(|(_, cost)| cost.dollars())
            .fold(0.0_f64, f64::max)
            .max(f64::EPSILON);
        let show_labels = label_rows > 0 && col_width >= LABEL_WIDTH;
        let bar_style = Style::default().fg(palette.accent_primary.into());
        let label_style = Style::default().fg(palette.muted.into());

        for (index, (date, cost)) in shown.iter().enumerate() {
            let x = area.x + offset + col_width * index as u16;
            let share = (cost.dollars() / peak).clamp(0.0, 1.0);
            // At least one eighth for any day that actually spent something,
            // for the same reason the sparkline floors a non-zero sample: a
            // day that cost a cent must not read as a day that cost nothing.
            let eighths = if cost.dollars() > 0.0 {
                ((share * (bar_rows as usize * 8) as f64).round() as usize).max(1)
            } else {
                0
            };
            draw_bar(buf, x, col_width, area.y, bar_rows, eighths, bar_style);

            if show_labels {
                let label = format!("{:^width$}", date.format("%a"), width = col_width as usize);
                buf.set_string(x, area.y + bar_rows, label, label_style);
            }
        }
    }
}

/// Fills a `width`-wide bar from the bottom of its `rows`-tall column, using
/// `eighths` eighths of a row for sub-cell precision -- the same technique
/// `OutputSparkline`'s own `draw_column` uses, widened to cover every column
/// of a multi-cell-wide bar rather than just one.
fn draw_bar(
    buf: &mut Buffer,
    x: u16,
    width: u16,
    top: u16,
    rows: u16,
    eighths: usize,
    style: Style,
) {
    for row in 0..rows {
        let from_bottom = (rows - 1 - row) as usize;
        let here = eighths.saturating_sub(from_bottom * 8).min(8);
        if here == 0 {
            continue;
        }
        let glyph = SPARK_LEVELS[here - 1];
        for col in 0..width {
            buf.set_string(x + col, top + row, glyph, style);
        }
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

    fn day(ymd: (i32, u32, u32), dollars: f64) -> (NaiveDate, Usd) {
        (
            NaiveDate::from_ymd_opt(ymd.0, ymd.1, ymd.2).expect("a valid date"),
            Usd::new(dollars),
        )
    }

    fn render_rows(days: &[(NaiveDate, Usd)], width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        DailySpendChart::new(days).render(area, &mut buf, &palette());
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_busiest_day_reaches_the_top_of_the_chart() {
        let days = [
            day((2026, 8, 30), 1.0),
            day((2026, 8, 31), 5.0),
            day((2026, 9, 1), 10.0),
        ];
        let rows = render_rows(&days, 3, 4);
        assert_eq!(
            rows[0].chars().nth(2),
            Some('\u{2588}'),
            "the tallest day's column reaches row 0: {rows:?}"
        );
    }

    #[test]
    fn a_quiet_day_only_fills_the_bottom_of_a_tall_chart() {
        let days = [day((2026, 8, 31), 1.0), day((2026, 9, 1), 100.0)];
        let rows = render_rows(&days, 2, 5);
        assert_eq!(
            rows[0].chars().next(),
            Some(' '),
            "the quiet day's column stays empty at the top: {rows:?}"
        );
    }

    #[test]
    fn a_day_that_spent_something_never_renders_as_empty() {
        let days = [day((2026, 8, 31), 0.01), day((2026, 9, 1), 1_000.0)];
        let rows = render_rows(&days, 2, 4);
        // Row 3 is the weekday-label row at this height, not a bar row; the
        // bottommost *bar* row is row 2.
        assert_ne!(
            rows[2].chars().next(),
            Some(' '),
            "a cent of spend still draws a floor: {rows:?}"
        );
    }

    #[test]
    fn the_weekday_is_printed_beneath_a_wide_enough_bar() {
        let days = [day((2026, 8, 31), 3.0)];
        let rows = render_rows(&days, 5, 4);
        assert!(
            rows[3].contains("Mon"),
            "31 August 2026 is a Monday: {rows:?}"
        );
    }

    #[test]
    fn only_the_most_recent_days_that_fit_are_shown() {
        let days = [
            day((2026, 8, 28), 1.0),
            day((2026, 8, 29), 2.0),
            day((2026, 8, 30), 3.0),
            day((2026, 8, 31), 4.0),
            day((2026, 9, 1), 5.0),
        ];
        let rows = render_rows(&days, 2, 4);
        // Only two columns of width: the two most recent days survive, the
        // three older ones do not push the chart wider than it was given.
        assert!(rows.iter().all(|r| r.chars().count() == 2));
    }

    #[test]
    fn drawing_into_a_tiny_or_empty_area_does_not_panic() {
        let days = [day((2026, 9, 1), 3.0)];
        for (w, h) in [(0, 0), (1, 1), (1, 0), (0, 5)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), h.max(1)));
            DailySpendChart::new(&days).render(area, &mut buf, &palette());
        }
        // An empty series must not panic either -- this is what the panel
        // renderer falls back to when the view model carries no reading yet.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        DailySpendChart::new(&[]).render(Rect::new(0, 0, 10, 4), &mut buf, &palette());
    }
}

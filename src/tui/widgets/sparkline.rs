//! The output-per-response sparkline, with compaction boundaries marked.
//!
//! Ratatui's own `Sparkline` draws the bars; what it cannot do is say *why*
//! the line just fell off a cliff. A compaction resets the conversation, so
//! the response after one is short and the trace drops. Without a marker that
//! reads as "something broke". With one it reads as "the context was
//! compacted here", which is the actual story.
//!
//! When there are more samples than columns the series is decimated by taking
//! the *maximum* of each bucket rather than the mean. Averaging flattens the
//! spikes, and on a chart whose job is to show where the expensive turns were,
//! the spikes are the entire signal.
//!
//! The chart grows to fill whatever height it is given, drawing each column as
//! a stack of block characters from the bottom up. A one-row area still works
//! and looks like a conventional sparkline; taller areas simply get more
//! vertical resolution, which is what makes a quiet stretch distinguishable
//! from a flat one.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::tui::icons::{Icon, SPARK_LEVELS};
use crate::tui::theme::Theme;

/// A sparkline of output tokens per response.
pub struct OutputSparkline<'a> {
    values: &'a [u64],
    /// Indices into `values` where a compaction happened.
    compactions: &'a [usize],
}

impl<'a> OutputSparkline<'a> {
    /// A sparkline over `values`, marking the given compaction points.
    #[must_use]
    pub const fn new(values: &'a [u64], compactions: &'a [usize]) -> Self {
        Self {
            values,
            compactions,
        }
    }

    /// Reduces the series to at most `width` buckets, keeping peaks.
    fn decimate(&self, width: usize) -> Vec<Bucket> {
        if width == 0 || self.values.is_empty() {
            return Vec::new();
        }
        if self.values.len() <= width {
            return self
                .values
                .iter()
                .enumerate()
                .map(|(i, &value)| Bucket {
                    value,
                    compacted: self.compactions.contains(&i),
                })
                .collect();
        }

        let per_bucket = self.values.len().div_ceil(width);
        self.values
            .chunks(per_bucket)
            .enumerate()
            .map(|(bucket, chunk)| {
                let start = bucket * per_bucket;
                let end = start + chunk.len();
                Bucket {
                    value: chunk.iter().copied().max().unwrap_or(0),
                    compacted: self
                        .compactions
                        .iter()
                        .any(|&i| (start..end).contains(&i)),
                }
            })
            .collect()
    }
}

/// One drawn column.
struct Bucket {
    value: u64,
    compacted: bool,
}

impl Widget for OutputSparkline<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let buckets = self.decimate(area.width as usize);
        // Scaling against the tallest bar makes the chart self-normalising, so
        // a quiet stretch of small responses is still legible rather than a
        // flat line at the bottom.
        let peak = buckets.iter().map(|b| b.value).max().unwrap_or(0).max(1);

        // Right-aligned: the newest samples are the ones being watched, so
        // they stay pinned to the same edge as the series grows.
        let offset = area.width as usize - buckets.len();
        let rows = area.height as usize;

        for (index, bucket) in buckets.iter().enumerate() {
            let x = area.x + (offset + index) as u16;
            if bucket.compacted {
                draw_compaction_marker(buf, x, area);
                continue;
            }
            let share = bucket.value as f64 / peak as f64;
            let column_style = Style::default().fg(Theme::ramp(share));
            // At least one eighth for any non-zero sample: a turn that
            // produced output must not render as though it produced none.
            let eighths = if bucket.value == 0 {
                0
            } else {
                ((share * (rows * 8) as f64).round() as usize).max(1)
            };
            draw_column(buf, x, area, eighths, column_style);
        }
    }
}

/// Fills one column from the bottom up with `eighths` eighths of a row.
fn draw_column(buf: &mut Buffer, x: u16, area: Rect, eighths: usize, style: Style) {
    for row in 0..area.height {
        // Row 0 is the top of the area, so the bottom row is the last one.
        let from_bottom = (area.height - 1 - row) as usize;
        let here = eighths.saturating_sub(from_bottom * 8).min(8);
        if here == 0 {
            continue;
        }
        buf.set_string(x, area.y + row, SPARK_LEVELS[here - 1], style);
    }
}

/// Draws a compaction as a full-height rule with an arrow at the top.
///
/// Full height rather than a single glyph because a compaction is a boundary
/// between two segments of the chart, not a data point within one. Drawn as a
/// rule, the eye reads the columns either side of it as separate runs, which
/// is exactly what they are.
fn draw_compaction_marker(buf: &mut Buffer, x: u16, area: Rect) {
    let style = Style::default().fg(Theme::VIOLET);
    for row in 0..area.height {
        let glyph = if row == 0 { Icon::MARKER } else { "\u{2502}" };
        buf.set_string(x, area.y + row, glyph, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders into a one-row area and returns that row.
    fn render(values: &[u64], compactions: &[usize], width: u16) -> String {
        render_rows(values, compactions, width, 1).remove(0)
    }

    /// Renders into a `width` x `height` area and returns every row, top first.
    fn render_rows(
        values: &[u64],
        compactions: &[usize],
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        OutputSparkline::new(values, compactions).render(area, &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn a_tall_area_is_filled_from_the_bottom_up() {
        let rows = render_rows(&[10], &[], 1, 4);
        assert_eq!(rows[0], "\u{2588}", "the peak reaches the top row");
        assert_eq!(rows[3], "\u{2588}", "and is solid all the way down");
    }

    #[test]
    fn a_small_sample_occupies_only_the_bottom_of_a_tall_area() {
        let rows = render_rows(&[1, 100], &[], 2, 4);
        let short_column: Vec<char> = rows.iter().map(|r| r.chars().next().unwrap()).collect();
        assert_eq!(short_column[0], ' ', "the top row stays empty");
        assert_ne!(short_column[3], ' ', "the bottom row is drawn");
    }

    #[test]
    fn a_response_that_produced_output_never_renders_as_empty() {
        // One token against a peak of a million still rounds to zero eighths
        // without the floor, and a turn that did work would look like a turn
        // that did none.
        let rows = render_rows(&[1, 1_000_000], &[], 2, 4);
        assert_ne!(rows[3].chars().next(), Some(' '));
    }

    #[test]
    fn the_tallest_sample_reaches_the_top_of_the_chart() {
        let drawn = render(&[1, 5, 10], &[], 3);
        assert!(drawn.ends_with('\u{2588}'), "got {drawn:?}");
    }

    #[test]
    fn a_short_series_is_pinned_to_the_right_edge() {
        let drawn = render(&[10], &[], 5);
        assert!(drawn.starts_with("    "), "got {drawn:?}");
    }

    #[test]
    fn a_compaction_draws_a_full_height_rule_with_an_arrow_on_top() {
        let rows = render_rows(&[10, 10, 10], &[1], 3, 3);
        assert_eq!(rows[0].chars().nth(1), Some('\u{2193}'), "got {rows:?}");
        assert_eq!(rows[1].chars().nth(1), Some('\u{2502}'), "got {rows:?}");
        assert_eq!(rows[2].chars().nth(1), Some('\u{2502}'), "got {rows:?}");
    }

    #[test]
    fn decimation_keeps_the_peak_of_each_bucket_rather_than_its_average() {
        // Ten samples into two columns: each column must show its bucket's
        // maximum, so the second column is the tall one.
        let values: Vec<u64> = vec![1, 1, 1, 1, 1, 1, 1, 1, 1, 100];
        let drawn = render(&values, &[], 2);
        assert!(drawn.ends_with('\u{2588}'), "the spike must survive: {drawn:?}");
    }

    #[test]
    fn an_empty_series_draws_nothing_and_does_not_panic() {
        assert_eq!(render(&[], &[], 4).trim(), "");
    }
}

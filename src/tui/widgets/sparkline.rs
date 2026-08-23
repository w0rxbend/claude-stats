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

        for (index, bucket) in buckets.iter().enumerate() {
            let x = area.x + (offset + index) as u16;
            if bucket.compacted {
                buf.set_string(x, area.y, Icon::MARKER, Style::default().fg(Theme::VIOLET));
                continue;
            }
            if bucket.value == 0 {
                buf.set_string(x, area.y, " ", Style::default().fg(Theme::FAINT));
                continue;
            }
            let level = ((bucket.value as f64 / peak as f64) * SPARK_LEVELS.len() as f64).ceil()
                as usize;
            let level = level.clamp(1, SPARK_LEVELS.len());
            let shade = Theme::ramp(bucket.value as f64 / peak as f64);
            buf.set_string(
                x,
                area.y,
                SPARK_LEVELS[level - 1],
                Style::default().fg(shade),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(values: &[u64], compactions: &[usize], width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        OutputSparkline::new(values, compactions).render(area, &mut buf);
        (0..width).map(|x| buf[(x, 0)].symbol().to_owned()).collect()
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
    fn a_compaction_replaces_its_column_with_a_marker() {
        let drawn = render(&[10, 10, 10], &[1], 3);
        assert_eq!(drawn.chars().nth(1), Some('\u{2193}'), "got {drawn:?}");
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

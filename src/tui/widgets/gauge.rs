//! The context-fill gauge: the single most important thing on the dashboard.
//!
//! Ratatui ships a `Gauge`, and this is not it. Three things are needed that
//! the built-in one does not do, and each of them is the reason the bar exists
//! at all:
//!
//! 1. **A gradient along the length.** The colour at the leading edge tells
//!    you how full the window is before you have read the number beside it.
//! 2. **A compaction marker.** Compaction fires with head-room still free, so
//!    the interesting threshold is not the end of the bar. A tick shows where
//!    it actually is, and the bar past it is drawn dimmed to say "you will
//!    never get here".
//! 3. **Sub-cell precision.** A whole-cell bar on a 30-cell layout moves in 3%
//!    jumps, which looks broken next to a number that moves smoothly.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::domain::context::ContextFill;
use crate::tui::icons::{EIGHTHS, Icon};
use crate::tui::theme::Theme;

/// A horizontal bar showing how full the context window is.
#[derive(Debug, Clone, Copy)]
pub struct ContextGauge {
    fill: ContextFill,
}

impl ContextGauge {
    /// A gauge for the given fill reading.
    #[must_use]
    pub const fn new(fill: ContextFill) -> Self {
        Self { fill }
    }

    /// Where the compaction threshold sits along the bar, in `0.0..=1.0`.
    fn threshold_ratio(self) -> f64 {
        let window = self.fill.window();
        if window == 0 {
            return 1.0;
        }
        let threshold =
            window.saturating_sub(crate::domain::model::ModelCatalog::COMPACTION_BUFFER);
        threshold as f64 / window as f64
    }
}

impl Widget for ContextGauge {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let row = area.y;
        let width = area.width as usize;
        let filled_eighths = (self.fill.ratio() * (width * 8) as f64).round() as usize;
        let threshold_cell = (self.threshold_ratio() * width as f64).round() as usize;

        for cell in 0..width {
            let x = area.x + cell as u16;
            let position = if width > 1 {
                cell as f64 / (width - 1) as f64
            } else {
                0.0
            };
            let eighths_here = filled_eighths.saturating_sub(cell * 8).min(8);

            let (symbol, colour) = if eighths_here == 0 {
                // Past the compaction threshold the track is dimmer still:
                // that space exists in the window but will never be used.
                let track = if cell >= threshold_cell {
                    Theme::FAINT
                } else {
                    Theme::BORDER
                };
                (Icon::BAR_EMPTY, track)
            } else {
                (EIGHTHS[eighths_here - 1], Theme::ramp(position))
            };

            buf.set_string(x, row, symbol, Style::default().fg(colour));
        }

        // The threshold tick is drawn last so it survives on top of the fill.
        // Once the bar has reached it the tick would be redundant -- the bar
        // is already crimson -- so it is only drawn while it still warns of
        // something that has not happened yet.
        if threshold_cell < width && filled_eighths < threshold_cell * 8 {
            buf.set_string(
                area.x + threshold_cell as u16,
                row,
                Icon::MARKER,
                Style::default().fg(Theme::VIOLET),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use super::*;

    fn render(fill: ContextFill, width: u16) -> Buffer {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        ContextGauge::new(fill).render(area, &mut buf);
        buf
    }

    fn row(buf: &Buffer) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn an_empty_window_draws_only_track() {
        let buf = render(ContextFill::new(0, 200_000), 10);
        assert!(
            row(&buf)
                .chars()
                .all(|c| c == '\u{2591}' || c == '\u{2193}'),
            "expected track and a threshold tick, got {:?}",
            row(&buf)
        );
    }

    #[test]
    fn a_full_window_draws_only_solid_blocks() {
        let buf = render(ContextFill::new(200_000, 200_000), 10);
        assert_eq!(row(&buf), "\u{2588}".repeat(10));
    }

    #[test]
    fn the_threshold_tick_disappears_once_the_bar_has_passed_it() {
        // 200k window, 33k buffer: the threshold sits at 83.5%.
        let before = render(ContextFill::new(100_000, 200_000), 20);
        assert!(row(&before).contains('\u{2193}'), "tick should warn ahead");

        let after = render(ContextFill::new(190_000, 200_000), 20);
        assert!(
            !row(&after).contains('\u{2193}'),
            "tick is redundant once passed"
        );
    }

    #[test]
    fn a_zero_width_area_is_not_drawn_into() {
        let area = Rect::new(0, 0, 0, 1);
        let mut buf = Buffer::empty(area);
        ContextGauge::new(ContextFill::new(1, 2)).render(area, &mut buf);
    }
}

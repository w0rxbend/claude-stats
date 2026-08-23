//! An animated spinner, and the frame counter that drives every animation on
//! the dashboard.
//!
//! `ratatui-spinner` on crates.io is a name reservation with no code in it, so
//! this is the local replacement. It is deliberately tiny, and it does not own
//! a timer: animation phase is passed in from the one clock the application
//! already has. Widgets that keep their own timers drift apart from each other
//! within seconds, and the result looks broken in a way that is oddly hard to
//! diagnose.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// A cycle of frames to animate through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerStyle {
    /// A rotating braille dot. Smooth, and one cell wide in every font.
    Braille,
    /// A pulsing dot, for a "still alive" indicator that should not draw the
    /// eye the way a rotating one does.
    Pulse,
    /// A bar sweeping left and right, for longer waits.
    Sweep,
    /// A rotating quadrant, the heaviest of the four.
    Quadrant,
}

impl SpinnerStyle {
    /// The frames of this cycle.
    const fn frames(self) -> &'static [&'static str] {
        match self {
            Self::Braille => &[
                "\u{2801}", "\u{2802}", "\u{2804}", "\u{2840}", "\u{2880}", "\u{2820}", "\u{2810}",
                "\u{2808}",
            ],
            Self::Pulse => &["\u{00b7}", "\u{2022}", "\u{25cf}", "\u{2022}"],
            Self::Sweep => &["\u{2596}", "\u{2598}", "\u{259d}", "\u{2597}"],
            Self::Quadrant => &["\u{25f4}", "\u{25f5}", "\u{25f6}", "\u{25f7}"],
        }
    }

    /// The frame to draw at animation step `phase`.
    ///
    /// Takes the phase rather than reading a clock, so every spinner on screen
    /// advances on the same beat.
    #[must_use]
    pub const fn frame(self, phase: u64) -> &'static str {
        let frames = self.frames();
        frames[(phase as usize) % frames.len()]
    }
}

/// A one-cell spinner.
#[derive(Debug, Clone, Copy)]
pub struct Spinner {
    style: SpinnerStyle,
    phase: u64,
    text_style: Style,
}

impl Spinner {
    /// A spinner of the given style, at the given animation phase.
    #[must_use]
    pub const fn new(style: SpinnerStyle, phase: u64) -> Self {
        Self {
            style,
            phase,
            text_style: Style::new(),
        }
    }

    /// Sets the colour.
    #[must_use]
    pub const fn styled(mut self, text_style: Style) -> Self {
        self.text_style = text_style;
        self
    }

    /// The glyph this spinner would draw, for callers composing a `Line`
    /// rather than rendering a widget into an area of its own.
    #[must_use]
    pub const fn glyph(&self) -> &'static str {
        self.style.frame(self.phase)
    }
}

impl Widget for Spinner {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        buf.set_string(area.x, area.y, self.glyph(), self.text_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_cycle_wraps_instead_of_running_off_the_end() {
        let style = SpinnerStyle::Braille;
        assert_eq!(style.frame(0), style.frame(8));
        assert_eq!(style.frame(3), style.frame(11));
    }

    #[test]
    fn every_frame_of_every_style_is_a_single_character() {
        for style in [
            SpinnerStyle::Braille,
            SpinnerStyle::Pulse,
            SpinnerStyle::Sweep,
            SpinnerStyle::Quadrant,
        ] {
            for frame in style.frames() {
                assert_eq!(frame.chars().count(), 1, "{frame:?} in {style:?}");
            }
        }
    }

    #[test]
    fn two_spinners_at_the_same_phase_show_the_same_frame() {
        // This is the whole reason phase is passed in rather than read from a
        // clock inside the widget.
        let a = Spinner::new(SpinnerStyle::Pulse, 7);
        let b = Spinner::new(SpinnerStyle::Pulse, 7);
        assert_eq!(a.glyph(), b.glyph());
    }
}

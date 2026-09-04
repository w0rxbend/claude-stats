//! The oversized headline, drawn with `tui-big-text`.
//!
//! `tui-big-text` renders text through an 8x8 pixel font, so a single line of
//! it costs eight terminal rows at full size -- a large share of a dashboard's
//! vertical budget. The rule applied here is that the banner shrinks before
//! anything else does, and disappears entirely before any metric is clipped:
//! the context percentage is why the tool exists, and the branding is not.
//!
//! What it shows is the live context percentage, not a product name. If a
//! quarter of the screen is going to be given over to something four feet
//! readable, it should be the number that decides whether the session is about
//! to compact.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use tui_big_text::{BigText, PixelSize};

use crate::domain::context::ContextFill;
use crate::tui::palette::Palette;

/// The context percentage at four-feet size.
pub struct ContextBanner {
    fill: ContextFill,
}

impl ContextBanner {
    /// The rows needed at each pixel size, tallest first.
    ///
    /// `Full` spends one cell per font pixel, `HalfHeight` squeezes two rows
    /// into one with half blocks, and `Quadrant` halves both axes.
    const SIZES: [(PixelSize, u16); 3] = [
        (PixelSize::Full, 8),
        (PixelSize::HalfHeight, 4),
        (PixelSize::Quadrant, 4),
    ];

    /// A banner for the given fill.
    #[must_use]
    pub const fn new(fill: ContextFill) -> Self {
        Self { fill }
    }

    /// The largest pixel size that fits in `height` rows, if any fits at all.
    fn size_for(height: u16) -> Option<PixelSize> {
        Self::SIZES
            .into_iter()
            .find(|(_, rows)| *rows <= height)
            .map(|(size, _)| size)
    }
}

impl ContextBanner {
    /// Draws the banner, coloured by how full the context window is.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        let Some(pixel_size) = Self::size_for(area.height) else {
            // Not enough room to render legibly. Drawing a clipped glyph is
            // worse than drawing nothing: half a digit still looks like a
            // digit, and it is the wrong one.
            return;
        };

        let text = format!("{:.0}%", self.fill.percent());
        let colour = palette.severity(self.fill.severity());

        BigText::builder()
            .pixel_size(pixel_size)
            .centered()
            .style(Style::default().fg(colour).add_modifier(Modifier::BOLD))
            .lines(vec![Line::from(text)])
            .build()
            .render(area, buf);
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

    #[test]
    fn the_banner_steps_down_a_size_before_it_would_be_clipped() {
        assert_eq!(ContextBanner::size_for(8), Some(PixelSize::Full));
        assert_eq!(ContextBanner::size_for(7), Some(PixelSize::HalfHeight));
        assert_eq!(ContextBanner::size_for(4), Some(PixelSize::HalfHeight));
    }

    #[test]
    fn below_the_smallest_size_the_banner_yields_its_space_entirely() {
        assert_eq!(ContextBanner::size_for(3), None);
        assert_eq!(ContextBanner::size_for(0), None);
    }

    #[test]
    fn drawing_into_a_short_area_leaves_it_untouched_instead_of_panicking() {
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        ContextBanner::new(ContextFill::new(50, 100)).render(area, &mut buf, &palette());
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }
}

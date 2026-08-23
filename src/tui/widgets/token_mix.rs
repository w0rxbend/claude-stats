//! Where the tokens went, as a pie chart.
//!
//! Drawn with the `tui-piechart` crate at braille resolution, which packs
//! eight dots into every character cell and gets a circle that reads as a
//! circle in a panel barely twenty cells wide.
//!
//! The four slices are the four things a session is billed for, and seeing
//! them side by side answers the question a single "total cost" number cannot:
//! *why* is it that much. A pie dominated by pale cache reads is a healthy,
//! cheap session. One dominated by output is an expensive one, and the chart
//! says so before the invoice does.

use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Widget};
use tui_piechart::{LegendPosition, PieChart, PieSlice, Resolution};

use crate::domain::tokens::TokenUsage;
use crate::tui::theme::Theme;

/// A pie chart of the four token kinds.
pub struct TokenMix {
    usage: TokenUsage,
}

impl TokenMix {
    /// A chart of the given usage.
    #[must_use]
    pub const fn new(usage: TokenUsage) -> Self {
        Self { usage }
    }

    /// The slices, in a fixed order with fixed colours.
    ///
    /// The order never changes with the data. A chart whose slices reorder
    /// themselves as the numbers move is unreadable at a glance, because the
    /// reader has to consult the legend on every look instead of learning the
    /// layout once.
    ///
    /// Zero-valued slices are dropped rather than drawn as a hairline, which
    /// would otherwise clutter the legend of a session that has not yet
    /// written anything to the cache.
    fn slices(&self) -> Vec<PieSlice<'static>> {
        [
            ("cache read", self.usage.cache_read, Theme::CYAN),
            ("cache write", self.usage.cache_creation, Theme::VIOLET),
            ("input", self.usage.input, Theme::AZURE),
            ("output", self.usage.output, Theme::AMBER),
        ]
        .into_iter()
        .filter(|(_, value, _)| *value > 0)
        .map(|(label, value, colour)| PieSlice::new(label, value as f64, colour))
        .collect()
    }
}

impl Widget for TokenMix {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Theme::BORDER))
            .style(Style::default().bg(Theme::SURFACE))
            .title(ratatui::text::Span::styled(
                " token mix ",
                Theme::title(Theme::VIOLET),
            ));

        let slices = self.slices();
        if slices.is_empty() {
            // Nothing has been billed yet. The empty frame still holds the
            // space, so the layout does not jump when the first response lands.
            block.render(area, buf);
            return;
        }

        PieChart::new(slices)
            .block(block)
            .style(Style::default().bg(Theme::SURFACE).fg(Theme::TEXT))
            .resolution(Resolution::Braille)
            .legend_position(LegendPosition::Right)
            .show_percentages(true)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_keep_a_fixed_order_regardless_of_their_sizes() {
        let mix = TokenMix::new(TokenUsage {
            input: 1,
            cache_read: 1_000_000,
            cache_creation: 5,
            output: 50,
        });
        let labels: Vec<&str> = mix
            .slices()
            .iter()
            .map(tui_piechart::PieSlice::label)
            .collect();
        assert_eq!(labels, ["cache read", "cache write", "input", "output"]);
    }

    #[test]
    fn a_kind_with_no_tokens_is_left_out_of_the_legend() {
        let mix = TokenMix::new(TokenUsage {
            input: 10,
            cache_read: 0,
            cache_creation: 0,
            output: 5,
        });
        let labels: Vec<&str> = mix
            .slices()
            .iter()
            .map(tui_piechart::PieSlice::label)
            .collect();
        assert_eq!(labels, ["input", "output"]);
    }

    #[test]
    fn an_unused_session_draws_an_empty_frame_rather_than_panicking() {
        let area = ratatui::layout::Rect::new(0, 0, 24, 10);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        TokenMix::new(TokenUsage::ZERO).render(area, &mut buf);
    }
}

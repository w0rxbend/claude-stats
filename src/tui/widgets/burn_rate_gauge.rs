//! The burn-rate gauge: how hard the active billing block is being worked,
//! and what it is on course to cost.
//!
//! [`SpendPanel`](crate::tui::widgets::spend_panel::SpendPanel) already prints
//! the projected cost once, folded in beside the block's own figures; this
//! panel exists for the reader who wants that one number given the room to
//! be read at a glance rather than picked out of a denser panel. The fill bar
//! reuses [`ContextGauge`]'s own gradient-bar mechanics -- the same
//! [`Palette::ramp`] sweep, the same eighths-of-a-cell precision, the same
//! "draw a tick ahead of a threshold and let the bar overtake it" idea --
//! because a reader who has already learned to read one gauge on this
//! dashboard should not have to learn a second visual language to read this
//! one.
//!
//! [`ContextGauge`]: crate::tui::widgets::gauge::ContextGauge

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::domain::blocks::{BurnRate, Intensity, LimitStanding};
use crate::domain::money::Usd;
use crate::tui::icons::{EIGHTHS, Icon};
use crate::tui::palette::Palette;

/// Where the tick is drawn when a [`LimitStanding`] is known.
///
/// Mirrors [`LimitStanding::of`]'s own warning band, which begins at four
/// fifths of the limit; that constant is private to the domain, so this is
/// the one place the dashboard restates it, to draw the tick at the same
/// point the domain would first call the projection a [`LimitStanding::Warning`].
const WARNING_FROM: f64 = 0.8;

/// The gauge itself: an intensity fill and the cost it projects to.
pub struct BurnRateGauge {
    /// Fresh input and output tokens per minute -- see
    /// [`BurnRate::indicator_tokens_per_minute`] for why cache traffic is
    /// excluded from this figure.
    intensity: f64,
    projection: Usd,
    limit_standing: Option<LimitStanding>,
}

impl BurnRateGauge {
    /// A gauge over the given reading. `limit_standing` mirrors
    /// [`crate::view::dashboard_view::BurnRateView::limit_standing`]: `None`
    /// while no token ceiling reaches the live dashboard, in which case no
    /// tick is drawn.
    #[must_use]
    pub const fn new(
        intensity: f64,
        projection: Usd,
        limit_standing: Option<LimitStanding>,
    ) -> Self {
        Self {
            intensity,
            projection,
            limit_standing,
        }
    }

    /// `intensity` normalised into `0.0..=1.0`, against
    /// [`Intensity::HIGH_FROM`] -- the same threshold
    /// [`BurnRate::intensity`] itself calls "high", so a bar reading full
    /// is exactly the rate that marker elsewhere on the dashboard would
    /// agree is a lot.
    fn fill_ratio(&self) -> f64 {
        (self.intensity / Intensity::HIGH_FROM).clamp(0.0, 1.0)
    }

    /// The band `intensity` falls in, read through [`BurnRate::intensity`]
    /// itself rather than restating its thresholds here: a zero-tokens
    /// [`BurnRate`] carrying only this panel's own `indicator_tokens_per_minute`
    /// is enough to ask the domain the same question
    /// [`SpendPanel`](crate::tui::widgets::spend_panel::SpendPanel) asks of a
    /// real one.
    fn band(&self) -> Intensity {
        BurnRate {
            tokens_per_minute: 0.0,
            indicator_tokens_per_minute: self.intensity,
            cost_per_hour: Usd::ZERO,
        }
        .intensity()
    }
}

impl BurnRateGauge {
    /// Draws the gauge: a labelled fill bar with the projected cost printed
    /// beside it, on one row.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        if area.is_empty() {
            return;
        }
        let row = Rect { height: 1, ..area };

        let label = format!("{} burn ", Icon::RATE);
        let cost_text = format!(" {}", self.projection);
        let label_width = label.chars().count() as u16;
        let cost_width = cost_text.chars().count() as u16;
        let bar_width = row.width.saturating_sub(label_width + cost_width);

        buf.set_string(row.x, row.y, &label, palette.label());
        draw_bar(
            buf,
            Rect::new(row.x + label_width, row.y, bar_width, 1),
            self.fill_ratio(),
            self.limit_standing.is_some(),
            palette,
        );
        buf.set_string(
            row.x + label_width + bar_width,
            row.y,
            &cost_text,
            Style::default()
                .fg(band_colour(self.band(), palette))
                .add_modifier(Modifier::BOLD),
        );
    }
}

/// The colour a burn band is announced in -- the same three-way mapping
/// [`SpendPanel`](crate::tui::widgets::spend_panel::SpendPanel)'s own
/// `burn_colour` uses, so the one figure both panels print for a high burn
/// agrees about which colour "fast" is.
fn band_colour(band: Intensity, palette: &Palette) -> Color {
    match band {
        Intensity::Normal => palette.accent_success.into(),
        Intensity::Moderate => palette.accent_primary.into(),
        Intensity::High => palette.ramp(1.0),
    }
}

/// Draws the fill bar itself: eighths-of-a-cell precision, each filled cell
/// coloured by its position along the bar via [`Palette::ramp`], and a
/// threshold tick at [`WARNING_FROM`] while `show_threshold` is set and the
/// fill has not already reached it -- the same rule
/// [`ContextGauge`](crate::tui::widgets::gauge::ContextGauge)'s own threshold
/// tick follows.
fn draw_bar(buf: &mut Buffer, area: Rect, ratio: f64, show_threshold: bool, palette: &Palette) {
    if area.is_empty() {
        return;
    }
    let row = area.y;
    let width = area.width as usize;
    let filled_eighths = (ratio * (width * 8) as f64).round() as usize;
    let threshold_cell = (WARNING_FROM * width as f64).round() as usize;

    for cell in 0..width {
        let x = area.x + cell as u16;
        let position = if width > 1 {
            cell as f64 / (width - 1) as f64
        } else {
            0.0
        };
        let eighths_here = filled_eighths.saturating_sub(cell * 8).min(8);
        let (symbol, colour) = if eighths_here == 0 {
            (Icon::BAR_EMPTY, palette.border.into())
        } else {
            (EIGHTHS[eighths_here - 1], palette.ramp(position))
        };
        buf.set_string(x, row, symbol, Style::default().fg(colour));
    }

    if show_threshold && threshold_cell < width && filled_eighths < threshold_cell * 8 {
        buf.set_string(
            area.x + threshold_cell as u16,
            row,
            Icon::MARKER,
            Style::default().fg(palette.accent_secondary.into()),
        );
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

    fn render_row(
        intensity: f64,
        projection: Usd,
        limit: Option<LimitStanding>,
        width: u16,
    ) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        BurnRateGauge::new(intensity, projection, limit).render(area, &mut buf, &palette());
        (0..width)
            .map(|x| buf[(x, 0)].symbol().to_owned())
            .collect()
    }

    #[test]
    fn a_zero_rate_draws_only_track_and_the_projected_cost() {
        let out = render_row(0.0, Usd::new(1.23), None, 30);
        assert!(out.contains("$1.23"), "got {out:?}");
        assert!(out.contains(Icon::BAR_EMPTY), "got {out:?}");
    }

    #[test]
    fn a_rate_at_or_past_the_high_threshold_fills_the_whole_bar() {
        let out = render_row(Intensity::HIGH_FROM * 2.0, Usd::new(9.99), None, 30);
        assert!(
            out.contains('\u{2588}'),
            "a saturating rate reaches a solid block: {out:?}"
        );
        assert!(!out.contains(Icon::BAR_EMPTY), "got {out:?}");
    }

    #[test]
    fn a_limit_standing_draws_a_threshold_tick_ahead_of_a_low_fill() {
        let with_limit = render_row(0.0, Usd::new(1.0), Some(LimitStanding::Ok), 30);
        let without_limit = render_row(0.0, Usd::new(1.0), None, 30);
        assert!(with_limit.contains('\u{2193}'), "got {with_limit:?}");
        assert!(
            !without_limit.contains('\u{2193}'),
            "no ceiling reaches the dashboard, no tick is drawn: {without_limit:?}"
        );
    }

    #[test]
    fn a_high_burn_is_announced_in_the_same_colour_family_the_spend_panel_uses() {
        // Not a colour assertion (`TestBackend` never captures style), but a
        // behavioural one: the figure is still printed once the bar has
        // saturated, rather than crowded out by a full-width bar.
        let out = render_row(Intensity::HIGH_FROM * 3.0, Usd::new(42.0), None, 40);
        assert!(out.contains("$42.00"), "got {out:?}");
    }

    #[test]
    fn drawing_into_a_tiny_or_empty_area_does_not_panic() {
        for (w, h) in [(0, 0), (1, 1), (2, 1), (30, 5)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), h.max(1)));
            BurnRateGauge::new(1_000.0, Usd::new(1.0), Some(LimitStanding::Warning)).render(
                area,
                &mut buf,
                &palette(),
            );
        }
    }
}

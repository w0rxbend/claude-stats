//! The colour palette, and the rules for applying it.
//!
//! Every colour the dashboard draws comes from here. That is the point: a
//! palette defined in one place can be checked for contrast once and swapped
//! wholesale, whereas colours sprinkled through twenty widgets drift until
//! nothing quite matches anything else.
//!
//! The palette is a cool "aurora" ramp -- deep indigo ground, cyan and violet
//! accents -- with a warm amber-to-crimson ramp reserved exclusively for
//! *pressure*: a filling context window, a rising error count. Keeping the
//! warm end reserved means a glance at the screen answers "is anything wrong?"
//! before any number has been read.
//!
//! All colours are 24-bit RGB. Terminals that cannot manage true colour
//! approximate them to the nearest palette entry, which degrades gracefully;
//! choosing 16-colour names instead would cap the design at what a 1980s
//! terminal could do for every user.

use ratatui::style::{Color, Modifier, Style};

use crate::domain::activity::ToolKind;
use crate::domain::context::FillSeverity;

/// The dashboard's colours.
pub struct Theme;

impl Theme {
    // ── ground and structure ──────────────────────────────────────────

    /// The page background.
    pub const BACKGROUND: Color = Color::Rgb(11, 13, 26);
    /// The background of a raised panel, one step lighter than the page.
    pub const SURFACE: Color = Color::Rgb(19, 22, 40);
    /// Panel borders at rest.
    pub const BORDER: Color = Color::Rgb(48, 54, 92);
    /// The border of the panel that currently has focus.
    pub const BORDER_ACTIVE: Color = Color::Rgb(122, 162, 255);

    // ── text ──────────────────────────────────────────────────────────

    /// Body text.
    pub const TEXT: Color = Color::Rgb(222, 228, 255);
    /// Labels and units -- present, but not competing with the numbers.
    pub const MUTED: Color = Color::Rgb(120, 132, 178);
    /// The dimmest readable tone, for hints and separators.
    pub const FAINT: Color = Color::Rgb(72, 82, 122);

    // ── accents ───────────────────────────────────────────────────────

    /// The primary accent: cache, cost, headline figures.
    pub const CYAN: Color = Color::Rgb(86, 226, 232);
    /// The secondary accent: compaction, structural events.
    pub const VIOLET: Color = Color::Rgb(167, 139, 250);
    /// Success and healthy readings.
    pub const MINT: Color = Color::Rgb(94, 234, 168);
    /// A neutral highlight for activity that is neither good nor bad.
    pub const AZURE: Color = Color::Rgb(96, 165, 250);
    /// Reserved for pressure, step one.
    pub const AMBER: Color = Color::Rgb(251, 191, 36);
    /// Reserved for pressure, step two.
    pub const ORANGE: Color = Color::Rgb(251, 146, 60);
    /// Reserved for pressure, step three, and for errors.
    pub const CRIMSON: Color = Color::Rgb(248, 113, 113);
    /// Sub-agents and skills -- the "something else is running" colour.
    pub const MAGENTA: Color = Color::Rgb(244, 114, 182);

    /// The base style every frame starts from.
    #[must_use]
    pub fn base() -> Style {
        Style::default().bg(Self::BACKGROUND).fg(Self::TEXT)
    }

    /// A panel's title: bright, bold, and never the same colour as its
    /// contents, so the eye can find the edges of a dense layout.
    #[must_use]
    pub fn title(accent: Color) -> Style {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    }

    /// A metric's label.
    #[must_use]
    pub fn label() -> Style {
        Style::default().fg(Self::MUTED)
    }

    /// A metric's value.
    #[must_use]
    pub fn value(accent: Color) -> Style {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    }

    /// The colour for a given context-fill band.
    ///
    /// This is the single mapping from "how full" to "how alarming". Widgets
    /// ask for it rather than comparing percentages themselves, so the bar,
    /// the tile and the header can never disagree about whether the session is
    /// in trouble.
    #[must_use]
    pub const fn severity(severity: FillSeverity) -> Color {
        match severity {
            FillSeverity::Comfortable => Self::MINT,
            FillSeverity::Warm => Self::CYAN,
            FillSeverity::Hot => Self::AMBER,
            FillSeverity::Critical => Self::CRIMSON,
        }
    }

    /// The colour for a kind of tool call.
    #[must_use]
    pub const fn tool_kind(kind: ToolKind) -> Color {
        match kind {
            ToolKind::Read => Self::AZURE,
            ToolKind::Write => Self::MINT,
            ToolKind::Search => Self::CYAN,
            ToolKind::Shell => Self::VIOLET,
            ToolKind::Agent => Self::MAGENTA,
            ToolKind::Skill => Self::AMBER,
            ToolKind::Network => Self::ORANGE,
            ToolKind::Other => Self::MUTED,
        }
    }

    /// A colour picked from the cool-to-warm ramp by position.
    ///
    /// `position` runs `0.0..=1.0`. Used to shade a bar along its length so a
    /// nearly-full gauge reads as hot at the leading edge before the number
    /// beside it has been read. Interpolating in plain RGB is enough here
    /// because the stops are close together in lightness; the muddy midpoint
    /// that plagues RGB interpolation between complements never arises.
    #[must_use]
    pub fn ramp(position: f64) -> Color {
        const STOPS: [(u8, u8, u8); 5] = [
            (94, 234, 168),  // mint
            (86, 226, 232),  // cyan
            (167, 139, 250), // violet
            (251, 146, 60),  // orange
            (248, 113, 113), // crimson
        ];
        let position = position.clamp(0.0, 1.0);
        let span = (STOPS.len() - 1) as f64;
        let scaled = position * span;
        let index = (scaled.floor() as usize).min(STOPS.len() - 2);
        let t = scaled - index as f64;
        let (r1, g1, b1) = STOPS[index];
        let (r2, g2, b2) = STOPS[index + 1];
        Color::Rgb(
            lerp(r1, r2, t),
            lerp(g1, g2, t),
            lerp(b1, b2, t),
        )
    }
}

/// Linear interpolation between two channel values.
fn lerp(from: u8, to: u8, t: f64) -> u8 {
    (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ramp_starts_at_mint_and_ends_at_crimson() {
        assert_eq!(Theme::ramp(0.0), Color::Rgb(94, 234, 168));
        assert_eq!(Theme::ramp(1.0), Color::Rgb(248, 113, 113));
    }

    #[test]
    fn a_position_outside_the_range_is_clamped_rather_than_wrapping() {
        assert_eq!(Theme::ramp(-5.0), Theme::ramp(0.0));
        assert_eq!(Theme::ramp(5.0), Theme::ramp(1.0));
    }

    #[test]
    fn the_warm_end_of_the_palette_is_reserved_for_pressure() {
        assert_eq!(Theme::severity(FillSeverity::Comfortable), Theme::MINT);
        assert_eq!(Theme::severity(FillSeverity::Critical), Theme::CRIMSON);
    }
}

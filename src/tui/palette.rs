//! The colour palette, as data rather than as compiled-in constants.
//!
//! [`Palette`] is a Value Object (Fowler, *`PoEAA`*): two palettes with the
//! same seventeen colours are the same palette, it is never mutated once
//! built, and every widget that wants a colour is handed one rather than
//! reaching for a global. That is the whole point of the change this module
//! makes over the unit-struct constant table it replaces -- a bare `CYAN`
//! constant was a compile-time fact, so the only way to offer a second theme
//! would have been a second copy of every widget. A `Palette` threaded
//! through as a parameter can be swapped per frame, per session, or per user
//! preference, and no widget has to know that happened.
//!
//! The role names below (`accent_primary`, `pressure_mid`, and so on)
//! describe what a colour is *for*, not what it looks like. A bare `CYAN`
//! constant told you a hex value; `accent_primary` tells you that whatever
//! theme is active, this is the one used for headline figures. That
//! indirection is what makes twenty-seven themes possible without
//! twenty-seven times the call sites: every widget asks for a role, and only
//! the palette itself knows which colour currently answers it.
//!
//! Every built-in palette keeps the "aurora" design's founding rule: routine
//! figures stay on the cool `accent_*` colours, and the `pressure_*` ramp is
//! reserved for context fill, compaction distance and error counts. A glance
//! at the screen answers "is anything wrong?" before any number has been
//! read, in every theme, because the warm end of the ramp is never spent on
//! something that isn't a warning.

use ratatui::style::{Color, Modifier, Style};

use crate::domain::activity::ToolKind;
use crate::domain::context::FillSeverity;

pub mod builtins;
pub mod registry;

/// A 24-bit colour, kept as its own type rather than [`ratatui::style::Color`]
/// directly so a [`Palette`] can derive `Serialize`/`Deserialize` -- `Color`
/// carries terminal-indexed variants that do not round-trip through JSON the
/// way a plain triple of bytes does, and a palette that cannot be saved and
/// reloaded is no use to the theme picker later epics are going to build on
/// top of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<Rgb> for Color {
    fn from(Rgb(r, g, b): Rgb) -> Self {
        Self::Rgb(r, g, b)
    }
}

/// A complete set of the colours the dashboard draws with.
///
/// Terminals that cannot manage true colour approximate every field to the
/// nearest palette entry, which degrades gracefully; choosing 16-colour names
/// instead would cap every theme at what a 1980s terminal could do for every
/// user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Palette {
    /// The registry key this palette is looked up and saved by, e.g.
    /// `"aurora"` or `"catppuccin-mocha"`.
    pub name: String,

    // ── ground and structure ──────────────────────────────────────────
    /// The page background.
    pub background: Rgb,
    /// The background of a raised panel, one step lighter than the page.
    pub surface: Rgb,
    /// The background of a popup drawn over everything else, e.g. the help
    /// overlay -- one step darker than the page so it reads as being in
    /// front of it rather than merely a differently-coloured panel.
    pub overlay: Rgb,
    /// Panel borders at rest.
    pub border: Rgb,
    /// The border of the panel that currently has focus.
    pub border_active: Rgb,

    // ── text ──────────────────────────────────────────────────────────
    /// Body text.
    pub text: Rgb,
    /// Labels and units -- present, but not competing with the numbers.
    pub muted: Rgb,
    /// The dimmest readable tone, for hints and separators.
    pub faint: Rgb,
    /// Text drawn on top of a filled accent, e.g. a highlighted list row --
    /// the readable colour against `accent_primary` rather than against
    /// `background`.
    pub inverted_text: Rgb,

    // ── accents ───────────────────────────────────────────────────────
    /// The primary accent: cache, cost, headline figures.
    pub accent_primary: Rgb,
    /// The secondary accent: compaction, structural events.
    pub accent_secondary: Rgb,
    /// Success and healthy readings.
    pub accent_success: Rgb,
    /// A neutral highlight for activity that is neither good nor bad.
    pub accent_info: Rgb,
    /// Sub-agents and skills -- the "something else is running" colour.
    pub accent_special: Rgb,

    // ── pressure ──────────────────────────────────────────────────────
    /// Reserved for pressure, step one.
    pub pressure_low: Rgb,
    /// Reserved for pressure, step two.
    pub pressure_mid: Rgb,
    /// Reserved for pressure, step three, and for errors.
    pub pressure_high: Rgb,
}

impl Palette {
    /// The five-stop cool-to-warm ramp a gauge shades along its length.
    ///
    /// Kept as its own method, distinct from [`Palette::chart_series`],
    /// because a gauge and a pie chart want different things from a ramp: the
    /// gauge needs exactly these five stops in this order so that
    /// [`Palette::ramp`] can interpolate between them, while a chart legend
    /// wants as many *distinguishable* colours as it has slices.
    #[must_use]
    pub fn gauge_ramp(&self) -> [Color; 5] {
        self.ramp_stops().map(Into::into)
    }

    /// The raw stops behind [`Palette::gauge_ramp`] and [`Palette::ramp`],
    /// kept as one array so the two methods can never drift apart about what
    /// the ramp actually contains.
    fn ramp_stops(&self) -> [Rgb; 5] {
        [
            self.accent_success,
            self.accent_primary,
            self.accent_secondary,
            self.pressure_mid,
            self.pressure_high,
        ]
    }

    /// A colour picked from the cool-to-warm ramp by position.
    ///
    /// `position` runs `0.0..=1.0`. Used to shade a bar along its length so a
    /// nearly-full gauge reads as hot at the leading edge before the number
    /// beside it has been read. Interpolating in plain RGB is enough here
    /// because the stops are close together in lightness; the muddy midpoint
    /// that plagues RGB interpolation between complements never arises.
    #[must_use]
    pub fn ramp(&self, position: f64) -> Color {
        let stops = self.ramp_stops();
        let position = position.clamp(0.0, 1.0);
        let span = (stops.len() - 1) as f64;
        let scaled = position * span;
        let index = (scaled.floor() as usize).min(stops.len() - 2);
        let t = scaled - index as f64;
        let Rgb(r1, g1, b1) = stops[index];
        let Rgb(r2, g2, b2) = stops[index + 1];
        Color::Rgb(lerp(r1, r2, t), lerp(g1, g2, t), lerp(b1, b2, t))
    }

    /// Eight colours for a legend that needs more entries than the five-stop
    /// [`Palette::gauge_ramp`] offers, e.g. the token-mix pie chart.
    ///
    /// The first five are named accents rather than the pressure ramp -- a
    /// chart legend is not a warning, so it stays off the colours this
    /// palette reserves for "something is wrong". The last three are tints
    /// and shades of three of those five, generated rather than hand-picked,
    /// so a chart with more slices than named accents still gets colours that
    /// are visibly related to the palette instead of falling back to grey.
    #[must_use]
    pub fn chart_series(&self) -> [Color; 8] {
        [
            self.accent_success.into(),
            self.accent_primary.into(),
            self.accent_secondary.into(),
            self.accent_info.into(),
            self.accent_special.into(),
            lighten(self.accent_primary, 0.15),
            darken(self.accent_secondary, 0.15),
            lighten(self.accent_success, 0.15),
        ]
    }

    /// The colour for a given context-fill band.
    ///
    /// This is the single mapping from "how full" to "how alarming". Widgets
    /// ask for it rather than comparing percentages themselves, so the bar,
    /// the tile and the header can never disagree about whether the session
    /// is in trouble.
    #[must_use]
    pub fn severity(&self, severity: FillSeverity) -> Color {
        match severity {
            FillSeverity::Comfortable => self.accent_success.into(),
            FillSeverity::Warm => self.accent_primary.into(),
            FillSeverity::Hot => self.pressure_mid.into(),
            FillSeverity::Critical => self.pressure_high.into(),
        }
    }

    /// The colour for a kind of tool call.
    #[must_use]
    pub fn tool_kind(&self, kind: ToolKind) -> Color {
        match kind {
            ToolKind::Read => self.accent_info.into(),
            ToolKind::Write => self.accent_success.into(),
            ToolKind::Search => self.accent_primary.into(),
            ToolKind::Shell => self.accent_secondary.into(),
            ToolKind::Agent => self.accent_special.into(),
            ToolKind::Skill => self.pressure_low.into(),
            ToolKind::Network => self.pressure_mid.into(),
            ToolKind::Other => self.muted.into(),
        }
    }

    /// The base style every frame starts from.
    #[must_use]
    pub fn base(&self) -> Style {
        Style::default()
            .bg(self.background.into())
            .fg(self.text.into())
    }

    /// A panel's title: bright, bold, and never the same colour as its
    /// contents, so the eye can find the edges of a dense layout.
    #[must_use]
    pub fn title(&self, accent: Color) -> Style {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    }

    /// A metric's label.
    #[must_use]
    pub fn label(&self) -> Style {
        Style::default().fg(self.muted.into())
    }
}

/// Linear interpolation between two channel values.
fn lerp(from: u8, to: u8, t: f64) -> u8 {
    (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8
}

/// Converts an sRGB triple to HSL, as `(hue in 0.0..360.0, saturation,
/// lightness)` with the latter two in `0.0..=1.0`.
fn rgb_to_hsl(Rgb(r, g, b): Rgb) -> (f32, f32, f32) {
    let r = f32::from(r) / 255.0;
    let g = f32::from(g) / 255.0;
    let b = f32::from(b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = f32::midpoint(max, min);

    if (max - min).abs() < f32::EPSILON {
        // A grey: hue is undefined and saturation is zero.
        return (0.0, 0.0, lightness);
    }

    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - r).abs() < f32::EPSILON {
        (g - b) / delta + if g < b { 6.0 } else { 0.0 }
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue * 60.0, saturation, lightness)
}

/// Converts HSL (hue in `0.0..360.0`, saturation and lightness in
/// `0.0..=1.0`) back to an sRGB triple.
fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> Rgb {
    if saturation.abs() < f32::EPSILON {
        let v = to_channel(lightness);
        return Rgb(v, v, v);
    }

    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let h = hue / 360.0;
    Rgb(
        to_channel(hue_to_channel(p, q, h + 1.0 / 3.0)),
        to_channel(hue_to_channel(p, q, h)),
        to_channel(hue_to_channel(p, q, h - 1.0 / 3.0)),
    )
}

/// One channel of the HSL-to-RGB conversion, for a hue rotated by a third of
/// the colour wheel per channel.
fn hue_to_channel(p: f32, q: f32, hue: f32) -> f32 {
    let hue = if hue < 0.0 {
        hue + 1.0
    } else if hue > 1.0 {
        hue - 1.0
    } else {
        hue
    };
    if hue < 1.0 / 6.0 {
        p + (q - p) * 6.0 * hue
    } else if hue < 0.5 {
        q
    } else if hue < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - hue) * 6.0
    } else {
        p
    }
}

fn to_channel(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A tint: `c` moved towards white by `amount` of lightness.
fn lighten(c: Rgb, amount: f32) -> Color {
    let (h, s, l) = rgb_to_hsl(c);
    hsl_to_rgb(h, s, (l + amount).clamp(0.0, 1.0)).into()
}

/// A shade: `c` moved towards black by `amount` of lightness.
fn darken(c: Rgb, amount: f32) -> Color {
    let (h, s, l) = rgb_to_hsl(c);
    hsl_to_rgb(h, s, (l - amount).clamp(0.0, 1.0)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Palette {
        registry::ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    #[test]
    fn the_ramp_starts_at_accent_success_and_ends_at_pressure_high() {
        let palette = sample();
        assert_eq!(palette.ramp(0.0), palette.accent_success.into());
        assert_eq!(palette.ramp(1.0), palette.pressure_high.into());
    }

    #[test]
    fn a_position_outside_the_range_is_clamped_rather_than_wrapping() {
        let palette = sample();
        assert_eq!(palette.ramp(-5.0), palette.ramp(0.0));
        assert_eq!(palette.ramp(5.0), palette.ramp(1.0));
    }

    #[test]
    fn the_warm_end_of_the_palette_is_reserved_for_pressure() {
        let palette = sample();
        assert_eq!(
            palette.severity(FillSeverity::Comfortable),
            palette.accent_success.into()
        );
        assert_eq!(
            palette.severity(FillSeverity::Critical),
            palette.pressure_high.into()
        );
    }

    #[test]
    fn lightening_a_colour_all_the_way_reaches_white() {
        assert_eq!(lighten(Rgb(10, 20, 30), 1.0), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn darkening_a_colour_all_the_way_reaches_black() {
        assert_eq!(darken(Rgb(200, 210, 220), 1.0), Color::Rgb(0, 0, 0));
    }
}

/// WCAG contrast-floor checks over every built-in theme.
///
/// These exist because a palette is data now, not a handful of constants a
/// reviewer can eyeball -- twenty-seven themes times seventeen colours is
/// well past what a glance at a diff can vouch for. The floors themselves are
/// the [Web Content Accessibility Guidelines' definition of contrast
/// ratio](https://www.w3.org/TR/WCAG21/#contrast-minimum), reimplemented here
/// directly rather than pulled in as a dependency: it is four short formulas,
/// and a crate for it would be a heavier and less legible way to say the same
/// thing.
#[cfg(test)]
mod contrast_tests {
    use super::Rgb;
    use super::registry::ThemeRegistry;

    /// The sRGB-to-linear step in the WCAG relative luminance formula.
    fn linearise(channel: u8) -> f64 {
        let c = f64::from(channel) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// WCAG relative luminance of an sRGB colour.
    fn relative_luminance(Rgb(r, g, b): Rgb) -> f64 {
        0.2126 * linearise(r) + 0.7152 * linearise(g) + 0.0722 * linearise(b)
    }

    /// The WCAG contrast ratio between two colours, always `>= 1.0`.
    fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
        let (l1, l2) = (relative_luminance(a), relative_luminance(b));
        let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn every_built_in_theme_clears_its_wcag_contrast_floor() {
        let registry = ThemeRegistry::builtin();
        for name in registry.names() {
            let palette = registry.get(name).expect("just listed by name");

            for (role, colour, floor) in [
                ("text", palette.text, 4.5),
                ("muted", palette.muted, 4.5),
                ("accent_primary", palette.accent_primary, 3.0),
                ("accent_secondary", palette.accent_secondary, 3.0),
                ("accent_success", palette.accent_success, 3.0),
                ("accent_info", palette.accent_info, 3.0),
                ("accent_special", palette.accent_special, 3.0),
                ("pressure_low", palette.pressure_low, 3.0),
                ("pressure_mid", palette.pressure_mid, 3.0),
                ("pressure_high", palette.pressure_high, 3.0),
            ] {
                let ratio = contrast_ratio(colour, palette.background);
                assert!(
                    ratio >= floor,
                    "{name}: {role} on background only manages a contrast \
                     ratio of {ratio:.2}, short of the {floor} floor"
                );
            }
        }
    }

    #[test]
    fn no_theme_reuses_a_pressure_hex_for_a_non_pressure_role() {
        let registry = ThemeRegistry::builtin();
        for name in registry.names() {
            let palette = registry.get(name).expect("just listed by name");
            let pressures = [
                ("pressure_low", palette.pressure_low),
                ("pressure_mid", palette.pressure_mid),
                ("pressure_high", palette.pressure_high),
            ];
            let others = [
                ("background", palette.background),
                ("surface", palette.surface),
                ("overlay", palette.overlay),
                ("border", palette.border),
                ("border_active", palette.border_active),
                ("text", palette.text),
                ("muted", palette.muted),
                ("faint", palette.faint),
                ("inverted_text", palette.inverted_text),
                ("accent_primary", palette.accent_primary),
                ("accent_secondary", palette.accent_secondary),
                ("accent_success", palette.accent_success),
                ("accent_info", palette.accent_info),
                ("accent_special", palette.accent_special),
            ];

            for (pressure_role, pressure_colour) in pressures {
                for (other_role, other_colour) in others {
                    assert_ne!(
                        pressure_colour, other_colour,
                        "{name}: {other_role} reuses the hex reserved for \
                         {pressure_role}, so a pressure warning would blend \
                         into routine chrome"
                    );
                }
            }
        }
    }
}

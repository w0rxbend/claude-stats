//! The animated "$" marker: how full today's spend is against the account's
//! own busiest day, drawn with `tui-big-text`'s single oversized glyph.
//!
//! [`DollarPulse`] draws in one of three [`AnimationStyle`]s. `Off` renders
//! the glyph once, in one colour, and stops there -- the provably inert
//! branch a `NO_ANIMATION`/`CLAUDE_STATS_NO_ANIMATION` environment must
//! always land on regardless of any later config, mirroring the `NO_COLOR`
//! convention. `Pulse` (Treatment B, the default) fills the glyph from the
//! bottom like a thermometer -- the bottom `level` fraction of its rows lit
//! in `accent`, the rest left `faint` -- with a brief brighter "overshoot"
//! row above the fill line while a rise in cost is still fresh. `Coin`
//! (Treatment A) narrows the glyph's own width across an eight-tick cycle
//! instead, swapping the `$` for a vertical bar at its narrowest point, the
//! way a coin looks edge-on mid-flip.
//!
//! [`PulseClock`] is the small piece of state that arms `Pulse`'s overshoot:
//! it watches the account's own spend for `today` tick over and remembers
//! how many frames ago that last happened. It is an Extract Class (Fowler,
//! *Refactoring*) over what would otherwise be `App` holding two
//! loosely-related fields -- "the last cost seen" and "when it last rose" --
//! with nothing to say they belong together except that they only ever
//! change on the same line. `App` owns exactly one `PulseClock`, the same
//! way it owns exactly one `phase` counter for every animated widget on the
//! dashboard to read -- see [`crate::tui::widgets::spinner`]'s own module
//! doc for why a widget that keeps its own timer drifts out of step with
//! the rest of the screen within seconds.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;
use tui_big_text::{BigText, PixelSize};

use crate::domain::money::Usd;

/// How many frames a cost rise stays "fresh" enough to brighten `Pulse`'s
/// overshoot row.
///
/// Roughly 1.75 seconds at the dashboard's 8fps/125ms frame budget (see
/// `crate::tui::runtime::FRAME_BUDGET`) -- long enough that a glance at the
/// marker just after a tool call finishes still catches the flash, short
/// enough that it has always faded again well before the next one is
/// likely.
const PULSE_BUDGET_FRAMES: u64 = 14;

/// How many ticks the `Coin` treatment's narrow-and-back cycle takes.
const COIN_CYCLE_TICKS: u64 = 8;

/// How the "$" marker animates, if at all.
///
/// `rename_all = "snake_case"` matters here beyond style: it is what makes
/// `config.json`'s documented `"animation": "off" | "pulse" | "coin"` (see
/// `crate::infrastructure::config`'s own module doc, which quotes exactly
/// that) actually parse. Without it `serde`'s default derive expects the
/// Rust variant spelling verbatim (`"Off"`, `"Pulse"`, `"Coin"`), so a user
/// who typed the documented lowercase value would silently fall back to the
/// default animation with nothing but a generic parse-error line to explain
/// why -- the exact failure mode `#[serde(deny_unknown_fields)]` on
/// `Config` was chosen *not* to have for a cosmetic setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnimationStyle {
    /// Treatment B: fills and drains like a thermometer. The default.
    #[default]
    Pulse,
    /// Treatment A: narrows to an edge-on sliver and back.
    Coin,
    /// No animation at all -- one glyph, one colour, forever.
    Off,
}

/// The animated "$" marker.
pub struct DollarPulse {
    level: f64,
    accent: Color,
    faint: Color,
    pulse: Option<u64>,
    style: AnimationStyle,
}

impl DollarPulse {
    /// The rows needed at each pixel size, tallest first -- exactly
    /// [`crate::tui::widgets::banner::ContextBanner::SIZES`]'s own table:
    /// this is the same eight-row `tui-big-text` glyph the context
    /// percentage is, so it earns the same two-step fallback rather than
    /// inventing its own.
    const SIZES: [(PixelSize, u16); 2] = [(PixelSize::Full, 8), (PixelSize::HalfHeight, 4)];

    /// A marker at `level` (`0.0..=1.0` of the account's own busiest day),
    /// drawn in `accent` where it is lit and `faint` where it is not.
    #[must_use]
    pub const fn new(level: f64, accent: Color, faint: Color) -> Self {
        Self {
            level,
            accent,
            faint,
            pulse: None,
            style: AnimationStyle::Pulse,
        }
    }

    /// Arms the `Pulse` overshoot: `frames_since_increment` is how many
    /// frames ago the figure this marker sits beside last rose, straight
    /// from [`PulseClock::frames_since`]. `None` (the default) is a resting
    /// frame with no overshoot.
    #[must_use]
    pub const fn pulsing(mut self, frames_since_increment: Option<u64>) -> Self {
        self.pulse = frames_since_increment;
        self
    }

    /// Chooses the animation treatment. [`AnimationStyle::Pulse`] by
    /// default.
    #[must_use]
    pub const fn style(mut self, style: AnimationStyle) -> Self {
        self.style = style;
        self
    }

    /// The largest pixel size that fits in `height` rows, if any fits at
    /// all -- see [`Self::SIZES`].
    fn size_for(height: u16) -> Option<PixelSize> {
        Self::SIZES
            .into_iter()
            .find(|(_, rows)| *rows <= height)
            .map(|(size, _)| size)
    }

    /// The rows [`Self::SIZES`] reserves for `pixel_size`.
    const fn rows_for(pixel_size: PixelSize) -> u16 {
        match pixel_size {
            PixelSize::Full => 8,
            _ => 4,
        }
    }

    /// Draws the marker.
    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || area.height < 4 || area.width < 8 {
            // Not enough room to render legibly -- see
            // `ContextBanner::render` for why drawing nothing beats drawing
            // a clipped glyph.
            return;
        }
        let Some(pixel_size) = Self::size_for(area.height) else {
            return;
        };

        match self.style {
            // This is the whole of the `Off` branch: one call, one flat
            // colour, no second pass and nothing read from `self.pulse` --
            // see `off_style_never_produces_more_than_one_distinct_colour_across_a_full_pulse_window`
            // below, which renders every frame of a simulated pulse window
            // through this exact arm and asserts the buffer never changes.
            AnimationStyle::Off => Self::draw_glyph(area, buf, pixel_size, self.accent, "$"),
            AnimationStyle::Pulse => {
                let rows = Self::rows_for(pixel_size);
                self.render_pulse(area, buf, pixel_size, rows);
            }
            AnimationStyle::Coin => self.render_coin(area, buf, pixel_size),
        }
    }

    /// The whole glyph, in one flat colour.
    fn draw_glyph(
        area: Rect,
        buf: &mut Buffer,
        pixel_size: PixelSize,
        colour: Color,
        symbol: &str,
    ) {
        BigText::builder()
            .pixel_size(pixel_size)
            .centered()
            .style(Style::default().fg(colour).add_modifier(Modifier::BOLD))
            .lines(vec![Line::from(symbol)])
            .build()
            .render(area, buf);
    }

    /// The "$" character's own bounding rect inside `area`: eight columns
    /// wide -- both `PixelSize::Full` and `PixelSize::HalfHeight` map one
    /// column of terminal width to one pixel of the 8x8 font, they only
    /// differ vertically -- `rows` tall, horizontally centred and flush
    /// with the top of `area`. `tui_big_text::BigText` does not expose this
    /// rect itself, so this reproduces its own internal layout for a single,
    /// one-grapheme, centre-aligned line rather than depending on a private
    /// implementation detail.
    fn glyph_rect(area: Rect, rows: u16) -> Rect {
        const GLYPH_WIDTH: u16 = 8;
        let width = GLYPH_WIDTH.min(area.width);
        let offset = (area.width / 2)
            .saturating_sub(GLYPH_WIDTH / 2)
            .min(area.width.saturating_sub(width));
        Rect {
            x: area.x + offset,
            y: area.y,
            width,
            height: rows.min(area.height),
        }
    }

    /// Treatment B: outline the glyph in `faint`, then re-tint the bottom
    /// `level` fraction of it to `accent`, plus a brighter overshoot row
    /// while a rise is still fresh.
    ///
    /// The fraction is measured against the *glyph's* own rows, not
    /// `area`'s. `panel.dollar-pulse` is registered `Flex::Both`, so a wide
    /// dashboard can hand this widget far more height than an eight-row "$"
    /// needs, and the glyph is always flush with the top of whatever it is
    /// given (see [`Self::glyph_rect`]). Measuring the fill fraction against
    /// `area`'s full height would put the fill line somewhere below the
    /// glyph entirely on any panel taller than its own minimum, making the
    /// marker read as permanently empty exactly when it has the most room
    /// to be legible.
    fn render_pulse(self, area: Rect, buf: &mut Buffer, pixel_size: PixelSize, rows: u16) {
        Self::draw_glyph(area, buf, pixel_size, self.faint, "$");

        let glyph = Self::glyph_rect(area, rows);
        if glyph.is_empty() {
            return;
        }

        let level = self.level.clamp(0.0, 1.0);
        let filled = (f64::from(glyph.height) * level).round() as u16;
        let fill_from = glyph.height.saturating_sub(filled);

        for row in 0..glyph.height {
            let colour = if row >= fill_from {
                self.accent
            } else {
                self.faint
            };
            for col in 0..glyph.width {
                buf[(glyph.x + col, glyph.y + row)].set_style(colour);
            }
        }

        if let Some(frames_since) = self.pulse
            && frames_since < PULSE_BUDGET_FRAMES
            && fill_from > 0
        {
            let overshoot_row = fill_from - 1;
            let t = frames_since as f64 / PULSE_BUDGET_FRAMES as f64;
            let flash = ease_to_faint(self.faint, t);
            for col in 0..glyph.width {
                buf[(glyph.x + col, glyph.y + overshoot_row)].set_style(flash);
            }
        }
    }

    /// Treatment A: the glyph's own width narrows toward the centre and
    /// back across an eight-tick cycle keyed off `self.pulse` -- `None`
    /// (the rest frame) and a multiple of [`COIN_CYCLE_TICKS`] both render
    /// at full width. The narrowest tick, at the midpoint of the cycle,
    /// swaps the "$" for a single vertical bar and narrows to roughly a
    /// tenth of the panel's width, the way a coin looks edge-on mid-flip.
    ///
    /// This is a straightforward width interpolation of the `Rect` handed
    /// to `tui-big-text`'s own centred builder, not sub-cell interpolation:
    /// `BigText`'s `Rect`-based rendering already gives every intermediate
    /// tick a visibly narrower glyph on its own.
    fn render_coin(self, area: Rect, buf: &mut Buffer, pixel_size: PixelSize) {
        let tick = self
            .pulse
            .map_or(0, |frames_since| frames_since % COIN_CYCLE_TICKS);
        let half = COIN_CYCLE_TICKS / 2;
        let distance_from_rest = tick.min(COIN_CYCLE_TICKS - tick);
        let t = distance_from_rest as f64 / half as f64;
        let width_fraction = 1.0 - t * 0.9;

        let narrowed_width = ((f64::from(area.width) * width_fraction).round() as u16).max(1);
        let offset = (area.width - narrowed_width) / 2;
        let narrowed = Rect {
            x: area.x + offset,
            y: area.y,
            width: narrowed_width,
            height: area.height,
        };

        // A plain ASCII pipe rather than a box-drawing vertical line: the
        // `font8x8` crate `tui-big-text` renders through only covers the
        // ordinary ASCII range, so a box-drawing character would silently
        // draw nothing at all -- the coloured cell would be there, but no
        // visible glyph inside it.
        let symbol = if distance_from_rest == half { "|" } else { "$" };
        Self::draw_glyph(narrowed, buf, pixel_size, self.accent, symbol);
    }
}

/// A colour easing from a bright flash towards `faint` as `t` runs
/// `0.0..=1.0`.
///
/// By the time a colour reaches this widget it is already a plain
/// `ratatui::style::Color`, not `crate::tui::palette`'s own `Rgb` newtype --
/// every widget in this module takes its colours the same way, see that
/// module's doc for why the indirection exists. Inverting a `Color` back to
/// an `Rgb` to reuse `Palette`'s private `lighten` helper would tie this
/// widget to palette internals it has no other reason to depend on, so this
/// starts from plain white instead and eases towards `faint` -- the simpler
/// alternative this widget's own design deliberately allows for exactly
/// this case.
fn ease_to_faint(faint: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (fr, fg, fb) = as_rgb(faint);
    Color::Rgb(
        lerp_channel(255, fr, t),
        lerp_channel(255, fg, t),
        lerp_channel(255, fb, t),
    )
}

/// `faint` is always built from `Palette::faint`, an `Rgb`-backed colour, so
/// this only ever falls through to the white default in a test that hands
/// this widget a named colour on purpose.
fn as_rgb(colour: Color) -> (u8, u8, u8) {
    match colour {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (255, 255, 255),
    }
}

fn lerp_channel(from: u8, to: u8, t: f64) -> u8 {
    (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8
}

/// Tracks whether the figure this widget sits beside has just risen, so the
/// pulse can be armed without `App` owning two loosely-related fields that
/// only ever change together -- see the module doc for why this is an
/// Extract Class (Fowler, *Refactoring*) rather than two fields living
/// directly on `App`.
pub struct PulseClock {
    last_cost: Usd,
    started: Option<u64>,
}

impl PulseClock {
    /// A clock that has not yet seen a rise.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_cost: Usd::ZERO,
            started: None,
        }
    }

    /// Records the account's current total spend at animation `phase`.
    ///
    /// Arms the pulse only when `cost` is strictly greater than the last
    /// reading; a steady or falling total -- a fresh scan landing on the
    /// same figure, or a corrected one that comes back lower -- leaves the
    /// pulse exactly as it was, so neither ever reads as a fresh rise.
    pub fn observe(&mut self, cost: Usd, phase: u64) {
        if cost > self.last_cost {
            self.started = Some(phase);
        }
        self.last_cost = cost;
    }

    /// How many frames ago the pulse was armed, or `None` once that is
    /// further in the past than [`PULSE_BUDGET_FRAMES`] -- an overshoot
    /// this stale has nothing left to say.
    #[must_use]
    pub fn frames_since(&self, phase: u64) -> Option<u64> {
        self.started.and_then(|start| {
            let elapsed = phase.saturating_sub(start);
            if elapsed > PULSE_BUDGET_FRAMES {
                None
            } else {
                Some(elapsed)
            }
        })
    }
}

impl Default for PulseClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── `AnimationStyle` ────────────────────────────────────────────────

    #[test]
    fn the_documented_lowercase_config_spellings_actually_parse() {
        // `crate::infrastructure::config`'s own module doc quotes
        // `"animation": "pulse"` as the example, and the design this crate
        // shipped from names `"off" | "pulse" | "coin"` as the whole set --
        // both lowercase. Without `#[serde(rename_all = "snake_case")]` on
        // this enum, serde's default derive would only accept the Rust
        // variant spelling (`"Pulse"`), silently breaking every config file
        // written exactly as documented.
        for (json, expected) in [
            ("\"pulse\"", AnimationStyle::Pulse),
            ("\"coin\"", AnimationStyle::Coin),
            ("\"off\"", AnimationStyle::Off),
        ] {
            let parsed: AnimationStyle = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{json} failed to parse: {e}"));
            assert_eq!(parsed, expected, "{json} should parse as {expected:?}");
        }
    }

    // ── `PulseClock` ────────────────────────────────────────────────────

    #[test]
    fn a_rising_cost_arms_the_pulse() {
        let mut clock = PulseClock::new();
        clock.observe(Usd::new(1.0), 5);
        clock.observe(Usd::new(2.0), 9);
        assert_eq!(
            clock.frames_since(9),
            Some(0),
            "armed on the very frame it rose"
        );
    }

    #[test]
    fn an_unchanged_cost_does_not_re_arm_the_pulse() {
        let mut clock = PulseClock::new();
        clock.observe(Usd::new(1.0), 0); // the rise from zero arms it here
        clock.observe(Usd::new(1.0), PULSE_BUDGET_FRAMES + 5); // unchanged, long after
        // Had the second `observe` re-armed the pulse, this would read
        // `Some(0)`; instead it must still be counting from the original
        // rise at phase `0`, which is now long past its budget.
        assert_eq!(clock.frames_since(PULSE_BUDGET_FRAMES + 5), None);
    }

    #[test]
    fn a_falling_cost_does_not_re_arm_the_pulse() {
        // A falling total should never happen in practice -- spend only
        // accrues -- but a defensive reading (a corrected scan landing
        // lower, say) must not be misread as a fresh rise either.
        let mut clock = PulseClock::new();
        clock.observe(Usd::new(2.0), 0);
        clock.observe(Usd::new(1.0), PULSE_BUDGET_FRAMES + 5);
        assert_eq!(clock.frames_since(PULSE_BUDGET_FRAMES + 5), None);
    }

    #[test]
    fn frames_since_counts_forward_from_the_frame_the_rise_was_observed() {
        let mut clock = PulseClock::new();
        clock.observe(Usd::new(1.0), 100);
        assert_eq!(clock.frames_since(103), Some(3));
    }

    #[test]
    fn frames_since_returns_none_once_elapsed_exceeds_the_pulse_budget() {
        let mut clock = PulseClock::new();
        clock.observe(Usd::new(1.0), 0);
        assert_eq!(
            clock.frames_since(PULSE_BUDGET_FRAMES),
            Some(PULSE_BUDGET_FRAMES),
            "still within budget at the exact boundary"
        );
        assert_eq!(
            clock.frames_since(PULSE_BUDGET_FRAMES + 1),
            None,
            "one frame past the boundary, the overshoot has nothing left to say"
        );
    }

    // ── `DollarPulse` ───────────────────────────────────────────────────

    fn accent() -> Color {
        Color::Rgb(220, 90, 90)
    }

    fn faint() -> Color {
        Color::Rgb(70, 70, 80)
    }

    fn render(style: AnimationStyle, pulse: Option<u64>, level: f64, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        DollarPulse::new(level, accent(), faint())
            .pulsing(pulse)
            .style(style)
            .render(area, &mut buf);
        buf
    }

    #[test]
    fn off_style_never_produces_more_than_one_distinct_colour_across_a_full_pulse_window() {
        let area = Rect::new(0, 0, 10, 8);
        let baseline = render(AnimationStyle::Off, None, 0.6, area);

        for frame in 0..PULSE_BUDGET_FRAMES {
            let frame_buf = render(AnimationStyle::Off, Some(frame), 0.6, area);
            for y in 0..area.height {
                for x in 0..area.width {
                    let expected = &baseline[(x, y)];
                    let actual = &frame_buf[(x, y)];
                    assert_eq!(
                        actual.fg, expected.fg,
                        "fg changed at ({x},{y}) on frame {frame}, but Off must never animate"
                    );
                    assert_eq!(
                        actual.bg, expected.bg,
                        "bg changed at ({x},{y}) on frame {frame}"
                    );
                    assert_eq!(
                        actual.modifier, expected.modifier,
                        "modifier changed at ({x},{y}) on frame {frame}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_pulse_styles_resting_frame_fills_the_bottom_of_the_glyph_and_leaves_the_top_faint() {
        let area = Rect::new(0, 0, 10, 8);
        let buf = render(AnimationStyle::Pulse, None, 0.5, area);
        let glyph = DollarPulse::glyph_rect(area, 8);

        let top = buf[(glyph.x, glyph.y)].fg;
        let bottom = buf[(glyph.x, glyph.y + glyph.height - 1)].fg;
        assert_eq!(top, faint(), "the top of a half-full glyph stays faint");
        assert_eq!(bottom, accent(), "the bottom of a half-full glyph is lit");
        assert_ne!(top, bottom, "the fill/drain effect must be visible");
    }

    #[test]
    fn a_full_level_pulse_lights_every_row_of_the_glyph() {
        let area = Rect::new(0, 0, 10, 8);
        let buf = render(AnimationStyle::Pulse, None, 1.0, area);
        let glyph = DollarPulse::glyph_rect(area, 8);
        for row in 0..glyph.height {
            assert_eq!(buf[(glyph.x, glyph.y + row)].fg, accent());
        }
    }

    #[test]
    fn an_empty_level_pulse_leaves_every_row_faint() {
        let area = Rect::new(0, 0, 10, 8);
        let buf = render(AnimationStyle::Pulse, None, 0.0, area);
        let glyph = DollarPulse::glyph_rect(area, 8);
        for row in 0..glyph.height {
            assert_eq!(buf[(glyph.x, glyph.y + row)].fg, faint());
        }
    }

    #[test]
    fn a_fresh_rise_brightens_the_row_just_above_the_fill_line() {
        let area = Rect::new(0, 0, 10, 8);
        let resting = render(AnimationStyle::Pulse, None, 0.5, area);
        let pulsing = render(AnimationStyle::Pulse, Some(0), 0.5, area);
        let glyph = DollarPulse::glyph_rect(area, 8);
        let overshoot_row = glyph.y + glyph.height / 2 - 1;

        assert_ne!(
            resting[(glyph.x, overshoot_row)].fg,
            pulsing[(glyph.x, overshoot_row)].fg,
            "a frame-zero rise should brighten the overshoot row"
        );
    }

    #[test]
    fn an_overshoot_past_its_budget_is_not_drawn() {
        let area = Rect::new(0, 0, 10, 8);
        let resting = render(AnimationStyle::Pulse, None, 0.5, area);
        let stale = render(AnimationStyle::Pulse, Some(PULSE_BUDGET_FRAMES), 0.5, area);
        let glyph = DollarPulse::glyph_rect(area, 8);
        let overshoot_row = glyph.y + glyph.height / 2 - 1;

        assert_eq!(
            resting[(glyph.x, overshoot_row)].fg,
            stale[(glyph.x, overshoot_row)].fg,
            "a pulse this stale must not flash the overshoot row"
        );
    }

    #[test]
    fn the_marker_steps_down_a_size_before_it_would_be_clipped() {
        assert_eq!(DollarPulse::size_for(8), Some(PixelSize::Full));
        assert_eq!(DollarPulse::size_for(7), Some(PixelSize::HalfHeight));
        assert_eq!(DollarPulse::size_for(4), Some(PixelSize::HalfHeight));
    }

    #[test]
    fn below_the_smallest_size_the_marker_yields_its_space_entirely() {
        assert_eq!(DollarPulse::size_for(3), None);
        assert_eq!(DollarPulse::size_for(0), None);
    }

    #[test]
    fn drawing_into_a_tiny_area_leaves_it_untouched_instead_of_panicking() {
        for area in [
            Rect::new(0, 0, 4, 2),
            Rect::new(0, 0, 20, 3),
            Rect::new(0, 0, 3, 20),
        ] {
            let mut buf = Buffer::empty(area);
            DollarPulse::new(0.5, accent(), faint()).render(area, &mut buf);
            assert_eq!(buf[(area.x, area.y)].symbol(), " ");
        }
    }

    #[test]
    fn the_coin_style_narrows_toward_its_cycles_midpoint_and_swaps_the_glyph_there() {
        let area = Rect::new(0, 0, 40, 8);
        let rest = render(AnimationStyle::Coin, None, 0.0, area);
        let narrowest = render(AnimationStyle::Coin, Some(4), 0.0, area);

        let lit_in_row = |buf: &Buffer, y: u16| {
            (0..area.width)
                .filter(|&x| buf[(x, y)].symbol() != " ")
                .count()
        };
        let total_lit = |buf: &Buffer| (0..area.height).map(|y| lit_in_row(buf, y)).sum::<usize>();

        assert!(
            total_lit(&narrowest) < total_lit(&rest),
            "the narrowest tick should light fewer cells than the resting \"$\""
        );

        // "$" is a wide, curved glyph -- several of `rest`'s own rows light
        // more than one column (its resting render above confirms this
        // implicitly through `total_lit`). A single vertical bar lights at
        // most one column per row everywhere, which is what distinguishes
        // an actual glyph swap from merely squeezing the same "$" narrower.
        for y in 0..area.height {
            let width = lit_in_row(&narrowest, y);
            assert!(
                width <= 1,
                "row {y} of the narrowest tick should be at most one column wide, got {width}"
            );
        }
    }

    #[test]
    fn the_coin_style_is_at_full_width_both_at_rest_and_a_full_cycle_later() {
        let area = Rect::new(0, 0, 40, 8);
        let rest = render(AnimationStyle::Coin, None, 0.0, area);
        let full_cycle = render(AnimationStyle::Coin, Some(COIN_CYCLE_TICKS), 0.0, area);

        let lit = |buf: &Buffer| {
            (0..area.width)
                .filter(|&x| buf[(x, 0)].symbol() != " ")
                .count()
        };
        assert_eq!(lit(&rest), lit(&full_cycle));
    }
}

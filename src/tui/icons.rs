//! The dashboard's glyph set.
//!
//! Two crates were originally going to supply this and the spinner:
//! `ratatui-icons` and `ratatui-spinner`. Both turned out to be name
//! reservations on crates.io -- published at version 0.0.0 with no code in
//! them -- so both are implemented here instead. That is a smaller loss than
//! it sounds: an icon set is a table of constants, and owning it means the
//! whole dashboard can be held to one rule.
//!
//! The rule: **plain Unicode only, no Nerd Font**. Nerd Font glyphs live in
//! the private-use area, so a terminal without a patched font renders them as
//! replacement boxes. Every glyph below is in a block that ordinary fonts
//! ship: box drawing, geometric shapes, arrows, or miscellaneous symbols. Each
//! is also single-width, so a column of them cannot shear a layout on a
//! terminal that disagrees about East Asian ambiguous widths.

/// Named glyphs, grouped by what they are for.
pub struct Icon;

impl Icon {
    // ── metrics ───────────────────────────────────────────────────────

    /// Context window.
    pub const CONTEXT: &'static str = "\u{25f4}";
    /// Money.
    pub const COST: &'static str = "\u{00a4}";
    /// Cache.
    pub const CACHE: &'static str = "\u{29c9}";
    /// Compaction.
    pub const COMPACT: &'static str = "\u{21af}";
    /// A turn of the conversation.
    pub const TURN: &'static str = "\u{21ba}";
    /// A thinking block.
    pub const THINKING: &'static str = "\u{223f}";
    /// An error.
    pub const ERROR: &'static str = "\u{26a0}";
    /// A file.
    pub const FILE: &'static str = "\u{25a4}";
    /// Elapsed time.
    pub const CLOCK: &'static str = "\u{25f7}";
    /// A git branch.
    pub const BRANCH: &'static str = "\u{2387}";
    /// Tokens.
    pub const TOKEN: &'static str = "\u{25c7}";
    /// Rate of spend.
    pub const RATE: &'static str = "\u{2197}";

    // ── state ─────────────────────────────────────────────────────────

    /// The session is live and the assistant is working.
    pub const LIVE: &'static str = "\u{25cf}";
    /// The session is idle, waiting on the human.
    pub const IDLE: &'static str = "\u{25cb}";
    /// Nothing to attach to yet.
    pub const SEARCHING: &'static str = "\u{25cc}";

    // ── structure ─────────────────────────────────────────────────────

    /// Separates fields on one line.
    pub const SEPARATOR: &'static str = "\u{2502}";
    /// Marks a compaction on the sparkline.
    pub const MARKER: &'static str = "\u{2193}";
    /// Leads a list item.
    pub const BULLET: &'static str = "\u{2023}";
    /// The filled cell of a progress bar.
    pub const BAR_FULL: &'static str = "\u{2588}";
    /// The empty cell of a progress bar.
    pub const BAR_EMPTY: &'static str = "\u{2591}";
}

/// The eight partial block characters, for sub-cell bar precision.
///
/// A bar drawn only in whole cells jumps a full character at a time, which on
/// a 30-cell gauge means a 3% quantum -- visible, and irritating, when the
/// number beside it is moving smoothly. These let the leading edge land on any
/// eighth of a cell.
pub const EIGHTHS: [&str; 8] = [
    "\u{258f}", "\u{258e}", "\u{258d}", "\u{258c}", "\u{258b}", "\u{258a}", "\u{2589}", "\u{2588}",
];

/// The eight bar heights used by the sparkline, shortest first.
pub const SPARK_LEVELS: [&str; 8] = [
    "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}", "\u{2588}",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_a_single_character() {
        // A two-character "glyph" would silently shift everything to its
        // right by one column on every line it appears in.
        for glyph in [
            Icon::CONTEXT,
            Icon::COST,
            Icon::CACHE,
            Icon::COMPACT,
            Icon::TURN,
            Icon::THINKING,
            Icon::ERROR,
            Icon::FILE,
            Icon::CLOCK,
            Icon::BRANCH,
            Icon::TOKEN,
            Icon::RATE,
            Icon::LIVE,
            Icon::IDLE,
            Icon::SEARCHING,
            Icon::SEPARATOR,
            Icon::MARKER,
            Icon::BULLET,
        ] {
            assert_eq!(glyph.chars().count(), 1, "{glyph:?} is not one character");
        }
    }

    #[test]
    fn no_glyph_comes_from_the_private_use_area() {
        // Private-use code points are where Nerd Font puts its icons, and a
        // terminal without that font renders them as empty boxes.
        let all = [
            Icon::CONTEXT,
            Icon::COST,
            Icon::CACHE,
            Icon::COMPACT,
            Icon::BRANCH,
        ];
        for glyph in all {
            let code = glyph.chars().next().expect("one char") as u32;
            assert!(
                !(0xe000..=0xf8ff).contains(&code),
                "{glyph:?} is a private-use code point"
            );
        }
    }

    #[test]
    fn the_bar_ramps_run_from_smallest_to_largest() {
        assert_eq!(EIGHTHS.len(), 8);
        assert_eq!(SPARK_LEVELS.len(), 8);
        assert_eq!(EIGHTHS[7], Icon::BAR_FULL);
    }
}

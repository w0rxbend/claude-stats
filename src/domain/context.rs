//! How full the context window is, and how close the next compaction is.

use super::model::ModelCatalog;

/// How urgent the current context fill is.
///
/// The dashboard colours the context bar from this, rather than each widget
/// re-deriving its own thresholds from a raw percentage. Adding a band means
/// changing one enum and one match, not hunting for magic numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FillSeverity {
    /// Under 50% -- plenty of room.
    Comfortable,
    /// 50% to 75% -- worth an eye.
    Warm,
    /// 75% to 90% -- compaction is coming.
    Hot,
    /// Over 90%, or past the compaction threshold.
    Critical,
}

/// A snapshot of how much of the context window is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextFill {
    used: u64,
    window: u64,
}

impl ContextFill {
    /// Builds a fill reading from the prompt tokens of the latest response.
    #[must_use]
    pub const fn new(used: u64, window: u64) -> Self {
        Self { used, window }
    }

    /// Prompt tokens currently occupying the window.
    #[must_use]
    pub const fn used(self) -> u64 {
        self.used
    }

    /// The model's total context window, in tokens.
    #[must_use]
    pub const fn window(self) -> u64 {
        self.window
    }

    /// Tokens still free before the window is literally full.
    #[must_use]
    pub const fn remaining(self) -> u64 {
        self.window.saturating_sub(self.used)
    }

    /// Fill as a fraction in `0.0..=1.0`, clamped so the bar cannot overflow.
    ///
    /// The clamp is not defensive noise: a transcript can briefly report more
    /// prompt tokens than the catalogue's window when the model string is
    /// unrecognised and the conservative 200k default is being used.
    #[must_use]
    pub fn ratio(self) -> f64 {
        if self.window == 0 {
            return 0.0;
        }
        (self.used as f64 / self.window as f64).clamp(0.0, 1.0)
    }

    /// Fill as a percentage in `0.0..=100.0`.
    #[must_use]
    pub fn percent(self) -> f64 {
        self.ratio() * 100.0
    }

    /// Which colour band this fill falls into.
    #[must_use]
    pub fn severity(self) -> FillSeverity {
        match self.ratio() {
            r if r < 0.50 => FillSeverity::Comfortable,
            r if r < 0.75 => FillSeverity::Warm,
            r if r < 0.90 => FillSeverity::Hot,
            _ => FillSeverity::Critical,
        }
    }

    /// Tokens left before Claude Code auto-compacts.
    ///
    /// Compaction fires when free space falls below
    /// [`ModelCatalog::COMPACTION_BUFFER`], so the usable head-room is the
    /// window minus that buffer minus what is already used. Saturates at zero
    /// once the threshold has been passed.
    #[must_use]
    pub const fn tokens_until_compaction(self) -> u64 {
        self.window
            .saturating_sub(ModelCatalog::COMPACTION_BUFFER)
            .saturating_sub(self.used)
    }
}

/// An estimate of how many turns remain before the next auto-compaction.
///
/// Predicting this needs an average growth rate, and early in a session there
/// is not enough history to compute one. The three variants make that
/// three-way outcome explicit instead of encoding "unknown" as a sentinel
/// number the UI would have to remember to special-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionDistance {
    /// The compaction threshold has already been crossed.
    Imminent,
    /// Roughly this many more turns at the current growth rate.
    Turns(u32),
    /// Not enough turns yet to measure how fast context is growing.
    Unknown,
}

impl CompactionDistance {
    /// Estimates the distance from the current fill and the average growth.
    ///
    /// `average_growth_per_turn` is the mean increase in prompt tokens per
    /// turn since the last compaction. A non-positive rate means context is
    /// flat or shrinking, in which case no compaction is on the horizon and
    /// the honest answer is [`CompactionDistance::Unknown`] rather than a
    /// made-up large number.
    #[must_use]
    pub fn estimate(fill: ContextFill, average_growth_per_turn: f64) -> Self {
        let headroom = fill.tokens_until_compaction();
        if headroom == 0 {
            return Self::Imminent;
        }
        if average_growth_per_turn <= 0.0 {
            return Self::Unknown;
        }
        let turns = (headroom as f64 / average_growth_per_turn).floor();
        // Anything past a hundred turns is "not soon"; reporting a precise
        // four-digit number from a two-sample average would be false precision.
        if turns > 100.0 {
            return Self::Turns(100);
        }
        Self::Turns(turns as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fill_past_the_window_is_clamped_instead_of_overflowing_the_bar() {
        let fill = ContextFill::new(300_000, 200_000);
        assert!((fill.ratio() - 1.0).abs() < f64::EPSILON);
        assert_eq!(fill.remaining(), 0);
    }

    #[test]
    fn severity_bands_line_up_with_the_documented_thresholds() {
        let window = 1_000_000;
        assert_eq!(
            ContextFill::new(100_000, window).severity(),
            FillSeverity::Comfortable
        );
        assert_eq!(ContextFill::new(600_000, window).severity(), FillSeverity::Warm);
        assert_eq!(ContextFill::new(800_000, window).severity(), FillSeverity::Hot);
        assert_eq!(
            ContextFill::new(950_000, window).severity(),
            FillSeverity::Critical
        );
    }

    #[test]
    fn head_room_stops_at_the_compaction_buffer_not_at_the_window() {
        let fill = ContextFill::new(0, 200_000);
        assert_eq!(
            fill.tokens_until_compaction(),
            200_000 - ModelCatalog::COMPACTION_BUFFER
        );
    }

    #[test]
    fn crossing_the_threshold_reports_imminent() {
        let fill = ContextFill::new(199_000, 200_000);
        assert_eq!(
            CompactionDistance::estimate(fill, 1_000.0),
            CompactionDistance::Imminent
        );
    }

    #[test]
    fn a_flat_growth_rate_reports_unknown_rather_than_a_fabricated_distance() {
        let fill = ContextFill::new(10_000, 200_000);
        assert_eq!(
            CompactionDistance::estimate(fill, 0.0),
            CompactionDistance::Unknown
        );
    }

    #[test]
    fn a_far_off_compaction_is_reported_as_a_capped_hundred_turns() {
        let fill = ContextFill::new(0, 1_000_000);
        assert_eq!(
            CompactionDistance::estimate(fill, 10.0),
            CompactionDistance::Turns(100)
        );
    }
}

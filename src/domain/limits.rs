//! Account-wide usage across rolling time windows, and the rate limits that
//! have actually been hit.
//!
//! Everything else in this crate is about *one* session. Rate limits are not:
//! they are consumed by every session on the account at once, so answering
//! "how much have I used lately" means adding up work that happened in other
//! terminals, other projects and other days.
//!
//! # What is measured, and what is not
//!
//! Claude Code's limits are enforced on Anthropic's side, and the only number
//! that would say "you are 47% of the way to your limit" lives there. It is
//! not written to disk: `/usage` fetches it live. So this module deliberately
//! does **not** pretend to know a percentage. It reports two things, both of
//! which are either measured or exact:
//!
//! * [`WindowUsage`] -- what you actually spent inside a rolling window, added
//!   up from the transcripts themselves.
//! * [`LimitEvent`] -- the moments you were genuinely refused, taken from the
//!   `quotaLimits` block the API attaches to a 429, including the exact
//!   instant the limit lifts.
//!
//! Reading a bar in the first as "share of my limit" would be wrong. It is a
//! share of [`WindowUsage::peak`] -- your own busiest comparable window -- and
//! it is labelled that way on screen.

use chrono::{DateTime, Duration, Utc};

use super::money::Usd;
use super::tokens::TokenUsage;

/// One assistant response, reduced to what a usage window cares about.
///
/// The scanner turns whole transcripts into these and throws the rest away.
/// A window is then a filter over points, which keeps the arithmetic in the
/// domain and the file handling in the infrastructure where it belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsagePoint {
    /// When the response was recorded.
    pub at: DateTime<Utc>,
    /// What it consumed.
    pub tokens: TokenUsage,
    /// What it cost, priced at the model that produced it.
    pub cost: Usd,
}

/// Which rolling window a figure describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// The five-hour window Claude Code calls a "session limit".
    Session,
    /// The seven-day window Claude Code calls a "weekly limit".
    Week,
}

impl WindowKind {
    /// How far back the window reaches.
    #[must_use]
    pub fn span(self) -> Duration {
        match self {
            Self::Session => Duration::hours(5),
            Self::Week => Duration::days(7),
        }
    }

    /// The name to put on the panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Session => "session window",
            Self::Week => "week",
        }
    }

    /// The window's span, spelled the way the limit is spoken about.
    #[must_use]
    pub const fn span_label(self) -> &'static str {
        match self {
            Self::Session => "5h",
            Self::Week => "7d",
        }
    }
}

/// What was consumed inside one rolling window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowUsage {
    /// Which window this describes.
    pub kind: WindowKind,
    /// Tokens consumed inside it.
    pub tokens: TokenUsage,
    /// What those tokens cost.
    pub cost: Usd,
    /// How many distinct sessions contributed.
    pub sessions: usize,
    /// The instant the window opens: `now - kind.span()`.
    pub since: DateTime<Utc>,
    /// The most tokens any comparable window has held, over all the history
    /// that was scanned.
    ///
    /// This is what the on-screen bar is drawn against, because it is the only
    /// ceiling that is actually known. `None` when there is no history to
    /// compare against yet.
    pub peak: Option<u64>,
    /// How many distinct limit periods began inside this window.
    ///
    /// The count that answers "am I running into this repeatedly, or was that
    /// one bad afternoon". Zero for a window in which nothing was refused.
    pub limit_periods: usize,
    /// When the most recent limit inside this window began.
    pub last_limit_at: Option<DateTime<Utc>>,
}

impl WindowUsage {
    /// An empty window ending now.
    #[must_use]
    pub fn empty(kind: WindowKind, now: DateTime<Utc>) -> Self {
        Self {
            kind,
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            sessions: 0,
            since: now - kind.span(),
            peak: None,
            limit_periods: 0,
            last_limit_at: None,
        }
    }

    /// How full this window is compared with the busiest one on record, in
    /// `0.0..=1.0`.
    ///
    /// Explicitly *not* a share of any rate limit -- see the module docs.
    /// `None` when there is no peak to compare against, or the peak is zero,
    /// in which case a bar would be meaningless rather than empty.
    #[must_use]
    pub fn share_of_peak(&self) -> Option<f64> {
        let peak = self.peak?;
        if peak == 0 {
            return None;
        }
        Some((self.tokens.total() as f64 / peak as f64).clamp(0.0, 1.0))
    }
}

/// A moment when the API actually refused a request because a limit was hit.
///
/// Unlike [`WindowUsage`], none of this is inferred: it is read straight from
/// the `quotaLimits` block on a 429 response, so the reset instant is the
/// server's own answer rather than an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitEvent {
    /// When the request was refused.
    pub at: DateTime<Utc>,
    /// When the limit lifts, as the server reported it.
    pub resets_at: DateTime<Utc>,
    /// Which limit was hit.
    pub kind: WindowKind,
}

impl LimitEvent {
    /// Whether the limit is still in force at `now`.
    #[must_use]
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        now < self.resets_at
    }

    /// How long until the limit lifts, or `None` once it has.
    #[must_use]
    pub fn time_until_reset(&self, now: DateTime<Utc>) -> Option<Duration> {
        self.is_active_at(now).then(|| self.resets_at - now)
    }
}

/// Everything the dashboard knows about account-wide usage.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    /// The last five hours.
    pub session: WindowUsage,
    /// The last seven days.
    pub week: WindowUsage,
    /// Every refusal seen in the scanned history, oldest first.
    pub limit_events: Vec<LimitEvent>,
    /// When this was computed.
    pub measured_at: DateTime<Utc>,
}

impl AccountUsage {
    /// Nothing measured yet.
    #[must_use]
    pub fn empty(now: DateTime<Utc>) -> Self {
        Self {
            session: WindowUsage::empty(WindowKind::Session, now),
            week: WindowUsage::empty(WindowKind::Week, now),
            limit_events: Vec::new(),
            measured_at: now,
        }
    }

    /// Adds up `points` into both windows, and records which limits were hit.
    ///
    /// `points` may arrive in any order and from any number of sessions;
    /// `sessions` is how many distinct transcripts contributed each point, so
    /// the count is taken from the caller rather than guessed from timestamps.
    #[must_use]
    pub fn measure(
        now: DateTime<Utc>,
        contributions: &[SessionContribution],
        limit_events: Vec<LimitEvent>,
    ) -> Self {
        let mut usage = Self::empty(now);
        let mut limit_events = limit_events;
        limit_events.sort_by_key(|e| e.at);
        for kind in [WindowKind::Session, WindowKind::Week] {
            let since = now - kind.span();
            let mut window = WindowUsage::empty(kind, now);
            for contribution in contributions {
                let mut contributed = false;
                for point in &contribution.points {
                    if point.at >= since && point.at <= now {
                        window.tokens += point.tokens;
                        window.cost += point.cost;
                        contributed = true;
                    }
                }
                if contributed {
                    window.sessions += 1;
                }
            }
            window.peak = peak_window(contributions, kind, now);

            // A limit that bit inside this window is part of what the window
            // describes: "2.5B tokens and two limits" is a different week from
            // "2.5B tokens and none".
            let inside: Vec<&LimitEvent> = limit_events
                .iter()
                .filter(|e| e.at >= since && e.at <= now)
                .collect();
            window.limit_periods = inside.len();
            window.last_limit_at = inside.iter().map(|e| e.at).max();

            match kind {
                WindowKind::Session => usage.session = window,
                WindowKind::Week => usage.week = window,
            }
        }
        usage.limit_events = limit_events;
        usage
    }

    /// The limit currently in force, if any.
    #[must_use]
    pub fn active_limit(&self) -> Option<LimitEvent> {
        self.limit_events
            .iter()
            .filter(|e| e.is_active_at(self.measured_at))
            .max_by_key(|e| e.resets_at)
            .copied()
    }

    /// The most recent refusal, whether or not it is still in force.
    #[must_use]
    pub fn last_limit(&self) -> Option<LimitEvent> {
        self.limit_events.last().copied()
    }
}

/// One session's contribution to the account's usage.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionContribution {
    /// Which transcript these came from, so a session is counted once.
    pub session_id: String,
    /// Its responses, oldest first.
    pub points: Vec<UsagePoint>,
}

/// The busiest comparable window anywhere in the scanned history.
///
/// Every point is tried as a window *start*, which is enough: a window that
/// contains any points at all can always be slid forward until it begins on
/// one without losing any, so the maximum is always found at a point.
fn peak_window(
    contributions: &[SessionContribution],
    kind: WindowKind,
    now: DateTime<Utc>,
) -> Option<u64> {
    let mut points: Vec<(DateTime<Utc>, u64)> = contributions
        .iter()
        .flat_map(|c| c.points.iter())
        .map(|p| (p.at, p.tokens.total()))
        .collect();
    if points.is_empty() {
        return None;
    }
    points.sort_by_key(|(at, _)| *at);

    let span = kind.span();
    let mut peak = 0;
    let mut running = 0;
    let mut start = 0;
    for end in 0..points.len() {
        running += points[end].1;
        // Drop everything that has fallen out of the back of the window.
        while points[end].0 - points[start].0 >= span {
            running -= points[start].1;
            start += 1;
        }
        peak = peak.max(running);
    }
    // A window that is still filling should not be measured against itself:
    // it would sit at 100% from the first response of the day.
    let current = points
        .iter()
        .filter(|(at, _)| *at >= now - span)
        .map(|(_, tokens)| tokens)
        .sum::<u64>();
    if peak == current { None } else { Some(peak) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(minutes * 60, 0).expect("valid timestamp")
    }

    fn point(minutes: i64, tokens: u64) -> UsagePoint {
        UsagePoint {
            at: at(minutes),
            tokens: TokenUsage {
                input: tokens,
                ..TokenUsage::ZERO
            },
            cost: Usd::new(1.0),
        }
    }

    fn contribution(id: &str, points: Vec<UsagePoint>) -> SessionContribution {
        SessionContribution {
            session_id: id.to_owned(),
            points,
        }
    }

    #[test]
    fn a_window_counts_only_what_falls_inside_it() {
        let now = at(600);
        // 10 hours in: the first point is outside the five-hour window, the
        // second is inside it, and both are inside the week.
        let usage = AccountUsage::measure(
            now,
            &[contribution("a", vec![point(60, 100), point(400, 250)])],
            Vec::new(),
        );

        assert_eq!(usage.session.tokens.total(), 250);
        assert_eq!(usage.week.tokens.total(), 350);
    }

    #[test]
    fn a_session_is_counted_once_however_many_responses_it_contributed() {
        let now = at(600);
        let usage = AccountUsage::measure(
            now,
            &[
                contribution("a", vec![point(400, 10), point(410, 10), point(420, 10)]),
                contribution("b", vec![point(430, 10)]),
            ],
            Vec::new(),
        );

        assert_eq!(usage.session.sessions, 2);
        assert_eq!(usage.session.tokens.total(), 40);
    }

    #[test]
    fn a_session_that_contributed_nothing_to_the_window_is_not_counted() {
        let now = at(600);
        let usage = AccountUsage::measure(
            now,
            &[
                contribution("recent", vec![point(400, 10)]),
                contribution("ancient", vec![point(1, 10)]),
            ],
            Vec::new(),
        );

        assert_eq!(usage.session.sessions, 1);
        assert_eq!(usage.week.sessions, 2, "both are inside the week");
    }

    #[test]
    fn the_peak_is_the_busiest_window_anywhere_in_the_history() {
        let now = at(10_000);
        // A burst of 900 early on, then a quiet 100 recently. The busiest
        // five-hour window is the burst, not the window we are in.
        let usage = AccountUsage::measure(
            now,
            &[contribution(
                "a",
                vec![point(100, 400), point(150, 500), point(9_900, 100)],
            )],
            Vec::new(),
        );

        assert_eq!(usage.session.peak, Some(900));
        assert_eq!(usage.session.tokens.total(), 100);
        let share = usage.session.share_of_peak().expect("a peak to compare to");
        assert!((share - 100.0 / 900.0).abs() < 1e-9);
    }

    #[test]
    fn the_current_window_is_not_measured_against_itself() {
        let now = at(600);
        // The only activity there has ever been is inside the current window,
        // so it *is* the peak. Drawing a full bar would say "you are at your
        // busiest ever" from the first response of the day.
        let usage = AccountUsage::measure(now, &[contribution("a", vec![point(590, 100)])], vec![]);

        assert_eq!(usage.session.peak, None);
        assert_eq!(usage.session.share_of_peak(), None);
    }

    #[test]
    fn each_window_counts_the_limits_that_bit_inside_it() {
        let now = at(60 * 24 * 3); // three days in
        let recent = LimitEvent {
            at: now - Duration::hours(2),
            resets_at: now - Duration::hours(1),
            kind: WindowKind::Session,
        };
        let older = LimitEvent {
            at: now - Duration::days(2),
            resets_at: now - Duration::days(2) + Duration::hours(1),
            kind: WindowKind::Session,
        };
        let ancient = LimitEvent {
            at: now - Duration::days(20),
            resets_at: now - Duration::days(20) + Duration::hours(1),
            kind: WindowKind::Session,
        };
        let usage = AccountUsage::measure(now, &[], vec![ancient, recent, older]);

        assert_eq!(usage.session.limit_periods, 1, "only the one two hours ago");
        assert_eq!(usage.session.last_limit_at, Some(recent.at));

        assert_eq!(usage.week.limit_periods, 2, "the ancient one is outside");
        assert_eq!(
            usage.week.last_limit_at,
            Some(recent.at),
            "the most recent inside the week"
        );
    }

    #[test]
    fn a_window_with_no_refusals_reports_none_rather_than_a_zero_date() {
        let now = at(600);
        let usage = AccountUsage::measure(now, &[], Vec::new());

        assert_eq!(usage.session.limit_periods, 0);
        assert_eq!(usage.session.last_limit_at, None);
        assert_eq!(usage.active_limit(), None);
    }

    #[test]
    fn an_active_limit_is_the_one_that_lifts_last() {
        let now = at(600);
        let expired = LimitEvent {
            at: at(100),
            resets_at: at(200),
            kind: WindowKind::Session,
        };
        let active = LimitEvent {
            at: at(580),
            resets_at: at(700),
            kind: WindowKind::Session,
        };
        let usage = AccountUsage::measure(now, &[], vec![active, expired]);

        assert_eq!(usage.active_limit(), Some(active));
        assert_eq!(
            usage.last_limit(),
            Some(active),
            "events are sorted by time"
        );
        assert_eq!(
            active.time_until_reset(now).map(|d| d.num_minutes()),
            Some(100)
        );
        assert_eq!(expired.time_until_reset(now), None);
    }
}

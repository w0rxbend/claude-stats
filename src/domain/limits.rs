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

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use super::blocks::{self, BillingBlock, BlockKind, BurnRate, DEFAULT_SPAN_HOURS, Projection};
use super::entry::Entry;
use super::money::Usd;
use super::period::Zone;
use super::pricing::{CostMode, PriceSheet};
use super::project::{Project, SessionId};
use super::report::ModelBreakdown;
use super::tokens::TokenUsage;

/// How many of the busiest projects the live panel keeps.
///
/// Five, matching the panel's own budget of rows: a busiest-projects list
/// that scrolled off the bottom of its own widget would be a list nobody
/// could act on, so the truncation is a domain rule rather than something the
/// widget decides for itself while rendering.
const TOP_PROJECTS_LIMIT: usize = 5;

/// The first instant of the calendar month containing `at`, in UTC.
///
/// UTC rather than local time, because every timestamp in a transcript is UTC
/// and mixing zones would make the month totals disagree with the windows
/// above them by a few hours at each boundary.
///
/// # Panics
///
/// Never in practice: the first of a month at midnight exists for every date.
#[must_use]
pub fn month_start(at: DateTime<Utc>) -> DateTime<Utc> {
    at.date_naive()
        .with_day(1)
        .expect("the first of a month always exists")
        .and_hms_opt(0, 0, 0)
        .expect("midnight always exists")
        .and_utc()
}

/// The first instant of the calendar month before the one containing `at`.
#[must_use]
pub fn previous_month_start(at: DateTime<Utc>) -> DateTime<Utc> {
    month_start(month_start(at) - Duration::days(1))
}

/// Which rolling window a figure describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

/// What one calendar month cost, added up from the scanned transcripts.
///
/// Unlike a [`WindowUsage`] this is anchored to the calendar rather than
/// rolling back from now, because "what have I spent this month" is a question
/// about a billing-shaped period, not a sliding one. Like everything else
/// here it is measured from what is on disk: transcripts older than the scan
/// horizon are not opened, so a month that started before the horizon reports
/// only the part of it that was scanned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonthUsage {
    /// The first instant of the month, so a renderer can name it.
    pub starts: DateTime<Utc>,
    /// Tokens consumed inside the month.
    pub tokens: TokenUsage,
    /// What those tokens cost.
    pub cost: Usd,
    /// How many distinct sessions contributed.
    pub sessions: usize,
}

impl MonthUsage {
    /// An empty month beginning at `starts`.
    #[must_use]
    pub const fn empty(starts: DateTime<Utc>) -> Self {
        Self {
            starts,
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            sessions: 0,
        }
    }

    /// The month's name, e.g. `"August"`.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.starts.month() {
            1 => "January",
            2 => "February",
            3 => "March",
            4 => "April",
            5 => "May",
            6 => "June",
            7 => "July",
            8 => "August",
            9 => "September",
            10 => "October",
            11 => "November",
            _ => "December",
        }
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

    /// Collapses a run of refusals into the limit periods they belong to.
    ///
    /// Being refused ten times in the twenty minutes before a reset is one
    /// limit being hit, not ten: every one of those refusals carries the same
    /// reset instant, and that instant together with the kind of limit is what
    /// identifies the period. The earliest refusal in each period is the one
    /// kept, because that is when the limit began to bite.
    ///
    /// This is a rule about what a limit period *is*, so it lives here rather
    /// than in whichever adapter happened to collect the refusals. Running it
    /// over an already-collapsed list changes nothing.
    #[must_use]
    pub fn collapse_periods(mut events: Vec<Self>) -> Vec<Self> {
        // The sort key must start with the whole dedup key: `dedup_by_key`
        // only removes *consecutive* duplicates, so sorting by `resets_at`
        // alone would interleave a five-hour and a weekly refusal that share a
        // reset instant and let both survive. The trailing `at` is what makes
        // the earliest refusal of each period the one that stays.
        events.sort_by_key(|e| (e.resets_at, e.kind, e.at));
        events.dedup_by_key(|e| (e.resets_at, e.kind));
        events
    }
}

/// What was spent inside one named calendar period.
///
/// A Value Object in Fowler's sense, and deliberately a lighter one than
/// [`WindowUsage`]: "today" has no peak to compare against and no limits of
/// its own to count, because it is not a rolling window at all -- it is one
/// calendar day, named so the panel that shows it does not have to invent a
/// label.
#[derive(Debug, Clone, PartialEq)]
pub struct PeriodUsage {
    /// The name to put beside the figures, e.g. `"today"`.
    pub label: String,
    /// Tokens consumed inside the period.
    pub tokens: TokenUsage,
    /// What those tokens cost.
    pub cost: Usd,
    /// How many distinct sessions contributed.
    pub sessions: usize,
}

impl PeriodUsage {
    /// Nothing measured yet for the period named `label`.
    #[must_use]
    pub fn empty(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tokens: TokenUsage::ZERO,
            cost: Usd::ZERO,
            sessions: 0,
        }
    }
}

/// What one project cost inside the window the busiest-projects panel looks
/// at.
///
/// Another Value Object of the same shape as [`PeriodUsage`], carrying a
/// [`Project`] instead of a label because the panel needs the whole path, not
/// just something to print: [`Project::display_name`] is a rendering
/// decision the widget makes for itself, and the domain has no business
/// deciding how much of a path fits on a screen.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectUsage {
    /// The working directory this spend was run from.
    pub project: Project,
    /// Tokens consumed by it inside the window.
    pub tokens: TokenUsage,
    /// What those tokens cost.
    pub cost: Usd,
    /// How many distinct sessions ran from this project inside the window.
    pub sessions: usize,
}

/// Everything the dashboard knows about account-wide usage.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    /// The last five hours.
    pub session: WindowUsage,
    /// The last seven days.
    pub week: WindowUsage,
    /// The calendar month containing `measured_at`.
    pub this_month: MonthUsage,
    /// The calendar month before it.
    pub last_month: MonthUsage,
    /// One event per distinct limit period in the scanned history, oldest
    /// first. A run of refusals sharing a reset instant is collapsed to its
    /// earliest by [`LimitEvent::collapse_periods`].
    pub limit_events: Vec<LimitEvent>,
    /// When this was computed.
    pub measured_at: DateTime<Utc>,
    /// The local calendar day containing [`Self::measured_at`].
    ///
    /// Bucketed on the zone `measure` was given, not on UTC every other field
    /// here is stamped in: a person's "today" is the day on their own wall
    /// clock, and a response written after midnight UTC but before midnight
    /// at home still belongs to yesterday from where they are sitting. See
    /// [`crate::domain::period`] for why the zone is a parameter rather than
    /// something read from the environment here.
    pub today: PeriodUsage,
    /// The billing block currently running, if any.
    ///
    /// Computed by the same [`blocks::identify`] fold that
    /// `claude-stats blocks` uses, over the same entries and the same span,
    /// so the live panel and the printed report can never disagree about
    /// when the current window opened.
    pub active_block: Option<BillingBlock>,
    /// How fast [`Self::active_block`] is being consumed.
    pub active_burn: Option<BurnRate>,
    /// Where [`Self::active_block`] is expected to land if the current rate
    /// holds for the rest of its window.
    pub active_projection: Option<Projection>,
    /// The busiest projects inside the last seven days, dearest first,
    /// capped at [`TOP_PROJECTS_LIMIT`].
    pub top_projects: Vec<ProjectUsage>,
    /// What was spent on each of the last seven calendar days, oldest first.
    ///
    /// A day with no activity is absent rather than present with a zero --
    /// the same convention [`crate::domain::report::UsageReport`]'s own daily
    /// grouping already keeps, for the same reason: a gap in the series says
    /// "nothing happened", which is a different claim from "something
    /// happened and cost nothing". Bucketed on [`Zone::local_date`], the same
    /// zone [`Self::today`] is bucketed on, so a response just after local
    /// midnight lands on the calendar day a person watching their own clock
    /// would agree it belongs to.
    pub daily_spend: Vec<(NaiveDate, Usd)>,
    /// What each model contributed inside the last seven days, dearest
    /// first -- the same window, and the same [`ModelBreakdown`] shape,
    /// [`crate::domain::report::UsageRow::breakdown`] already uses for one
    /// bucket of a historical report.
    pub model_breakdown: Vec<ModelBreakdown>,
}

impl AccountUsage {
    /// Nothing measured yet.
    #[must_use]
    pub fn empty(now: DateTime<Utc>) -> Self {
        Self {
            session: WindowUsage::empty(WindowKind::Session, now),
            week: WindowUsage::empty(WindowKind::Week, now),
            this_month: MonthUsage::empty(month_start(now)),
            last_month: MonthUsage::empty(previous_month_start(now)),
            limit_events: Vec::new(),
            measured_at: now,
            today: PeriodUsage::empty("today"),
            active_block: None,
            active_burn: None,
            active_projection: None,
            top_projects: Vec::new(),
            daily_spend: Vec::new(),
            model_breakdown: Vec::new(),
        }
    }

    /// Adds up `entries` into both windows and both months, and records which
    /// limits bit inside each one.
    ///
    /// `entries` is the deduplicated stream of billable responses -- the same
    /// atom every other historical report in this crate folds over. It used to
    /// be a nested shape of one list per transcript, which was a second way of
    /// saying the same thing and therefore a second thing that could disagree
    /// about a total; an [`Entry`] already carries its own session, so the
    /// nesting bought nothing and cost the crate a duplicate deduplication
    /// story. Entries may arrive in any order: one counts toward a window when
    /// `since <= entry.at <= now`.
    ///
    /// [`WindowUsage::sessions`] counts *distinct* [`Entry::session`] values,
    /// not entries and not the transcripts they were read from. All three
    /// differ, and by a lot: a session's sub-agents each write their own
    /// transcript while belonging to the session that spawned them, so
    /// counting files would report a morning's work as several hundred
    /// sessions.
    ///
    /// Cost is worked out here rather than being carried in, so that a
    /// correction to the rates corrects this reading too instead of leaving it
    /// quoting a figure that was never charged. It goes through
    /// [`CostMode::default`] -- that is, [`CostMode::Auto`] -- rather than
    /// pricing the counters outright, so that an entry which does state what
    /// it cost is believed rather than silently recomputed. No Claude Code
    /// transcript states one today, so the two agree everywhere; the point is
    /// that they will still agree the day one does.
    ///
    /// `prices` is handed in rather than reached for, which is the whole point
    /// of [`PriceSheet`] being a value: this reading can then be costed at the
    /// rates the rest of the run used, including a user's own corrections, and
    /// a test can cost it at rates it chose. A model no row matches is charged
    /// the sheet's fallback rather than nothing -- an uncatalogued model was
    /// still sold, and pricing it at zero would quietly subtract it from the
    /// account total.
    ///
    /// `limit_events` may be the raw refusals; they are collapsed into distinct
    /// limit periods by [`LimitEvent::collapse_periods`] and then sorted oldest
    /// first. Each window's [`WindowUsage::limit_periods`] and
    /// [`WindowUsage::last_limit_at`] reflect only the periods inside it.
    ///
    /// `zone` decides whose calendar [`Self::today`] is bucketed on. Every
    /// other field here is anchored either to UTC (the months, because every
    /// timestamp on disk is UTC and mixing zones would make them disagree
    /// with the windows by a few hours at each boundary) or to a rolling span
    /// that has no calendar at all (the windows, the active block). "Today"
    /// is the one figure this function produces that is a calendar question,
    /// and a calendar question has no zone-free answer -- see
    /// [`crate::domain::period`] for the reasoning in full. The composition
    /// root passes [`Zone::Local`], so a user's "today" is their own; every
    /// test here passes an explicit zone instead, so the bucketing stays
    /// deterministic wherever the test happens to run.
    #[must_use]
    pub fn measure(
        now: DateTime<Utc>,
        entries: &[Entry],
        limit_events: Vec<LimitEvent>,
        prices: &PriceSheet,
        zone: &Zone,
    ) -> Self {
        let mut usage = Self::empty(now);
        let mut limit_events = LimitEvent::collapse_periods(limit_events);
        // Collapsing groups by reset instant; the field is documented oldest
        // first by the moment of refusal, which the reports and the panel
        // iterate in order.
        limit_events.sort_by_key(|e| e.at);

        // Every entry is priced exactly once, up front. Four passes follow --
        // two windows and two months -- and pricing inside each of them would
        // ask the catalogue the same question four times per response, for
        // hundreds of thousands of responses, to get the same answer.
        let costs: Vec<Usd> = entries
            .iter()
            .map(|entry| CostMode::default().cost_of(entry, prices))
            .collect();

        // The peak needs the whole history in time order, and it needs it for
        // both window kinds. Sorting once here rather than inside
        // `peak_window` is the difference between one sort and one per kind.
        let mut timeline: Vec<(DateTime<Utc>, u64)> = entries
            .iter()
            .map(|entry| (entry.at, entry.tokens.total()))
            .collect();
        timeline.sort_unstable_by_key(|(at, _)| *at);

        for kind in [WindowKind::Session, WindowKind::Week] {
            let since = now - kind.span();
            let mut window = WindowUsage::empty(kind, now);
            let mut contributing: std::collections::BTreeSet<&SessionId> =
                std::collections::BTreeSet::new();
            for (entry, cost) in entries.iter().zip(&costs) {
                if entry.at >= since && entry.at <= now {
                    window.tokens += entry.tokens;
                    window.cost += *cost;
                    contributing.insert(&entry.session);
                }
            }
            window.sessions = contributing.len();
            window.peak = peak_window(&timeline, kind, now);

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

        // The two calendar months. A month is a half-open range: everything
        // from its first instant up to (but not including) the next month's,
        // so no entry can be counted in both.
        let this_start = month_start(now);
        let last_start = previous_month_start(now);
        let next_start = month_start(this_start + Duration::days(32));
        for (month, from, until) in [
            (&mut usage.this_month, this_start, next_start),
            (&mut usage.last_month, last_start, this_start),
        ] {
            let mut contributing: std::collections::BTreeSet<&SessionId> =
                std::collections::BTreeSet::new();
            for (entry, cost) in entries.iter().zip(&costs) {
                if entry.at >= from && entry.at < until {
                    month.tokens += entry.tokens;
                    month.cost += *cost;
                    contributing.insert(&entry.session);
                }
            }
            month.sessions = contributing.len();
        }

        // Today's bucket, the busiest projects and models inside the week,
        // and the week's daily series, gathered in one further pass over the
        // entries already priced above rather than in three or four more of
        // their own -- see the helper's own doc.
        let week = WeekBreakdowns::gather(now, entries, &costs, zone);
        usage.today = week.today;
        usage.top_projects = week.top_projects;
        usage.daily_spend = week.daily_spend;
        usage.model_breakdown = week.model_breakdown;

        // The billing block currently running, and how it is going.
        let (active_block, active_burn, active_projection) =
            active_block_reading(entries, now, prices);
        usage.active_block = active_block;
        usage.active_burn = active_burn;
        usage.active_projection = active_projection;

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

/// Today's bucket, the busiest projects and models inside the week, and the
/// week's own daily series, gathered in one pass over `entries` and their
/// already-priced `costs`.
///
/// Split out of [`AccountUsage::measure`] so that function stays a list of
/// steps rather than a single sprawling block: all four questions are folded
/// together here for the reason given at the call site (one traversal rather
/// than several), and pulling the traversal out into its own named function
/// is what keeps that reason visible without the caller having to hold this
/// much detail in mind at once.
struct WeekBreakdowns {
    today: PeriodUsage,
    top_projects: Vec<ProjectUsage>,
    daily_spend: Vec<(NaiveDate, Usd)>,
    model_breakdown: Vec<ModelBreakdown>,
}

impl WeekBreakdowns {
    fn gather(now: DateTime<Utc>, entries: &[Entry], costs: &[Usd], zone: &Zone) -> Self {
        let today_bounds = zone.day_bounds(zone.local_date(now));
        let week_since = now - WindowKind::Week.span();
        let mut today = PeriodUsage::empty("today");
        let mut today_sessions: std::collections::BTreeSet<&SessionId> =
            std::collections::BTreeSet::new();
        let mut projects: std::collections::HashMap<
            &Project,
            (TokenUsage, Usd, std::collections::BTreeSet<&SessionId>),
        > = std::collections::HashMap::new();
        let mut models: std::collections::HashMap<&super::model::ModelId, (TokenUsage, Usd)> =
            std::collections::HashMap::new();
        // A `BTreeMap` rather than a `HashMap`, so the days it holds come out
        // in calendar order for free -- `daily_spend` promises oldest first,
        // and a day with no entry inside the week never gets a key at all,
        // which is what keeps a quiet day absent rather than present with a
        // zero (see `AccountUsage::daily_spend`'s own doc for why).
        let mut days: std::collections::BTreeMap<NaiveDate, Usd> =
            std::collections::BTreeMap::new();

        for (entry, cost) in entries.iter().zip(costs) {
            if entry.at >= today_bounds.0 && entry.at < today_bounds.1 {
                today.tokens += entry.tokens;
                today.cost += *cost;
                today_sessions.insert(&entry.session);
            }
            if entry.at >= week_since && entry.at <= now {
                let bucket = projects.entry(&entry.project).or_insert_with(|| {
                    (
                        TokenUsage::ZERO,
                        Usd::ZERO,
                        std::collections::BTreeSet::new(),
                    )
                });
                bucket.0 += entry.tokens;
                bucket.1 += *cost;
                bucket.2.insert(&entry.session);

                let model_bucket = models
                    .entry(&entry.model)
                    .or_insert((TokenUsage::ZERO, Usd::ZERO));
                model_bucket.0 += entry.tokens;
                model_bucket.1 += *cost;

                *days.entry(zone.local_date(entry.at)).or_insert(Usd::ZERO) += *cost;
            }
        }
        today.sessions = today_sessions.len();

        let mut top_projects: Vec<ProjectUsage> = projects
            .into_iter()
            .map(|(project, (tokens, cost, sessions))| ProjectUsage {
                project: project.clone(),
                tokens,
                cost,
                sessions: sessions.len(),
            })
            .collect();
        // Dearest first, with the project itself as a tie-break so that two
        // projects costing exactly the same do not swap places between two
        // runs depending on a hash map's iteration order.
        top_projects.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.project.cmp(&b.project))
        });
        top_projects.truncate(TOP_PROJECTS_LIMIT);

        let mut model_breakdown: Vec<ModelBreakdown> = models
            .into_iter()
            .map(|(model, (tokens, cost))| ModelBreakdown {
                model: model.clone(),
                tokens,
                cost,
            })
            .collect();
        // Dearest first, for the same reason and the same tie-break as
        // `top_projects` above -- and the same order
        // `crate::domain::report::UsageRow::breakdown` already keeps its own
        // per-model shares in.
        model_breakdown.sort_by(|a, b| {
            b.cost
                .partial_cmp(&a.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.model.cmp(&b.model))
        });

        let daily_spend = days.into_iter().collect();

        Self {
            today,
            top_projects,
            daily_spend,
            model_breakdown,
        }
    }
}

/// The billing block currently running, its burn rate and its projection.
///
/// Split out of [`AccountUsage::measure`] for the same reason as
/// [`today_and_top_projects`]. `entries` is folded through
/// [`blocks::identify`] -- the same fold `claude-stats blocks` uses, over the
/// same span, so the two can never disagree about when the current window
/// opened. Only the last block `identify` produces can ever be
/// [`BlockKind::Active`], so popping it and checking its kind is the whole
/// question.
fn active_block_reading(
    entries: &[Entry],
    now: DateTime<Utc>,
    prices: &PriceSheet,
) -> (Option<BillingBlock>, Option<BurnRate>, Option<Projection>) {
    let span = Duration::hours(DEFAULT_SPAN_HOURS);
    let mut identified = blocks::identify(entries, span, now, CostMode::default(), prices);
    let active_block = identified
        .pop()
        .filter(|block| block.kind == BlockKind::Active);
    let active_burn = active_block.as_ref().and_then(BurnRate::measure);
    let active_projection = match (&active_block, active_burn) {
        (Some(block), Some(rate)) => Projection::of(block, rate, now),
        _ => None,
    };
    (active_block, active_burn, active_projection)
}

/// The busiest comparable window anywhere in the scanned history.
///
/// `points` is every response as `(when, tokens)`, already in time order --
/// sorted once by the caller, because the answer is wanted for each window
/// kind and re-sorting the same history per kind is work that buys nothing.
///
/// Every point is tried as a window *start*, which is enough: a window that
/// contains any points at all can always be slid forward until it begins on
/// one without losing any, so the maximum is always found at a point.
fn peak_window(
    points: &[(DateTime<Utc>, u64)],
    kind: WindowKind,
    now: DateTime<Utc>,
) -> Option<u64> {
    if points.is_empty() {
        return None;
    }

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
    use crate::domain::entry::EntryId;
    use crate::domain::model::ModelId;
    use crate::domain::period::Zone;
    use crate::domain::project::Project;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(minutes * 60, 0).expect("valid timestamp")
    }

    /// One billable response in `session`, `minutes` after the epoch.
    ///
    /// Every entry is given an identity of its own, because `measure` is fed a
    /// stream that has already been deduplicated for it: two entries here are
    /// always two responses that were really charged for.
    fn entry(session: &str, minutes: i64, tokens: u64) -> Entry {
        Entry {
            id: EntryId {
                message_id: format!("msg-{session}-{minutes}-{tokens}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at: at(minutes),
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input: tokens,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// One billable response in `session`, at a stated instant rather than a
    /// stated offset from the epoch.
    ///
    /// [`entry`] above is enough for every test that only cares about the
    /// order of things; the ones below care about *which hour* and *which
    /// calendar day* a response falls in, and an offset from 1970 cannot
    /// express that legibly.
    fn entry_at(session: &str, when: &str, tokens: u64) -> Entry {
        let at_time: DateTime<Utc> = when.parse().expect("a valid timestamp");
        Entry {
            id: EntryId {
                message_id: format!("msg-{session}-{when}-{tokens}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at: at_time,
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input: tokens,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// One billable response in `project` rather than in the fixed one
    /// [`entry`] always uses -- what the busiest-projects tests need, and
    /// nothing else does.
    fn entry_in(project: &str, session: &str, minutes: i64, tokens: u64) -> Entry {
        Entry {
            project: Project::new(project),
            ..entry(session, minutes, tokens)
        }
    }

    #[test]
    fn spend_is_split_between_the_current_and_previous_calendar_month() {
        // 1970-01-01 is a Thursday and the epoch, so minutes count from the
        // start of January. Put "now" in February, one point in January, and
        // two in February -- one of them just inside the month boundary.
        let january = entry("a", 60 * 24 * 20, 100); // Jan 21st
        let february_boundary = entry("a", 60 * 24 * 31, 200); // Feb 1st, 00:00 exactly
        let february = entry("a", 60 * 24 * 33, 400); // Feb 3rd
        let now = at(60 * 24 * 35); // Feb 5th

        let usage = AccountUsage::measure(
            now,
            &[january, february_boundary, february],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(
            usage.this_month.tokens.total(),
            600,
            "the boundary point belongs to February, not January"
        );
        assert_eq!(usage.last_month.tokens.total(), 100);
        assert_eq!(usage.this_month.sessions, 1);
        assert_eq!(usage.this_month.name(), "February");
        assert_eq!(usage.last_month.name(), "January");
        assert_eq!(usage.last_month.starts, at(0));
    }

    #[test]
    fn the_previous_month_start_crosses_a_year_boundary() {
        // 1970-01-15: the previous month is December 1969.
        let mid_january = at(60 * 24 * 14);
        assert_eq!(
            previous_month_start(mid_january),
            at(-(60 * 24 * 31)),
            "December 1st, 1969"
        );
    }

    #[test]
    fn a_run_of_refusals_before_one_reset_is_a_single_limit_period() {
        let now = at(600);
        let resets_at = now + Duration::hours(1);
        // Ten refusals in the twenty minutes before one reset. That is one
        // limit being hit, not ten.
        let refusals: Vec<LimitEvent> = (0..10)
            .map(|i| LimitEvent {
                at: now - Duration::minutes(20 - i),
                resets_at,
                kind: WindowKind::Session,
            })
            .collect();
        let earliest = refusals[0].at;

        let usage = AccountUsage::measure(now, &[], refusals, &PriceSheet::builtin(), &Zone::Utc);

        assert_eq!(usage.limit_events.len(), 1);
        assert_eq!(usage.session.limit_periods, 1);
        assert_eq!(
            usage.session.last_limit_at,
            Some(earliest),
            "the period is dated from when it began to bite"
        );
    }

    #[test]
    fn two_kinds_of_limit_sharing_a_reset_stay_two_periods() {
        let now = at(600);
        let resets_at = now + Duration::hours(1);
        // Interleaved on purpose: session, week, session. Grouping only by
        // reset instant would leave the two session refusals non-adjacent, and
        // collapsing consecutive duplicates would then miss one of them.
        let refusals = vec![
            LimitEvent {
                at: now - Duration::minutes(30),
                resets_at,
                kind: WindowKind::Session,
            },
            LimitEvent {
                at: now - Duration::minutes(20),
                resets_at,
                kind: WindowKind::Week,
            },
            LimitEvent {
                at: now - Duration::minutes(10),
                resets_at,
                kind: WindowKind::Session,
            },
        ];

        let usage = AccountUsage::measure(now, &[], refusals, &PriceSheet::builtin(), &Zone::Utc);

        assert_eq!(
            usage.limit_events.len(),
            2,
            "one period per kind, not one per refusal"
        );
        assert_eq!(usage.session.limit_periods, 2);
    }

    #[test]
    fn a_window_counts_only_what_falls_inside_it() {
        let now = at(600);
        // 10 hours in: the first point is outside the five-hour window, the
        // second is inside it, and both are inside the week.
        let usage = AccountUsage::measure(
            now,
            &[entry("a", 60, 100), entry("a", 400, 250)],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
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
                entry("a", 400, 10),
                entry("a", 410, 10),
                entry("a", 420, 10),
                entry("b", 430, 10),
            ],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(usage.session.sessions, 2);
        assert_eq!(usage.session.tokens.total(), 40);
    }

    #[test]
    fn a_window_counts_distinct_sessions_rather_than_contributing_transcripts() {
        // Each sub-agent writes its own transcript while belonging to the
        // session that spawned it, so four entries arrive from four files
        // carrying only two session ids. Their tokens all count; the sessions
        // count twice. Counting files here would report a morning's work as
        // several hundred sessions.
        let now = at(600);
        let usage = AccountUsage::measure(
            now,
            &[
                entry("session-1", 400, 10),
                entry("session-1", 405, 10),
                entry("session-1", 410, 10),
                entry("session-2", 420, 10),
            ],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(usage.session.sessions, 2, "two sessions, not four files");
        assert_eq!(
            usage.session.tokens.total(),
            40,
            "every sub-agent's tokens are still charged"
        );
    }

    #[test]
    fn a_session_that_contributed_nothing_to_the_window_is_not_counted() {
        let now = at(600);
        let usage = AccountUsage::measure(
            now,
            &[entry("recent", 400, 10), entry("ancient", 1, 10)],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
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
            &[
                entry("a", 100, 400),
                entry("a", 150, 500),
                entry("a", 9_900, 100),
            ],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
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
        let usage = AccountUsage::measure(
            now,
            &[entry("a", 590, 100)],
            vec![],
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

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
        let usage = AccountUsage::measure(
            now,
            &[],
            vec![ancient, recent, older],
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

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
        let usage = AccountUsage::measure(now, &[], Vec::new(), &PriceSheet::builtin(), &Zone::Utc);

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
        let usage = AccountUsage::measure(
            now,
            &[],
            vec![active, expired],
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

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

    #[test]
    fn todays_spend_is_bucketed_in_the_callers_zone_rather_than_in_utc() {
        // Early evening in New York (UTC-4 in September) is already
        // tomorrow by UTC's own midnight, so the same response belongs to
        // "today" on one calendar and to "yesterday" on the other.
        let new_york = Zone::parse("America/New_York").expect("New York is a real zone");
        let now: DateTime<Utc> = "2026-09-03T02:00:00Z".parse().expect("a valid timestamp");
        let response = entry_at("a", "2026-09-02T23:30:00Z", 100);

        let in_new_york = AccountUsage::measure(
            now,
            std::slice::from_ref(&response),
            Vec::new(),
            &PriceSheet::builtin(),
            &new_york,
        );
        let in_utc = AccountUsage::measure(
            now,
            &[response],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(
            in_new_york.today.tokens.total(),
            100,
            "still this evening in New York"
        );
        assert_eq!(
            in_utc.today.tokens.total(),
            0,
            "already tomorrow by UTC's midnight"
        );
    }

    #[test]
    fn the_active_block_on_the_dashboard_is_the_same_block_the_report_would_show() {
        let entries = [
            entry_at("a", "2026-09-01T09:30:00Z", 100_000),
            entry_at("a", "2026-09-01T09:50:00Z", 100_000),
        ];
        let now: DateTime<Utc> = "2026-09-01T10:00:00Z".parse().expect("a valid timestamp");

        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        // Asked of `blocks::identify` directly, over the same entries and the
        // same span, rather than reimplemented here: the whole point is that
        // the panel and the report cannot come to disagree about this.
        let independently = blocks::identify(
            &entries,
            Duration::hours(DEFAULT_SPAN_HOURS),
            now,
            CostMode::default(),
            &PriceSheet::builtin(),
        );
        let expected = independently
            .last()
            .cloned()
            .filter(|block| block.kind == BlockKind::Active);

        assert!(
            usage.active_block.is_some(),
            "this block really is still running"
        );
        assert_eq!(usage.active_block, expected);
    }

    #[test]
    fn a_project_is_counted_once_per_session_rather_than_once_per_transcript() {
        // Four entries standing in for four transcript files -- the main
        // thread and three of its sub-agents -- all belonging to one session
        // and one project. The busiest-projects panel must count one
        // session, not four transcripts, exactly as the rolling windows do.
        let now = at(600);
        let usage = AccountUsage::measure(
            now,
            &[
                entry("session-1", 400, 10),
                entry("session-1", 405, 10),
                entry("session-1", 410, 10),
                entry("session-1", 415, 10),
            ],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(usage.top_projects.len(), 1);
        assert_eq!(
            usage.top_projects[0].sessions, 1,
            "one session, not four transcripts"
        );
        assert_eq!(
            usage.top_projects[0].tokens.total(),
            40,
            "every sub-agent's tokens are still counted"
        );
    }

    #[test]
    fn the_top_projects_are_ordered_by_cost_descending_and_capped_at_five() {
        let now = at(600);
        // Six projects with distinct, descending token counts, so the
        // dearest-first order is checkable by eye and the cap has something
        // to actually drop.
        let entries: Vec<Entry> = (0..6)
            .map(|i| entry_in(&format!("/home/ada/project-{i}"), "a", 500, 100 * (6 - i)))
            .collect();

        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(
            usage.top_projects.len(),
            5,
            "capped at five even though six projects contributed"
        );
        let costs: Vec<f64> = usage
            .top_projects
            .iter()
            .map(|project| project.cost.dollars())
            .collect();
        let mut dearest_first = costs.clone();
        dearest_first.sort_by(|a, b| b.partial_cmp(a).expect("comparable costs"));
        assert_eq!(costs, dearest_first, "dearest first");
        assert_eq!(
            usage.top_projects[0].project,
            Project::new("/home/ada/project-0"),
            "the busiest project leads"
        );
        assert!(
            !usage
                .top_projects
                .iter()
                .any(|project| project.project == Project::new("/home/ada/project-5")),
            "the cheapest of the six was dropped by the cap"
        );
    }

    #[test]
    fn daily_spend_buckets_by_calendar_day_and_skips_the_quiet_one_between_them() {
        // Two days apart, both inside the seven-day window, with a quiet day
        // in between that contributes no entry at all.
        let day_one = entry("a", 0, 100); // 1970-01-01
        let day_three = entry("a", 60 * 24 * 2, 300); // 1970-01-03
        let now = at(60 * 24 * 2 + 60);

        let usage = AccountUsage::measure(
            now,
            &[day_one, day_three],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(
            usage.daily_spend.len(),
            2,
            "the quiet day in between gets no bucket at all: {:?}",
            usage.daily_spend
        );
        assert!(
            usage.daily_spend[0].0 < usage.daily_spend[1].0,
            "oldest first: {:?}",
            usage.daily_spend
        );
        assert!(
            usage.daily_spend[1].1.dollars() > usage.daily_spend[0].1.dollars(),
            "the day with three times the tokens costs more: {:?}",
            usage.daily_spend
        );
    }

    #[test]
    fn model_breakdown_is_ordered_by_cost_descending() {
        let opus = entry("a", 30, 1); // claude-opus-5, by `entry`'s own default
        let sonnet = Entry {
            model: ModelId::new("claude-sonnet-5"),
            // Enough tokens that the dearer-per-token model still trails,
            // whatever the two models' rates happen to be -- the property
            // under test is the ordering, not either model's own price.
            tokens: TokenUsage {
                input: 10_000_000,
                ..TokenUsage::ZERO
            },
            ..entry("b", 60, 1)
        };
        let now = at(90);

        let usage = AccountUsage::measure(
            now,
            &[opus, sonnet],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        assert_eq!(usage.model_breakdown.len(), 2);
        assert_eq!(
            usage.model_breakdown[0].model,
            ModelId::new("claude-sonnet-5"),
            "the model that cost more leads: {:?}",
            usage.model_breakdown
        );
        assert!(
            usage.model_breakdown[0].cost.dollars() >= usage.model_breakdown[1].cost.dollars(),
            "dearest first: {:?}",
            usage.model_breakdown
        );
    }
}

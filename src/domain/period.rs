//! Which calendar bucket a billable response falls into, and whose calendar
//! decides.
//!
//! Every timestamp this crate reads is UTC, because that is what a transcript
//! records. Every question a user asks about those timestamps is local: "what
//! did I spend today", "which week was the expensive one". Those two are not
//! the same thing, and the difference is not academic -- a response written at
//! 21:30 in Tokyo is stamped 12:30 UTC the same day, but one written at 09:30
//! in Tokyo is stamped 00:30 UTC on the *previous* day. Group by UTC and a
//! Tokyo morning's work lands on yesterday's row. A "today" that disagrees
//! with the wall clock on the same desk is worse than no daily report at all,
//! because the reader has no way of telling that it disagreed.
//!
//! # Why the zone is always a parameter
//!
//! Nothing in this module reads `TZ`, calls `Local::now()` to discover an
//! offset, or otherwise asks the environment what time it is. The zone arrives
//! as a [`Zone`] the caller chose, and the caller is the composition root,
//! which is the one place allowed to know that an environment exists. Two
//! things fall out of that. The bucketing rules become testable without a
//! process-wide mutable global that no test can set safely; and the same
//! entries can be bucketed twice, in two zones, in one run -- which is exactly
//! what the tests below do to prove the rule works at all.

use std::fmt;
use std::str::FromStr;

use chrono::{
    DateTime, Datelike, Days, LocalResult, Months, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday,
};

/// Whose calendar a report's days, weeks and months are measured on.
///
/// A Value Object: three ways of naming a calendar, compared and copied
/// freely, with no identity of its own. [`Self::Local`] is the default because
/// the overwhelmingly common case is a person reading their own machine's
/// figures, and asking them to spell out their own time zone before a daily
/// report means anything would be a poor trade.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Zone {
    /// Coordinated Universal Time: the zone the transcripts themselves are
    /// stamped in.
    ///
    /// Worth asking for when comparing figures with somebody in another
    /// country, because it is the one calendar you both already agree on.
    Utc,
    /// Whatever the machine running the report is set to.
    ///
    /// Resolved through [`chrono::Local`], which reads the host's own
    /// configuration -- so `TZ=Asia/Tokyo claude-stats daily` groups by Tokyo
    /// days with no flag at all, and a laptop that has been carried across an
    /// ocean groups by where it landed.
    #[default]
    Local,
    /// One named IANA zone, whatever the machine is set to.
    Named(chrono_tz::Tz),
}

impl Zone {
    /// Reads a zone from what a user typed.
    ///
    /// `utc` and `local` are accepted in any casing, because they are words
    /// rather than identifiers and nobody should have to remember which. An
    /// IANA name is matched exactly as the database spells it (`Asia/Tokyo`,
    /// not `asia/tokyo`), because that is the spelling every other tool on the
    /// machine uses and quietly accepting a second one invites two spellings
    /// of the same zone to appear in the same script.
    ///
    /// # Errors
    ///
    /// A name the database does not know is refused, and the message names the
    /// forms that would have worked. The tempting alternative -- fall back to
    /// UTC and carry on -- produces a report that is silently measured on
    /// somebody else's calendar, which is the one failure mode this whole
    /// module exists to prevent.
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        let trimmed = name.trim();
        if trimmed.eq_ignore_ascii_case("utc") {
            return Ok(Self::Utc);
        }
        if trimmed.eq_ignore_ascii_case("local") {
            return Ok(Self::Local);
        }
        chrono_tz::Tz::from_str(trimmed)
            .map(Self::Named)
            .map_err(|_| {
                anyhow::anyhow!(
                    "unknown time zone {trimmed:?}: expected `utc`, `local`, \
                 or an IANA name such as `Asia/Tokyo`"
                )
            })
    }

    /// Which calendar day `at` falls on, in this zone.
    ///
    /// The whole of the daily, weekly and monthly bucketing is this one
    /// question asked three ways, which is why it is a method on the zone
    /// rather than a free function taking one.
    #[must_use]
    pub fn local_date(&self, at: DateTime<Utc>) -> NaiveDate {
        match self {
            Self::Utc => at.date_naive(),
            Self::Local => at.with_timezone(&chrono::Local).date_naive(),
            Self::Named(zone) => at.with_timezone(zone).date_naive(),
        }
    }

    /// What a clock in this zone read at `at`.
    ///
    /// The display counterpart of [`Self::local_date`]: that answers which day
    /// an instant belongs to, this answers what the wall clock said while it
    /// was happening. A report that has to print an instant rather than bucket
    /// one -- when a billing window opened, and therefore when it resets --
    /// needs the second question answered on the same calendar as the first,
    /// or the table disagrees with its own headings.
    ///
    /// A [`NaiveDateTime`] rather than a formatted string, because how a stamp
    /// is written is a presentation decision and this module is not the
    /// presentation layer. What it owns is the offset, which is the part a
    /// renderer must not work out for itself.
    #[must_use]
    pub fn wall_clock(&self, at: DateTime<Utc>) -> NaiveDateTime {
        match self {
            Self::Utc => at.naive_utc(),
            Self::Local => at.with_timezone(&chrono::Local).naive_local(),
            Self::Named(zone) => at.with_timezone(zone).naive_local(),
        }
    }

    /// The half-open span of UTC instants that `date` covers in this zone.
    ///
    /// Half-open -- `start <= t < end` -- because that is the only shape that
    /// tiles: the end of one day is the start of the next, so no instant can
    /// be counted twice and none can fall between two days. This is what turns
    /// `--since 20260901` into a bound a [`crate::application::ports::UsageQuery`]
    /// can be given, and it is why a query built this way agrees with the
    /// bucketing above rather than being a few hours out at each edge.
    ///
    /// The two awkward days of the year are handled deliberately. On a day
    /// that skips an hour into daylight saving, local midnight may not exist
    /// at all; the first instant that *does* exist is used, so the day starts
    /// when the clocks say it started rather than an hour before. On a day
    /// that repeats an hour coming out of daylight saving, local midnight
    /// happens twice; the earlier of the two is used, so the day contains both
    /// copies of the repeated hour rather than losing one.
    #[must_use]
    pub fn day_bounds(&self, date: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
        let next = date.checked_add_days(Days::new(1)).unwrap_or(date);
        (self.day_start(date), self.day_start(next))
    }

    /// The first UTC instant belonging to `date` in this zone.
    fn day_start(&self, date: NaiveDate) -> DateTime<Utc> {
        match self {
            Self::Utc => first_instant_of(&Utc, date),
            Self::Local => first_instant_of(&chrono::Local, date),
            Self::Named(zone) => first_instant_of(zone, date),
        }
    }
}

/// The first instant of `date` in `zone`, whatever the clocks did that night.
///
/// Generic over [`TimeZone`] so that the three [`Zone`] arms share one
/// implementation: `Utc`, `chrono::Local` and `chrono_tz::Tz` are three types
/// with nothing in common except the trait, and writing the daylight-saving
/// rule three times is how two of the copies come to disagree.
fn first_instant_of<Z: TimeZone>(zone: &Z, date: NaiveDate) -> DateTime<Utc> {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight always exists");
    match zone.from_local_datetime(&midnight) {
        LocalResult::Single(at) => at.with_timezone(&Utc),
        // The clocks went back, so this local time happened twice. The earlier
        // of the two is the start of the day: taking the later one would push
        // the first repeated hour into the previous day, which is a day that
        // has already been reported on.
        LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
        // The clocks went forward over local midnight, so the day begins at
        // whatever time it began at instead. Stepping a minute at a time
        // rather than assuming an hour, because the shift is not always an
        // hour -- Lord Howe Island moves by thirty minutes, and historical
        // entries in the database move by stranger amounts still.
        LocalResult::None => {
            let mut probe = midnight;
            for _ in 0..MINUTES_TO_PROBE_FOR_A_LOST_MIDNIGHT {
                probe += chrono::Duration::minutes(1);
                if let LocalResult::Single(at) | LocalResult::Ambiguous(at, _) =
                    zone.from_local_datetime(&probe)
                {
                    return at.with_timezone(&Utc);
                }
            }
            // Unreachable against the real database: no zone has ever skipped
            // a quarter of a day. Falling back to UTC midnight keeps the
            // function total rather than panicking inside a report.
            midnight.and_utc()
        }
    }
}

/// How far past a missing local midnight to look before giving up.
///
/// Six hours, which is three times the largest jump the IANA database records
/// and an order of magnitude more than any zone in use today.
const MINUTES_TO_PROBE_FOR_A_LOST_MIDNIGHT: u32 = 360;

/// How wide a bucket a report groups into.
///
/// Three values rather than three report types, because "daily", "weekly" and
/// "monthly" differ only in this: everything else about producing the table --
/// the deduplication, the pricing, the ordering, the rendering -- is identical,
/// and three copies of it would be three chances for one of them to drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationPeriod {
    /// One calendar day in the report's zone.
    Day,
    /// Seven days beginning on a chosen weekday.
    Week {
        /// Which weekday a week begins on.
        ///
        /// Configurable because there is no single right answer: ISO 8601 says
        /// Monday, most of the Americas say Sunday, and a person comparing
        /// this report with an invoice wants whichever their billing period
        /// uses.
        starts_on: Weekday,
    },
    /// One calendar month in the report's zone.
    Month,
}

impl AggregationPeriod {
    /// The weekday a week begins on when nobody said.
    ///
    /// Sunday, matching the tools people compare these figures against rather
    /// than matching ISO 8601. A default that disagrees with the neighbouring
    /// tool produces two weekly tables that differ by a day's traffic with
    /// nothing on either to explain it.
    pub const DEFAULT_WEEK_START: Weekday = Weekday::Sun;

    /// The bucket `at` belongs to, on `zone`'s calendar.
    #[must_use]
    pub fn key_of(self, at: DateTime<Utc>, zone: &Zone) -> PeriodKey {
        let date = zone.local_date(at);
        match self {
            Self::Day => PeriodKey(date.format("%Y-%m-%d").to_string()),
            Self::Week { starts_on } => {
                PeriodKey(week_start(date, starts_on).format("%Y-%m-%d").to_string())
            }
            Self::Month => PeriodKey(date.format("%Y-%m").to_string()),
        }
    }

    /// The inclusive span covered by the most recent `count` buckets of this
    /// kind, on `today`'s calendar -- what `--last <N>` means.
    ///
    /// The later end is always `today` itself: "the last three days" ends
    /// with the one in progress, not with yesterday, because a report that
    /// silently dropped today's traffic would look like a tool that had not
    /// noticed today had started. `count` of zero is treated as one for the
    /// same reason a negative one is refused by the parser before this is
    /// ever called -- "the most recent zero periods" names nothing a caller
    /// could sensibly mean, and a report narrowed to today is a kinder answer
    /// than one covering everything or nothing.
    #[must_use]
    pub fn last_n_bounds(self, count: u32, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        let periods_back = count.max(1) - 1;
        let since = match self {
            Self::Day => today
                .checked_sub_days(Days::new(u64::from(periods_back)))
                .unwrap_or(today),
            Self::Week { starts_on } => {
                let this_week_start = week_start(today, starts_on);
                this_week_start
                    .checked_sub_days(Days::new(u64::from(periods_back) * 7))
                    .unwrap_or(this_week_start)
            }
            Self::Month => {
                // `with_day(1)` only fails on a date that was never valid to
                // begin with, which `today` cannot be.
                let this_month_start = today.with_day(1).unwrap_or(today);
                this_month_start
                    .checked_sub_months(Months::new(periods_back))
                    .unwrap_or(this_month_start)
            }
        };
        (since, today)
    }
}

/// The `starts_on` day of the week containing `date`.
///
/// Worked out by counting days rather than through a calendar helper, because
/// the arithmetic is the whole rule and a reader checking a weekly total
/// against a paper diary should be able to see it. `num_days_from_monday`
/// numbers both days on the same scale, the subtraction is taken modulo seven
/// so that a week starting on Sunday does not run backwards on a Monday, and
/// the result is therefore always in `0..7`.
fn week_start(date: NaiveDate, starts_on: Weekday) -> NaiveDate {
    let offset = (date.weekday().num_days_from_monday() + 7 - starts_on.num_days_from_monday()) % 7;
    date.checked_sub_days(Days::new(u64::from(offset)))
        .unwrap_or(date)
}

/// The label of one bucket, and the value rows are grouped and sorted by.
///
/// A string rather than a date because the three periods do not share a shape:
/// a month has no day and a week is named by a date that is not the date of
/// any particular entry in it. Rendering each one to its own fixed-width form
/// makes them all comparable in the only way a report needs them to be.
///
/// The three shapes -- `2026-09-02`, `2026-09` and a week's `2026-08-30` --
/// are all big-endian, zero-padded and fixed width, which is what makes plain
/// lexicographic ordering identical to calendar ordering. That is not a happy
/// accident to be relied on quietly: it is the reason the format was chosen,
/// and changing it to anything with a variable-width field (`2026-9-2`) would
/// silently reorder every report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeriodKey(String);

impl PeriodKey {
    /// Wraps an already-rendered key.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key with no period at all.
    ///
    /// What a report grouped only by project or only by session uses: there is
    /// exactly one bucket per group and no calendar in the question, so every
    /// row shares this key and the ordering falls through to the project and
    /// the session.
    #[must_use]
    pub fn none() -> Self {
        Self(String::new())
    }

    /// The label itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this row is not about a calendar period.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for PeriodKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which way round the rows come out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Order {
    /// Oldest bucket first, which is how a table is read from the top down.
    #[default]
    Ascending,
    /// Newest bucket first, which is what somebody piping to `head` wants.
    Descending,
}

/// What a report is grouped by, and in what order.
///
/// A second Query Object beside
/// [`crate::application::ports::UsageQuery`], and deliberately a separate one.
/// That query says *which* responses count; this says *how they are piled up*.
/// Keeping them apart is what lets one loaded set of entries be reported on
/// several ways without going back to disk, and stops a change to the grouping
/// from looking like a change to the selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupingSpec {
    /// The calendar bucket, or `None` for a report with no time axis.
    pub period: Option<AggregationPeriod>,
    /// Whether rows are split per working directory.
    pub by_project: bool,
    /// Whether rows are split per conversation.
    pub by_session: bool,
    /// Which way round the rows come out, unless the grouping overrides it.
    pub order: Order,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn tokyo() -> Zone {
        Zone::parse("Asia/Tokyo").expect("Tokyo is in the IANA database")
    }

    fn new_york() -> Zone {
        Zone::parse("America/New_York").expect("New York is in the IANA database")
    }

    #[test]
    fn a_response_late_on_a_local_evening_lands_on_that_local_day_not_the_next_utc_one() {
        // 21:30 in Tokyo on the 2nd is 12:30 UTC on the 2nd, so this one is
        // easy. Its westward mirror below is the one that used to break.
        let evening = at("2026-09-02T12:30:00Z");
        assert_eq!(
            AggregationPeriod::Day.key_of(evening, &tokyo()).as_str(),
            "2026-09-02"
        );

        // New York is behind UTC, so 21:30 there on the 2nd is already the
        // 3rd in UTC. Bucketing by UTC would file an evening's work under
        // tomorrow, and tomorrow is a day the reader has not lived yet.
        let evening_in_new_york = at("2026-09-03T01:30:00Z");
        assert_eq!(
            AggregationPeriod::Day
                .key_of(evening_in_new_york, &new_york())
                .as_str(),
            "2026-09-02"
        );
        assert_eq!(
            AggregationPeriod::Day
                .key_of(evening_in_new_york, &Zone::Utc)
                .as_str(),
            "2026-09-03",
            "the same instant really is the 3rd in UTC"
        );
    }

    #[test]
    fn a_response_early_on_a_local_morning_lands_on_that_local_day_not_the_previous_utc_one() {
        // 09:30 in Tokyo on the 2nd is 00:30 UTC on the *same* day only
        // because Tokyo is nine hours ahead; at 08:30 it is still the 1st in
        // UTC. That is the morning's work a UTC bucket loses.
        let morning = at("2026-09-01T23:30:00Z");
        assert_eq!(
            AggregationPeriod::Day.key_of(morning, &tokyo()).as_str(),
            "2026-09-02"
        );
        assert_eq!(
            AggregationPeriod::Day.key_of(morning, &Zone::Utc).as_str(),
            "2026-09-01",
            "the same instant really is the 1st in UTC"
        );
    }

    #[test]
    fn the_same_instant_buckets_to_different_days_in_different_zones() {
        // One response, three calendars, three answers -- and all three are
        // right. This is the reason the zone is a parameter rather than a
        // constant: there is no zone-free answer to give.
        let instant = at("2026-09-02T23:30:00Z");
        assert_eq!(
            AggregationPeriod::Day.key_of(instant, &tokyo()).as_str(),
            "2026-09-03"
        );
        assert_eq!(
            AggregationPeriod::Day.key_of(instant, &Zone::Utc).as_str(),
            "2026-09-02"
        );
        assert_eq!(
            AggregationPeriod::Day.key_of(instant, &new_york()).as_str(),
            "2026-09-02"
        );
    }

    #[test]
    fn a_day_that_loses_an_hour_to_daylight_saving_is_still_one_bucket() {
        // New York goes forward at 02:00 local on 8 March 2026, so that day is
        // twenty-three hours long. It is still one day, and every instant in
        // it must land on it.
        let zone = new_york();
        let short_day = NaiveDate::from_ymd_opt(2026, 3, 8).expect("a real date");
        let (start, end) = zone.day_bounds(short_day);

        assert_eq!(start, at("2026-03-08T05:00:00Z"), "midnight EST");
        assert_eq!(end, at("2026-03-09T04:00:00Z"), "midnight EDT the next day");
        assert_eq!(
            (end - start).num_hours(),
            23,
            "the day really did lose an hour"
        );

        for probe in [
            start,
            start + chrono::Duration::hours(12),
            end - chrono::Duration::seconds(1),
        ] {
            assert_eq!(
                AggregationPeriod::Day.key_of(probe, &zone).as_str(),
                "2026-03-08",
                "{probe} fell outside its own day"
            );
        }
    }

    #[test]
    fn a_day_that_repeats_an_hour_does_not_produce_two_buckets_for_it() {
        // New York goes back at 02:00 local on 1 November 2026, so 01:30 local
        // happens twice -- once at 05:30 UTC and once at 06:30 UTC. Both are
        // the 1st, and the day is twenty-five hours long rather than being
        // split in two.
        let zone = new_york();
        let long_day = NaiveDate::from_ymd_opt(2026, 11, 1).expect("a real date");
        let (start, end) = zone.day_bounds(long_day);

        assert_eq!(start, at("2026-11-01T04:00:00Z"), "midnight EDT");
        assert_eq!(end, at("2026-11-02T05:00:00Z"), "midnight EST the next day");
        assert_eq!(
            (end - start).num_hours(),
            25,
            "the day really did gain an hour"
        );

        let first_pass = at("2026-11-01T05:30:00Z");
        let second_pass = at("2026-11-01T06:30:00Z");
        assert_eq!(
            AggregationPeriod::Day.key_of(first_pass, &zone),
            AggregationPeriod::Day.key_of(second_pass, &zone),
            "both readings of 01:30 belong to the same day"
        );
        assert_eq!(
            AggregationPeriod::Day.key_of(first_pass, &zone).as_str(),
            "2026-11-01"
        );
    }

    #[test]
    fn a_day_whose_local_midnight_never_happens_starts_when_the_clocks_say_it_did() {
        // Lord Howe Island goes forward by thirty minutes, and some zones have
        // historically skipped midnight itself. Chile is the modern example:
        // on 6 September 2026 the clocks jump from 23:59:59 on the 5th
        // straight to 01:00 on the 6th, so local midnight does not exist.
        let zone = Zone::parse("America/Santiago").expect("Santiago is in the database");
        let date = NaiveDate::from_ymd_opt(2026, 9, 6).expect("a real date");
        let (start, _) = zone.day_bounds(date);

        assert_eq!(
            zone.local_date(start),
            date,
            "the day starts on itself rather than an hour before"
        );
        assert_eq!(
            start.with_timezone(&chrono_tz::America::Santiago).time(),
            chrono::NaiveTime::from_hms_opt(1, 0, 0).expect("a real time"),
            "the first instant that exists is 01:00"
        );
    }

    #[test]
    fn a_week_starts_on_the_configured_day_rather_than_always_on_monday() {
        // Wednesday 2 September 2026.
        let midweek = at("2026-09-02T12:00:00Z");
        let sunday = AggregationPeriod::Week {
            starts_on: Weekday::Sun,
        };
        let monday = AggregationPeriod::Week {
            starts_on: Weekday::Mon,
        };

        assert_eq!(sunday.key_of(midweek, &Zone::Utc).as_str(), "2026-08-30");
        assert_eq!(monday.key_of(midweek, &Zone::Utc).as_str(), "2026-08-31");
        assert_eq!(
            AggregationPeriod::DEFAULT_WEEK_START,
            Weekday::Sun,
            "the default matches the tools these figures get compared with"
        );
    }

    #[test]
    fn a_week_bucket_is_labelled_by_the_day_it_starts() {
        // Every day from Sunday the 30th to Saturday the 5th carries the
        // Sunday's date, and the day after rolls over to the next Sunday.
        let week = AggregationPeriod::Week {
            starts_on: Weekday::Sun,
        };
        for day in 30..=31 {
            let stamp = format!("2026-08-{day:02}T09:00:00Z");
            assert_eq!(week.key_of(at(&stamp), &Zone::Utc).as_str(), "2026-08-30");
        }
        for day in 1..=5 {
            let stamp = format!("2026-09-{day:02}T09:00:00Z");
            assert_eq!(week.key_of(at(&stamp), &Zone::Utc).as_str(), "2026-08-30");
        }
        assert_eq!(
            week.key_of(at("2026-09-06T09:00:00Z"), &Zone::Utc).as_str(),
            "2026-09-06",
            "the next Sunday opens the next week"
        );
    }

    #[test]
    fn last_n_days_reaches_back_n_minus_one_days_from_today() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 3).expect("valid date");
        assert_eq!(
            AggregationPeriod::Day.last_n_bounds(1, today),
            (today, today),
            "the last one day is just today"
        );
        assert_eq!(
            AggregationPeriod::Day.last_n_bounds(3, today),
            (
                NaiveDate::from_ymd_opt(2026, 9, 1).expect("valid date"),
                today
            ),
            "the last three days are today and the two before it"
        );
    }

    #[test]
    fn last_n_weeks_are_measured_from_the_start_of_the_current_week_not_from_today() {
        // Wednesday 2 September 2026, in a week starting on Sunday.
        let today = NaiveDate::from_ymd_opt(2026, 9, 2).expect("valid date");
        let week = AggregationPeriod::Week {
            starts_on: Weekday::Sun,
        };
        assert_eq!(
            week.last_n_bounds(1, today),
            (
                NaiveDate::from_ymd_opt(2026, 8, 30).expect("valid date"),
                today
            ),
            "the last one week is the week today falls in, not just today"
        );
        assert_eq!(
            week.last_n_bounds(2, today),
            (
                NaiveDate::from_ymd_opt(2026, 8, 23).expect("valid date"),
                today
            ),
            "the week before starts seven days earlier"
        );
    }

    #[test]
    fn last_n_months_start_on_the_first_of_a_month_n_minus_one_months_back() {
        // 15 September 2026, so "the last two months" reaches back into
        // August, not merely to the start of September minus thirty-one days.
        let today = NaiveDate::from_ymd_opt(2026, 9, 15).expect("valid date");
        assert_eq!(
            AggregationPeriod::Month.last_n_bounds(2, today),
            (
                NaiveDate::from_ymd_opt(2026, 8, 1).expect("valid date"),
                today
            )
        );
    }

    #[test]
    fn a_count_of_zero_is_treated_as_one_rather_than_as_everything() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 3).expect("valid date");
        assert_eq!(
            AggregationPeriod::Day.last_n_bounds(0, today),
            AggregationPeriod::Day.last_n_bounds(1, today),
            "the most recent zero periods means the same as the most recent one"
        );
    }

    #[test]
    fn a_month_bucket_crosses_a_year_boundary_correctly() {
        let month = AggregationPeriod::Month;
        assert_eq!(
            month
                .key_of(at("2026-12-31T23:59:59Z"), &Zone::Utc)
                .as_str(),
            "2026-12"
        );
        assert_eq!(
            month
                .key_of(at("2027-01-01T00:00:00Z"), &Zone::Utc)
                .as_str(),
            "2027-01"
        );
        // The keys are compared as text, so December has to sort before the
        // January that follows it rather than after it.
        assert!(
            month.key_of(at("2026-12-31T23:59:59Z"), &Zone::Utc)
                < month.key_of(at("2027-01-01T00:00:00Z"), &Zone::Utc),
            "lexicographic order must be calendar order across a year"
        );
    }

    #[test]
    fn a_february_bucket_in_a_leap_year_has_its_extra_day() {
        // 2028 is a leap year, so the 29th exists and belongs to February.
        let month = AggregationPeriod::Month;
        assert_eq!(
            month
                .key_of(at("2028-02-29T12:00:00Z"), &Zone::Utc)
                .as_str(),
            "2028-02"
        );
        assert_eq!(
            AggregationPeriod::Day
                .key_of(at("2028-02-29T12:00:00Z"), &Zone::Utc)
                .as_str(),
            "2028-02-29"
        );
        // And the day itself is a whole day, not a gap.
        let leap_day = NaiveDate::from_ymd_opt(2028, 2, 29).expect("a real date");
        let (start, end) = Zone::Utc.day_bounds(leap_day);
        assert_eq!((end - start).num_hours(), 24);
        assert_eq!(end, at("2028-03-01T00:00:00Z"));
    }

    #[test]
    fn an_unparsed_timezone_name_is_refused_rather_than_silently_treated_as_utc() {
        let refusal = Zone::parse("Middle/Earth").expect_err("no such zone");
        let message = refusal.to_string();
        assert!(
            message.contains("Middle/Earth"),
            "the message must name what was rejected: {message}"
        );
        assert!(
            message.contains("Asia/Tokyo"),
            "and say what would have worked: {message}"
        );
        // The failure mode this guards against: a report measured on somebody
        // else's calendar, with nothing on it to say so.
        assert_ne!(Zone::parse("Middle/Earth").ok(), Some(Zone::Utc));
    }

    #[test]
    fn the_two_word_zones_are_accepted_in_any_casing_but_iana_names_are_not() {
        assert_eq!(Zone::parse("utc").expect("accepted"), Zone::Utc);
        assert_eq!(Zone::parse("UTC").expect("accepted"), Zone::Utc);
        assert_eq!(Zone::parse(" Local ").expect("accepted"), Zone::Local);
        assert_eq!(Zone::default(), Zone::Local, "no flag means the wall clock");
        assert!(
            Zone::parse("asia/tokyo").is_err(),
            "an IANA name is spelled the way the database spells it"
        );
    }

    #[test]
    fn a_days_bounds_are_half_open_so_no_instant_belongs_to_two_days() {
        let zone = tokyo();
        let first = NaiveDate::from_ymd_opt(2026, 9, 1).expect("a real date");
        let second = NaiveDate::from_ymd_opt(2026, 9, 2).expect("a real date");
        let (first_start, first_end) = zone.day_bounds(first);
        let (second_start, _) = zone.day_bounds(second);

        assert_eq!(first_end, second_start, "the days tile without a seam");
        assert_eq!(zone.local_date(first_start), first);
        assert_eq!(
            zone.local_date(first_end),
            second,
            "the end bound already belongs to the next day"
        );
    }
}

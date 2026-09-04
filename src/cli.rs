//! The command-line surface.
//!
//! Every subcommand answers the same question -- what is a Claude Code session
//! costing and how close is it to compacting -- at a different level of
//! ceremony. `monitor` is the live dashboard, `stats` is the same numbers
//! printed once and piped somewhere, `sessions` and `models` are the
//! supporting lookups, `daily`, `weekly`, `monthly` and `session` are the
//! period tables over the whole corpus, and `blocks` is the same corpus cut
//! into the five-hour windows the subscription is actually metered in.
//!
//! Nothing here reads a file, opens a terminal or prices a token. This module
//! is the delivery adapter in the hexagonal sense: it turns the words somebody
//! typed into the values the use cases already understand -- a
//! [`Zone`], a [`UsageQuery`], a [`CostMode`] -- and hands them to the
//! composition root. Keeping the translation here rather than in `main.rs` is
//! what makes it testable: a test can assert that `--since 20260901` in Tokyo
//! becomes a particular UTC instant without a corpus, a clock or a screen.

use std::path::PathBuf;

use chrono::{Duration, NaiveDate, Weekday};
use clap::{Parser, Subcommand};

use crate::application::blocks_report::{BlockOptions, DEFAULT_RECENT_DAYS, TokenLimit};
use crate::application::ports::{ProjectFilter, SessionSelector, UsageQuery};
use crate::domain::blocks::DEFAULT_SPAN_HOURS;
use crate::domain::period::{Order, Zone};
use crate::domain::pricing::CostMode;

/// A colourful terminal dashboard for Claude Code sessions.
#[derive(Debug, Parser)]
#[command(name = "claude-stats", version, about, long_about = None)]
pub struct Cli {
    /// What to run. Defaults to the live dashboard.
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub selection: Selection,
}

/// Which session to look at.
///
/// Flattened into every subcommand rather than repeated, so `--session` means
/// the same thing everywhere and gains new spellings in one place.
#[derive(Debug, Clone, clap::Args)]
pub struct Selection {
    /// Follow the session whose id starts with this prefix.
    #[arg(long, value_name = "PREFIX", global = true)]
    pub session: Option<String>,

    /// Follow the newest session belonging to this directory.
    #[arg(long, value_name = "DIR", global = true)]
    pub project: Option<PathBuf>,

    /// Follow this transcript file directly.
    #[arg(long, value_name = "FILE", global = true)]
    pub path: Option<PathBuf>,
}

impl Selection {
    /// Turns the flags into a selector.
    ///
    /// The most specific flag wins, and with none of them set the dashboard
    /// follows whatever session is currently active. Rather than rejecting
    /// combinations, the order is fixed and documented: someone who passes two
    /// gets the more specific one, which is what they almost certainly meant.
    #[must_use]
    pub fn selector(&self) -> SessionSelector {
        if let Some(path) = &self.path {
            return SessionSelector::Path(path.clone());
        }
        if let Some(prefix) = &self.session {
            return SessionSelector::Id(prefix.clone());
        }
        if let Some(project) = &self.project {
            return SessionSelector::Project(project.clone());
        }
        SessionSelector::Active
    }
}

/// Which way round a report's rows come out.
///
/// Spelled `asc`/`desc` rather than reusing the domain's own
/// [`Order`] because the two words are what a user types and what every
/// neighbouring tool accepts, and because a `clap::ValueEnum` derive on a
/// domain type would put a command-line concern inside the business rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum RowOrder {
    /// Oldest bucket first, which is how a table is read from the top down.
    Asc,
    /// Newest bucket first, which is what somebody piping to `head` wants.
    Desc,
}

impl From<RowOrder> for Order {
    fn from(order: RowOrder) -> Self {
        match order {
            RowOrder::Asc => Self::Ascending,
            RowOrder::Desc => Self::Descending,
        }
    }
}

/// How a response's cost is arrived at.
///
/// The command-line spelling of [`CostMode`]. Worth knowing before choosing
/// one: this transcript format records no per-message cost, so `display` finds
/// nothing to display and `auto` therefore agrees with `calculate` everywhere
/// today. The flag exists so that a script written against another tool keeps
/// working, and so that the day Claude Code does start recording a cost the
/// choice is already a flag rather than a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CostBasis {
    /// Use the recorded cost where there is one, otherwise price the tokens.
    Auto,
    /// Always price the tokens from the sheet.
    Calculate,
    /// Only ever report a cost the transcript itself recorded.
    Display,
}

impl From<CostBasis> for CostMode {
    fn from(basis: CostBasis) -> Self {
        match basis {
            CostBasis::Auto => Self::Auto,
            CostBasis::Calculate => Self::Calculate,
            CostBasis::Display => Self::Display,
        }
    }
}

/// The flags every period report shares.
///
/// Flattened into `daily`, `weekly`, `monthly` and `session` rather than
/// written out on each of them, so that `--since` cannot come to mean one
/// thing on the daily table and another on the monthly one, and so that the
/// next flag somebody needs is added in one place.
///
/// The struct as a whole is the outside half of Fowler's Query Object: the
/// criteria of a report made into a value, which [`Self::query`] then
/// translates into the [`UsageQuery`] the repository actually understands.
/// The translation is a method here rather than a block in `main.rs` because
/// it is the part with rules in it -- which calendar the dates are read on,
/// which end of a day each bound lands on -- and rules that nobody can write a
/// test for are rules that quietly stop holding.
// Five independent switches, which clippy reads as a state machine wanting to
// be an enum. It is not one: `--json`, `--breakdown`, `--offline`, `--online`
// and `--compact` are five separate questions a user answers separately, any
// combination of them is meaningful, and folding them into enums would put
// the command line's shape at odds with the flags people actually type.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, clap::Args)]
pub struct ReportOptions {
    /// Count only from this calendar day onwards, inclusive (YYYYMMDD).
    ///
    /// Read on the reporting time zone, not on UTC: `--since 20260901
    /// --timezone Asia/Tokyo` starts at midnight in Tokyo, which is 15:00 the
    /// previous afternoon in UTC.
    #[arg(short = 's', long, value_name = "YYYYMMDD", value_parser = calendar_date)]
    pub since: Option<NaiveDate>,

    /// Count only up to this calendar day, inclusive (YYYYMMDD).
    ///
    /// The whole day counts, right up to its last instant, on the same
    /// calendar `--since` is read on.
    #[arg(short = 'u', long, value_name = "YYYYMMDD", value_parser = calendar_date)]
    pub until: Option<NaiveDate>,

    /// Emit JSON instead of a table.
    #[arg(short = 'j', long)]
    pub json: bool,

    /// Add a sub-row per model beneath every row.
    #[arg(short = 'b', long)]
    pub breakdown: bool,

    /// Which way round the rows come out.
    #[arg(short = 'o', long, value_enum, default_value_t = RowOrder::Asc)]
    pub order: RowOrder,

    /// Whose calendar the days, weeks and months are measured on: `utc`,
    /// `local`, or an IANA name such as `Asia/Tokyo`.
    #[arg(short = 'z', long, value_name = "TZ", default_value = "local")]
    pub timezone: String,

    /// How a response's cost is arrived at.
    #[arg(short = 'm', long, value_enum, default_value_t = CostBasis::Auto)]
    pub mode: CostBasis,

    /// Count only responses recorded in this project.
    ///
    /// A filter over the whole corpus, which is a different question from the
    /// one the dashboard's own `--project` asks: this narrows what a total is
    /// made of, that picks the single conversation to follow. Matched against
    /// the working directory the transcript recorded, either in full
    /// (`/home/ada/Projects/api`) or by its final segment (`api`), because
    /// those are the two spellings a person actually has to hand.
    //
    // It shares clap's argument slot with [`Selection::project`], and carries
    // the same `PathBuf` type for that reason rather than because a bare `api`
    // is a path. clap propagates a global argument into every subcommand that
    // does not define one of its own, so two arguments spelled `--project`
    // must agree about the type they hold or the one typed after the
    // subcommand cannot be read back at the top level. Neither command looks
    // at the other's reading of it -- a period report ignores
    // [`Cli::selection`] entirely, and the dashboard has no [`ReportOptions`]
    // -- so one slot serving two questions costs nothing and keeps
    // `--project` spelled one way everywhere.
    #[arg(short = 'p', long, value_name = "NAME")]
    pub project: Option<PathBuf>,

    /// Price from the compiled-in sheet. This is the default and the only
    /// supported behaviour.
    #[arg(short = 'O', long)]
    pub offline: bool,

    /// Fetch prices from upstream. Not supported: the sheet is compiled in.
    //
    // Accepted rather than rejected by the parser on purpose; see
    // [`ReportOptions::ensure_offline`] for why refusing it honestly beats not
    // having it.
    #[arg(long, conflicts_with = "offline")]
    pub online: bool,

    /// Draw the narrow layout whatever the terminal is wide enough for.
    #[arg(long)]
    pub compact: bool,
}

impl ReportOptions {
    /// Which calendar the report is measured on.
    ///
    /// # Errors
    ///
    /// Returns an error for a zone name the IANA database does not know. That
    /// failure is deliberately fatal rather than a fallback to UTC: a report
    /// silently measured on somebody else's calendar looks exactly like a
    /// correct one, and there is nothing on the page to say otherwise.
    pub fn zone(&self) -> anyhow::Result<Zone> {
        Zone::parse(&self.timezone)
    }

    /// Refuses `--online`, explaining what to do instead.
    ///
    /// Accepting the flag and then saying no is a deliberate choice over not
    /// having it at all. A script written for another tool passes `--online`
    /// out of habit; without this it dies on an argument-parser error that
    /// says nothing about prices, and with it the person reading the failure
    /// is told both why there is nothing to fetch and where the one rate they
    /// care about can be corrected.
    ///
    /// # Errors
    ///
    /// Returns an error whenever `--online` was passed.
    pub fn ensure_offline(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.online,
            "--online is not supported: the price sheet is compiled in and this tool \
             never opens a socket. A rate that has moved since this release can be \
             corrected in ~/.config/claude-stats/prices.json"
        );
        Ok(())
    }

    /// The criteria these flags describe, with the calendar bounds resolved on
    /// `zone`'s calendar.
    ///
    /// The two bounds are asymmetric on purpose, and the asymmetry is the only
    /// interesting thing this function does.
    /// [`Zone::day_bounds`] is half-open -- the end of one day is the first
    /// instant of the next -- while [`UsageQuery`]'s ends are both inclusive.
    /// Handing the half-open end straight over would count the first instant of
    /// the day after `--until` into the report, so it is stepped back by the
    /// smallest amount a timestamp can hold instead.
    ///
    /// The session filter is deliberately left at its default here: which
    /// sessions a report is about is a property of the subcommand rather than
    /// of these shared flags, and the composition root sets it.
    #[must_use]
    pub fn query(&self, zone: &Zone) -> UsageQuery {
        UsageQuery {
            since: self.since.map(|date| zone.day_bounds(date).0),
            until: self
                .until
                .map(|date| zone.day_bounds(date).1 - chrono::Duration::nanoseconds(1)),
            projects: self.project.as_ref().map_or(ProjectFilter::All, |name| {
                ProjectFilter::Named(vec![name.to_string_lossy().into_owned()])
            }),
            ..UsageQuery::default()
        }
    }
}

// ---------------------------------------------------------------------------
// The `--last` flag, shared only by `daily`, `weekly` and `monthly`.
// ---------------------------------------------------------------------------

/// The `--last` flag `daily`, `weekly` and `monthly` share.
///
/// A struct of its own rather than a field on [`ReportOptions`], because
/// `session` and `blocks` do not accept it. "The last N sessions" and "the
/// last N billing blocks" are different questions from the one this flag
/// answers -- which of a report's own calendar buckets to keep -- and folding
/// it into the options every subcommand flattens would offer it somewhere it
/// cannot be honoured.
#[derive(Debug, Clone, clap::Args)]
pub struct LastFlag {
    /// Narrow the report to the most recent N periods: 1 is today, this week
    /// or this month, depending on which report this is.
    ///
    /// Counted on the same calendar `--timezone` names, and mutually
    /// exclusive with `--since`/`--until` -- both are ways of answering
    /// "which periods", and accepting both would mean one of them was
    /// silently ignored.
    #[arg(long, value_name = "N", conflicts_with_all = ["since", "until"])]
    pub last: Option<u32>,
}

// ---------------------------------------------------------------------------
// The flags `blocks` adds on top of the shared report options.
// ---------------------------------------------------------------------------

/// What `claude-stats blocks` asks for beyond the shared report flags.
///
/// A separate `clap::Args` struct rather than four fields on the subcommand,
/// for the same reason [`ReportOptions`] is one: the translation from what
/// somebody typed into the [`BlockOptions`] the use case understands has rules
/// in it -- `--recent` means three days, an absent `--token-limit` means no
/// ceiling at all rather than a ceiling of zero -- and a rule nobody can write
/// a test for is a rule that quietly stops holding.
#[derive(Debug, Clone, clap::Args)]
pub struct BlockFlags {
    /// Show only the window that is still running.
    #[arg(short = 'a', long)]
    pub active: bool,

    /// Show only the last three days of windows, and the running one.
    ///
    /// A switch rather than a number of days, because `--since` already
    /// answers "from when" and two spellings of one filter is one too many.
    /// What this one is for is the shorter question -- what have I been doing
    /// lately -- and three days is the answer to that, because a Monday
    /// morning wants to see Friday afternoon.
    //
    // The three lives in [`DEFAULT_RECENT_DAYS`] beside the use case that
    // applies it, and is spelled out again above because this sentence is
    // printed in a terminal by `--help`, where a rustdoc link is noise.
    #[arg(short = 'r', long)]
    pub recent: bool,

    /// Judge each projection against this many tokens, or against `max`.
    ///
    /// `max` is the busiest window already finished, which is the only ceiling
    /// this tool can honestly discover: the real allowance lives on
    /// Anthropic's side and is never written to disk.
    #[arg(short = 't', long, value_name = "N|max", value_parser = token_limit)]
    pub token_limit: Option<TokenLimit>,

    /// How many hours a window runs. At least 1, at most a year.
    ///
    /// Five is what Anthropic meters a subscription in, so it is the default;
    /// it is a flag rather than a constant so that a change to the plan is
    /// something a user can type rather than something that needs a release.
    /// A window shorter than an hour is refused rather than measured, because
    /// a block's start is anchored to the top of the hour and a window that
    /// closes inside that hour would close before its own first response.
    #[arg(short = 'n', long, value_name = "HOURS", default_value = "5", value_parser = session_length)]
    pub session_length: Duration,
}

impl BlockFlags {
    /// The options these flags describe.
    ///
    /// An absent `--token-limit` becomes [`TokenLimit::None`] rather than a
    /// ceiling of zero, which is the distinction that decides whether the
    /// table has a `[%]` column at all.
    #[must_use]
    pub fn options(&self) -> BlockOptions {
        BlockOptions {
            span: self.session_length,
            active_only: self.active,
            recent_days: self.recent.then_some(DEFAULT_RECENT_DAYS),
            token_limit: self.token_limit.unwrap_or_default(),
        }
    }
}

/// Reads `--token-limit`: a whole number of tokens, or the word `max`.
///
/// Refused here rather than further in, and the refusal names *both* spellings
/// on purpose. The two are not variations on one idea -- a number is a ceiling
/// the user knows and `max` is one the tool works out -- so somebody who typed
/// `--token-limit maximum` has no way of guessing which of the two they got
/// close to unless the message says.
fn token_limit(text: &str) -> Result<TokenLimit, String> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(TOKEN_LIMIT_MAX) {
        return Ok(TokenLimit::Max);
    }
    trimmed.parse::<u64>().map(TokenLimit::Exact).map_err(|_| {
        format!(
            "{text:?} is not a token limit; pass a whole number of tokens such as 88000, \
             or {TOKEN_LIMIT_MAX:?} to use the busiest window already on record"
        )
    })
}

/// The word that asks for the busiest finished window as the ceiling.
const TOKEN_LIMIT_MAX: &str = "max";

/// Reads `--session-length`: how many hours a window runs.
///
/// Accepts a fraction, because not every plan a user might be modelling is a
/// whole number of hours, and refuses anything outside
/// [`MIN_SESSION_LENGTH_HOURS`] to [`MAX_SESSION_LENGTH_HOURS`]. Refusing here
/// rather than further in is what keeps the failure attached to the flag that
/// caused it, before a single transcript is opened.
fn session_length(text: &str) -> Result<Duration, String> {
    let hours: f64 = text.trim().parse().map_err(|_| {
        format!("{text:?} is not a number of hours; pass a figure such as {DEFAULT_SPAN_HOURS}")
    })?;
    if !hours.is_finite() || hours < MIN_SESSION_LENGTH_HOURS || hours > MAX_SESSION_LENGTH_HOURS {
        return Err(format!(
            "a window of {hours} hours cannot be measured; pass at least \
             {MIN_SESSION_LENGTH_HOURS} and at most {MAX_SESSION_LENGTH_HOURS} hours"
        ));
    }
    Ok(Duration::seconds((hours * SECONDS_PER_HOUR).round() as i64))
}

/// The shortest window that is still a window.
///
/// One hour, and the reason is the block algorithm rather than taste. A block's
/// start is floored back to the top of the UTC hour its first response fell in,
/// because that is where the allowance resets. Give the window less than an
/// hour to run and that flooring outruns it: a block opened by a response at
/// 09:47 with a half-hour span is stamped as starting at 09:00 and ending at
/// 09:30, so it has closed before its own first response. Every later response
/// in that hour then opens another block stamped 09:00, which leaves several
/// blocks sharing one start instant -- the instant the JSON document uses as an
/// `id` precisely because two blocks cannot open in the same hour -- and none
/// of them can ever be the running one, so `--active` reports nothing over a
/// corpus that is being worked in right now.
///
/// Anything from an hour up is safe: a window at least as long as the hour it
/// is anchored to always closes no earlier than its own last response, and two
/// consecutive blocks always floor to different hours.
const MIN_SESSION_LENGTH_HOURS: f64 = 1.0;

/// The longest window that is still a window rather than a typo.
///
/// A year. Nothing about the arithmetic breaks above it, but a five-hour
/// allowance measured over a decade is a mistyped flag rather than a question,
/// and saying so is more use than a table with one row on it.
const MAX_SESSION_LENGTH_HOURS: f64 = 24.0 * 365.0;

/// Seconds in an hour, named so the conversion reads as a conversion.
const SECONDS_PER_HOUR: f64 = 3_600.0;

/// Reads a `YYYYMMDD` day, refusing anything else while the flags are still
/// being parsed.
///
/// A `value_parser` rather than a check further in, because the alternative is
/// worse than it looks. `20260230` is not a date; a parser that widened it to
/// the first of March, or silently dropped the bound, would produce a report
/// covering a range nobody asked for with nothing on it to say so. Refusing
/// here means the failure arrives before a single file is opened, attached to
/// the flag that caused it.
fn calendar_date(text: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(text, "%Y%m%d").map_err(|_| {
        format!("{text:?} is not a calendar date; expected eight digits as YYYYMMDD, e.g. 20260901")
    })
}

// ---------------------------------------------------------------------------
// The flags `statusline` adds.
// ---------------------------------------------------------------------------

/// How the burn rate's intensity band is shown on the statusline, beyond the
/// bare cost-per-hour figure.
///
/// The command-line spelling of [`crate::view::statusline::BurnDisplay`],
/// kept apart from it for the same reason [`RowOrder`] is kept apart from
/// [`Order`]: `off`, `emoji`, `text` and `emoji-text` are words a user types
/// at a shell, not a concept the view layer should have to know clap exists
/// to spell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BurnVisual {
    /// The bare rate, and nothing else. The default, because a statusline is
    /// embedded in somebody else's prompt and the quietest option is the one
    /// least likely to fight it.
    Off,
    /// The rate followed by a coloured-circle emoji.
    Emoji,
    /// The rate followed by a bracketed word.
    Text,
    /// Both together.
    #[value(name = "emoji-text")]
    EmojiText,
}

impl From<BurnVisual> for crate::view::statusline::BurnDisplay {
    fn from(visual: BurnVisual) -> Self {
        match visual {
            BurnVisual::Off => Self::Off,
            BurnVisual::Emoji => Self::Emoji,
            BurnVisual::Text => Self::Text,
            BurnVisual::EmojiText => Self::EmojiText,
        }
    }
}

/// What `claude-stats statusline` asks for beyond the account itself.
///
/// A separate `clap::Args` struct for the same reason [`BlockFlags`] is one:
/// several of these translations have a rule in them that is easy to get
/// backwards -- `--cache`/`--no-cache` default to caching *on*, and the two
/// context thresholds must stay in the order their names promise -- and a
/// rule nobody can write a test for is a rule that quietly stops holding.
#[derive(Debug, Clone, clap::Args)]
pub struct StatuslineFlags {
    /// Whose calendar "today" is measured on.
    #[arg(short = 'z', long, value_name = "TZ", default_value = "local")]
    pub timezone: String,

    /// How the burn rate's intensity is shown beyond the bare cost-per-hour
    /// figure.
    #[arg(short = 'B', long, value_enum, default_value_t = BurnVisual::Off)]
    pub visual_burn_rate: BurnVisual,

    /// Cache the rendered line between redraws. This is the default.
    //
    // `SetTrue` rather than a plain `bool` field: the flag has to be
    // *typeable* even though [`Self::cache_enabled`] never reads it directly,
    // because a script that passes both `--no-cache` and `--cache` in that
    // order -- to override an earlier setting -- needs the second one to win,
    // and `overrides_with` is what makes that true rather than a parse error
    // about a flag given twice.
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "no_cache")]
    pub cache: bool,

    /// Always render fresh, ignoring any cached line.
    #[arg(long, action = clap::ArgAction::SetTrue, overrides_with = "cache")]
    pub no_cache: bool,

    /// How many seconds a cached line is still trusted for.
    #[arg(long, value_name = "SECONDS", default_value_t = 1)]
    pub refresh_interval: u64,

    /// Below this percentage of the context window, a reading is
    /// unremarkable.
    #[arg(
        long,
        value_name = "0-100",
        default_value_t = 50,
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    pub context_low_threshold: u8,

    /// At and above this percentage of the context window, a reading is
    /// worth a second look.
    #[arg(
        long,
        value_name = "0-100",
        default_value_t = 80,
        value_parser = clap::value_parser!(u8).range(0..=100)
    )]
    pub context_medium_threshold: u8,
}

impl StatuslineFlags {
    /// Whose calendar "today" is measured on.
    ///
    /// # Errors
    ///
    /// Returns an error for a zone name the IANA database does not know, for
    /// the same reason [`ReportOptions::zone`] refuses one: a statusline
    /// silently measured on the wrong calendar looks exactly like a correct
    /// one, and there is no second line beneath it to say otherwise.
    pub fn zone(&self) -> anyhow::Result<Zone> {
        Zone::parse(&self.timezone)
    }

    /// Whether the rendered line is cached between runs.
    ///
    /// `--no-cache` is the only flag actually consulted: `--cache` exists so
    /// that one can be typed after it to cancel it back out, per
    /// [`Self::cache`]'s own documentation.
    #[must_use]
    pub const fn cache_enabled(&self) -> bool {
        !self.no_cache
    }

    /// How long a cached line stays trusted, as a duration rather than a bare
    /// number of seconds.
    #[must_use]
    pub const fn refresh_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.refresh_interval)
    }

    /// Refuses a low threshold that is not below the medium one.
    ///
    /// # Errors
    ///
    /// Returns an error naming both figures when the low threshold is not
    /// strictly below the medium one. Accepting the two out of order would
    /// mean a future reader of a corrected value trusting a promise the pair
    /// no longer keeps.
    pub fn ensure_context_thresholds_are_ordered(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.context_low_threshold < self.context_medium_threshold,
            "the context low threshold ({}) must be lower than the medium threshold ({}); \
             swap the two or widen the gap between them",
            self.context_low_threshold,
            self.context_medium_threshold
        );
        Ok(())
    }
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Watch a session live in a second terminal. This is the default.
    Monitor,

    /// Print a one-shot report for a session and exit.
    Stats {
        /// Emit JSON instead of a formatted report.
        #[arg(long)]
        json: bool,
    },

    /// List the sessions on this machine, newest first.
    Sessions {
        /// Show at most this many.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Print the model catalogue: context windows and prices.
    Models,

    /// Total every billable response by calendar day.
    Daily {
        /// Split each day by project as well.
        ///
        /// Named for what a day's traffic is split into rather than for the
        /// splitting, matching the tools these figures get compared against.
        #[arg(short = 'i', long)]
        instances: bool,

        #[command(flatten)]
        last: LastFlag,

        #[command(flatten)]
        options: ReportOptions,
    },

    /// Total every billable response by week.
    Weekly {
        /// Which weekday a week begins on.
        ///
        /// There is no single right answer -- ISO 8601 says Monday, most of
        /// the Americas say Sunday -- so it is a flag, defaulting to the
        /// Sunday the neighbouring tools use. Spelled in full (`sunday`) or in
        /// three letters (`sun`).
        #[arg(short = 'w', long, value_name = "DAY", default_value = "sunday")]
        start_of_week: Weekday,

        #[command(flatten)]
        last: LastFlag,

        #[command(flatten)]
        options: ReportOptions,
    },

    /// Total every billable response by calendar month.
    Monthly {
        #[command(flatten)]
        last: LastFlag,

        #[command(flatten)]
        options: ReportOptions,
    },

    /// Total every billable response by conversation, dearest first.
    Session {
        /// Report only the session whose id starts with this prefix.
        ///
        /// A prefix, so the eight characters `claude-stats sessions` prints
        /// are enough to name one.
        #[arg(short = 'i', long, value_name = "PREFIX")]
        id: Option<String>,

        #[command(flatten)]
        options: ReportOptions,
    },

    /// Group every billable response into the five-hour window it was billed
    /// in, and say where the running one is heading.
    ///
    /// The rows come out oldest first whatever `--order` says, because the
    /// table is read as a timeline: a gap row only means anything between the
    /// two windows it separates, and the `REMAINING` and `PROJECTED` rows hang
    /// underneath the running block rather than floating above it.
    Blocks {
        #[command(flatten)]
        blocks: BlockFlags,

        #[command(flatten)]
        options: ReportOptions,
    },

    /// Print one line for the Claude Code prompt: model, session and today's
    /// spend, the running billing block and its burn rate, and how full the
    /// context window is.
    ///
    /// Reads the hook payload Claude Code writes to stdin -- this is not
    /// meant to be typed by hand -- and prints exactly one line to stdout,
    /// whatever goes wrong while producing it. See
    /// [`crate::infrastructure::statusline::cache`] for what happens when it
    /// does.
    Statusline {
        #[command(flatten)]
        flags: StatuslineFlags,
    },
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_definition_is_internally_consistent() {
        // Catches duplicated flags, bad defaults and conflicting short names
        // at test time rather than at the user's first run.
        Cli::command().debug_assert();
    }

    #[test]
    fn with_no_flags_the_active_session_is_followed() {
        let cli = Cli::parse_from(["claude-stats"]);
        assert_eq!(cli.selection.selector(), SessionSelector::Active);
        assert!(cli.command.is_none());
    }

    #[test]
    fn the_more_specific_selector_wins_over_the_broader_one() {
        let cli = Cli::parse_from(["claude-stats", "--session", "abc", "--project", "/tmp"]);
        assert_eq!(
            cli.selection.selector(),
            SessionSelector::Id("abc".to_owned())
        );
    }

    #[test]
    fn selection_flags_work_after_a_subcommand_too() {
        let cli = Cli::parse_from(["claude-stats", "stats", "--session", "abc"]);
        assert!(matches!(cli.command, Some(Command::Stats { json: false })));
        assert_eq!(
            cli.selection.selector(),
            SessionSelector::Id("abc".to_owned())
        );
    }

    /// The shared flags of whichever period command was parsed.
    ///
    /// A helper rather than a match at every call site, because every test
    /// below cares about the options and none of them cares which of the four
    /// commands carried them.
    fn options_of(cli: &Cli) -> &ReportOptions {
        match cli.command.as_ref().expect("a subcommand was given") {
            Command::Daily { options, .. }
            | Command::Weekly { options, .. }
            | Command::Monthly { options, .. }
            | Command::Session { options, .. }
            | Command::Blocks { options, .. } => options,
            other => panic!("{other:?} is not a period report"),
        }
    }

    #[test]
    fn a_malformed_since_date_is_refused_at_parse_time_rather_than_quietly_ignored() {
        // February never has thirty days. The dangerous outcome is not the
        // error, it is the report that would otherwise be produced over a
        // range nobody asked for, with nothing on it to say so.
        let refusal = Cli::try_parse_from(["claude-stats", "daily", "--since", "20260230"])
            .expect_err("30 February is not a date");
        let message = refusal.to_string();

        assert!(
            message.contains("YYYYMMDD"),
            "the message must name the format it wanted: {message}"
        );
        assert!(
            message.contains("20260230"),
            "and repeat what was rejected: {message}"
        );
        assert!(
            Cli::try_parse_from(["claude-stats", "daily", "--since", "2026-09-01"]).is_err(),
            "the dashed spelling is a different format and is not quietly accepted"
        );
        assert!(Cli::try_parse_from(["claude-stats", "daily", "--since", "20260228"]).is_ok());
    }

    #[test]
    fn the_bounds_are_read_in_the_reporting_timezone_not_in_utc() {
        // Midnight on 1 September in Tokyo is 15:00 on 31 August in UTC, and
        // the last instant of that Tokyo day is 14:59:59.999999999 UTC on the
        // 1st. Reading the flags as UTC instead would shift a whole report by
        // nine hours of traffic at each end.
        let cli = Cli::parse_from([
            "claude-stats",
            "daily",
            "--since",
            "20260901",
            "--until",
            "20260901",
            "--timezone",
            "Asia/Tokyo",
        ]);
        let tokyo = options_of(&cli).zone().expect("Tokyo is a real zone");
        let query = options_of(&cli).query(&tokyo);

        assert_eq!(
            query.since.expect("a lower bound").to_rfc3339(),
            "2026-08-31T15:00:00+00:00"
        );
        assert_eq!(
            query.until.expect("an upper bound").to_rfc3339(),
            "2026-09-01T14:59:59.999999999+00:00"
        );

        // The same two flags on the UTC calendar cover a different nine hours.
        let utc = Zone::Utc;
        let same_days = options_of(&cli).query(&utc);
        assert_eq!(
            same_days.since.expect("a lower bound").to_rfc3339(),
            "2026-09-01T00:00:00+00:00"
        );
        assert_ne!(query.since, same_days.since, "the zone has to matter");
    }

    #[test]
    fn the_upper_bound_stops_short_of_the_day_after_it() {
        // `day_bounds` is half-open and `UsageQuery` is inclusive at both ends.
        // Handing the half-open end straight over would count the first instant
        // of the following day into the report, so it has to be stepped back.
        let cli = Cli::parse_from(["claude-stats", "monthly", "--until", "20260901"]);
        let query = options_of(&cli).query(&Zone::Utc);
        let next_day: chrono::DateTime<chrono::Utc> =
            "2026-09-02T00:00:00Z".parse().expect("a valid timestamp");

        assert!(query.until.expect("an upper bound") < next_day);
    }

    #[test]
    fn requesting_online_pricing_explains_that_the_sheet_is_compiled_in() {
        let cli = Cli::parse_from(["claude-stats", "daily", "--online"]);
        let refusal = options_of(&cli)
            .ensure_offline()
            .expect_err("there is nothing to fetch");
        let message = refusal.to_string();

        assert!(
            message.contains("compiled in"),
            "the message must say why there is nothing to fetch: {message}"
        );
        assert!(
            message.contains("prices.json"),
            "and where a wrong rate is corrected: {message}"
        );
        // The flag is accepted rather than rejected by the parser, which is
        // the whole point: a ccusage script that passes it gets an
        // explanation instead of a parse error it cannot act on.
        assert!(Cli::try_parse_from(["claude-stats", "daily", "--online"]).is_ok());
        // Offline is the default, so the same command without the flag runs.
        let offline = Cli::parse_from(["claude-stats", "daily"]);
        assert!(options_of(&offline).ensure_offline().is_ok());
    }

    #[test]
    fn the_shared_flags_default_to_a_local_ascending_priced_report() {
        let cli = Cli::parse_from(["claude-stats", "weekly"]);
        let options = options_of(&cli);

        assert_eq!(options.zone().expect("local always parses"), Zone::Local);
        assert_eq!(Order::from(options.order), Order::Ascending);
        assert_eq!(CostMode::from(options.mode), CostMode::Auto);
        assert!(!options.json && !options.breakdown && !options.compact);
        assert_eq!(options.query(&Zone::Utc), UsageQuery::default());
    }

    #[test]
    fn a_week_starts_on_sunday_unless_another_day_is_named() {
        let default = Cli::parse_from(["claude-stats", "weekly"]);
        assert!(matches!(
            default.command,
            Some(Command::Weekly {
                start_of_week: Weekday::Sun,
                ..
            })
        ));

        let monday = Cli::parse_from(["claude-stats", "weekly", "--start-of-week", "mon"]);
        assert!(matches!(
            monday.command,
            Some(Command::Weekly {
                start_of_week: Weekday::Mon,
                ..
            })
        ));
        assert!(
            Cli::try_parse_from(["claude-stats", "weekly", "-w", "thursdayish"]).is_err(),
            "a day that is not a day is refused rather than defaulted"
        );
    }

    #[test]
    fn last_is_available_on_daily_weekly_and_monthly_but_not_on_session_or_blocks() {
        for words in [
            vec!["claude-stats", "daily", "--last", "3"],
            vec!["claude-stats", "weekly", "--last", "3"],
            vec!["claude-stats", "monthly", "--last", "3"],
        ] {
            Cli::try_parse_from(&words).unwrap_or_else(|e| panic!("{words:?} should parse: {e}"));
        }
        for words in [
            vec!["claude-stats", "session", "--last", "3"],
            vec!["claude-stats", "blocks", "--last", "3"],
        ] {
            assert!(
                Cli::try_parse_from(&words).is_err(),
                "{words:?} names a report `--last` does not answer for"
            );
        }
    }

    #[test]
    fn last_conflicts_with_since_and_with_until() {
        assert!(
            Cli::try_parse_from([
                "claude-stats",
                "daily",
                "--last",
                "3",
                "--since",
                "20260901"
            ])
            .is_err(),
            "both are ways of answering \"which periods\""
        );
        assert!(
            Cli::try_parse_from([
                "claude-stats",
                "daily",
                "--last",
                "3",
                "--until",
                "20260901"
            ])
            .is_err()
        );
    }

    #[test]
    fn the_last_n_days_reach_back_from_today_on_the_reports_own_calendar() {
        use crate::domain::period::AggregationPeriod;

        let cli = Cli::parse_from(["claude-stats", "daily", "--last", "3", "--timezone", "utc"]);
        let Some(Command::Daily { last, options, .. }) = &cli.command else {
            panic!("a daily command");
        };
        assert_eq!(last.last, Some(3));

        let zone = options.zone().expect("utc always parses");
        let today = zone.local_date(chrono::Utc::now());
        let (since, until) = AggregationPeriod::Day.last_n_bounds(3, today);
        assert_eq!(since, today - chrono::Duration::days(2));
        assert_eq!(until, today);
    }

    #[test]
    fn the_corpus_project_filter_reaches_the_query_whichever_side_of_the_command_it_is_typed() {
        // One spelling, one slot, read as a corpus filter by every period
        // command. It has to work on both sides of the subcommand because a
        // global argument may legitimately be typed either way round, and a
        // filter that silently went missing on one of them would print a total
        // over every project while claiming to be about one.
        for words in [
            ["claude-stats", "daily", "--project", "api"],
            ["claude-stats", "--project", "api", "daily"],
        ] {
            let cli = Cli::parse_from(words);
            assert_eq!(
                options_of(&cli).query(&Zone::Utc).projects,
                ProjectFilter::Named(vec!["api".to_owned()]),
                "{words:?} lost the filter"
            );
        }

        // The dashboard's own reading of the same flag is untouched.
        let monitor = Cli::parse_from(["claude-stats", "--project", "/home/ada/api", "monitor"]);
        assert_eq!(
            monitor.selection.selector(),
            SessionSelector::Project(PathBuf::from("/home/ada/api"))
        );
    }

    /// The `blocks`-only flags of a parsed command line.
    fn blocks_of(cli: &Cli) -> &BlockFlags {
        match cli.command.as_ref().expect("a subcommand was given") {
            Command::Blocks { blocks, .. } => blocks,
            other => panic!("{other:?} is not the blocks report"),
        }
    }

    #[test]
    fn a_block_window_is_five_hours_unless_another_length_is_named() {
        // The default is spelled as a string for clap and as a constant for
        // the domain, and this is what keeps the two the same number: change
        // `DEFAULT_SPAN_HOURS` without changing the flag and the assertion
        // below fails rather than the two quietly disagreeing.
        let default = Cli::parse_from(["claude-stats", "blocks"]);
        assert_eq!(
            blocks_of(&default).options().span,
            Duration::hours(DEFAULT_SPAN_HOURS)
        );

        // A fraction is accepted, because not every plan worth modelling is a
        // whole number of hours.
        let half = Cli::parse_from(["claude-stats", "blocks", "-n", "1.5"]);
        assert_eq!(
            blocks_of(&half).options().span,
            Duration::minutes(90),
            "an hour and a half"
        );
    }

    #[test]
    fn a_window_shorter_than_the_hour_it_is_anchored_to_is_refused() {
        // A block's start is floored back to the top of its first response's
        // UTC hour. Give the window less than an hour and that flooring
        // outruns it: a response at 09:47 opens a block stamped 09:00 that
        // ends at 09:30, so it is closed before it began, every later response
        // in the hour opens another block stamped 09:00 -- the instant the
        // JSON uses as an id -- and none of them can ever be the running one.
        //
        // The parser is also the only thing standing between a fraction and a
        // span of zero: `0.0001` hours is well under half a second, which
        // `Duration::seconds` would round away to nothing at all.
        for bad in ["0.5", "0.0001", "0.999"] {
            let typed = format!("--session-length={bad}");
            let refusal = Cli::try_parse_from(["claude-stats", "blocks", &typed])
                .expect_err("shorter than the hour it is anchored to");
            assert!(
                refusal.to_string().contains("at least 1"),
                "the message must name the floor it wanted: {refusal}"
            );
        }

        // An hour exactly is the shortest window that still works, so the
        // assertion above is about the boundary rather than about the fixture.
        let hourly = Cli::parse_from(["claude-stats", "blocks", "--session-length=1"]);
        assert_eq!(blocks_of(&hourly).options().span, Duration::hours(1));
    }

    #[test]
    fn a_window_of_no_length_is_refused_rather_than_dividing_the_day_into_nothing() {
        // A span of zero puts every response in a window of its own and leaves
        // the burn rate with no stretch to measure across; a negative one
        // closes every block before it opens. Both are typing mistakes, and
        // both produce a table that looks like an answer.
        //
        // Written with an `=` so that the negative case reaches this parser at
        // all: `-n -1` is a leading dash, which clap reads as the next flag
        // rather than as a value, and the refusal would then be about an
        // unexpected argument instead of about a window length.
        for bad in ["0", "-1", "abc", "9000"] {
            let typed = format!("--session-length={bad}");
            let refusal = Cli::try_parse_from(["claude-stats", "blocks", &typed])
                .expect_err("not a window length");
            assert!(
                refusal.to_string().contains("hours"),
                "the message must say what it wanted: {refusal}"
            );
        }
    }

    #[test]
    fn a_token_limit_is_either_a_number_or_the_word_max() {
        let stated = Cli::parse_from(["claude-stats", "blocks", "--token-limit", "88000"]);
        assert_eq!(
            blocks_of(&stated).options().token_limit,
            TokenLimit::Exact(88_000)
        );

        // Spelled either way round, because a flag typed in a hurry is not
        // typed carefully.
        for spelling in ["max", "MAX"] {
            let discovered = Cli::parse_from(["claude-stats", "blocks", "-t", spelling]);
            assert_eq!(
                blocks_of(&discovered).options().token_limit,
                TokenLimit::Max
            );
        }

        // Absent, there is no ceiling at all -- which is a different thing
        // from a ceiling of zero, and decides whether the table has a
        // percentage column.
        let bare = Cli::parse_from(["claude-stats", "blocks"]);
        assert_eq!(blocks_of(&bare).options().token_limit, TokenLimit::None);

        let refusal = Cli::try_parse_from(["claude-stats", "blocks", "-t", "maximum"])
            .expect_err("neither a number nor the word");
        let message = refusal.to_string();
        assert!(
            message.contains("88000") && message.contains("max"),
            "the message must name both spellings: {message}"
        );
    }

    #[test]
    fn asking_for_recent_blocks_means_the_last_few_days_rather_than_a_date() {
        let bare = Cli::parse_from(["claude-stats", "blocks"]);
        assert_eq!(blocks_of(&bare).options().recent_days, None);
        assert!(!blocks_of(&bare).options().active_only);

        let recent = Cli::parse_from(["claude-stats", "blocks", "--recent", "--active"]);
        assert_eq!(
            recent
                .command
                .as_ref()
                .map(|_| blocks_of(&recent).options().recent_days),
            Some(Some(DEFAULT_RECENT_DAYS))
        );
        assert!(blocks_of(&recent).options().active_only);
    }

    #[test]
    fn the_blocks_command_shares_the_period_reports_flags() {
        // Flattened rather than repeated, so `--since` cannot come to mean one
        // thing here and another on the daily table.
        let cli = Cli::parse_from([
            "claude-stats",
            "blocks",
            "--since",
            "20260901",
            "--json",
            "--timezone",
            "UTC",
        ]);
        let options = options_of(&cli);

        assert!(options.json);
        assert_eq!(options.zone().expect("UTC is a real zone"), Zone::Utc);
        assert_eq!(
            options
                .query(&Zone::Utc)
                .since
                .expect("a lower bound")
                .to_rfc3339(),
            "2026-09-01T00:00:00+00:00"
        );
    }

    #[test]
    fn the_two_short_forms_of_i_belong_to_different_commands() {
        // `-i` groups a day by project on `daily` and names a session on
        // `session`. They never appear together, so one letter can serve both,
        // but only as long as nothing quietly swaps them.
        let daily = Cli::parse_from(["claude-stats", "daily", "-i"]);
        assert!(matches!(
            daily.command,
            Some(Command::Daily {
                instances: true,
                ..
            })
        ));

        let session = Cli::parse_from(["claude-stats", "session", "-i", "0f3a9c21"]);
        assert!(matches!(
            session.command,
            Some(Command::Session { id: Some(ref prefix), .. }) if prefix == "0f3a9c21"
        ));
    }

    /// The `statusline`-only flags of a parsed command line.
    fn statusline_of(cli: &Cli) -> &StatuslineFlags {
        match cli.command.as_ref().expect("a subcommand was given") {
            Command::Statusline { flags } => flags,
            other => panic!("{other:?} is not the statusline command"),
        }
    }

    #[test]
    fn statusline_defaults_to_a_local_cached_report_refreshed_every_second() {
        let cli = Cli::parse_from(["claude-stats", "statusline"]);
        let flags = statusline_of(&cli);

        assert_eq!(flags.zone().expect("local always parses"), Zone::Local);
        assert!(flags.cache_enabled(), "caching is on unless refused");
        assert_eq!(
            flags.refresh_interval(),
            Duration::seconds(1).to_std().unwrap()
        );
        assert_eq!(flags.visual_burn_rate, BurnVisual::Off);
        assert!(flags.ensure_context_thresholds_are_ordered().is_ok());
        assert_eq!(flags.context_low_threshold, 50);
        assert_eq!(flags.context_medium_threshold, 80);
    }

    #[test]
    fn no_cache_disables_caching_and_a_later_cache_flag_cancels_it_back_out() {
        let disabled = Cli::parse_from(["claude-stats", "statusline", "--no-cache"]);
        assert!(!statusline_of(&disabled).cache_enabled());

        // A script composing flags may pass both, in which case the one typed
        // last wins -- the same rule `--online`/`--offline` and every other
        // paired flag in this crate follows.
        let re_enabled = Cli::parse_from(["claude-stats", "statusline", "--no-cache", "--cache"]);
        assert!(statusline_of(&re_enabled).cache_enabled());
    }

    #[test]
    fn a_context_low_threshold_that_is_not_below_the_medium_one_is_refused() {
        let equal = Cli::parse_from([
            "claude-stats",
            "statusline",
            "--context-low-threshold",
            "80",
            "--context-medium-threshold",
            "80",
        ]);
        let refusal = statusline_of(&equal)
            .ensure_context_thresholds_are_ordered()
            .expect_err("equal thresholds keep no promise apart");
        let message = refusal.to_string();
        assert!(message.contains("80"), "must name both figures: {message}");

        let reversed = Cli::parse_from([
            "claude-stats",
            "statusline",
            "--context-low-threshold",
            "90",
            "--context-medium-threshold",
            "50",
        ]);
        assert!(
            statusline_of(&reversed)
                .ensure_context_thresholds_are_ordered()
                .is_err()
        );

        // Out of the 0-100 range entirely is refused before the ordering
        // rule ever runs.
        assert!(
            Cli::try_parse_from([
                "claude-stats",
                "statusline",
                "--context-low-threshold",
                "101",
            ])
            .is_err()
        );
    }

    #[test]
    fn the_visual_burn_rate_flag_accepts_all_four_spellings() {
        for (typed, expected) in [
            ("off", BurnVisual::Off),
            ("emoji", BurnVisual::Emoji),
            ("text", BurnVisual::Text),
            ("emoji-text", BurnVisual::EmojiText),
        ] {
            let cli = Cli::parse_from(["claude-stats", "statusline", "--visual-burn-rate", typed]);
            assert_eq!(statusline_of(&cli).visual_burn_rate, expected, "{typed}");
        }
    }
}

//! The dashboard's view model: the first step of a Two Step View (Fowler,
//! *Patterns of Enterprise Application Architecture*) over the whole live
//! dashboard.
//!
//! [`build`] turns a [`SessionSnapshot`], an optional [`AccountUsage`]
//! reading and a handful of animation parameters into a
//! [`DashboardViewModel`] -- a plain data tree with every string already
//! formatted and every colour decision already reduced to a
//! [`FillSeverity`] band rather than a concrete ratatui `Color`. Nothing here
//! imports ratatui or `crate::tui`, which is what lets every field on this
//! page be asserted in a unit test that never opens a terminal. It is the
//! same seam wave 1 already cut for the period reports in
//! [`crate::view::table`] and [`crate::view::usage_view`] -- this is that
//! seam, one level up, for the screen that redraws every frame rather than
//! once per command.
//!
//! `crate::tui::panels` is the second step: its [`PanelRegistry`] reads this
//! tree and turns each field into a widget call. Splitting the two is what
//! lets a panel's renderer function share one signature with every other
//! panel in the registry -- `(Frame, Rect, &DashboardViewModel, &Palette,
//! u64)` -- instead of each widget needing its own bespoke slice of
//! [`SessionSnapshot`] and [`AccountUsage`] threaded through by hand, which is
//! what `src/tui/screens/dashboard.rs` still does and what made it
//! impossible to lay panels out any way other than the one nested
//! `Layout::vertical`/`horizontal` call tree it hard-codes.
//!
//! [`PanelRegistry`]: crate::tui::panels::PanelRegistry
//!
//! # Why [`AccountView`] and [`SpendView`] hold a whole [`AccountUsage`]
//!
//! Every other view here takes a domain reading apart into the handful of
//! primitives its panel actually prints. [`AccountView`] and [`SpendView`]
//! do not, and that is deliberate rather than an oversight: their panels'
//! renderers build [`UsageWindows`] and [`SpendPanel`] "exactly as
//! `dashboard.rs` already does", and both of those widgets are constructed
//! from a whole `&AccountUsage` -- not from a decomposed handful of fields --
//! because [`AccountUsage`] is already the plain, ratatui-free read model
//! [`crate::domain::limits`] promises. Re-stating its fields here would only
//! create a second copy of the same data for that module's own doc comment
//! (on why the panel deliberately reports nothing it has not measured) to
//! drift out of step with. Holding the reading twice -- one clone per panel
//! that needs it -- costs a copy of a small, capacity-bounded struct once a
//! frame, which is a fair trade for two panels that can then each degrade
//! independently exactly as [`crate::tui::screens::dashboard`] already lets
//! them.
//!
//! [`UsageWindows`]: crate::tui::widgets::usage_windows::UsageWindows
//! [`SpendPanel`]: crate::tui::widgets::spend_panel::SpendPanel

use std::collections::VecDeque;

use chrono::NaiveDate;

use crate::domain::activity::ToolEvent;
use crate::domain::blocks::LimitStanding;
use crate::domain::context::{CompactionDistance, FillSeverity};
use crate::domain::limits::AccountUsage;
use crate::domain::model::ModelCatalog;
use crate::domain::money::Usd;
use crate::domain::project::Project;
use crate::domain::session::{SessionPhase, SessionSnapshot};

use super::format;

/// The session identity strip: which session this is, and whether it is
/// currently live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderView {
    /// A short label identifying the session -- the project it was started
    /// in when one is known, or the session id shortened to what a header
    /// has room for otherwise.
    pub session_label: String,
    /// Whether the assistant is mid-reply.
    pub is_live: bool,
}

/// One of the six headline tiles.
///
/// `icon_key` and `label` are chosen from a small closed set
/// (`"context"`/`"CONTEXT"`, `"cost"`/`"COST"`, and so on) rather than being
/// a free-form string, so that `crate::tui::panels`' shared tile renderer can
/// map `icon_key` to the matching glyph in `crate::tui::icons::Icon` with a
/// single match rather than needing six near-identical renderer functions.
#[derive(Debug, Clone, PartialEq)]
pub struct TileView {
    pub icon_key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub footnote: Option<String>,
    /// How urgent this reading is, when the tile has an opinion. `None` for
    /// a tile that is purely informational -- cost and turn count carry no
    /// notion of "too high", so they are drawn in the palette's ordinary
    /// accent rather than borrowing a colour that would claim otherwise.
    pub severity: Option<FillSeverity>,
}

/// The context-window gauge and the figures printed beside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextView {
    pub used_pct: f64,
    pub used_tokens: u64,
    pub window_tokens: u64,
    /// How many context tokens a turn has added on average, rounded down to
    /// a whole token -- the same figure `dashboard.rs::draw_context_panel`'s
    /// "growth/turn" field prints, carried here so `panel.context-gauge` can
    /// print it too rather than silently dropping a quarter of that panel's
    /// caption line.
    pub growth_per_turn_tokens: u64,
    pub severity: FillSeverity,
    /// Where the auto-compaction threshold sits, as a percentage of the
    /// window. `None` only for a session whose model is not yet known, where
    /// the window itself is `0` and a percentage of it is not a number.
    pub compaction_threshold_pct: Option<f64>,
}

/// The account-usage panel's reading: the rolling five-hour and seven-day
/// windows, the current and previous calendar months, and whatever limit
/// banner is currently in force -- everything [`UsageWindows`] prints, in the
/// same shape it already reads it in.
///
/// See the module doc for why this wraps the whole [`AccountUsage`] rather
/// than restating its fields.
///
/// [`UsageWindows`]: crate::tui::widgets::usage_windows::UsageWindows
#[derive(Debug, Clone, PartialEq)]
pub struct AccountView {
    pub usage: AccountUsage,
    /// Whether a reading has actually been taken yet. See
    /// [`SpendView::measured`] -- the two panels share one promise.
    pub measured: bool,
}

/// The spend panel's reading: today's spend, the active billing block and
/// its projection, and the busiest projects -- everything [`SpendPanel`]
/// prints, in the same shape it already reads it in.
///
/// See the module doc for why this wraps the whole [`AccountUsage`] rather
/// than restating its fields.
///
/// [`SpendPanel`]: crate::tui::widgets::spend_panel::SpendPanel
#[derive(Debug, Clone, PartialEq)]
pub struct SpendView {
    pub usage: AccountUsage,
    pub measured: bool,
}

/// The context fill, reduced to what [`ContextBanner`] needs to draw the
/// oversized percentage.
///
/// [`ContextBanner`]: crate::tui::widgets::banner::ContextBanner
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextBannerView {
    pub used_tokens: u64,
    pub window_tokens: u64,
}

/// The session-detail trend panel: the output-per-response sparkline, the
/// cache/efficiency/context meters beneath it, and the embedded context
/// banner it makes room for when the panel is tall enough.
#[derive(Debug, Clone, PartialEq)]
pub struct TrendView {
    /// Output tokens per response, oldest first.
    pub output_series: Vec<u64>,
    /// Indices into `output_series` where a compaction happened.
    pub compaction_markers: Vec<usize>,
    pub cache_ratio: Option<f64>,
    pub efficiency_ratio: Option<f64>,
    pub context_ratio: f64,
    pub context_severity: FillSeverity,
    /// The banner's own data. Optional so that a future panel preset without
    /// room for it can be built without inventing a fill reading of `0/0` to
    /// stand for "not drawn"; today's dashboard always has a fill to show,
    /// so `build` always fills this in and it is [`crate::tui::panels`] that
    /// decides, from the space it was given, whether to draw it.
    pub banner: Option<ContextBannerView>,
}

/// The four token kinds a session is billed for, as [`TokenMix`] draws them.
///
/// [`TokenMix`]: crate::tui::widgets::token_mix::TokenMix
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenMixView {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// The live tool-activity feed, exactly as [`ToolFeed`] already consumes it.
///
/// [`ToolFeed`]: crate::tui::widgets::tool_feed::ToolFeed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityView {
    /// Recent tool calls, oldest first -- [`ToolFeed`] reverses them itself.
    ///
    /// [`ToolFeed`]: crate::tui::widgets::tool_feed::ToolFeed
    pub events: VecDeque<ToolEvent>,
    /// Whether the assistant is mid-turn, which puts a spinner on the
    /// newest entry.
    pub running: bool,
}

/// The counters reset every time the user sends a new message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnView {
    pub tool_calls: u32,
    pub thinking_blocks: u32,
    pub tool_errors: u32,
    pub agents_running: u32,
    pub active_skill: Option<String>,
    /// The most recent error message for the session as a whole, not only
    /// this turn -- `dashboard.rs`'s "this turn" panel has always shown the
    /// session's last error rather than clearing it at the next turn
    /// boundary, on the reasoning that a user who just watched a tool fail
    /// wants to keep seeing that until something else happens, not have it
    /// vanish the moment they send the next message.
    pub last_error: Option<String>,
}

/// One calendar day's spend, for each of the last seven days that saw any
/// activity, oldest first -- exactly [`AccountUsage::daily_spend`].
///
/// `None` only when nothing has been scanned yet, mirroring
/// [`TopProjectsView`]: a dashboard that has only just opened shows no chart
/// rather than an honestly-empty one a reader could mistake for "every day
/// cost nothing".
#[derive(Debug, Clone, PartialEq)]
pub struct DailySpendView {
    pub days: Vec<(NaiveDate, Usd)>,
}

/// What each model contributed inside the last seven days, dearest first --
/// [`AccountUsage::model_breakdown`], with each [`ModelId`] rendered through
/// [`ModelCatalog::display_name_for`] the same way
/// [`crate::view::usage_view`]'s own model column already does, so the two
/// screens that ever name a model cannot disagree about what to call it.
///
/// [`ModelId`]: crate::domain::model::ModelId
#[derive(Debug, Clone, PartialEq)]
pub struct ModelBreakdownView {
    pub rows: Vec<(String, Usd)>,
}

/// How fast the active billing block is being spent, and where that lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurnRateView {
    /// Fresh input and output tokens per minute -- see
    /// [`crate::domain::blocks::BurnRate::indicator_tokens_per_minute`] for
    /// why cache traffic is excluded from this figure.
    pub intensity: f64,
    pub projection: Usd,
    /// How the projection sits against a token limit. Always `None` today:
    /// no token ceiling reaches the live dashboard, for the same reason
    /// [`crate::tui::widgets::usage_windows`] never draws a percentage of a
    /// limit -- see that module's doc. [`LimitStanding::of`] needs a `limit`
    /// nothing here is given, so the honest answer is `None` rather than
    /// measuring the projection against a number nobody supplied.
    pub limit_standing: Option<LimitStanding>,
}

/// The busiest projects inside the last seven days, dearest first, capped at
/// five -- exactly what [`SpendPanel`] already prints.
///
/// [`SpendPanel`]: crate::tui::widgets::spend_panel::SpendPanel
#[derive(Debug, Clone, PartialEq)]
pub struct TopProjectsView {
    pub rows: Vec<(String, Usd)>,
}

/// The animated "$" marker's own state: how bright it is drawn, and how many
/// frames it has been since the accrued cost last ticked up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DollarPulseView {
    /// Today's spend as a fraction of the busiest day in
    /// [`AccountUsage::daily_spend`], clamped to `1.0` -- a day can equal but
    /// never usefully exceed its own record. `0.0` whenever there is no
    /// usage reading at all, or nothing inside it has been scanned yet, so a
    /// dashboard that has only just opened shows the marker empty rather
    /// than a guess.
    pub level: f64,
    pub frames_since_increment: Option<u64>,
    /// Whether the `NO_ANIMATION`/`CLAUDE_STATS_NO_ANIMATION` environment
    /// variables forced every animation off.
    ///
    /// A plain `bool` rather than
    /// `crate::tui::widgets::dollar_pulse::AnimationStyle` itself: this
    /// module promises never to import `crate::tui` -- see the module doc --
    /// and a bare flag is all this epic's own hard-coded "`Pulse` or `Off`"
    /// choice needs. Widening this to carry the full style is a later
    /// epic's job, once a config value can actually select something other
    /// than those two.
    pub off: bool,
}

/// The whole dashboard, as plain data.
///
/// A Value Object in Fowler's sense: built once by [`build`], compared by
/// what it holds, and never mutated by anything that reads it. Every field on
/// it is either a primitive, a `String` already formatted for the screen, or
/// one of the domain's own read-model types -- see the module doc for why
/// [`AccountView`] and [`SpendView`] are the two places that last category
/// shows through.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardViewModel {
    pub header: HeaderView,
    pub tiles: [TileView; 6],
    pub context: ContextView,
    pub account: Option<AccountView>,
    pub spend: Option<SpendView>,
    pub trend: TrendView,
    pub token_mix: TokenMixView,
    pub activity: ActivityView,
    pub turn: TurnView,
    pub daily_spend: Option<DailySpendView>,
    pub model_breakdown: Option<ModelBreakdownView>,
    pub burn_rate: Option<BurnRateView>,
    pub top_projects: Option<TopProjectsView>,
    pub dollar_pulse: DollarPulseView,
}

/// Builds the dashboard's view model for one frame.
///
/// `usage` mirrors exactly what `dashboard.rs::draw` is handed today: the
/// account-wide reading together with whether it has actually been measured
/// yet, or `None` while nothing is tracking account usage at all.
///
/// `phase` is accepted but not read here. Every panel in
/// [`crate::tui::panels::PanelRegistry`] receives the live animation phase
/// directly as its own trailing `u64` argument at render time -- the same
/// tick, fresh every frame -- so nothing in this immutable, once-per-frame
/// tree needs to freeze a copy of it. It stays a parameter of `build` even so,
/// for the same reason `pulse_level` and `pulse_frames_since` are: a caller
/// building a view model should not need to know which fields happen to use
/// which animation input this epic, only that all three are handed to it
/// alongside the session and the usage reading.
///
/// `pulse_frames_since` is threaded straight through into
/// [`DollarPulseView::frames_since_increment`] because, unlike `phase`, it is
/// not shared across every panel -- it belongs to exactly one meter's own
/// `crate::tui::widgets::dollar_pulse::PulseClock`, which lives in
/// `crate::tui` and so cannot be a parameter here (see the module doc's
/// opening paragraph). `DollarPulseView::level` is not a parameter for the
/// opposite reason: it is derived entirely from `usage`, which this function
/// already has, so computing it here (in [`build_dollar_pulse`]) rather than
/// asking every caller to compute it first keeps that arithmetic in the one
/// place [`build_daily_spend`] and [`build_top_projects`] already do the
/// same kind of work.
///
/// `pulse_off` mirrors `pulse_frames_since` for the same reason:
/// `AnimationStyle` itself is a `crate::tui` type, so the caller reduces its
/// choice to the one bit this tree can honestly carry -- see
/// [`DollarPulseView::off`]'s own doc.
#[must_use]
pub fn build(
    snapshot: &SessionSnapshot,
    usage: Option<(&AccountUsage, bool)>,
    _phase: u64,
    pulse_frames_since: Option<u64>,
    pulse_off: bool,
) -> DashboardViewModel {
    DashboardViewModel {
        header: build_header(snapshot),
        tiles: build_tiles(snapshot),
        context: build_context(snapshot),
        account: build_account(usage),
        spend: build_spend(usage),
        trend: build_trend(snapshot),
        token_mix: build_token_mix(snapshot),
        activity: build_activity(snapshot),
        turn: build_turn(snapshot),
        daily_spend: build_daily_spend(usage),
        model_breakdown: build_model_breakdown(usage),
        burn_rate: build_burn_rate(usage),
        top_projects: build_top_projects(usage),
        dollar_pulse: build_dollar_pulse(usage, pulse_frames_since, pulse_off),
    }
}

fn build_header(snapshot: &SessionSnapshot) -> HeaderView {
    HeaderView {
        session_label: header_label(snapshot),
        is_live: snapshot.phase == SessionPhase::Thinking,
    }
}

/// The project a session was started in, when one is known; the session id
/// shortened to what a header has room for otherwise.
///
/// Reuses [`Project::display_name`] rather than re-deriving "the last path
/// segment" here, so the two places on screen that ever name a project --
/// this header and the spend panel's project rows -- cannot disagree about
/// what counts as the display name of a working directory.
fn header_label(snapshot: &SessionSnapshot) -> String {
    snapshot.project_dir.as_deref().map_or_else(
        || format::session_id(&snapshot.session_id).to_owned(),
        |dir| Project::new(dir).display_name().to_owned(),
    )
}

/// The six headline tiles, in the fixed order `dashboard.rs` has always
/// drawn them: context, cost, cache, compaction, turns, errors.
fn build_tiles(snapshot: &SessionSnapshot) -> [TileView; 6] {
    let fill = snapshot.context_fill();
    let cache_ratio = snapshot.cache_hit_ratio();
    let compaction = snapshot.compaction_distance();

    [
        TileView {
            icon_key: "context",
            label: "CONTEXT",
            value: format::percent_precise(fill.ratio()),
            footnote: Some(format!(
                "{} / {}",
                format::tokens(fill.used()),
                format::tokens(fill.window())
            )),
            severity: Some(fill.severity()),
        },
        TileView {
            icon_key: "cost",
            label: "COST",
            value: snapshot.cost().to_string(),
            footnote: Some(format!("{}/turn", snapshot.cost_per_turn())),
            // Cost carries no notion of "too high" on its own -- unlike the
            // context fill, there is no threshold past which a dollar figure
            // becomes urgent in a way this view model can judge -- so it is
            // drawn in the palette's ordinary accent rather than borrowing a
            // colour that would claim otherwise.
            severity: None,
        },
        TileView {
            icon_key: "cache",
            label: "CACHE",
            value: cache_ratio.map_or_else(|| "\u{2014}".to_owned(), format::percent_precise),
            footnote: Some(format!(
                "{} read",
                format::tokens(snapshot.totals.cache_read)
            )),
            severity: cache_severity(cache_ratio),
        },
        TileView {
            icon_key: "compaction",
            label: "COMPACTION",
            value: compaction_text(compaction),
            footnote: Some(format!("{} so far", snapshot.compactions.len())),
            severity: compaction_severity(compaction),
        },
        TileView {
            icon_key: "turns",
            label: "TURNS",
            value: snapshot.turns.to_string(),
            footnote: Some(format!("{} tools", snapshot.tool_calls())),
            // A turn count is a fact about how long the conversation has
            // run, not a reading with a "bad" end of its scale.
            severity: None,
        },
        TileView {
            icon_key: "errors",
            label: "ERRORS",
            value: snapshot.tool_errors.to_string(),
            footnote: Some(format!("{} files", snapshot.files_touched())),
            severity: Some(if snapshot.tool_errors == 0 {
                FillSeverity::Comfortable
            } else {
                FillSeverity::Critical
            }),
        },
    ]
}

/// The cache tile's urgency band.
///
/// A *low* ratio is the bad one here, which is the opposite of every other
/// severity in this file: the conversation prefix is being re-sent rather
/// than reused, and that is where most of an unexpected bill comes from.
/// Reusing [`FillSeverity`] for that inverted scale rather than inventing a
/// second enum keeps every tile's urgency in the one vocabulary
/// `crate::tui::palette::Palette::severity` already knows how to colour;
/// only three of its four bands are used, because the cache tile has never
/// drawn a middling "warm" tone of its own -- it has always been read as
/// good, borderline or bad, and [`FillSeverity::Hot`] carries the borderline
/// colour that the pre-migration widget's amber used to.
fn cache_severity(ratio: Option<f64>) -> Option<FillSeverity> {
    let ratio = ratio?;
    Some(if ratio >= 0.85 {
        FillSeverity::Comfortable
    } else if ratio >= 0.50 {
        FillSeverity::Hot
    } else {
        FillSeverity::Critical
    })
}

/// The compaction tile's urgency band, following the same four readings
/// `dashboard.rs` has always drawn: imminent, comfortably far off, close
/// enough to watch, or too early in the session to know.
fn compaction_severity(distance: CompactionDistance) -> Option<FillSeverity> {
    match distance {
        CompactionDistance::Imminent => Some(FillSeverity::Critical),
        CompactionDistance::Turns(n) if n >= 100 => Some(FillSeverity::Comfortable),
        CompactionDistance::Turns(n) if n <= 3 => Some(FillSeverity::Hot),
        CompactionDistance::Turns(_) => Some(FillSeverity::Warm),
        // Not enough history yet to estimate a distance at all, so there is
        // nothing to grade -- an early session is not "urgent", it is simply
        // unmeasured.
        CompactionDistance::Unknown => None,
    }
}

/// The compaction tile's headline text, exactly as `dashboard.rs` has always
/// worded it.
fn compaction_text(distance: CompactionDistance) -> String {
    match distance {
        CompactionDistance::Imminent => "imminent".to_owned(),
        CompactionDistance::Turns(n) if n >= 100 => "100+ turns".to_owned(),
        CompactionDistance::Turns(n) => format!("~{n} turn{}", if n == 1 { "" } else { "s" }),
        CompactionDistance::Unknown => "\u{2014}".to_owned(),
    }
}

fn build_context(snapshot: &SessionSnapshot) -> ContextView {
    let fill = snapshot.context_fill();
    ContextView {
        used_pct: fill.percent(),
        used_tokens: fill.used(),
        window_tokens: fill.window(),
        growth_per_turn_tokens: snapshot.average_context_growth_per_turn() as u64,
        severity: fill.severity(),
        compaction_threshold_pct: compaction_threshold_pct(fill.window()),
    }
}

/// Where the auto-compaction threshold sits along the window, as a
/// percentage.
///
/// `None` only when the window itself is `0` -- a session whose model string
/// is not yet known -- because a percentage of nothing is not a number a
/// gauge can place a marker at.
fn compaction_threshold_pct(window: u64) -> Option<f64> {
    if window == 0 {
        return None;
    }
    let threshold = window.saturating_sub(ModelCatalog::COMPACTION_BUFFER);
    Some(threshold as f64 / window as f64 * 100.0)
}

/// See the module doc for why this clones the whole reading rather than
/// taking it apart.
fn build_account(usage: Option<(&AccountUsage, bool)>) -> Option<AccountView> {
    usage.map(|(usage, measured)| AccountView {
        usage: usage.clone(),
        measured,
    })
}

/// See the module doc for why this clones the whole reading rather than
/// taking it apart.
fn build_spend(usage: Option<(&AccountUsage, bool)>) -> Option<SpendView> {
    usage.map(|(usage, measured)| SpendView {
        usage: usage.clone(),
        measured,
    })
}

/// The last seven days' spend, oldest first.
///
/// `None` when nothing has been scanned yet, for the same reason
/// [`build_top_projects`] returns `None` rather than an empty list.
fn build_daily_spend(usage: Option<(&AccountUsage, bool)>) -> Option<DailySpendView> {
    let (usage, _measured) = usage?;
    if usage.daily_spend.is_empty() {
        return None;
    }
    Some(DailySpendView {
        days: usage.daily_spend.clone(),
    })
}

/// What each model contributed inside the last seven days, dearest first.
///
/// `None` when nothing has been scanned yet, for the same reason
/// [`build_top_projects`] returns `None` rather than an empty list.
fn build_model_breakdown(usage: Option<(&AccountUsage, bool)>) -> Option<ModelBreakdownView> {
    let (usage, _measured) = usage?;
    if usage.model_breakdown.is_empty() {
        return None;
    }
    Some(ModelBreakdownView {
        rows: usage
            .model_breakdown
            .iter()
            .map(|share| {
                (
                    ModelCatalog::display_name_for(share.model.as_str()),
                    share.cost,
                )
            })
            .collect(),
    })
}

/// The active billing block's burn rate and projection, when there is one
/// running.
///
/// `None` whenever there is no active block, or the block has not yet seen
/// the two responses [`crate::domain::blocks::BurnRate::measure`] needs to
/// measure a rate from -- in both cases there is nothing true to project, and
/// [`crate::domain::limits::AccountUsage::measure`] already guarantees
/// `active_burn` and `active_projection` agree about which one it is.
fn build_burn_rate(usage: Option<(&AccountUsage, bool)>) -> Option<BurnRateView> {
    let (usage, _measured) = usage?;
    let burn = usage.active_burn?;
    let projection = usage.active_projection?;
    Some(BurnRateView {
        intensity: burn.indicator_tokens_per_minute,
        projection: projection.cost,
        limit_standing: None,
    })
}

/// The busiest projects inside the last seven days, dearest first.
///
/// `None` when nothing has been scanned yet, so a dashboard that has only
/// just opened shows no list rather than an honestly-empty one that a reader
/// could mistake for "every project cost nothing".
fn build_top_projects(usage: Option<(&AccountUsage, bool)>) -> Option<TopProjectsView> {
    let (usage, _measured) = usage?;
    if usage.top_projects.is_empty() {
        return None;
    }
    Some(TopProjectsView {
        rows: usage
            .top_projects
            .iter()
            .map(|project| (project.project.display_name().to_owned(), project.cost))
            .collect(),
    })
}

/// The animated "$" marker's reading -- see [`DollarPulseView`]'s own doc
/// for what each field means.
fn build_dollar_pulse(
    usage: Option<(&AccountUsage, bool)>,
    frames_since_increment: Option<u64>,
    off: bool,
) -> DollarPulseView {
    DollarPulseView {
        level: dollar_pulse_level(usage),
        frames_since_increment,
        off,
    }
}

/// Today's spend as a fraction of the busiest day inside
/// [`AccountUsage::daily_spend`] -- see [`DollarPulseView::level`]'s own doc
/// for the exact rule.
fn dollar_pulse_level(usage: Option<(&AccountUsage, bool)>) -> f64 {
    let Some((usage, _measured)) = usage else {
        return 0.0;
    };
    let busiest_day = usage
        .daily_spend
        .iter()
        .map(|(_, cost)| cost.dollars())
        .fold(0.0_f64, f64::max);
    if busiest_day <= 0.0 {
        return 0.0;
    }
    (usage.today.cost.dollars() / busiest_day).clamp(0.0, 1.0)
}

fn build_trend(snapshot: &SessionSnapshot) -> TrendView {
    let fill = snapshot.context_fill();
    TrendView {
        output_series: snapshot.output_series(),
        compaction_markers: snapshot.compaction_marker_indices(),
        cache_ratio: snapshot.cache_hit_ratio(),
        efficiency_ratio: snapshot.efficiency(),
        context_ratio: fill.ratio(),
        context_severity: fill.severity(),
        banner: Some(ContextBannerView {
            used_tokens: fill.used(),
            window_tokens: fill.window(),
        }),
    }
}

fn build_token_mix(snapshot: &SessionSnapshot) -> TokenMixView {
    TokenMixView {
        input: snapshot.totals.input,
        output: snapshot.totals.output,
        cache_read: snapshot.totals.cache_read,
        cache_write: snapshot.totals.cache_creation(),
    }
}

fn build_activity(snapshot: &SessionSnapshot) -> ActivityView {
    ActivityView {
        events: snapshot.recent_tools.clone(),
        running: snapshot.phase == SessionPhase::Thinking,
    }
}

fn build_turn(snapshot: &SessionSnapshot) -> TurnView {
    TurnView {
        tool_calls: snapshot.turn.tool_calls(),
        thinking_blocks: snapshot.turn.thinking_blocks,
        tool_errors: snapshot.turn.tool_errors,
        agents_running: snapshot.turn.agents_running,
        active_skill: snapshot.turn.active_skill.clone(),
        last_error: snapshot.last_error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::model::ModelId;
    use crate::domain::period::Zone;
    use crate::domain::pricing::PriceSheet;
    use crate::domain::project::SessionId;
    use crate::domain::session::ResponseSample;
    use crate::domain::tokens::TokenUsage;

    /// The instant the tests below measure account usage as of. Fixed rather
    /// than read from the system clock, so a block and the windows built
    /// from it come out the same on every run and every machine -- the same
    /// fixture `dashboard.rs`'s own tests use, under the same name.
    fn fixed_now() -> DateTime<Utc> {
        "2026-09-01T10:00:00Z".parse().expect("a valid timestamp")
    }

    /// The session `dashboard.rs`'s own tests build, reproduced here rather
    /// than shared: the two modules are not allowed to depend on one
    /// another's `#[cfg(test)]` items, and the shape is small enough that
    /// keeping two copies in step is cheaper than inventing a third crate
    /// just to hold one fixture.
    fn sample_snapshot() -> SessionSnapshot {
        let mut s = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abcdef".to_owned());
        s.model_id = "claude-opus-5".to_owned();
        s.project_dir = Some("/home/ada/code/app".to_owned());
        s.git_branch = Some("main".to_owned());
        s.turns = 4;
        s.totals.input = 1_000;
        s.totals.cache_read = 500_000;
        s.totals.output = 20_000;
        s.samples.push(ResponseSample {
            turn: 4,
            prompt_tokens: 501_000,
            output_tokens: 900,
            at: Utc::now(),
        });
        s
    }

    /// One billable response, for the tests that need account usage measured
    /// from real entries rather than [`AccountUsage::empty`].
    fn measured_entry(session: &str, when: &str, project: &str, input: u64) -> Entry {
        let at: DateTime<Utc> = when.parse().expect("a valid timestamp");
        Entry {
            id: EntryId {
                message_id: format!("msg-{session}-{when}-{input}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at,
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new(session),
            project: Project::new(project),
            is_sidechain: false,
        }
    }

    #[test]
    fn building_the_view_model_carries_the_sessions_headline_metrics() {
        let model = build(&sample_snapshot(), None, 0, None, false);

        assert_eq!(model.header.session_label, "app");
        assert!(!model.header.is_live, "the fixture is idle");
        assert_eq!(model.tiles[0].label, "CONTEXT");
        assert_eq!(model.tiles[1].value, sample_snapshot().cost().to_string());
        assert!(model.account.is_none(), "no usage reading was given");
        assert!(model.spend.is_none());
        assert!(model.daily_spend.is_none());
        assert!(model.model_breakdown.is_none());
        assert!(model.burn_rate.is_none(), "no active block in the fixture");
        assert!(model.top_projects.is_none());
        assert!((model.dollar_pulse.level - 0.0).abs() < f64::EPSILON);
        assert!(!model.dollar_pulse.off);
    }

    #[test]
    fn building_the_view_model_carries_the_pulses_own_animation_inputs_through() {
        let model = build(&sample_snapshot(), None, 0, Some(3), true);
        assert_eq!(model.dollar_pulse.frames_since_increment, Some(3));
        assert!(model.dollar_pulse.off);
    }

    #[test]
    fn dollar_pulse_level_is_zero_without_a_usage_reading() {
        assert!((dollar_pulse_level(None) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn dollar_pulse_level_is_zero_when_nothing_has_been_scanned() {
        let usage = AccountUsage::empty(fixed_now());
        assert!((dollar_pulse_level(Some((&usage, true))) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn dollar_pulse_level_is_todays_spend_over_the_busiest_day_on_record() {
        let now = fixed_now();
        let entries = [
            // The busiest day: 400k tokens, priced at $5/million for Opus 5
            // input, is $2.00.
            measured_entry("a", "2026-08-31T09:30:00Z", "/home/ada/api", 400_000),
            // Today: half that day's spend.
            measured_entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 200_000),
        ];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let level = dollar_pulse_level(Some((&usage, true)));
        assert!((level - 0.5).abs() < 1e-9, "got {level}");
    }

    #[test]
    fn dollar_pulse_level_is_clamped_to_one_even_on_the_record_day_itself() {
        let now = fixed_now();
        let entries = [measured_entry(
            "a",
            "2026-09-01T09:30:00Z",
            "/home/ada/api",
            400_000,
        )];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let level = dollar_pulse_level(Some((&usage, true)));
        assert!((level - 1.0).abs() < 1e-9, "got {level}");
    }

    #[test]
    fn build_header_names_the_project_the_session_was_started_in() {
        let header = build_header(&sample_snapshot());
        assert_eq!(header.session_label, "app");
    }

    #[test]
    fn build_header_falls_back_to_the_session_id_without_a_project() {
        let mut snapshot = sample_snapshot();
        snapshot.project_dir = None;
        let header = build_header(&snapshot);
        assert_eq!(header.session_label, "abcdef");
    }

    #[test]
    fn build_tiles_reads_the_context_fill_into_the_first_tile() {
        let tiles = build_tiles(&sample_snapshot());
        let fill = sample_snapshot().context_fill();

        assert_eq!(tiles[0].icon_key, "context");
        assert_eq!(tiles[0].value, format::percent_precise(fill.ratio()));
        assert_eq!(tiles[0].severity, Some(fill.severity()));
    }

    #[test]
    fn build_tiles_grades_the_cache_tile_the_opposite_way_round() {
        let mut snapshot = sample_snapshot();
        // A cache ratio under 50% is the bad end of this tile's scale, the
        // opposite of every other tile.
        snapshot.totals.input = 900_000;
        snapshot.totals.cache_read = 100_000;
        let tiles = build_tiles(&snapshot);

        assert_eq!(tiles[2].icon_key, "cache");
        assert_eq!(tiles[2].severity, Some(FillSeverity::Critical));
    }

    #[test]
    fn build_tiles_leaves_cost_and_turns_without_a_severity() {
        let tiles = build_tiles(&sample_snapshot());
        assert_eq!(tiles[1].severity, None, "cost has no bad end of its scale");
        assert_eq!(tiles[4].severity, None, "nor does a turn count");
    }

    #[test]
    fn build_context_reads_the_same_fill_the_context_tile_does() {
        let context = build_context(&sample_snapshot());
        let fill = sample_snapshot().context_fill();

        assert_eq!(context.used_tokens, fill.used());
        assert_eq!(context.window_tokens, fill.window());
        assert_eq!(context.severity, fill.severity());
    }

    #[test]
    fn build_context_carries_the_same_growth_per_turn_the_gauges_caption_prints() {
        let snapshot = sample_snapshot();
        let context = build_context(&snapshot);

        assert_eq!(
            context.growth_per_turn_tokens,
            snapshot.average_context_growth_per_turn() as u64,
            "panel.context-gauge's caption line has nothing else to print this from"
        );
    }

    #[test]
    fn build_context_places_the_compaction_threshold_as_a_percentage_of_the_window() {
        let context = build_context(&sample_snapshot());
        let window = sample_snapshot().context_window();
        let expected = (window - ModelCatalog::COMPACTION_BUFFER) as f64 / window as f64 * 100.0;

        assert!(
            (context.compaction_threshold_pct.expect("a known window") - expected).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn build_account_is_none_without_a_usage_reading() {
        assert!(build_account(None).is_none());
    }

    #[test]
    fn build_account_carries_the_measured_flag_through() {
        let usage = AccountUsage::empty(fixed_now());
        let account = build_account(Some((&usage, false))).expect("a reading was given");
        assert!(!account.measured);
        assert_eq!(account.usage, usage);
    }

    #[test]
    fn build_spend_mirrors_build_account_over_the_same_reading() {
        let usage = AccountUsage::empty(fixed_now());
        let spend = build_spend(Some((&usage, true))).expect("a reading was given");
        assert!(spend.measured);
        assert_eq!(spend.usage, usage);
    }

    #[test]
    fn build_daily_spend_is_none_when_nothing_has_been_scanned() {
        let usage = AccountUsage::empty(fixed_now());
        assert!(build_daily_spend(Some((&usage, true))).is_none());
        assert!(build_daily_spend(None).is_none());
    }

    #[test]
    fn build_daily_spend_carries_the_accounts_own_daily_series_through() {
        let now = fixed_now();
        let entries = [
            measured_entry("a", "2026-08-31T09:30:00Z", "/home/ada/api", 100_000),
            measured_entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 50_000),
        ];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let daily_spend = build_daily_spend(Some((&usage, true))).expect("two days scanned");

        assert_eq!(
            daily_spend.days, usage.daily_spend,
            "oldest first, unchanged"
        );
        assert_eq!(daily_spend.days.len(), 2);
    }

    #[test]
    fn build_model_breakdown_is_none_when_nothing_has_been_scanned() {
        let usage = AccountUsage::empty(fixed_now());
        assert!(build_model_breakdown(Some((&usage, true))).is_none());
    }

    #[test]
    fn build_model_breakdown_names_each_model_through_its_display_name() {
        let now = fixed_now();
        let entries = [measured_entry(
            "a",
            "2026-09-01T09:30:00Z",
            "/home/ada/api",
            100_000,
        )];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let model_breakdown =
            build_model_breakdown(Some((&usage, true))).expect("one model scanned");

        assert_eq!(model_breakdown.rows.len(), 1);
        assert_eq!(
            model_breakdown.rows[0].0,
            ModelCatalog::display_name_for("claude-opus-5"),
            "the same display name usage_view's own model column would print"
        );
        assert_eq!(model_breakdown.rows[0].1, usage.model_breakdown[0].cost);
    }

    #[test]
    fn build_burn_rate_is_none_without_an_active_block() {
        let usage = AccountUsage::empty(fixed_now());
        assert!(build_burn_rate(Some((&usage, true))).is_none());
        assert!(build_burn_rate(None).is_none());
    }

    #[test]
    fn build_burn_rate_reads_the_intensity_and_projection_off_the_active_block() {
        let now = fixed_now();
        let entries = [
            measured_entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 100_000),
            measured_entry("a", "2026-09-01T09:50:00Z", "/home/ada/api", 100_000),
        ];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );
        let expected_burn = usage.active_burn.expect("two responses give a rate");
        let expected_projection = usage.active_projection.expect("a running block");

        let burn_rate = build_burn_rate(Some((&usage, true))).expect("an active block");

        assert!(
            (burn_rate.intensity - expected_burn.indicator_tokens_per_minute).abs() < f64::EPSILON
        );
        assert_eq!(burn_rate.projection, expected_projection.cost);
        assert_eq!(
            burn_rate.limit_standing, None,
            "no token ceiling reaches the live dashboard yet"
        );
    }

    #[test]
    fn build_top_projects_is_none_when_nothing_has_been_scanned() {
        let usage = AccountUsage::empty(fixed_now());
        assert!(build_top_projects(Some((&usage, true))).is_none());
        assert!(build_top_projects(None).is_none());
    }

    #[test]
    fn build_top_projects_lists_the_busiest_projects_dearest_first() {
        let now = fixed_now();
        let entries = [
            measured_entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 100_000),
            measured_entry("b", "2026-09-01T09:50:00Z", "/home/ada/web", 50_000),
        ];
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let top_projects = build_top_projects(Some((&usage, true))).expect("two projects scanned");

        assert_eq!(top_projects.rows.len(), 2);
        assert_eq!(top_projects.rows[0].0, "api", "the busier project leads");
        assert!(
            top_projects.rows[0].1.dollars() >= top_projects.rows[1].1.dollars(),
            "dearest first"
        );
    }

    #[test]
    fn build_trend_carries_the_output_series_and_its_compaction_markers() {
        let trend = build_trend(&sample_snapshot());
        assert_eq!(trend.output_series, sample_snapshot().output_series());
        assert_eq!(
            trend.compaction_markers,
            sample_snapshot().compaction_marker_indices()
        );
        assert!(trend.banner.is_some(), "a fill is always known");
    }

    #[test]
    fn build_token_mix_splits_cache_writes_from_cache_reads() {
        let mut snapshot = sample_snapshot();
        snapshot.totals.cache_write_5m = 10;
        snapshot.totals.cache_write_1h = 20;
        let mix = build_token_mix(&snapshot);

        assert_eq!(mix.cache_write, 30);
        assert_eq!(mix.cache_read, snapshot.totals.cache_read);
    }

    #[test]
    fn build_activity_reflects_whether_the_session_is_mid_turn() {
        let mut snapshot = sample_snapshot();
        snapshot.phase = SessionPhase::Thinking;
        let activity = build_activity(&snapshot);
        assert!(activity.running);
    }

    #[test]
    fn build_turn_carries_the_sessions_last_error_rather_than_only_this_turns() {
        let mut snapshot = sample_snapshot();
        snapshot.last_error = Some("boom".to_owned());
        let turn = build_turn(&snapshot);
        assert_eq!(turn.last_error.as_deref(), Some("boom"));
    }
}

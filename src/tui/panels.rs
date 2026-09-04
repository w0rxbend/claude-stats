//! The panel catalogue: every drawable region of the dashboard, looked up by
//! [`PanelId`] rather than hard-coded into `src/tui/screens/dashboard.rs`'s
//! `draw`.
//!
//! [`PanelRegistry`] is a Registry (Fowler, *Patterns of Enterprise
//! Application Architecture*): a well-known catalogue other code asks "how
//! small can `panel.spend-panel` be, and how do I draw it" rather than each
//! call site carrying its own idea of the answer. `crate::tui::palette`'s
//! `ThemeRegistry` is the same pattern over palettes rather than panels, and
//! this module follows its shape deliberately -- an `OnceLock`-backed
//! `builtin()` built once from a flat literal table, a `get()` that looks a
//! key up and hands back nothing more than what was asked for.
//!
//! Every [`PanelRenderer`] here does the same three things, in the same
//! order, as the free functions `dashboard.rs` already has: read the slice of
//! [`DashboardViewModel`] it needs, build the existing widget struct from
//! `crate::tui::widgets` exactly as `dashboard.rs` already does, and call its
//! `render`. None of them decide *whether* they get drawn or *how much room*
//! they get -- that is `crate::tui::layout::solve`'s job, driven by the
//! [`PanelSpec`] each renderer is registered with here. Splitting "what a
//! panel draws" from "whether it fits" is what let this land without
//! `dashboard.rs`'s own `draw` changing at all: every panel below is reachable
//! through the registry today, and nothing yet asks the registry for one.

use std::collections::HashMap;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Wrap};

use crate::domain::context::{ContextFill, FillSeverity};
use crate::domain::tokens::TokenUsage;
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::layout::{Flex, PanelId};
use crate::tui::palette::Palette;
use crate::tui::widgets::banner::ContextBanner;
use crate::tui::widgets::burn_rate_gauge::BurnRateGauge;
use crate::tui::widgets::daily_spend_chart::DailySpendChart;
use crate::tui::widgets::dollar_pulse::{AnimationStyle, DollarPulse};
use crate::tui::widgets::gauge::ContextGauge;
use crate::tui::widgets::meter::meter_line;
use crate::tui::widgets::model_breakdown::ModelBreakdown;
use crate::tui::widgets::sparkline::OutputSparkline;
use crate::tui::widgets::spend_panel::SpendPanel;
use crate::tui::widgets::stat_tile::StatTile;
use crate::tui::widgets::token_mix::TokenMix;
use crate::tui::widgets::tool_feed::ToolFeed;
use crate::tui::widgets::top_projects::TopProjects;
use crate::tui::widgets::usage_windows::UsageWindows;
use crate::view::dashboard_view::{DashboardViewModel, TileView};

/// One panel's render function.
///
/// A plain function pointer rather than a `Box<dyn Fn(..)>` or a closure:
/// every panel below is a top-level `fn` with no state of its own to
/// capture, so the extra indirection and heap allocation a trait object
/// would cost buys nothing here. The signature is the one thing every panel
/// agrees on regardless of what it draws -- see the module doc.
pub type PanelRenderer = fn(&mut Frame<'_>, Rect, &DashboardViewModel, &Palette, u64);

/// A panel's own layout metadata: how small it can honestly be, and which
/// way it is worth growing past that.
///
/// This is read by whatever builds a [`crate::tui::layout::Node`] for the
/// current preset -- to choose a [`crate::tui::layout::SizeHint`] and to
/// answer `crate::tui::layout::solve`'s `min_sizes` question -- not by the
/// renderer itself, which draws into whatever [`ratatui::layout::Rect`] it is
/// handed and trusts that rect to already respect this minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSpec {
    pub min: (u16, u16),
    pub flex: Flex,
}

/// The catalogue of every registered panel.
pub struct PanelRegistry {
    panels: HashMap<PanelId, (PanelSpec, PanelRenderer)>,
}

impl PanelRegistry {
    /// The registry of every panel this crate ships.
    ///
    /// Built once and shared for the life of the process, the same way
    /// [`crate::tui::palette::registry::ThemeRegistry::builtin`] is: the
    /// table below is immutable data, so there is nothing to gain from
    /// rebuilding the same `HashMap` on every lookup.
    #[must_use]
    pub fn builtin() -> &'static Self {
        static REGISTRY: OnceLock<PanelRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| {
            let mut panels = HashMap::with_capacity(BUILTIN_PANELS.len());
            for &(id, min, flex, renderer) in BUILTIN_PANELS {
                panels.insert(PanelId(id), (PanelSpec { min, flex }, renderer));
            }
            Self { panels }
        })
    }

    /// The spec and renderer registered under `id`, if any.
    #[must_use]
    pub fn get(&self, id: &PanelId) -> Option<(&PanelSpec, PanelRenderer)> {
        self.panels
            .get(id)
            .map(|(spec, renderer)| (spec, *renderer))
    }
}

/// The panel catalogue, as a flat literal table.
///
/// A table rather than a sequence of `registry.insert(..)` calls, mirroring
/// `crate::tui::keymap::defaults`'s own binding tables: every row is a panel
/// id, its minimum size, its [`Flex`] behaviour and its renderer, and the
/// whole catalogue can be read at a glance rather than stepped through.
///
/// Fourteen of the first fifteen rows here draw one of the six headline tiles
/// or a panel `dashboard.rs` already renders by hand; `panel.tile-row` is the
/// fifteenth, a convenience alias over the same six tiles for a preset that
/// wants them fused into one strip rather than six separately-placed panels.
/// Four more are the aggregate panels wave 1's data made honest -- daily
/// spend, model breakdown, burn rate, top projects -- each reading the field
/// [`crate::view::dashboard_view::build`] already carries for it. The last,
/// `panel.dollar-pulse`, is the animated "$" marker -- see
/// [`render_dollar_pulse`]'s own doc for the one behaviour choice
/// (`NO_ANIMATION`) it makes on top of what `crate::tui::widgets::dollar_pulse`
/// already decides.
const BUILTIN_PANELS: &[(&str, (u16, u16), Flex, PanelRenderer)] = &[
    ("tile.context", (14, 4), Flex::Width, render_tile_context),
    ("tile.cost", (14, 4), Flex::Width, render_tile_cost),
    ("tile.cache", (14, 4), Flex::Width, render_tile_cache),
    (
        "tile.compaction",
        (14, 4),
        Flex::Width,
        render_tile_compaction,
    ),
    ("tile.turns", (14, 4), Flex::Width, render_tile_turns),
    ("tile.errors", (14, 4), Flex::Width, render_tile_errors),
    ("panel.tile-row", (36, 4), Flex::Width, render_tile_row),
    (
        "panel.context-gauge",
        (40, 4),
        Flex::Width,
        render_context_gauge,
    ),
    (
        // Quantised: this panel renders at its four-row minimum up to
        // `ContextBanner`'s half-height size, then steps straight to an
        // eight-row full-height rendering with nothing useful in between --
        // see `ContextBanner::SIZES`.
        "panel.context-banner",
        (20, 4),
        Flex::Quantised,
        render_context_banner,
    ),
    (
        // Grows from a session-window-only reading at its four-row minimum
        // up to eight rows once there is room for the weekly window and the
        // month line too -- see `UsageWindows::render`'s own height gates.
        "panel.account-usage",
        (40, 4),
        Flex::Height,
        render_account_usage,
    ),
    (
        "panel.spend-panel",
        (30, 11),
        Flex::Height,
        render_spend_panel,
    ),
    (
        // Grows from the sparkline and meters alone at six rows up to
        // eleven once there is room for the embedded context banner too.
        "panel.output-trend",
        (30, 6),
        Flex::Height,
        render_output_trend,
    ),
    ("panel.token-mix", (20, 8), Flex::Both, render_token_mix),
    ("panel.tool-feed", (30, 5), Flex::Both, render_tool_feed),
    ("panel.this-turn", (24, 4), Flex::Height, render_this_turn),
    (
        "panel.daily-spend-chart",
        (40, 8),
        Flex::Both,
        render_daily_spend_chart,
    ),
    (
        "panel.model-breakdown",
        (24, 6),
        Flex::Both,
        render_model_breakdown,
    ),
    (
        "panel.burn-rate-gauge",
        (30, 5),
        Flex::Width,
        render_burn_rate_gauge,
    ),
    (
        "panel.top-projects",
        (24, 6),
        Flex::Height,
        render_top_projects,
    ),
    (
        "panel.dollar-pulse",
        // Height 4, not 3: `DollarPulse::render` itself refuses to draw
        // anything below four rows (see its own tiny-area guard), so a
        // registered minimum of three would tell `crate::tui::layout::solve`
        // this panel can honestly do something with three rows when it
        // cannot -- the same floor `panel.context-banner` registers for the
        // same `tui-big-text` reason.
        (10, 4),
        Flex::Both,
        render_dollar_pulse,
    ),
];

// ── tiles ─────────────────────────────────────────────────────────────

fn render_tile_context(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[0], palette);
}

fn render_tile_cost(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[1], palette);
}

fn render_tile_cache(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[2], palette);
}

fn render_tile_compaction(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[3], palette);
}

fn render_tile_turns(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[4], palette);
}

fn render_tile_errors(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    render_tile(frame, area, &model.tiles[5], palette);
}

/// `panel.tile-row`: all six tiles, split into an even `Ratio(1, 6)` row
/// internally -- the same split `dashboard.rs::draw_tiles` uses -- for a
/// preset that wants the fused strip rather than six separately-placed
/// panels.
///
/// Its registered minimum, `(36, 4)`, is six times [`StatTile`]'s own
/// absolute floor of six columns each -- not six times the fourteen-wide
/// minimum a *standalone* `tile.*` panel is registered at. A lone tile
/// panel's fourteen columns are a legibility choice with nothing else on its
/// row to share the blame with; six tiles sharing one row already divide
/// whatever width they are given and individually blank out below
/// `StatTile`'s own floor, so the row as a whole only needs to promise that
/// floor is reachable, not that every tile gets the more generous width a
/// panel drawn on its own would want. Registering this any higher would make
/// `presets::live`, which places this panel first in its degradation order,
/// hide the *entire* rest of the dashboard on any terminal narrower than
/// that -- a hair-trigger nobody asked for, and not how `dashboard.rs`'s own
/// pre-epic tile row ever behaved.
fn render_tile_row(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let cells = Layout::horizontal([Constraint::Ratio(1, 6); 6]).split(area);
    for (tile, cell) in model.tiles.iter().zip(cells.iter()) {
        render_tile(frame, *cell, tile, palette);
    }
}

/// Builds and draws one [`StatTile`] from a [`TileView`].
///
/// `severity` decides both the tile's accent colour (via
/// [`Palette::severity`], falling back to `palette.accent_primary` for a
/// tile with no opinion) and whether its border is drawn in that colour: any
/// tile at [`FillSeverity::Hot`] or worse is emphasised, generalising the
/// rule `dashboard.rs::draw_tiles` used to apply only to the context tile
/// (severity at or past `Hot`) and the compaction tile (only when
/// imminent, which [`crate::view::dashboard_view::build`] already maps to
/// [`FillSeverity::Critical`]) into the one rule every tile now shares.
fn render_tile(frame: &mut Frame<'_>, area: Rect, tile: &TileView, palette: &Palette) {
    let accent = tile.severity.map_or_else(
        || palette.accent_primary.into(),
        |severity| palette.severity(severity),
    );
    let emphasised = tile
        .severity
        .is_some_and(|severity| severity >= FillSeverity::Hot);

    let mut stat_tile = StatTile::new(icon_for(tile.icon_key), tile.label, tile.value.clone())
        .accent(accent)
        .emphasised(emphasised);
    if let Some(footnote) = &tile.footnote {
        stat_tile = stat_tile.footnote(footnote.clone());
    }
    stat_tile.render(area, frame.buffer_mut(), palette);
}

/// The glyph a [`TileView::icon_key`] stands for.
///
/// [`crate::view::dashboard_view::build`] is the only producer of a
/// [`TileView`], and it only ever writes one of the six keys matched here,
/// so the wildcard arm is unreachable in practice; it exists so this
/// function cannot panic if that ever stops being true, which is worth more
/// on a screen that redraws sixty times a second than a key this module
/// itself already controls both ends of.
fn icon_for(icon_key: &str) -> &'static str {
    match icon_key {
        "context" => Icon::CONTEXT,
        "cost" => Icon::COST,
        "cache" => Icon::CACHE,
        "compaction" => Icon::COMPACT,
        "turns" => Icon::TURN,
        "errors" => Icon::ERROR,
        _ => Icon::BULLET,
    }
}

// ── the context panels ───────────────────────────────────────────────

/// `panel.context-gauge`: the context-fill bar, its title bordered and
/// coloured by severity, with a caption line of the figures beside it --
/// the same shape `dashboard.rs::draw_context_panel` draws today.
fn render_context_gauge(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let colour = palette.severity(model.context.severity);
    let block = titled_block("context window", colour, palette);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let [bar, caption] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let fill = ContextFill::new(model.context.used_tokens, model.context.window_tokens);
    ContextGauge::new(fill).render(bar, frame.buffer_mut(), palette);

    if caption.height == 0 {
        return;
    }
    let line = Line::from(vec![
        labelled_field(Icon::TOKEN, "used", &format::tokens(fill.used()), colour),
        labelled_field(
            Icon::CONTEXT,
            "free",
            &format::tokens(fill.remaining()),
            palette.muted.into(),
        ),
        labelled_field(
            Icon::COMPACT,
            "until compaction",
            &format::tokens(fill.tokens_until_compaction()),
            palette.accent_secondary.into(),
        ),
        labelled_field(
            Icon::RATE,
            "growth/turn",
            &format::tokens(model.context.growth_per_turn_tokens),
            palette.accent_info.into(),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), caption);
}

/// `panel.context-banner`: the oversized context percentage on its own,
/// with no surrounding chrome -- exactly [`ContextBanner`] alone, the same
/// widget the embedded banner inside `panel.output-trend` below shares.
fn render_context_banner(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let fill = ContextFill::new(model.context.used_tokens, model.context.window_tokens);
    ContextBanner::new(fill).render(area, frame.buffer_mut(), palette);
}

// ── the account and spend panels ─────────────────────────────────────

/// `panel.account-usage`: absent entirely while nothing is tracking account
/// usage, exactly as `dashboard.rs` only ever draws this row when it has a
/// reading to show.
fn render_account_usage(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(account) = &model.account else {
        return;
    };
    UsageWindows::new(&account.usage, account.measured).render(area, frame.buffer_mut(), palette);
}

/// `panel.spend-panel`: likewise absent without a usage reading.
fn render_spend_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(spend) = &model.spend else {
        return;
    };
    SpendPanel::new(&spend.usage, spend.measured).render(area, frame.buffer_mut(), palette);
}

// ── the session detail panels ────────────────────────────────────────

/// Rows the embedded context banner takes inside `panel.output-trend`, and
/// the panel height below which it is dropped in favour of the sparkline --
/// the same two figures `dashboard.rs`'s own `BANNER_ROWS` and
/// `MIN_HEIGHT_FOR_BANNER` name, kept here under their own names because
/// those constants are private to `dashboard.rs` and this panel does not
/// import it.
const TREND_BANNER_ROWS: u16 = 4;
const TREND_MIN_HEIGHT_FOR_BANNER: u16 = 11;

/// `panel.output-trend`: the output-per-response sparkline, the
/// cache/efficiency/context meters beneath it, and the embedded context
/// banner when the panel is tall enough to spare the rows for it -- the same
/// shape `dashboard.rs::draw_trend` draws today.
fn render_output_trend(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let trend = &model.trend;
    let block = titled_block(
        "output per response",
        palette.accent_primary.into(),
        palette,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    let banner_rows = if inner.height >= TREND_MIN_HEIGHT_FOR_BANNER {
        TREND_BANNER_ROWS
    } else {
        0
    };
    let [banner, spark, meters] = Layout::vertical([
        Constraint::Length(banner_rows),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(inner);

    if let Some(banner_data) = &trend.banner {
        let fill = ContextFill::new(banner_data.used_tokens, banner_data.window_tokens);
        ContextBanner::new(fill).render(banner, frame.buffer_mut(), palette);
    }

    OutputSparkline::new(&trend.output_series, &trend.compaction_markers).render(
        spark,
        frame.buffer_mut(),
        palette,
    );

    let bar_width = (meters.width as usize).saturating_sub(20).clamp(4, 24);
    let mut lines = Vec::new();
    if let Some(ratio) = trend.cache_ratio {
        lines.push(meter_line(
            "cache",
            ratio,
            format::percent_precise(ratio),
            palette.accent_primary.into(),
            bar_width,
            palette,
        ));
    }
    if let Some(ratio) = trend.efficiency_ratio {
        lines.push(meter_line(
            "efficiency",
            ratio,
            format::percent(ratio),
            palette.accent_success.into(),
            bar_width,
            palette,
        ));
    }
    lines.push(meter_line(
        "context",
        trend.context_ratio,
        format::percent_precise(trend.context_ratio),
        palette.severity(trend.context_severity),
        bar_width,
        palette,
    ));
    frame.render_widget(Paragraph::new(lines), meters);
}

/// `panel.token-mix`: the pie chart over the four token kinds, rebuilding a
/// [`TokenUsage`] from [`crate::view::dashboard_view::TokenMixView`] so
/// [`TokenMix`] can be built exactly as `dashboard.rs` already builds it.
///
/// The rebuilt `TokenUsage` splits `cache_write` back into the 5-minute
/// lease field and leaves the 1-hour one at zero, which is a safe fabrication
/// rather than a lossy one: [`TokenMix`] only ever reads their *sum*, via
/// [`TokenUsage::cache_creation`], for its "cache write" slice, and never
/// looks at either counter on its own.
fn render_token_mix(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let mix = model.token_mix;
    let usage = TokenUsage {
        input: mix.input,
        cache_read: mix.cache_read,
        cache_write_5m: mix.cache_write,
        cache_write_1h: 0,
        output: mix.output,
    };
    TokenMix::new(usage).render(area, frame.buffer_mut(), palette);
}

/// `panel.tool-feed`: the live tool-activity feed inside its own bordered
/// panel, exactly as `dashboard.rs::draw_activity` draws it -- the one
/// renderer in this table that actually reads the live animation `phase` it
/// is given, for the spinner on the newest running call.
fn render_tool_feed(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    phase: u64,
) {
    let block = titled_block("live tool activity", palette.accent_special.into(), palette);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    ToolFeed::new(&model.activity.events, model.activity.running, phase).render(
        inner,
        frame.buffer_mut(),
        palette,
    );
}

/// `panel.this-turn`: the counters reset every user message, exactly as
/// `dashboard.rs::draw_turn` draws them.
fn render_this_turn(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let turn = &model.turn;
    let block = titled_block("this turn", palette.accent_info.into(), palette);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let mut lines = vec![Line::from(vec![
        labelled_field(
            Icon::TOKEN,
            "tools",
            &turn.tool_calls.to_string(),
            palette.accent_info.into(),
        ),
        labelled_field(
            Icon::THINKING,
            "thinking",
            &turn.thinking_blocks.to_string(),
            palette.accent_secondary.into(),
        ),
        labelled_field(
            Icon::ERROR,
            "errors",
            &turn.tool_errors.to_string(),
            if turn.tool_errors == 0 {
                palette.muted.into()
            } else {
                palette.pressure_high.into()
            },
        ),
    ])];

    if turn.agents_running > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} sub-agent(s) running",
                Icon::BULLET,
                turn.agents_running
            ),
            Style::default().fg(palette.accent_special.into()),
        )));
    }
    if let Some(skill) = &turn.active_skill {
        lines.push(Line::from(Span::styled(
            format!("{} skill /{skill}", Icon::BULLET),
            Style::default().fg(palette.pressure_low.into()),
        )));
    }
    if let Some(error) = &turn.last_error {
        lines.push(Line::from(Span::styled(
            format!("{} {error}", Icon::ERROR),
            Style::default().fg(palette.pressure_high.into()),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

// ── the aggregate panels ──────────────────────────────────────────────

/// `panel.daily-spend-chart`: absent while nothing has been scanned, exactly
/// as the account and spend panels above are.
fn render_daily_spend_chart(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(daily_spend) = &model.daily_spend else {
        return;
    };
    DailySpendChart::new(&daily_spend.days).render(area, frame.buffer_mut(), palette);
}

/// `panel.model-breakdown`: likewise absent without a reading.
fn render_model_breakdown(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(model_breakdown) = &model.model_breakdown else {
        return;
    };
    ModelBreakdown::new(&model_breakdown.rows).render(area, frame.buffer_mut(), palette);
}

/// `panel.burn-rate-gauge`: absent without an active billing block to burn
/// through -- see [`crate::view::dashboard_view::BurnRateView`].
fn render_burn_rate_gauge(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(burn_rate) = &model.burn_rate else {
        return;
    };
    BurnRateGauge::new(
        burn_rate.intensity,
        burn_rate.projection,
        burn_rate.limit_standing,
    )
    .render(area, frame.buffer_mut(), palette);
}

/// `panel.top-projects`: absent without a reading, exactly as `panel.spend-panel`'s own project rows are.
fn render_top_projects(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let Some(top_projects) = &model.top_projects else {
        return;
    };
    TopProjects::new(&top_projects.rows).render(area, frame.buffer_mut(), palette);
}

/// `panel.dollar-pulse`: the animated "$" marker, built from
/// [`crate::view::dashboard_view::DollarPulseView`] exactly as
/// [`crate::tui::widgets::dollar_pulse::DollarPulse`]'s own doc describes.
///
/// `AnimationStyle::Pulse` (Treatment B, the fill/drain thermometer) is
/// hard-coded here rather than read from anywhere -- letting a user or a
/// config file choose `Coin` instead is a later epic's job, not this one's.
/// `DollarPulseView::off` is the one override this renderer does honour: it
/// carries forward whatever `NO_ANIMATION`/`CLAUDE_STATS_NO_ANIMATION`
/// resolved to at startup, ahead of that hard-coded default, mirroring the
/// `NO_COLOR` convention -- see that field's own doc for why it is a bare
/// `bool` rather than the full [`AnimationStyle`].
fn render_dollar_pulse(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &DashboardViewModel,
    palette: &Palette,
    _phase: u64,
) {
    let pulse = &model.dollar_pulse;
    let style = if pulse.off {
        AnimationStyle::Off
    } else {
        AnimationStyle::Pulse
    };
    DollarPulse::new(pulse.level, palette.ramp(pulse.level), palette.faint.into())
        .pulsing(pulse.frames_since_increment)
        .style(style)
        .render(area, frame.buffer_mut());
}

// ── shared building blocks ────────────────────────────────────────────

/// A titled, bordered panel in the house style.
///
/// `dashboard.rs` carried a private helper of exactly this shape before this
/// module existed; now that every panel it drew is drawn from here instead,
/// that copy is gone and this is the one definition, shared by every panel in
/// this module that needs a border rather than each restating the same eight
/// lines.
fn titled_block<'a>(title: &'a str, accent: Color, palette: &Palette) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), palette.title(accent)))
}

/// One `icon label value` group, for packing several readings onto a line --
/// `dashboard.rs`'s own equivalent private helper is gone for the same reason
/// [`titled_block`]'s is.
fn labelled_field<'a>(icon: &'a str, label: &'a str, value: &str, colour: Color) -> Span<'a> {
    Span::styled(
        format!("{icon} {label} {value}   "),
        Style::default().fg(colour),
    )
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::domain::session::SessionSnapshot;
    use crate::tui::palette::registry::ThemeRegistry;
    use crate::view::dashboard_view;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    fn model() -> DashboardViewModel {
        let snapshot = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abcdef".to_owned());
        dashboard_view::build(&snapshot, None, 0, None, false)
    }

    /// Every id the module doc and the epic spec both say should be
    /// registered, in the same order [`BUILTIN_PANELS`] lists them.
    ///
    /// The epic's own arithmetic reads "14 from epic 3 plus these 4 plus the
    /// dollar-pulse stub is 19" -- but epic 3's `BUILTIN_PANELS` already held
    /// fifteen rows, not fourteen, and its own notes flagged that as the
    /// spec's off-by-one rather than a bug to fix (`panel.tile-row` is a
    /// sixteenth-in-spirit convenience alias over six tiles that were already
    /// each counted on their own). Carrying that acknowledged discrepancy
    /// forward the same way epic 3 did -- registering every id actually
    /// listed rather than trimming one to make a header count agree -- this
    /// table, and the registry it is checked against, holds twenty.
    const EXPECTED: [(&str, (u16, u16), Flex); 20] = [
        ("tile.context", (14, 4), Flex::Width),
        ("tile.cost", (14, 4), Flex::Width),
        ("tile.cache", (14, 4), Flex::Width),
        ("tile.compaction", (14, 4), Flex::Width),
        ("tile.turns", (14, 4), Flex::Width),
        ("tile.errors", (14, 4), Flex::Width),
        ("panel.tile-row", (36, 4), Flex::Width),
        ("panel.context-gauge", (40, 4), Flex::Width),
        ("panel.context-banner", (20, 4), Flex::Quantised),
        ("panel.account-usage", (40, 4), Flex::Height),
        ("panel.spend-panel", (30, 11), Flex::Height),
        ("panel.output-trend", (30, 6), Flex::Height),
        ("panel.token-mix", (20, 8), Flex::Both),
        ("panel.tool-feed", (30, 5), Flex::Both),
        ("panel.this-turn", (24, 4), Flex::Height),
        ("panel.daily-spend-chart", (40, 8), Flex::Both),
        ("panel.model-breakdown", (24, 6), Flex::Both),
        ("panel.burn-rate-gauge", (30, 5), Flex::Width),
        ("panel.top-projects", (24, 6), Flex::Height),
        ("panel.dollar-pulse", (10, 4), Flex::Both),
    ];

    #[test]
    fn every_expected_panel_is_registered_with_its_exact_min_and_flex() {
        let registry = PanelRegistry::builtin();
        for (id, min, flex) in EXPECTED {
            let (spec, _renderer) = registry
                .get(&PanelId(id))
                .unwrap_or_else(|| panic!("{id} is not registered"));
            assert_eq!(spec.min, min, "{id}'s minimum size");
            assert_eq!(spec.flex, flex, "{id}'s flex behaviour");
        }
    }

    #[test]
    fn the_registry_holds_exactly_the_expected_panels_and_no_others() {
        let registry = PanelRegistry::builtin();
        assert_eq!(registry.panels.len(), EXPECTED.len());
    }

    #[test]
    fn an_unregistered_panel_id_is_none_rather_than_a_panic() {
        assert!(
            PanelRegistry::builtin()
                .get(&PanelId("panel.not-a-real-panel"))
                .is_none()
        );
    }

    fn render(renderer: PanelRenderer, model: &DashboardViewModel, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        let palette = palette();
        terminal
            .draw(|frame| renderer(frame, frame.area(), model, &palette, 0))
            .expect("a frame");
    }

    /// Draws `renderer` into a `width` x `height` screen and returns every
    /// row of symbols, top first, so a test can look for the text a panel is
    /// supposed to have printed.
    fn render_screen(
        renderer: PanelRenderer,
        model: &DashboardViewModel,
        width: u16,
        height: u16,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        let palette = palette();
        terminal
            .draw(|frame| renderer(frame, frame.area(), model, &palette, 0))
            .expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_registered_renderer_draws_into_its_own_minimum_without_panicking() {
        // The one guarantee every panel owes the layout engine: given at
        // least its own declared minimum, it does not panic. `solve` is what
        // decides whether a panel actually gets that much room on a real
        // screen; this only has to prove the renderer honours the contract
        // once it does.
        let model = model();
        for &(id, min, _flex, renderer) in BUILTIN_PANELS {
            render(renderer, &model, min.0, min.1);
            let _ = id;
        }
    }

    #[test]
    fn a_tile_renderer_prints_its_own_label() {
        let mut terminal = Terminal::new(TestBackend::new(14, 4)).expect("a test terminal");
        let palette = palette();
        let model = model();
        terminal
            .draw(|frame| render_tile_context(frame, frame.area(), &model, &palette, 0))
            .expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        let screen: String = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        assert!(screen.contains("CONTEXT"), "got {screen:?}");
    }

    #[test]
    fn icon_for_falls_back_rather_than_panicking_on_an_unknown_key() {
        assert_eq!(icon_for("not-a-real-key"), Icon::BULLET);
    }

    // ── the aggregate panels ─────────────────────────────────────────

    #[test]
    fn the_daily_spend_chart_is_absent_without_a_reading() {
        let screen = render_screen(render_daily_spend_chart, &model(), 40, 8);
        assert!(screen.trim().is_empty(), "got {screen:?}");
    }

    #[test]
    fn the_daily_spend_chart_draws_the_days_it_is_given() {
        use chrono::NaiveDate;

        use crate::domain::money::Usd;
        use crate::view::dashboard_view::DailySpendView;

        let mut with_reading = model();
        with_reading.daily_spend = Some(DailySpendView {
            days: vec![(
                NaiveDate::from_ymd_opt(2026, 8, 31).expect("a valid date"),
                Usd::new(3.0),
            )],
        });
        let screen = render_screen(render_daily_spend_chart, &with_reading, 40, 8);
        assert!(screen.contains("Mon"), "got {screen:?}");
    }

    #[test]
    fn the_model_breakdown_is_absent_without_a_reading() {
        let screen = render_screen(render_model_breakdown, &model(), 24, 6);
        assert!(!screen.contains('$'), "got {screen:?}");
    }

    #[test]
    fn the_model_breakdown_lists_the_dearest_model_first() {
        use crate::domain::money::Usd;
        use crate::view::dashboard_view::ModelBreakdownView;

        let mut with_reading = model();
        with_reading.model_breakdown = Some(ModelBreakdownView {
            rows: vec![("claude-opus-5".to_owned(), Usd::new(9.0))],
        });
        // Wider than the panel's own registered minimum: at that minimum the
        // name column is too narrow to hold a full model id without
        // truncating it, which `ModelBreakdown`'s own widget tests already
        // cover on their own terms.
        let screen = render_screen(render_model_breakdown, &with_reading, 40, 6);
        assert!(screen.contains("claude-opus-5"), "got {screen:?}");
        assert!(screen.contains("$9.00"), "got {screen:?}");
    }

    #[test]
    fn the_burn_rate_gauge_is_absent_without_an_active_block() {
        let screen = render_screen(render_burn_rate_gauge, &model(), 30, 5);
        assert!(screen.trim().is_empty(), "got {screen:?}");
    }

    #[test]
    fn the_burn_rate_gauge_prints_the_projected_cost() {
        use crate::domain::money::Usd;
        use crate::view::dashboard_view::BurnRateView;

        let mut with_reading = model();
        with_reading.burn_rate = Some(BurnRateView {
            intensity: 1_000.0,
            projection: Usd::new(4.56),
            limit_standing: None,
        });
        let screen = render_screen(render_burn_rate_gauge, &with_reading, 30, 5);
        assert!(screen.contains("$4.56"), "got {screen:?}");
    }

    #[test]
    fn the_top_projects_panel_is_absent_without_a_reading() {
        let screen = render_screen(render_top_projects, &model(), 24, 6);
        assert!(!screen.contains('$'), "got {screen:?}");
    }

    #[test]
    fn the_top_projects_panel_lists_the_busiest_project() {
        use crate::domain::money::Usd;
        use crate::view::dashboard_view::TopProjectsView;

        let mut with_reading = model();
        with_reading.top_projects = Some(TopProjectsView {
            rows: vec![("api".to_owned(), Usd::new(3.0))],
        });
        let screen = render_screen(render_top_projects, &with_reading, 24, 6);
        assert!(screen.contains("api"), "got {screen:?}");
        assert!(screen.contains("$3.00"), "got {screen:?}");
    }

    #[test]
    fn the_dollar_pulse_panel_draws_a_glyph_once_it_has_room() {
        let mut with_reading = model();
        with_reading.dollar_pulse.level = 1.0;
        let screen = render_screen(render_dollar_pulse, &with_reading, 10, 8);
        assert!(
            screen.contains('\u{2588}'),
            "expected a rendered \"$\" glyph: {screen:?}"
        );
    }

    #[test]
    fn the_dollar_pulse_panel_still_draws_a_glyph_at_its_own_registered_minimum() {
        // `panel.dollar-pulse`'s entry in `BUILTIN_PANELS` promises
        // `crate::tui::layout::solve` that (10, 4) is a size this panel can
        // honestly do something with -- see that entry's own doc comment.
        // `every_registered_renderer_draws_into_its_own_minimum_without_panicking`
        // only proves this size does not panic; this proves it is not
        // merely tolerated but actually paints the glyph, catching the case
        // where a registered minimum quietly falls short of the widget's
        // own tiny-area guard.
        let mut with_reading = model();
        with_reading.dollar_pulse.level = 1.0;
        let (width, height) = BUILTIN_PANELS
            .iter()
            .find(|&&(id, ..)| id == "panel.dollar-pulse")
            .expect("panel.dollar-pulse is registered")
            .1;
        let screen = render_screen(render_dollar_pulse, &with_reading, width, height);
        assert!(
            screen.contains('\u{2588}'),
            "expected a rendered \"$\" glyph at the registered minimum {width}x{height}: {screen:?}"
        );
    }

    #[test]
    fn the_dollar_pulse_panel_honours_the_no_animation_override() {
        // `DollarPulseView::off` is the one behaviour choice this renderer
        // makes on top of the widget it builds -- see its own doc. This
        // does not re-prove `AnimationStyle::Off`'s own single-colour
        // guarantee, which `crate::tui::widgets::dollar_pulse` already
        // covers on its own terms; it only proves the flag actually reaches
        // the widget rather than being silently dropped on the way there.
        let mut off = model();
        off.dollar_pulse.level = 0.5;
        off.dollar_pulse.off = true;
        let mut pulsing = model();
        pulsing.dollar_pulse.level = 0.5;
        pulsing.dollar_pulse.off = false;

        let palette = palette();
        let render_buf = |m: &DashboardViewModel| {
            let area = Rect::new(0, 0, 10, 8);
            let mut terminal = Terminal::new(TestBackend::new(10, 8)).expect("a test terminal");
            terminal
                .draw(|frame| render_dollar_pulse(frame, area, m, &palette, 0))
                .expect("a frame");
            terminal.backend().buffer().clone()
        };

        let off_buf = render_buf(&off);
        let pulsing_buf = render_buf(&pulsing);
        // A half-full `Pulse` render tints only its bottom rows; `Off`
        // tints every cell of the glyph the same accent colour. The two
        // buffers must therefore disagree on at least one cell's colour.
        let differs = (0..8).any(|y| (0..10).any(|x| off_buf[(x, y)].fg != pulsing_buf[(x, y)].fg));
        assert!(differs, "the off flag had no visible effect");
    }
}

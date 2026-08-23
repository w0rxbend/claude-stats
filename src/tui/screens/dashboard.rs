//! The main dashboard: everything about the attached session on one screen.
//!
//! The layout is ordered by how urgent each thing is, top to bottom, because
//! that is the order a reader's eye takes:
//!
//! 1. The header -- which session am I even looking at.
//! 2. The tile row -- the six numbers that answer "is this session healthy".
//! 3. The context gauge -- the one reading that changes what you do next.
//! 4. Two columns of detail, for when the answer above was "no".
//!
//! Panels give up space in reverse order as the terminal shrinks, so a small
//! window keeps the urgent things and loses the detail, never the other way
//! round.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Wrap};

use crate::domain::context::{CompactionDistance, FillSeverity};
use crate::domain::session::{SessionPhase, SessionSnapshot};
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::theme::Theme;
use crate::tui::widgets::banner::ContextBanner;
use crate::tui::widgets::gauge::ContextGauge;
use crate::tui::widgets::meter::meter_line;
use crate::tui::widgets::sparkline::OutputSparkline;
use crate::tui::widgets::spinner::{Spinner, SpinnerStyle};
use crate::tui::widgets::stat_tile::StatTile;
use crate::tui::widgets::token_mix::TokenMix;
use crate::tui::widgets::tool_feed::ToolFeed;

/// The height below which the detail columns are dropped entirely.
///
/// Under this, a two-column split would give each panel one or two usable
/// rows, which is not enough to say anything true.
const MIN_HEIGHT_FOR_DETAIL: u16 = 18;

/// The width below which the layout collapses to a single column.
const MIN_WIDTH_FOR_COLUMNS: u16 = 90;

/// Rows given to the oversized context percentage.
///
/// Four, which is `PixelSize::HalfHeight`, not the eight a full-size banner
/// wants. At eight it dominates the panel it shares with the chart, and the
/// chart is the element carrying information the tiles do not already show.
const BANNER_ROWS: u16 = 4;

/// The panel height below which the banner is dropped in favour of the chart.
const MIN_HEIGHT_FOR_BANNER: u16 = 11;

/// Draws the dashboard for `snapshot` into `area`.
pub fn draw(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot, phase: u64) {
    let [header, tiles, gauge, rest] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Min(0),
    ])
    .areas(area);

    draw_header(frame, header, snapshot, phase);
    draw_tiles(frame, tiles, snapshot);
    draw_context_panel(frame, gauge, snapshot);

    if rest.height >= MIN_HEIGHT_FOR_DETAIL {
        draw_detail(frame, rest, snapshot, phase);
    }
}

// ── header ────────────────────────────────────────────────────────────

fn draw_header(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot, phase: u64) {
    let live = snapshot.phase == SessionPhase::Thinking;
    let (marker, marker_colour, state) = if live {
        (
            Spinner::new(SpinnerStyle::Braille, phase).glyph(),
            Theme::MINT,
            "working",
        )
    } else {
        (Icon::IDLE, Theme::MUTED, "idle")
    };

    let mut spans = vec![
        Span::styled(
            " claude-stats ",
            Style::default()
                .fg(Theme::BACKGROUND)
                .bg(Theme::CYAN)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(format!("{marker} "), Style::default().fg(marker_colour)),
        Span::styled(state, Style::default().fg(marker_colour)),
    ];

    let project = snapshot
        .project_dir
        .as_deref()
        .map(|p| format::fit(p, 34, true));
    for (icon, text, colour) in [
        (
            Icon::TOKEN,
            Some(snapshot.model_display_name()),
            Theme::VIOLET,
        ),
        (Icon::FILE, project, Theme::TEXT),
        (Icon::BRANCH, snapshot.git_branch.clone(), Theme::MINT),
        (
            Icon::CLOCK,
            snapshot.duration().map(format::duration),
            Theme::MUTED,
        ),
    ] {
        let Some(text) = text else { continue };
        spans.push(Span::styled(
            format!("  {} ", Icon::SEPARATOR),
            Style::default().fg(Theme::FAINT),
        ));
        spans.push(Span::styled(
            format!("{icon} "),
            Style::default().fg(Theme::FAINT),
        ));
        spans.push(Span::styled(text, Style::default().fg(colour)));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Theme::SURFACE)),
        area,
    );
}

// ── the six headline tiles ────────────────────────────────────────────

fn draw_tiles(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot) {
    let fill = snapshot.context_fill();
    let severity = fill.severity();
    let severity_colour = Theme::severity(severity);

    let cache = snapshot.cache_hit_ratio();
    // The cache tile is the one place a *low* number is the bad one, so its
    // colour scale runs the opposite way to everything else. Below half, the
    // conversation prefix is being re-sent rather than reused, and that is
    // where most of an unexpected bill comes from.
    let cache_colour = match cache {
        Some(r) if r >= 0.85 => Theme::MINT,
        Some(r) if r >= 0.50 => Theme::AMBER,
        Some(_) => Theme::CRIMSON,
        None => Theme::MUTED,
    };

    let (compaction_text, compaction_colour) = match snapshot.compaction_distance() {
        CompactionDistance::Imminent => ("imminent".to_owned(), Theme::CRIMSON),
        CompactionDistance::Turns(n) if n >= 100 => ("100+ turns".to_owned(), Theme::MINT),
        CompactionDistance::Turns(n) => (
            format!("~{n} turn{}", if n == 1 { "" } else { "s" }),
            if n <= 3 { Theme::ORANGE } else { Theme::VIOLET },
        ),
        CompactionDistance::Unknown => ("\u{2014}".to_owned(), Theme::MUTED),
    };

    let error_colour = if snapshot.tool_errors == 0 {
        Theme::MINT
    } else {
        Theme::CRIMSON
    };

    let tiles = [
        StatTile::new(
            Icon::CONTEXT,
            "CONTEXT",
            format::percent_precise(fill.ratio()),
        )
        .accent(severity_colour)
        .emphasised(severity >= FillSeverity::Hot)
        .footnote(format!(
            "{} / {}",
            format::tokens(fill.used()),
            format::tokens(fill.window())
        )),
        StatTile::new(Icon::COST, "COST", snapshot.cost().to_string())
            .accent(Theme::CYAN)
            .footnote(format!("{}/turn", snapshot.cost_per_turn())),
        StatTile::new(
            Icon::CACHE,
            "CACHE",
            cache.map_or_else(|| "\u{2014}".to_owned(), format::percent_precise),
        )
        .accent(cache_colour)
        .footnote(format!(
            "{} read",
            format::tokens(snapshot.totals.cache_read)
        )),
        StatTile::new(Icon::COMPACT, "COMPACTION", compaction_text)
            .accent(compaction_colour)
            .emphasised(matches!(
                snapshot.compaction_distance(),
                CompactionDistance::Imminent
            ))
            .footnote(format!("{} so far", snapshot.compactions.len())),
        StatTile::new(Icon::TURN, "TURNS", snapshot.turns.to_string())
            .accent(Theme::AZURE)
            .footnote(format!("{} tools", snapshot.tool_calls())),
        StatTile::new(Icon::ERROR, "ERRORS", snapshot.tool_errors.to_string())
            .accent(error_colour)
            .footnote(format!("{} files", snapshot.files_touched())),
    ];

    let areas = Layout::horizontal([Constraint::Ratio(1, 6); 6]).split(area);
    for (tile, cell) in tiles.into_iter().zip(areas.iter()) {
        frame.render_widget(tile, *cell);
    }
}

// ── the context panel ─────────────────────────────────────────────────

fn draw_context_panel(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot) {
    let fill = snapshot.context_fill();
    let colour = Theme::severity(fill.severity());

    let block = panel("context window", colour);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let [bar, caption] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    frame.render_widget(ContextGauge::new(fill), bar);

    let until = fill.tokens_until_compaction();
    let caption_line = Line::from(vec![
        field(Icon::TOKEN, "used", &format::tokens(fill.used()), colour),
        field(
            Icon::CONTEXT,
            "free",
            &format::tokens(fill.remaining()),
            Theme::MUTED,
        ),
        field(
            Icon::COMPACT,
            "until compaction",
            &format::tokens(until),
            Theme::VIOLET,
        ),
        field(
            Icon::RATE,
            "growth/turn",
            &format::tokens(snapshot.average_context_growth_per_turn() as u64),
            Theme::AZURE,
        ),
    ]);
    frame.render_widget(Paragraph::new(caption_line), caption);
}

// ── the detail columns ────────────────────────────────────────────────

fn draw_detail(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot, phase: u64) {
    if area.width < MIN_WIDTH_FOR_COLUMNS {
        // Too narrow to split. The activity feed is the panel worth keeping,
        // because it is the only one that answers "what is happening right
        // now" -- everything else is a summary that can wait for a wider
        // terminal.
        draw_activity(frame, area, snapshot, phase);
        return;
    }

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).areas(area);

    let [trend, mix] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(left);
    draw_trend(frame, trend, snapshot);
    frame.render_widget(TokenMix::new(snapshot.totals), mix);

    let [activity, turn] =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(right);
    draw_activity(frame, activity, snapshot, phase);
    draw_turn(frame, turn, snapshot);
}

fn draw_trend(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot) {
    let block = panel("output per response", Theme::CYAN);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 {
        return;
    }

    // The banner is the first thing to go when the panel is short: it is the
    // only element here that repeats a number shown elsewhere, so it costs
    // nothing to drop and the chart gets its rows back.
    let banner_rows = if inner.height >= MIN_HEIGHT_FOR_BANNER {
        BANNER_ROWS
    } else {
        0
    };
    let [banner, spark, meters] = Layout::vertical([
        Constraint::Length(banner_rows),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(inner);

    frame.render_widget(ContextBanner::new(snapshot.context_fill()), banner);

    let series = snapshot.output_series();
    let markers = snapshot.compaction_marker_indices();
    frame.render_widget(OutputSparkline::new(&series, &markers), spark);

    let bar_width = (meters.width as usize).saturating_sub(20).clamp(4, 24);
    let mut lines = Vec::new();
    if let Some(ratio) = snapshot.cache_hit_ratio() {
        lines.push(meter_line(
            "cache",
            ratio,
            format::percent_precise(ratio),
            Theme::CYAN,
            bar_width,
        ));
    }
    if let Some(ratio) = snapshot.efficiency() {
        lines.push(meter_line(
            "efficiency",
            ratio,
            format::percent(ratio),
            Theme::MINT,
            bar_width,
        ));
    }
    let fill = snapshot.context_fill();
    lines.push(meter_line(
        "context",
        fill.ratio(),
        format::percent_precise(fill.ratio()),
        Theme::severity(fill.severity()),
        bar_width,
    ));
    frame.render_widget(Paragraph::new(lines), meters);
}

fn draw_activity(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot, phase: u64) {
    let running = snapshot.phase == SessionPhase::Thinking;
    let block = panel("live tool activity", Theme::MAGENTA);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(ToolFeed::new(&snapshot.recent_tools, running, phase), inner);
}

fn draw_turn(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot) {
    let block = panel("this turn", Theme::AZURE);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let mut lines = vec![Line::from(vec![
        field(
            Icon::TOKEN,
            "tools",
            &snapshot.turn.tool_calls().to_string(),
            Theme::AZURE,
        ),
        field(
            Icon::THINKING,
            "thinking",
            &snapshot.turn.thinking_blocks.to_string(),
            Theme::VIOLET,
        ),
        field(
            Icon::ERROR,
            "errors",
            &snapshot.turn.tool_errors.to_string(),
            if snapshot.turn.tool_errors == 0 {
                Theme::MUTED
            } else {
                Theme::CRIMSON
            },
        ),
    ])];

    if snapshot.turn.agents_running > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} {} sub-agent(s) running",
                Icon::BULLET,
                snapshot.turn.agents_running
            ),
            Style::default().fg(Theme::MAGENTA),
        )));
    }
    if let Some(skill) = &snapshot.turn.active_skill {
        lines.push(Line::from(Span::styled(
            format!("{} skill /{skill}", Icon::BULLET),
            Style::default().fg(Theme::AMBER),
        )));
    }
    if let Some(error) = &snapshot.last_error {
        lines.push(Line::from(Span::styled(
            format!("{} {error}", Icon::ERROR),
            Style::default().fg(Theme::CRIMSON),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

// ── shared building blocks ────────────────────────────────────────────

/// A titled panel in the house style.
fn panel(title: &str, accent: ratatui::style::Color) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Theme::BORDER))
        .style(Style::default().bg(Theme::SURFACE))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), Theme::title(accent)))
}

/// One `icon label value` group, for packing several readings onto a line.
fn field<'a>(
    icon: &'a str,
    label: &'a str,
    value: &str,
    colour: ratatui::style::Color,
) -> Span<'a> {
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
    use crate::domain::session::ResponseSample;

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
            at: chrono::Utc::now(),
        });
        s
    }

    fn render_at(width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        let snapshot = sample_snapshot();
        terminal
            .draw(|frame| draw(frame, frame.area(), &snapshot, 0))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_full_dashboard_shows_every_headline_metric() {
        let screen = render_at(140, 40);
        for expected in ["CONTEXT", "COST", "CACHE", "COMPACTION", "TURNS", "ERRORS"] {
            assert!(screen.contains(expected), "missing {expected}");
        }
        assert!(screen.contains("Opus 5"), "model name missing");
        assert!(screen.contains("main"), "branch missing");
    }

    #[test]
    fn a_narrow_terminal_keeps_the_activity_feed_and_drops_the_columns() {
        let screen = render_at(70, 30);
        assert!(screen.contains("live tool activity"));
        assert!(!screen.contains("token mix"), "columns should be dropped");
    }

    #[test]
    fn a_short_terminal_keeps_the_tiles_and_drops_the_detail() {
        let screen = render_at(140, 12);
        assert!(screen.contains("CONTEXT"));
        assert!(!screen.contains("live tool activity"));
    }

    #[test]
    fn drawing_into_a_tiny_terminal_does_not_panic() {
        // Terminals get resized to absurd sizes while being dragged, and a
        // panic there takes the whole dashboard down.
        for (width, height) in [(1, 1), (4, 3), (20, 5), (200, 2)] {
            let _ = render_at(width, height);
        }
    }
}

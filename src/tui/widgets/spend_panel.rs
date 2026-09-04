//! The spend panel: what today has cost, how the current billing block is
//! going, and which projects have been the busiest lately.
//!
//! Every figure this widget prints already exists in [`AccountUsage`] --
//! [`AccountUsage::today`], [`AccountUsage::active_block`] and
//! [`AccountUsage::top_projects`] are all computed in the domain, from the
//! same fold [`crate::domain::blocks::identify`] that the printed `blocks`
//! report uses. This widget follows the crate's standing rule for the whole
//! `tui` layer: it renders those three [`PeriodUsage`]/[`ProjectUsage`] Value
//! Objects (Fowler, *`PoEAA`*) and computes nothing of its own -- there is no
//! arithmetic here that a test in [`crate::domain::limits`] could not already
//! check without a terminal.
//!
//! # Why the burn intensity is the one figure allowed a warm colour
//!
//! Every [`Palette`] reserves its `pressure_*` ramp for pressure and keeps
//! every routine figure -- cost included -- on the cool `accent_*` colours.
//! This panel prints more dollar figures than any other on the dashboard, so
//! it is the panel most tempted to break that rule. It does not: every cost
//! here stays on `accent_primary`, and the only thing that ever reaches into
//! the warm ramp is the burn-rate marker beside the projection, and only when
//! [`Intensity::High`] says the block really is running hot.

use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};

use crate::domain::blocks::{BillingBlock, BurnRate, Intensity, Projection};
use crate::domain::limits::{AccountUsage, PeriodUsage, ProjectUsage};
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::palette::Palette;

/// How many project rows the panel has room for.
///
/// [`AccountUsage::top_projects`] is already capped to this many by the
/// domain, so this is a second statement of the same number rather than a
/// second decision -- it exists only so the widget can size itself without
/// reading the length of a vector it has not been given yet.
const MAX_PROJECT_ROWS: usize = 5;

/// The spend panel.
pub struct SpendPanel<'a> {
    usage: &'a AccountUsage,
    /// Whether a reading has been taken yet, so a dashboard that has only
    /// just opened says "measuring" rather than a confident row of zeroes.
    ///
    /// [`UsageWindows`](crate::tui::widgets::usage_windows::UsageWindows) --
    /// this panel's neighbour in the same row -- makes the same promise, and
    /// the two share one [`AccountUsage`]: without this flag the account
    /// panel would say "measuring recent usage..." while this one sat beside
    /// it printing `today $0.00`, which is not a reading, it is the absence
    /// of one wearing a reading's clothes.
    measured: bool,
}

impl<'a> SpendPanel<'a> {
    /// A panel over the given reading, `measured` exactly as
    /// [`UsageWindows::new`](crate::tui::widgets::usage_windows::UsageWindows::new)
    /// takes it.
    #[must_use]
    pub const fn new(usage: &'a AccountUsage, measured: bool) -> Self {
        Self { usage, measured }
    }
}

impl SpendPanel<'_> {
    /// Draws the panel: today's spend, the active block and its projection,
    /// then the busiest projects.
    pub fn render(self, area: Rect, buf: &mut Buffer, palette: &Palette) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(palette.border.into()))
            .style(Style::default().bg(palette.surface.into()))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " spend ",
                palette.title(palette.accent_primary.into()),
            ));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 {
            return;
        }

        if !self.measured {
            Paragraph::new(Line::from(Span::styled(
                "measuring recent usage...",
                Style::default().fg(palette.muted.into()),
            )))
            .render(inner, buf);
            return;
        }

        let mut lines = vec![today_line(&self.usage.today, palette)];
        lines.extend(block_lines(
            self.usage.active_block.as_ref(),
            self.usage.active_burn,
            self.usage.active_projection.as_ref(),
            self.usage.measured_at,
            palette,
        ));
        lines.push(Line::raw(""));
        lines.extend(project_lines(
            &self.usage.top_projects,
            inner.width,
            palette,
        ));

        Paragraph::new(lines).render(inner, buf);
    }
}

/// `today   $12.34   1.2M tokens   3 sessions`.
fn today_line(today: &PeriodUsage, palette: &Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{} {:<8}", Icon::COST, today.label),
            palette.label(),
        ),
        Span::styled(
            format!("{}", today.cost),
            Style::default()
                .fg(palette.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   {} tokens", format::tokens(today.tokens.total())),
            Style::default().fg(palette.muted.into()),
        ),
        Span::styled(
            format!(
                "   {} session{}",
                today.sessions,
                if today.sessions == 1 { "" } else { "s" }
            ),
            Style::default().fg(palette.muted.into()),
        ),
    ])
}

/// The block section: either `block $4.56 started 14:00 2h45m left` and a
/// projection beneath it, or one line saying there is no block running.
fn block_lines(
    block: Option<&BillingBlock>,
    burn: Option<BurnRate>,
    projection: Option<&Projection>,
    now: DateTime<Utc>,
    palette: &Palette,
) -> Vec<Line<'static>> {
    let Some(block) = block else {
        // Stated outright rather than left as a blank row: a blank row in a
        // panel of figures reads as a figure that failed to render, and a
        // dashboard that cannot tell "no block" from "a rendering bug" is
        // less useful than one that never has to be told the difference.
        return vec![Line::from(Span::styled(
            format!("{} no active block", Icon::CLOCK),
            Style::default().fg(palette.muted.into()),
        ))];
    };

    let headline = Line::from(vec![
        Span::styled(format!("{} {:<8}", Icon::CLOCK, "block"), palette.label()),
        Span::styled(
            format!("{}", block.cost),
            Style::default()
                .fg(palette.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   started {}", block.started_at.format("%H:%M")),
            Style::default().fg(palette.muted.into()),
        ),
        Span::styled(
            format!("   {} left", format::duration(block.ends_at - now)),
            Style::default().fg(palette.muted.into()),
        ),
    ]);

    let mut lines = vec![headline];
    if let Some(projection) = projection {
        lines.push(projected_line(
            projection,
            burn.map_or(Intensity::Normal, BurnRate::intensity),
            palette,
        ));
    }
    lines
}

/// `projected $9.80`, with the burn intensity as the one marker on this
/// panel allowed to reach into the warm ramp -- see the module doc.
fn projected_line(
    projection: &Projection,
    intensity: Intensity,
    palette: &Palette,
) -> Line<'static> {
    let mut spans = vec![
        Span::styled("  projected ", palette.label()),
        Span::styled(
            format!("{}", projection.cost),
            Style::default()
                .fg(palette.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if intensity == Intensity::High {
        spans.push(Span::styled(
            "  fast",
            Style::default()
                .fg(burn_colour(intensity, palette))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// The colour a burn intensity is announced in.
///
/// Only [`Intensity::High`] is ever handed to [`Palette::ramp`], and only at
/// its warmest stop: everything routine stays on the cool ramp, which is what
/// makes the one warm marker on this panel mean something when it appears.
fn burn_colour(intensity: Intensity, palette: &Palette) -> Color {
    match intensity {
        Intensity::Normal => palette.accent_success.into(),
        Intensity::Moderate => palette.accent_primary.into(),
        Intensity::High => palette.ramp(1.0),
    }
}

/// Up to [`MAX_PROJECT_ROWS`] rows of `<final path segment>  $x.xx`, costs
/// right-aligned and names truncated rather than wrapped.
fn project_lines(projects: &[ProjectUsage], width: u16, palette: &Palette) -> Vec<Line<'static>> {
    // The domain has already capped this to `MAX_PROJECT_ROWS`; the `take`
    // here is a second statement of the same limit rather than a second
    // decision, kept so this function is correct even if it is ever handed
    // an untruncated list by a future caller.
    let cost_column = COST_COLUMN_WIDTH;
    let name_column = (width as usize).saturating_sub(cost_column + 1).max(1);

    projects
        .iter()
        .take(MAX_PROJECT_ROWS)
        .map(|project| {
            let name = format::fit(project.project.display_name(), name_column, false);
            Line::from(vec![
                Span::styled(
                    format!("{name:<name_column$}"),
                    Style::default().fg(palette.text.into()),
                ),
                Span::styled(
                    format!("{:>cost_column$}", format!("{}", project.cost)),
                    Style::default().fg(palette.accent_primary.into()),
                ),
            ])
        })
        .collect()
}

/// How many characters the cost column gets in the projects list.
///
/// Wide enough for `$1234.56` -- four figures of spend inside a seven-day
/// window is a realistic top project, and a column that had to widen itself
/// past that would shove every name in the list sideways to make room.
const COST_COLUMN_WIDTH: usize = 8;

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::model::ModelId;
    use crate::domain::period::Zone;
    use crate::domain::pricing::PriceSheet;
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn entry(session: &str, when: &str, project: &str, input: u64) -> Entry {
        Entry {
            id: EntryId {
                message_id: format!("msg-{session}-{when}-{input}"),
                request_id: None,
                session: SessionId::new(session),
            },
            at: at(when),
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

    fn render(usage: &AccountUsage, width: u16, height: u16) -> String {
        render_measured(usage, true, width, height)
    }

    fn render_measured(usage: &AccountUsage, measured: bool, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        let palette = palette();
        terminal
            .draw(|frame| {
                SpendPanel::new(usage, measured).render(frame.area(), frame.buffer_mut(), &palette);
            })
            .expect("a frame");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn todays_spend_is_shown_with_its_tokens_and_sessions() {
        let now = at("2026-09-01T12:00:00Z");
        let usage = AccountUsage::measure(
            now,
            &[entry("a", "2026-09-01T09:00:00Z", "/home/ada/api", 400_000)],
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let out = render(&usage, 60, 12);
        assert!(out.contains("today"));
        assert!(out.contains("$2.00"), "400k Opus 5 input at $5 per million");
        assert!(out.contains("400.0k tokens"));
        assert!(out.contains("1 session"));
    }

    #[test]
    fn an_inactive_block_says_so_rather_than_leaving_the_row_blank() {
        let usage = AccountUsage::empty(at("2026-09-01T12:00:00Z"));
        let out = render(&usage, 40, 12);

        assert!(out.contains("no active block"));
    }

    #[test]
    fn before_the_first_scan_it_says_so_rather_than_showing_zeroes() {
        // A dashboard that has only just opened has an `AccountUsage::empty`
        // reading and nothing more, exactly like `UsageWindows` beside it in
        // the same row -- see that widget's own test of the same name. Zero
        // figures here would read as "you spent nothing today", which is a
        // different claim from the true one, "this has not been measured
        // yet".
        let usage = AccountUsage::empty(at("2026-09-01T12:00:00Z"));
        let out = render_measured(&usage, false, 60, 12);

        assert!(out.contains("measuring recent usage..."));
        assert!(
            !out.contains("today") && !out.contains("no active block"),
            "no figures are printed before the first reading: {out}"
        );
    }

    #[test]
    fn an_active_block_shows_its_start_and_time_remaining() {
        let entries = [
            entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 100_000),
            entry("b", "2026-09-01T09:50:00Z", "/home/ada/api", 100_000),
        ];
        let now = at("2026-09-01T10:00:00Z");
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let out = render(&usage, 60, 12);
        assert!(out.contains("block"));
        assert!(out.contains("started 09:00"));
        assert!(out.contains("left"));
        assert!(out.contains("projected"));
    }

    #[test]
    fn a_high_burn_rate_is_the_only_cost_marker_that_reaches_the_warm_ramp() {
        // Two responses a minute apart, carrying enough fresh input tokens to
        // land the indicator well past the high-intensity threshold.
        let entries = [
            entry("a", "2026-09-01T09:00:00Z", "/home/ada/api", 6_000),
            entry("b", "2026-09-01T09:01:00Z", "/home/ada/api", 6_000),
        ];
        let now = at("2026-09-01T09:02:00Z");
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );
        let rate = usage.active_burn.expect("a rate to measure");

        assert_eq!(rate.intensity(), Intensity::High);
        let out = render(&usage, 40, 12);
        assert!(out.contains("projected"));
        assert!(out.contains("fast"), "the high burn is called out: {out}");
    }

    #[test]
    fn the_busiest_projects_are_listed_with_their_costs_right_aligned() {
        let entries = [
            entry("a", "2026-08-30T09:00:00Z", "/home/ada/api", 600_000),
            entry("b", "2026-08-30T09:00:00Z", "/home/ada/web", 100_000),
        ];
        let now = at("2026-09-01T12:00:00Z");
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let out = render(&usage, 40, 14);
        assert!(out.contains("api"), "the busier project is named: {out}");
        assert!(out.contains("web"));
        assert!(out.contains("$3.00"), "600k Opus 5 input at $5 per million");
    }

    #[test]
    fn a_long_project_name_is_truncated_rather_than_wrapped() {
        let entries = [entry(
            "a",
            "2026-08-30T09:00:00Z",
            "/home/ada/a-very-long-working-directory-name-indeed",
            100_000,
        )];
        let now = at("2026-09-01T12:00:00Z");
        let usage = AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &PriceSheet::builtin(),
            &Zone::Utc,
        );

        let out = render(&usage, 30, 12);
        assert!(
            out.contains('\u{2026}'),
            "the long name is cut with an ellipsis rather than wrapped: {out}"
        );
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let usage = AccountUsage::empty(at("2026-09-01T12:00:00Z"));
        for (w, h) in [(1, 1), (4, 2), (20, 3), (40, 1)] {
            let _ = render(&usage, w, h);
        }
    }
}

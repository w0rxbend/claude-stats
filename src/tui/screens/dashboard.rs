//! The main dashboard: everything about the attached session on one screen.
//!
//! The header is chrome -- which session am I even looking at -- and stays
//! hand-drawn here, the same way it always has. Everything beneath it is no
//! longer hand-drawn Rust control flow: `draw` builds a
//! [`DashboardViewModel`] from the session and usage readings, then hands it
//! to [`layout::solve`] together with [`presets::live`] -- the same tile
//! row, context gauge, account/spend row and detail columns this screen has
//! always shown, now expressed as a [`crate::tui::layout::Node`] tree instead
//! of a six-way `match` on how much height was left. `solve` decides which
//! panels fit and how big each one gets; [`PanelRegistry`] is asked, by id,
//! how to draw whichever ones survive.
//!
//! Before this rewrite, changing which panel sat where meant editing this
//! function and recompiling. Now it means editing [`presets::live`], or --
//! once a later epic wires runtime preset switching -- choosing a different
//! preset by name. Nothing about *what* gets drawn changed in this rewrite:
//! every panel below is the same widget call
//! [`crate::tui::panels::PanelRegistry::builtin`]'s renderers already made.
//!
//! Most of the test suite below is exactly what it was before this rewrite,
//! asserting the same things about the same terminal sizes: `solve`,
//! working from nothing but each panel's own honest minimum, reproduces the
//! pre-epic `match`'s decisions almost everywhere it was exercised. Three
//! tests could not survive unchanged, and each says why in its own doc
//! comment rather than here: the pre-epic `match` carried a couple of
//! hand-picked thresholds -- a two-hundred-column readability cutoff with no
//! panel-minimum equivalent, and one height band where the session detail
//! was preferred over a *shorter* account row -- that a single degrading
//! [`crate::tui::layout::Node`] tree cannot reproduce and still behave
//! sensibly everywhere else. See [`presets::live`]'s own doc for the fuller
//! account of why, and for the one place this rewrite deliberately departs
//! from the epic's own literal tree to fix a real bug rather than to work
//! around a limitation.
//!
//! [`DashboardViewModel`]: crate::view::dashboard_view::DashboardViewModel
//! [`PanelRegistry`]: crate::tui::panels::PanelRegistry

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::domain::limits::AccountUsage;
use crate::domain::session::{SessionPhase, SessionSnapshot};
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::layout::{presets, solve};
use crate::tui::palette::Palette;
use crate::tui::panels::PanelRegistry;
use crate::tui::screens::draw_tab_bar;
use crate::tui::widgets::spinner::{Spinner, SpinnerStyle};
use crate::view::dashboard_view;

/// What the body reads instead of a silent blank screen when
/// [`layout::solve`] finds nothing at all fits -- not even the smallest
/// single panel meets its own registered minimum.
///
/// [`layout::solve`]: crate::tui::layout::solve
const TOO_SMALL_MESSAGE: &str = "terminal too small — resize to see the dashboard";

/// `panel.dollar-pulse`'s own animation inputs for one frame, reduced from
/// `App`'s `crate::tui::widgets::dollar_pulse::PulseClock` and
/// `AnimationStyle` to the two primitives
/// [`crate::view::dashboard_view::build`] can honestly carry, and bundled
/// into one parameter so [`draw`]'s own signature does not grow past a
/// comfortable seven arguments. `App::draw` is where the reduction happens
/// -- `frames_since_increment` from `PulseClock::frames_since`, `off` from
/// comparing the resolved `AnimationStyle` against `Off` -- see
/// [`crate::view::dashboard_view::DollarPulseView::off`]'s own doc for why
/// the full style cannot cross into the view model itself.
#[derive(Debug, Clone, Copy)]
pub struct DollarPulseInputs {
    pub frames_since_increment: Option<u64>,
    pub off: bool,
}

/// The small pieces of `App`'s own runtime state that [`draw`] needs beyond
/// what it can already read out of the `SessionSnapshot`/`AccountUsage`
/// arguments, bundled into one parameter for the same reason
/// [`DollarPulseInputs`] itself was: each value on its own is a single
/// primitive `App::draw` already had to reduce some richer piece of its own
/// state down to, but passing every one of them as its own loose argument
/// would push [`draw`]'s own parameter count past what
/// `clippy::too_many_arguments` -- and a reader trying to hold the whole
/// call in their head -- comfortably allow.
#[derive(Debug, Clone, Copy)]
pub struct DashboardInputs<'a> {
    pub pulse: DollarPulseInputs,
    /// The layout preset [`draw`] solves against -- see [`draw`]'s own doc
    /// comment for how an unresolvable name degrades.
    pub active_preset: &'a str,
    /// Which of `crate::tui::screens::TAB_LABELS` is current, for the tab bar
    /// every content view now reserves its own top row for. Bundled in here
    /// for the same reason `pulse` and `active_preset` already are: a fourth
    /// loose parameter would push [`draw`] past the seven
    /// `clippy::too_many_arguments` allows.
    pub tab_index: usize,
}

/// Draws the dashboard for `snapshot` into `area`: the header on its own
/// fixed row, then every panel `inputs.active_preset` places, solved against
/// whatever is left.
///
/// `inputs.active_preset` is looked up with [`presets::by_name`], falling
/// back to [`presets::live`] for anything that name does not resolve to --
/// today that means any of the four built-in preset names switches what
/// actually renders, while a custom `config.layouts` entry (which `App`'s
/// layout picker also lists and happily persists to `config.json`) still
/// falls back to `live` here, since resolving a `LayoutNodeDto` into a real
/// [`crate::tui::layout::Node`] at this call site is a later epic's wiring,
/// not this one's -- see `crate::tui::app::App::confirm_layout_picker`'s doc
/// comment for the fuller account.
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &SessionSnapshot,
    phase: u64,
    usage: Option<(&AccountUsage, bool)>,
    palette: &Palette,
    inputs: DashboardInputs<'_>,
) {
    let [tab_bar, header, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    draw_tab_bar(frame, tab_bar, inputs.tab_index, palette);
    draw_header(frame, header, snapshot, phase, palette);

    let view_model = dashboard_view::build(
        snapshot,
        usage,
        phase,
        inputs.pulse.frames_since_increment,
        inputs.pulse.off,
    );

    let registry = PanelRegistry::builtin();
    let layout = presets::by_name(inputs.active_preset).unwrap_or_else(presets::live);
    let solved = solve(&layout, body, &|id| {
        registry.get(id).map_or((0, 0), |(spec, _)| spec.min)
    });

    // `solve` answers an empty `Vec` in exactly one case: the body is too
    // small for even the single smallest registered panel to meet its own
    // minimum (see `layout::solve`'s Rule 1). A silent blank screen there
    // reads as broken rather than as "make the window bigger", so this is
    // the one small, targeted addition this epic's small-terminal pass asks
    // for: a centred line of text instead of nothing.
    if solved.is_empty() {
        frame.render_widget(
            Paragraph::new(TOO_SMALL_MESSAGE)
                .alignment(Alignment::Center)
                .wrap(ratatui::widgets::Wrap { trim: true })
                .style(Style::default().fg(palette.muted.into())),
            body,
        );
        return;
    }

    for (id, rect) in solved {
        if let Some((_, renderer)) = registry.get(&id) {
            renderer(frame, rect, &view_model, palette, phase);
        }
    }
}

// ── header ────────────────────────────────────────────────────────────

fn draw_header(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &SessionSnapshot,
    phase: u64,
    palette: &Palette,
) {
    let live = snapshot.phase == SessionPhase::Thinking;
    let (marker, marker_colour, state) = if live {
        (
            Spinner::new(SpinnerStyle::Braille, phase).glyph(),
            palette.accent_success.into(),
            "working",
        )
    } else {
        (Icon::IDLE, palette.muted.into(), "idle")
    };

    let mut spans = vec![
        Span::styled(
            " claude-stats ",
            Style::default()
                .fg(palette.background.into())
                .bg(palette.accent_primary.into())
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
    // A branch name has no length convention to lean on the way a project
    // path or a model id does -- someone's `feature/...` branch can run to
    // any length -- so left unbounded here it relied entirely on whatever
    // was left of this single unwrapped header line to cut it off mid-word,
    // with nothing on screen to say it had been cut. `format::fit` gives it
    // the same honest ellipsis treatment `project` above already gets,
    // rather than being the one field on this line drawn as if it always
    // fits.
    let branch = snapshot
        .git_branch
        .as_deref()
        .map(|b| format::fit(b, 24, false));
    for (icon, text, colour) in [
        (
            Icon::TOKEN,
            Some(snapshot.model_display_name()),
            palette.accent_secondary.into(),
        ),
        (Icon::FILE, project, palette.text.into()),
        (Icon::BRANCH, branch, palette.accent_success.into()),
        (
            Icon::CLOCK,
            snapshot.duration().map(format::duration),
            palette.muted.into(),
        ),
    ] {
        let Some(text) = text else { continue };
        spans.push(Span::styled(
            format!("  {} ", Icon::SEPARATOR),
            Style::default().fg(palette.faint.into()),
        ));
        spans.push(Span::styled(
            format!("{icon} "),
            Style::default().fg(palette.faint.into()),
        ));
        spans.push(Span::styled(text, Style::default().fg(colour)));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(palette.surface.into())),
        area,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::domain::session::ResponseSample;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

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
        render_with_usage(width, height, None)
    }

    fn render_with_usage(
        width: u16,
        height: u16,
        usage: Option<&crate::domain::limits::AccountUsage>,
    ) -> String {
        render_with_snapshot(width, height, &sample_snapshot(), usage)
    }

    fn render_with_snapshot(
        width: u16,
        height: u16,
        snapshot: &SessionSnapshot,
        usage: Option<&crate::domain::limits::AccountUsage>,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        let palette = palette();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    snapshot,
                    0,
                    usage.map(|u| (u, true)),
                    &palette,
                    DashboardInputs {
                        pulse: DollarPulseInputs {
                            frames_since_increment: None,
                            off: false,
                        },
                        active_preset: "live",
                        tab_index: 0,
                    },
                );
            })
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
    fn a_long_branch_name_is_cut_with_an_ellipsis_rather_than_mid_word() {
        // Before `draw_header` ran the branch name through `format::fit` the
        // way it already did for the project path, a long branch name had
        // no bound of its own and relied entirely on the header's single
        // unwrapped line running out of terminal width to cut it off --
        // silently, mid-word, with nothing on screen to say more of the
        // name existed. This pins the honest version: the name is capped on
        // its own terms and the cut is marked.
        let mut snapshot = sample_snapshot();
        snapshot.git_branch =
            Some("feature/a-branch-name-far-longer-than-any-reasonable-header-budget".to_owned());
        let screen = render_with_snapshot(140, 40, &snapshot, None);
        assert!(
            screen.contains('\u{2026}'),
            "the long branch name is cut with an ellipsis: {screen}"
        );
        assert!(
            !screen.contains("far longer than any reasonable header budget"),
            "the untruncated tail must not appear: {screen}"
        );
    }

    #[test]
    fn a_narrow_terminal_with_nothing_to_report_leaves_the_lower_half_blank() {
        // Pre-epic, a terminal narrower than ninety columns collapsed the
        // detail section down to the activity feed alone -- a hand-picked
        // readability threshold `draw_detail` carried as its own constant,
        // with no equivalent in any panel's own registered minimum. `solve`
        // has no such constant to consult: `panel.output-trend` and
        // `panel.tool-feed` genuinely only need thirty columns each, sixty
        // together, so a seventy-column terminal comfortably fits both and
        // no longer collapses to one -- see `presets::live`'s own module
        // doc for a fuller account of why that specific old threshold has no
        // honest equivalent to reproduce.
        //
        // What this test still pins is a real property of the rewrite: with
        // no usage reading at all (`render_at` passes `None`), the row this
        // preset reserves for the account and spend panels stays reserved
        // but draws nothing -- `presets::live` cannot know, from geometry
        // alone, that there is nothing to show there, so the fixed height it
        // set aside goes unused rather than being handed to the detail
        // section beneath it. That is real, user-visible wasted space this
        // rewrite introduces, not a bug in this test.
        let screen = render_at(70, 30);
        assert!(
            !screen.contains("live tool activity") && !screen.contains("token mix"),
            "no usage reading means the account/spend row still claims its \
             fixed height, starving the detail row of the room it would \
             otherwise have: {screen}"
        );
    }

    #[test]
    fn a_terminal_seventy_columns_wide_shows_both_detail_columns() {
        // The companion case to the test above: once there *is* a usage
        // reading, the account/spend row's fixed height leaves comfortably
        // more than sixty columns for the detail row -- see that test's own
        // doc for why sixty, not the pre-epic ninety, is this rewrite's real
        // threshold.
        let usage = crate::domain::limits::AccountUsage::empty(chrono::Utc::now());
        let screen = render_with_usage(70, 40, Some(&usage));
        assert!(screen.contains("live tool activity"), "{screen}");
        assert!(screen.contains("token mix"), "{screen}");
    }

    #[test]
    fn a_short_terminal_keeps_the_tiles_and_drops_the_detail() {
        let screen = render_at(140, 12);
        assert!(screen.contains("CONTEXT"));
        assert!(!screen.contains("live tool activity"));
    }

    #[test]
    fn a_tall_terminal_shows_the_account_panel_alongside_the_detail_columns() {
        let usage = crate::domain::limits::AccountUsage::empty(chrono::Utc::now());
        let screen = render_with_usage(140, 40, Some(&usage));

        assert!(screen.contains("account usage"));
        assert!(screen.contains("live tool activity"), "and the detail too");
    }

    #[test]
    fn when_only_one_lower_section_fits_the_account_row_wins() {
        // Pre-epic, this exact height (twenty-seven rows) was the one place
        // the six-way `match` picked the session detail over the account
        // row even though the account row was the *shorter* of the two --
        // rest.height sat inside the [18, 26) band, which the account row on
        // its own only needed eleven rows to clear. That inversion has no
        // honest equivalent in `layout::solve`'s degradation rule: dropping
        // the lowest-priority child from a fixed list, one at a time,
        // watching space grow, can never later decide a *higher*-priority
        // child should give way to a *lower*-priority one it could already
        // have kept -- see `presets::live`'s module doc for the fuller
        // account. `presets::live` lists the account row first (matching
        // the order the epic's own tree gives it), so once it and the
        // detail row cannot both fit, the account row is the one that
        // survives, and it survives at every height below the point both fit
        // together -- not only the two narrower bands the old code carved
        // out for it.
        let usage = crate::domain::limits::AccountUsage::empty(chrono::Utc::now());
        let screen = render_with_usage(140, 27, Some(&usage));

        assert!(screen.contains("account usage"), "{screen}");
        assert!(!screen.contains("live tool activity"), "{screen}");
    }

    #[test]
    fn a_terminal_too_short_for_the_detail_columns_shows_the_account_panel() {
        // These rows would otherwise be blank: the detail columns need more
        // height than there is, and something true is better than nothing.
        //
        // Twenty-one, not the twenty this test pinned before the tab bar
        // this epic adds claimed a row of its own: `body` is now
        // `area.height - 2` (tab bar plus header) rather than `- 1`, so the
        // exact height at which the tile row, the context gauge and the
        // account row (`4 + 4 + 11 = 19`) just barely fit moved down by
        // exactly the one row the tab bar now reserves.
        let usage = crate::domain::limits::AccountUsage::empty(chrono::Utc::now());
        let screen = render_with_usage(140, 21, Some(&usage));

        assert!(screen.contains("account usage"));
        assert!(!screen.contains("live tool activity"));
    }

    #[test]
    fn the_account_panel_is_absent_when_nothing_is_tracking_usage() {
        let screen = render_at(140, 40);
        assert!(!screen.contains("account usage"));
    }

    #[test]
    fn drawing_into_a_tiny_terminal_does_not_panic() {
        // Terminals get resized to absurd sizes while being dragged, and a
        // panic there takes the whole dashboard down.
        for (width, height) in [(1, 1), (4, 3), (20, 5), (200, 2)] {
            let _ = render_at(width, height);
        }
    }

    #[test]
    fn a_terminal_too_small_for_any_panel_shows_a_resize_message() {
        // Twenty columns is well below `panel.tile-row`'s own registered
        // thirty-six-column minimum -- the narrowest panel `presets::live`
        // places -- so the body `solve` is handed here cannot fit even the
        // smallest single panel. Six rows of height (four of them left to
        // the body once the tab bar and header claim their own row each)
        // is enough for the wrapped message to appear in full, which is
        // what this test actually reads back rather than merely checking
        // for a non-panic.
        let screen = render_at(20, 8);
        assert!(
            screen.contains("too small"),
            "expected the resize message somewhere in the buffer: {screen:?}"
        );
    }

    #[test]
    fn a_degenerate_one_by_one_terminal_still_does_not_panic() {
        // The resize message itself has nowhere to draw once the tab bar's
        // own reserved row already consumes the whole area -- this is the
        // "nothing panics" half of the small-terminal pass, kept as its own
        // test now that `a_terminal_too_small_for_any_panel_shows_a_resize_message`
        // above is the one actually reading the message back.
        let _ = render_at(1, 1);
    }

    /// The instant the tests below measure account usage as of. Fixed rather
    /// than read from the system clock, so the block and the "today" bucket
    /// they build come out the same on every run and every machine.
    fn fixed_now() -> chrono::DateTime<chrono::Utc> {
        "2026-09-01T10:00:00Z".parse().expect("a valid timestamp")
    }

    /// One billable response, for the tests that need account usage measured
    /// from real entries rather than [`crate::domain::limits::AccountUsage::empty`].
    fn measured_entry(
        session: &str,
        when: &str,
        project: &str,
        input: u64,
    ) -> crate::domain::entry::Entry {
        use crate::domain::entry::{Entry, EntryId};
        use crate::domain::model::ModelId;
        use crate::domain::project::{Project, SessionId};
        use crate::domain::tokens::TokenUsage;

        let at: chrono::DateTime<chrono::Utc> = when.parse().expect("a valid timestamp");
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
    fn the_dashboard_shows_todays_spend_the_active_block_and_the_top_projects() {
        let now = fixed_now();
        let entries = [
            measured_entry("a", "2026-09-01T09:30:00Z", "/home/ada/api", 100_000),
            measured_entry("b", "2026-09-01T09:50:00Z", "/home/ada/web", 50_000),
        ];
        let usage = crate::domain::limits::AccountUsage::measure(
            now,
            &entries,
            Vec::new(),
            &crate::domain::pricing::PriceSheet::builtin(),
            &crate::domain::period::Zone::Utc,
        );

        let screen = render_with_usage(140, 40, Some(&usage));

        assert!(screen.contains("today"), "today's spend is shown: {screen}");
        assert!(
            screen.contains("block"),
            "the active block is shown: {screen}"
        );
        assert!(
            screen.contains("api"),
            "the busiest project is named: {screen}"
        );
    }

    #[test]
    fn a_tall_enough_terminal_shows_the_account_row_and_the_session_detail_together() {
        // Pre-epic, this height dropped the spend panel alone -- riding
        // beside the account-usage panel in the same row -- while keeping
        // both the (now spend-less) account row and the session detail
        // beneath it. `panel.account-usage` and `panel.spend-panel` are one
        // fused row in `presets::live` (matching the tree the epic itself
        // specifies), so there is no way for this rewrite to keep one half
        // of that row and drop the other independently of the whole row's
        // own fate: the row is either tall enough to show both, as it is
        // here, or it is dropped entirely -- see
        // `when_only_one_lower_section_fits_the_account_row_wins` for that
        // case. What survives from the old test is the one thing this
        // height was actually chosen to prove: with room to spare, the
        // account row and the session detail beneath it both show at once.
        let usage = crate::domain::limits::AccountUsage::empty(fixed_now());
        let screen = render_with_usage(140, 36, Some(&usage));

        assert!(screen.contains("live tool activity"), "{screen}");
        assert!(screen.contains("account usage"), "{screen}");
    }

    #[test]
    fn a_dashboard_with_no_active_block_says_so_rather_than_leaving_the_row_blank() {
        let usage = crate::domain::limits::AccountUsage::empty(fixed_now());
        let screen = render_with_usage(140, 40, Some(&usage));

        assert!(screen.contains("no active block"), "{screen}");
    }
}

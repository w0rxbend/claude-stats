//! The account-usage panel: what the last five hours and seven days cost, and
//! whether a rate limit is currently in force.
//!
//! # Why there is no percentage here
//!
//! The obvious design is a gauge reading "63% of your session limit". That
//! number cannot be computed on this machine: limits are enforced on
//! Anthropic's side and the live figure is never written to disk. Drawing a
//! plausible-looking gauge from a guessed ceiling would be worse than drawing
//! nothing, because it would be believed.
//!
//! So each window shows what was actually measured -- tokens, cost, how many
//! sessions -- and the bar beside it is drawn against
//! [`WindowUsage::peak`](crate::domain::limits::WindowUsage::peak): the
//! busiest comparable window in your own history. That is a real comparison
//! and it is labelled `vs peak` on screen so it cannot be misread as a limit.
//!
//! The one exact thing is the countdown. When the API has actually refused a
//! request, it says when the limit lifts, and that line is the server's own
//! answer rather than anything inferred here.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Widget};

use crate::domain::limits::{AccountUsage, LimitEvent, WindowKind, WindowUsage};
use crate::tui::format;
use crate::tui::icons::{EIGHTHS, Icon};
use crate::tui::theme::Theme;

/// Width of the `vs peak` bar. Narrow, because the figures beside it are the
/// point and the bar is only there to give them a shape.
const BAR_WIDTH: usize = 14;

/// The account-usage panel.
pub struct UsageWindows<'a> {
    usage: &'a AccountUsage,
    /// Whether a reading has been taken yet, so a dashboard that has only just
    /// opened says "measuring" rather than a confident row of zeroes.
    measured: bool,
}

impl<'a> UsageWindows<'a> {
    /// A panel over the given reading.
    #[must_use]
    pub const fn new(usage: &'a AccountUsage, measured: bool) -> Self {
        Self { usage, measured }
    }
}

impl Widget for UsageWindows<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let limit = self.usage.active_limit();
        // A live limit sets the whole panel's colour: it is the one state here
        // that changes what the reader should do next.
        let accent = if limit.is_some() {
            Theme::CRIMSON
        } else {
            Theme::MAGENTA
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Theme::BORDER))
            .style(Style::default().bg(Theme::SURFACE))
            .padding(Padding::horizontal(1))
            .title(Span::styled(" account usage ", Theme::title(accent)));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 {
            return;
        }

        if !self.measured {
            Paragraph::new(Line::from(Span::styled(
                "measuring recent usage...",
                Style::default().fg(Theme::MUTED),
            )))
            .render(inner, buf);
            return;
        }

        // The limit banner is worth a row only when there is one; without it
        // the two windows get the space instead.
        let banner_rows = u16::from(limit.is_some());
        let [banner, windows] =
            Layout::vertical([Constraint::Length(banner_rows), Constraint::Min(0)]).areas(inner);

        if let Some(limit) = limit {
            Paragraph::new(limit_line(limit, self.usage.measured_at)).render(banner, buf);
        }

        let mut lines = Vec::new();
        lines.extend(window_lines(&self.usage.session, accent));
        if windows.height >= 4 {
            lines.push(Line::raw(""));
            lines.extend(window_lines(&self.usage.week, Theme::AZURE));
        }
        Paragraph::new(lines).render(windows, buf);
    }
}

/// The two lines describing one window: the bar, then the figures.
fn window_lines(window: &WindowUsage, accent: Color) -> Vec<Line<'static>> {
    let headline = Line::from(vec![
        Span::styled(
            format!("{} {:<8}", Icon::CLOCK, window.kind.span_label()),
            Theme::label(),
        ),
        Span::styled(bar(window.share_of_peak()), Style::default().fg(accent)),
        Span::raw(" "),
        Span::styled(
            format::tokens(window.tokens.total()),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ]);

    // "vs peak" rather than a bare percentage: the comparison is to the user's
    // own busiest window, and saying so is the whole point.
    let comparison = window.share_of_peak().map_or_else(
        || "no busier window on record".to_owned(),
        |share| format!("{} vs peak", format::percent(share)),
    );
    let detail = Line::from(vec![
        Span::raw("           "),
        Span::styled(format!("{}", window.cost), Style::default().fg(Theme::CYAN)),
        Span::styled(
            format!(
                "  {} {} session{}",
                Icon::SEPARATOR,
                window.sessions,
                if window.sessions == 1 { "" } else { "s" }
            ),
            Style::default().fg(Theme::MUTED),
        ),
        Span::styled(
            format!("  {} {comparison}", Icon::SEPARATOR),
            Style::default().fg(Theme::FAINT),
        ),
    ]);

    vec![headline, detail]
}

/// The banner shown while a limit is actually in force.
fn limit_line(limit: LimitEvent, now: chrono::DateTime<chrono::Utc>) -> Line<'static> {
    let remaining = limit
        .time_until_reset(now)
        .map_or_else(|| "any moment".to_owned(), format::duration);
    let what = match limit.kind {
        WindowKind::Session => "session limit reached",
        WindowKind::Week => "weekly limit reached",
    };
    Line::from(vec![
        Span::styled(
            format!("{} {what}", Icon::ERROR),
            Style::default()
                .fg(Theme::CRIMSON)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} resets in {remaining}", Icon::SEPARATOR),
            Style::default().fg(Theme::AMBER),
        ),
    ])
}

/// A partial-block bar, or a row of dashes when there is nothing to compare to.
fn bar(share: Option<f64>) -> String {
    let Some(share) = share else {
        return Icon::BAR_EMPTY.repeat(BAR_WIDTH);
    };
    let eighths = (share.clamp(0.0, 1.0) * (BAR_WIDTH * 8) as f64).round() as usize;
    (0..BAR_WIDTH)
        .map(|cell| {
            let here = eighths.saturating_sub(cell * 8).min(8);
            if here == 0 {
                Icon::BAR_EMPTY
            } else {
                EIGHTHS[here - 1]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::domain::limits::{SessionContribution, UsagePoint};
    use crate::domain::money::Usd;
    use crate::domain::tokens::TokenUsage;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000, 0)
            .single()
            .expect("a time")
    }

    fn point(minutes_ago: i64, tokens: u64) -> UsagePoint {
        UsagePoint {
            at: now() - chrono::Duration::minutes(minutes_ago),
            tokens: TokenUsage {
                input: tokens,
                ..TokenUsage::ZERO
            },
            cost: Usd::new(3.5),
        }
    }

    fn render(usage: &AccountUsage, measured: bool, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(UsageWindows::new(usage, measured), frame.area());
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
    fn both_windows_and_their_costs_are_shown() {
        let usage = AccountUsage::measure(
            now(),
            &[SessionContribution {
                session_id: "a".to_owned(),
                points: vec![point(30, 400_000), point(60 * 24 * 3, 100_000)],
            }],
            Vec::new(),
        );
        let out = render(&usage, true, 70, 8);

        assert!(out.contains("account usage"));
        assert!(out.contains("5h"), "the session window");
        assert!(out.contains("7d"), "the weekly window");
        assert!(out.contains("400.0k"), "the five-hour total");
        assert!(out.contains("500.0k"), "the seven-day total");
    }

    #[test]
    fn no_percentage_of_any_limit_is_claimed() {
        // The panel must never imply it knows the account's real ceiling.
        let usage = AccountUsage::measure(
            now(),
            &[SessionContribution {
                session_id: "a".to_owned(),
                points: vec![point(30, 400_000)],
            }],
            Vec::new(),
        );
        let out = render(&usage, true, 70, 8).to_lowercase();

        assert!(!out.contains("of limit"));
        assert!(!out.contains("of your limit"));
        assert!(!out.contains("remaining"));
    }

    #[test]
    fn an_active_limit_is_announced_with_its_countdown() {
        let limit = LimitEvent {
            at: now() - chrono::Duration::minutes(10),
            resets_at: now() + chrono::Duration::minutes(42),
            kind: WindowKind::Session,
        };
        let usage = AccountUsage::measure(now(), &[], vec![limit]);
        let out = render(&usage, true, 70, 8);

        assert!(out.contains("session limit reached"));
        assert!(
            out.contains("42m"),
            "the countdown, from the server's reset"
        );
    }

    #[test]
    fn an_expired_limit_is_not_announced() {
        let limit = LimitEvent {
            at: now() - chrono::Duration::hours(9),
            resets_at: now() - chrono::Duration::hours(4),
            kind: WindowKind::Session,
        };
        let usage = AccountUsage::measure(now(), &[], vec![limit]);
        let out = render(&usage, true, 70, 8);

        assert!(!out.contains("limit reached"));
    }

    #[test]
    fn before_the_first_scan_it_says_so_rather_than_showing_zeroes() {
        let usage = AccountUsage::empty(now());
        let out = render(&usage, false, 70, 8);

        assert!(out.contains("measuring"));
        assert!(!out.contains("0 sessions"));
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let usage = AccountUsage::empty(now());
        for (w, h) in [(1, 1), (4, 2), (20, 3), (70, 1)] {
            let _ = render(&usage, true, w, h);
        }
    }
}

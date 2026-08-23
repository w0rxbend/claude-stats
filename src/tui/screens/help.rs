//! The keybinding overlay, and the "looking for a session" splash.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::tui::icons::Icon;
use crate::tui::theme::Theme;
use crate::tui::widgets::spinner::{Spinner, SpinnerStyle};

/// Every key the dashboard listens for, with what it does.
///
/// The full list, shown in this overlay. The footer hint in
/// `App::draw_footer` is a deliberately shorter per-view subset, written by
/// hand and worded for its view, so a new binding has to be added there too.
pub const KEYS: &[(&str, &str)] = &[
    ("q / Esc", "quit"),
    ("d", "dashboard"),
    ("l", "event log"),
    ("o", "session picker"),
    ("Enter", "attach to the selected session"),
    ("j / k", "move down / up"),
    ("g / G", "jump to the oldest / newest entry"),
    ("r", "re-read the transcript and re-measure usage"),
    ("?", "this help"),
];

/// Draws the help overlay centred over whatever is behind it.
pub fn draw(frame: &mut Frame<'_>, area: Rect) {
    let height = KEYS.len() as u16 + 4;
    let popup = centred(area, 52, height);

    // Clearing first is what makes this an overlay rather than a transparency
    // effect: without it, the dashboard behind shows through the gaps between
    // the characters of this panel.
    frame.render_widget(Clear, popup);

    let lines: Vec<Line<'_>> = KEYS
        .iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:>9}  "),
                    Style::default()
                        .fg(Theme::CYAN)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*description, Style::default().fg(Theme::TEXT)),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Theme::BORDER_ACTIVE))
                .style(Style::default().bg(Theme::SURFACE))
                .padding(Padding::uniform(1))
                .title(Span::styled(" keys ", Theme::title(Theme::CYAN))),
        ),
        popup,
    );
}

/// Draws the splash shown while no session has been found yet.
///
/// This is a real screen rather than a blank one because "nothing is
/// happening" and "the tool is broken" look identical otherwise, and the first
/// thing a new user does is run `claude-stats monitor` before starting a session.
pub fn draw_searching(frame: &mut Frame<'_>, area: Rect, phase: u64) {
    let spinner = Spinner::new(SpinnerStyle::Quadrant, phase / 2).glyph();
    let lines = vec![
        Line::from(Span::styled(
            "claude-stats",
            Style::default()
                .fg(Theme::CYAN)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{spinner} looking for an active session"),
            Style::default().fg(Theme::TEXT),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "{} start Claude Code in another terminal, or press o to pick one",
                Icon::BULLET
            ),
            Style::default().fg(Theme::MUTED),
        )),
    ];

    let popup = centred(area, 68, 9);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Theme::BORDER))
                .style(Style::default().bg(Theme::SURFACE))
                .padding(Padding::uniform(1)),
        ),
        popup,
    );
}

/// A `width` x `height` rectangle centred in `area`, never larger than it.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_popup_never_grows_beyond_the_screen_it_is_centred_in() {
        let tiny = Rect::new(0, 0, 10, 4);
        let popup = centred(tiny, 52, 20);
        assert!(popup.width <= tiny.width);
        assert!(popup.height <= tiny.height);
    }

    #[test]
    fn a_popup_is_actually_centred() {
        let screen = Rect::new(0, 0, 100, 50);
        let popup = centred(screen, 50, 10);
        assert_eq!(popup.x, 25);
        assert_eq!(popup.y, 20);
    }
}

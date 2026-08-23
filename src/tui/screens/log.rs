//! The scrollable event log.
//!
//! The dashboard shows the last few tool calls; this shows everything, with
//! timestamps, so a session can be reconstructed after the fact. It is a
//! separate view rather than a panel because it wants the full width -- a
//! command line truncated to thirty columns is not evidence of anything.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState};

use crate::domain::session::{LogEntry, LogLevel, SessionSnapshot};
use crate::tui::format;
use crate::tui::theme::Theme;

/// Draws the log, scrolled so that `offset` entries from the bottom are hidden.
///
/// The offset is from the *bottom* because the newest entry is the one being
/// followed; anchoring to the top would make the view slide out from under the
/// reader every time a line is appended.
pub fn draw(frame: &mut Frame<'_>, area: Rect, snapshot: &SessionSnapshot, offset: usize) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Theme::BORDER))
        .style(Style::default().bg(Theme::SURFACE))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" event log \u{00b7} {} entries ", snapshot.events.len()),
            Theme::title(Theme::CYAN),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 12 {
        return;
    }

    let rows = inner.height as usize;
    let total = snapshot.events.len();
    let offset = offset.min(total.saturating_sub(rows));
    let end = total.saturating_sub(offset);
    let start = end.saturating_sub(rows);

    let lines: Vec<Line<'_>> = snapshot
        .events
        .iter()
        .skip(start)
        .take(end - start)
        .map(|entry| line_for(entry, inner.width as usize))
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);

    if total > rows {
        let mut state = ScrollbarState::new(total.saturating_sub(rows)).position(end - rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(Theme::BORDER)),
            area,
            &mut state,
        );
    }
}

fn line_for(entry: &LogEntry, width: usize) -> Line<'_> {
    let colour = match entry.level {
        LogLevel::Info => Theme::TEXT,
        LogLevel::Notice => Theme::VIOLET,
        LogLevel::Error => Theme::CRIMSON,
    };
    let mut style = Style::default().fg(colour);
    if entry.level != LogLevel::Info {
        style = style.add_modifier(Modifier::BOLD);
    }
    // Nine columns go to the timestamp and its trailing space.
    let text = format::fit(&entry.text, width.saturating_sub(9), false);
    Line::from(vec![
        Span::styled(
            entry.at.format("%H:%M:%S").to_string(),
            Style::default().fg(Theme::FAINT),
        ),
        Span::raw(" "),
        Span::styled(text, style),
    ])
}

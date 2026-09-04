//! The scrollable event log.
//!
//! The dashboard shows the last few tool calls; this shows everything, with
//! timestamps, so a session can be reconstructed after the fact. It is a
//! separate view rather than a panel because it wants the full width -- a
//! command line truncated to thirty columns is not evidence of anything.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use super::draw_tab_bar;
use crate::domain::session::{LogEntry, LogLevel, SessionSnapshot};
use crate::tui::format;
use crate::tui::palette::Palette;

/// Draws the tab bar on its own reserved top row, then the log beneath it,
/// scrolled so that `offset` entries from the bottom are hidden.
///
/// The offset is from the *bottom* because the newest entry is the one being
/// followed; anchoring to the top would make the view slide out from under the
/// reader every time a line is appended.
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    snapshot: &SessionSnapshot,
    offset: usize,
    tab_index: usize,
    palette: &Palette,
) {
    let [tab_bar, area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    draw_tab_bar(frame, tab_bar, tab_index, palette);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" event log \u{00b7} {} entries ", snapshot.events.len()),
            palette.title(palette.accent_primary.into()),
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
        .map(|entry| line_for(entry, inner.width as usize, palette))
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);

    if total > rows {
        let mut state = ScrollbarState::new(total.saturating_sub(rows)).position(end - rows);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(palette.border.into())),
            area,
            &mut state,
        );
    }
}

fn line_for<'a>(entry: &'a LogEntry, width: usize, palette: &Palette) -> Line<'a> {
    let colour = match entry.level {
        LogLevel::Info => palette.text,
        LogLevel::Notice => palette.accent_secondary,
        LogLevel::Error => palette.pressure_high,
    };
    let mut style = Style::default().fg(colour.into());
    if entry.level != LogLevel::Info {
        style = style.add_modifier(Modifier::BOLD);
    }
    // Nine columns go to the timestamp and its trailing space.
    let text = format::fit(&entry.text, width.saturating_sub(9), false);
    Line::from(vec![
        Span::styled(
            entry.at.format("%H:%M:%S").to_string(),
            Style::default().fg(palette.faint.into()),
        ),
        Span::raw(" "),
        Span::styled(text, style),
    ])
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    #[test]
    fn the_tab_bar_is_drawn_above_the_log_panel() {
        let snapshot = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abc".to_owned());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &snapshot, 0, 5, &palette()))
            .expect("draw succeeds");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            screen.contains("Dashboard"),
            "the tab bar names every tab: {screen}"
        );
        assert!(
            screen.contains("event log"),
            "and the log panel still shows: {screen}"
        );
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let snapshot = SessionSnapshot::empty("/tmp/t.jsonl".into(), "abc".to_owned());
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &snapshot, 0, 5, &palette()))
            .expect("draw succeeds");
    }
}

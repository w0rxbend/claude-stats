//! The session picker.
//!
//! Lists every transcript on the machine, newest first, so the dashboard can
//! be pointed at a session other than the active one -- to look at what a
//! session in another checkout is doing, or to review one that has finished.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};

use crate::application::ports::TranscriptRef;
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::theme::Theme;

/// Draws the picker with `selected` highlighted.
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    sessions: &[TranscriptRef],
    selected: usize,
    attached: Option<&TranscriptRef>,
) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Theme::BORDER_ACTIVE))
        .style(Style::default().bg(Theme::SURFACE))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" sessions \u{00b7} {} found ", sessions.len()),
            Theme::title(Theme::CYAN),
        ));

    let width = block.inner(area).width as usize;
    let items: Vec<ListItem<'_>> = sessions
        .iter()
        .map(|session| {
            let live = attached.is_some_and(|a| a.path == session.path);
            let marker = if live { Icon::LIVE } else { Icon::BULLET };
            let colour = if live { Theme::MINT } else { Theme::MUTED };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(colour)),
                Span::styled(
                    format!("{:<9}", &session.session_id[..8.min(session.session_id.len())]),
                    Style::default().fg(Theme::CYAN),
                ),
                Span::styled(
                    format!("{:<7}", session.modified_at.format("%H:%M")),
                    Style::default().fg(Theme::FAINT),
                ),
                Span::styled(
                    format::fit(&session.project_dir, width.saturating_sub(20), true),
                    Style::default().fg(Theme::TEXT),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(
            Style::default()
                .bg(Theme::BORDER)
                .add_modifier(Modifier::BOLD),
        ),
        area,
        &mut state,
    );
}

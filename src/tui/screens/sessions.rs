//! The session picker overlay.
//!
//! Lists every transcript on the machine, newest first, so the dashboard can
//! be pointed at a session other than the active one -- to look at what a
//! session in another checkout is doing, or to review one that has finished.
//!
//! Drawn as a popup over whichever content tab is showing, via the same
//! `Clear` + bordered `Block` mechanism [`super::help`] uses, rather than as
//! a full-screen view of its own: [`crate::tui::app::View`] no longer has a
//! `Sessions` variant (see that enum's own doc comment for why), and the tab
//! underneath keeps rendering right up until this overlay's `Clear` wipes the
//! rectangle it occupies.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding};

use super::centred;
use crate::application::ports::TranscriptRef;
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::palette::Palette;

/// Draws the picker, popped up over `area` with `selected` highlighted.
///
/// The popup is sized generously -- most of `area`, with a small margin --
/// rather than to a fixed width the way [`super::theme_picker`]'s and
/// [`super::layout_picker`]'s short name lists are: a session row carries a
/// project directory that can run to dozens of characters, and a popup that
/// clipped it on every machine but the reviewer's would not be much of a
/// picker.
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    sessions: &[TranscriptRef],
    selected: usize,
    attached: Option<&TranscriptRef>,
    palette: &Palette,
) {
    let popup = centred(
        area,
        area.width.saturating_sub(8).max(20),
        area.height.saturating_sub(4).max(6),
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border_active.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" sessions \u{00b7} {} found ", sessions.len()),
            palette.title(palette.accent_primary.into()),
        ));

    let width = block.inner(popup).width as usize;
    let items: Vec<ListItem<'_>> = sessions
        .iter()
        .map(|session| {
            let live = attached.is_some_and(|a| a.path == session.path);
            let marker = if live { Icon::LIVE } else { Icon::BULLET };
            let colour: ratatui::style::Color = if live {
                palette.accent_success.into()
            } else {
                palette.muted.into()
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(colour)),
                Span::styled(
                    format!("{:<9}", format::session_id(&session.session_id)),
                    Style::default().fg(palette.accent_primary.into()),
                ),
                Span::styled(
                    format!("{:<7}", session.modified_at.format("%H:%M")),
                    Style::default().fg(palette.faint.into()),
                ),
                Span::styled(
                    format::fit(&session.project_dir, width.saturating_sub(20), true),
                    Style::default().fg(palette.text.into()),
                ),
            ]))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(
            Style::default()
                .bg(palette.border.into())
                .add_modifier(Modifier::BOLD),
        ),
        popup,
        &mut state,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell as BufferCell;

    use super::*;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    fn transcript(id: &str) -> TranscriptRef {
        TranscriptRef {
            path: format!("/tmp/{id}.jsonl").into(),
            session_id: id.to_owned(),
            project_dir: "/project".to_owned(),
            modified_at: chrono::Utc::now(),
            size_bytes: 0,
        }
    }

    #[test]
    fn the_overlay_clears_only_a_popup_smaller_than_the_full_area() {
        let screen = Rect::new(0, 0, 100, 40);
        let popup = centred(
            screen,
            screen.width.saturating_sub(8).max(20),
            screen.height.saturating_sub(4).max(6),
        );
        assert!(
            popup.width < screen.width,
            "the tab underneath still shows at the edges"
        );
        assert!(popup.height < screen.height);
    }

    #[test]
    fn the_picker_renders_every_session_without_panicking() {
        let sessions = [transcript("a"), transcript("b")];
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &sessions, 0, None, &palette()))
            .expect("draw succeeds");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(BufferCell::symbol)
            .collect();
        assert!(screen.contains("sessions"), "{screen}");
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &[], 0, None, &palette()))
            .expect("draw succeeds");
    }
}

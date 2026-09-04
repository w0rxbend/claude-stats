//! The layout picker overlay.
//!
//! The same Application Controller (Fowler, *`PoEAA`*) shape as
//! [`super::theme_picker`] and, before that, [`super::sessions`]: a bordered
//! list, `selected` highlighted, `App` moves it and confirms it. This picker
//! is the one that keeps `sessions`' original Enter-confirms/Esc-cancels
//! rhythm exactly -- unlike the theme picker, `L` alone does not also cycle a
//! selection on repeated presses, because switching the whole dashboard's
//! panel arrangement mid-keystroke reads as far more disruptive than
//! previewing a colour scheme does, so this picker asks for an explicit
//! Enter before it changes anything on screen.
//!
//! # Why some listed names do not yet change what is drawn
//!
//! `names` is `crate::tui::layout::presets::by_name`'s four built-in preset
//! names plus whatever custom trees the user has written under
//! `config.layouts`. Only the four presets are actually wired into
//! [`crate::tui::screens::dashboard::draw`]'s call site this epic touches --
//! see `App::confirm_layout_picker`'s doc comment for the honest account of
//! why a custom name still shows here, still persists to `config.json`, and
//! still does not (yet) change the live dashboard.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding};

use super::centred;
use crate::tui::palette::Palette;

/// Draws the picker, listing `names` with `selected` highlighted.
pub fn draw(frame: &mut Frame<'_>, area: Rect, names: &[&str], selected: usize, palette: &Palette) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border_active.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            " layout ",
            palette.title(palette.accent_primary.into()),
        ));

    let height = u16::try_from(names.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let popup = centred(area, 32, height);
    frame.render_widget(Clear, popup);

    let items: Vec<ListItem<'_>> = names
        .iter()
        .map(|name| {
            ListItem::new(Line::from(Span::styled(
                *name,
                Style::default().fg(palette.text.into()),
            )))
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(
        List::new(items).block(block).highlight_style(
            Style::default()
                .fg(palette.inverted_text.into())
                .bg(palette.accent_primary.into())
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

    use super::*;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    #[test]
    fn the_selected_row_is_styled_differently_from_the_rest() {
        let names = ["live", "spend", "minimal", "wide"];
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &names, 2, &palette()))
            .expect("draw succeeds");

        let buffer = terminal.backend().buffer();
        let popup = centred(Rect::new(0, 0, 40, 20), 32, 8);
        let inner = Block::bordered()
            .padding(Padding::horizontal(1))
            .inner(popup);

        let selected_style = &buffer[(inner.x, inner.y + 2)];
        let other_style = &buffer[(inner.x, inner.y)];

        assert_ne!(
            (selected_style.bg, selected_style.modifier),
            (other_style.bg, other_style.modifier),
            "the selected row must read differently from the rest: {selected_style:?} vs {other_style:?}"
        );
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let names = ["live"];
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &names, 0, &palette()))
            .expect("draw succeeds");
    }
}

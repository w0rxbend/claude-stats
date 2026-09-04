//! The theme picker overlay.
//!
//! An Application Controller (Fowler, *`PoEAA`*) over a flat list of theme
//! names, copied deliberately from [`super::sessions`]'s own shape: a
//! bordered list, `selected` highlighted, up/down moves it and Enter
//! confirms. `App` drives both the same way -- see
//! `crate::tui::app::App::handle`'s `MoveDown`/`MoveUp`/`Confirm` arms -- so
//! this module owns none of that state itself, only how to draw it. The one
//! way this picker departs from `sessions`' pattern is `t` itself: pressing
//! it again while the picker is already open advances `selected` and applies
//! the theme immediately, without waiting for Enter, which is `App`'s job to
//! drive (see [`crate::tui::keymap::NormalAction::CycleTheme`]) rather than
//! anything this module decides -- `draw` here only ever renders whatever
//! `selected` already is.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding};

use super::centred;
use crate::tui::palette::Palette;

/// Draws the picker, listing `names` with `selected` highlighted.
///
/// `names` is a plain `&[&str]` rather than something that reaches into
/// [`crate::tui::palette::registry::ThemeRegistry`] itself, so this module
/// stays a Template View (Fowler, *`PoEAA`*) over data its caller already
/// resolved -- the same reason [`super::sessions::draw`] takes a slice of
/// already-loaded sessions rather than a catalogue to query.
pub fn draw(frame: &mut Frame<'_>, area: Rect, names: &[&str], selected: usize, palette: &Palette) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border_active.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            " theme ",
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
        let names = ["aurora", "nord", "dracula"];
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &names, 1, &palette()))
            .expect("draw succeeds");

        let buffer = terminal.backend().buffer();
        let popup = centred(Rect::new(0, 0, 40, 20), 32, 7);
        let inner = Block::bordered()
            .padding(Padding::horizontal(1))
            .inner(popup);

        let selected_style = &buffer[(inner.x, inner.y + 1)];
        let other_style = &buffer[(inner.x, inner.y)];

        assert_ne!(
            (selected_style.bg, selected_style.modifier),
            (other_style.bg, other_style.modifier),
            "the selected row must read differently from the rest: {selected_style:?} vs {other_style:?}"
        );
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let names = ["aurora"];
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &names, 0, &palette()))
            .expect("draw succeeds");
    }
}

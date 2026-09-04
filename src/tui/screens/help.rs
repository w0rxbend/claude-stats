//! The keybinding overlay, and the "looking for a session" splash.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use super::centred;
use crate::tui::icons::Icon;
use crate::tui::keymap::{Group, Keymap};
use crate::tui::palette::Palette;
use crate::tui::widgets::spinner::{Spinner, SpinnerStyle};

/// Draws the help overlay centred over whatever is behind it.
///
/// Every row here comes from [`Keymap::help_rows`] -- there is no
/// hand-written list to fall out of step with the real bindings any more.
/// Before the keymap module existed this overlay had its own `KEYS`
/// constant, a second, independently-maintained summary of the same facts
/// `App`'s key handling already knew; see `crate::tui::keymap` for why that
/// duplication was worth removing.
pub fn draw(frame: &mut Frame<'_>, area: Rect, palette: &Palette, keymap: &Keymap) {
    let lines = help_lines(&keymap.help_rows(), palette);
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let popup = centred(area, 56, height);

    // Clearing first is what makes this an overlay rather than a transparency
    // effect: without it, the dashboard behind shows through the gaps between
    // the characters of this panel.
    frame.render_widget(Clear, popup);

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(palette.border_active.into()))
                .style(Style::default().bg(palette.surface.into()))
                .padding(Padding::uniform(1))
                .title(Span::styled(
                    " keys ",
                    palette.title(palette.accent_primary.into()),
                )),
        ),
        popup,
    );
}

/// Turns `Keymap::help_rows`' flat `(group, key, description)` triples into
/// display lines, inserting a heading each time the group changes.
///
/// `help_rows` already sorts by group, so a single pass that only opens a
/// new heading when the group actually differs from the previous row is
/// enough -- no separate grouping pass is needed.
fn help_lines<'a>(rows: &[(Group, &'a str, &'a str)], palette: &Palette) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    let mut current: Option<Group> = None;

    for (group, key, description) in rows {
        if current != Some(*group) {
            if current.is_some() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(
                group.label(),
                Style::default()
                    .fg(palette.muted.into())
                    .add_modifier(Modifier::BOLD),
            )));
            current = Some(*group);
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!("{key:>9}  "),
                Style::default()
                    .fg(palette.accent_primary.into())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(*description, Style::default().fg(palette.text.into())),
        ]));
    }

    lines
}

/// Draws the splash shown while no session has been found yet.
///
/// This is a real screen rather than a blank one because "nothing is
/// happening" and "the tool is broken" look identical otherwise, and the first
/// thing a new user does is run `claude-stats monitor` before starting a session.
pub fn draw_searching(frame: &mut Frame<'_>, area: Rect, phase: u64, palette: &Palette) {
    let spinner = Spinner::new(SpinnerStyle::Quadrant, phase / 2).glyph();
    let lines = vec![
        Line::from(Span::styled(
            "claude-stats",
            Style::default()
                .fg(palette.accent_primary.into())
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{spinner} looking for an active session"),
            Style::default().fg(palette.text.into()),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "{} start Claude Code in another terminal, or press o to pick one",
                Icon::BULLET
            ),
            Style::default().fg(palette.muted.into()),
        )),
    ];

    let popup = centred(area, 68, 9);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(palette.border.into()))
                .style(Style::default().bg(palette.surface.into()))
                .padding(Padding::uniform(1)),
        ),
        popup,
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

    #[test]
    fn every_group_the_keymap_uses_gets_its_own_heading() {
        let keymap = Keymap::default_bindings();
        let rows = keymap.help_rows();
        let lines = help_lines(&rows, &palette());

        let rendered: Vec<String> = lines.iter().map(ratatui::text::Line::to_string).collect();
        for group_label in [
            Group::Global.label(),
            Group::Motion.label(),
            Group::Jumps.label(),
            Group::Views.label(),
            Group::Panes.label(),
            Group::Appearance.label(),
            Group::Search.label(),
            Group::Command.label(),
        ] {
            assert!(
                rendered.contains(&group_label.to_owned()),
                "expected a heading for {group_label:?} among {rendered:?}"
            );
        }
    }

    #[test]
    fn the_overlay_renders_every_binding_without_panicking() {
        let keymap = Keymap::default_bindings();
        let mut terminal = Terminal::new(TestBackend::new(80, 60)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), &palette(), &keymap))
            .expect("draw succeeds");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            screen.contains("quit"),
            "the overlay should show the actual bindings"
        );
    }
}

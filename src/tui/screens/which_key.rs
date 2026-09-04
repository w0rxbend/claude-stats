//! The which-key popup: a small hint shown the instant `g` opens a chord,
//! naming what the next key could complete it into.
//!
//! Every row here is read straight out of [`Keymap::help_rows`] -- the same
//! Registry the help overlay itself reads (see `crate::tui::keymap`'s module
//! doc for why there is exactly one table these two screens, and the footer
//! hint, all read from) -- filtered down to the `g`-chords rather than
//! hand-written a second time. A test in this module (and a crate-wide grep
//! the acceptance criteria for this epic calls for) is what stands between
//! that promise and a second, quietly drifting copy of "gg jumps to the top"
//! showing up here.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::tui::keymap::Keymap;
use crate::tui::palette::Palette;

/// The rows this popup shows: every binding [`Keymap::help_rows`] lists whose
/// label is a two-key chord beginning with `g` -- `gg`, `gt`, `gT` today, and
/// whichever a future binding adds without a line of this module changing.
///
/// A `KeySeq::Two` chord's label is always exactly its two characters (see
/// `crate::tui::keymap::key_label`'s own `CHORD_LABELS` table), which is what
/// makes `label.len() == 2` a safe stand-in here for "this row came from a
/// `KeySeq::Two`" without `help_rows` having to hand back the `KeySeq` itself
/// -- its return type is deliberately just `(Group, &str, &str)`, the same
/// flat shape the help overlay and the footer hint already read.
fn chord_rows(keymap: &Keymap) -> Vec<(&str, &str)> {
    keymap
        .help_rows()
        .into_iter()
        .filter(|(_, label, _)| label.len() == 2 && label.starts_with('g'))
        .map(|(_, label, description)| (label, description))
        .collect()
}

/// Draws the popup, anchored just above `footer_y` (the top row of the
/// footer, in `screen`'s own coordinates) rather than centred in `screen` the
/// way [`super::help::draw`]'s overlay is -- a which-key hint is read while a
/// hand is still mid-chord, and putting it next to the row that already
/// explains what is pending (`App::status_badge`) is what makes the two read
/// as one piece of feedback rather than two unrelated panels.
pub fn draw(
    frame: &mut Frame<'_>,
    screen: Rect,
    footer_y: u16,
    palette: &Palette,
    keymap: &Keymap,
) {
    // Every line reads `"{label:>3} {description}"` -- a fixed four columns
    // for the label field (three, right-aligned, plus its trailing space)
    // and however long the description is -- inside a block whose border
    // and horizontal padding (`Padding::horizontal(1)`) each claim two more
    // columns on top of that. Getting this short is what earlier left a
    // description clipped by exactly the width of the padding this comment
    // now accounts for.
    const LABEL_FIELD: u16 = 4;
    const CHROME: u16 = 4; // two columns of border, two of padding

    let rows = chord_rows(keymap);
    if rows.is_empty() {
        return;
    }

    let content_width = rows
        .iter()
        .map(|(_, description)| description.chars().count() as u16)
        .max()
        .unwrap_or(0)
        + LABEL_FIELD;
    let width = content_width + CHROME;
    let height = u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2);

    let x = screen.x + screen.width.saturating_sub(width) / 2;
    let y = footer_y.saturating_sub(height).max(screen.y);
    let popup = Rect {
        x,
        y,
        width: width.min(screen.width),
        height: height.min(screen.height),
    };

    frame.render_widget(Clear, popup);

    let lines: Vec<Line<'_>> = rows
        .into_iter()
        .map(|(label, description)| {
            Line::from(vec![
                Span::styled(
                    format!("{label:>3} "),
                    Style::default().fg(palette.accent_primary.into()),
                ),
                Span::styled(description, Style::default().fg(palette.text.into())),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(palette.border_active.into()))
                .style(Style::default().bg(palette.surface.into()))
                .padding(Padding::horizontal(1)),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::keymap::Group;
    use crate::tui::palette::registry::ThemeRegistry;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    #[test]
    fn the_rows_shown_are_exactly_the_g_chords_help_rows_already_knows_about() {
        let keymap = Keymap::default_bindings();
        let expected: Vec<(&str, &str)> = keymap
            .help_rows()
            .into_iter()
            // `Group::Views` also lists `o` (`open the session picker`),
            // which is a single key, not a `g`-chord -- `label.len() == 2`
            // is what actually distinguishes "a two-key chord" here, the
            // same test `chord_rows` itself relies on.
            .filter(|(group, label, _)| {
                label.len() == 2 && (*group == Group::Views || *label == "gg")
            })
            .map(|(_, label, description)| (label, description))
            .collect();

        let mut actual = chord_rows(&keymap);
        let mut expected = expected;
        actual.sort_unstable();
        expected.sort_unstable();

        assert_eq!(
            actual, expected,
            "the popup must show exactly the g-chords help_rows lists, nothing hand-written"
        );
    }

    #[test]
    fn the_popup_renders_every_chord_label_without_panicking() {
        let keymap = Keymap::default_bindings();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 23, &palette(), &keymap))
            .expect("draw succeeds");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        for (label, _) in chord_rows(&keymap) {
            assert!(screen.contains(label), "{label} missing from {screen:?}");
        }
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let keymap = Keymap::default_bindings();
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 1, &palette(), &keymap))
            .expect("draw succeeds");
    }
}

//! The Blocks tab: the five-hour billing windows
//! [`crate::application::report_source::ReportSource::blocks`] produces from
//! the same [`crate::view::blocks_view::table`] Two Step View
//! `claude-stats blocks` prints from. See [`super::daily`]'s module doc for
//! the fuller account of why this screen only ever draws a [`TableView`] it
//! is handed rather than reading anything itself.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::tui::palette::Palette;
use crate::view::table::TableView;

use super::{draw_tab_bar, draw_table_view};

const EMPTY: &str = "no billing blocks available yet";

/// Draws the tab bar, then `view` as a bordered table (or [`EMPTY`] when
/// there is none).
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    tab_index: usize,
    view: Option<&TableView>,
    palette: &Palette,
) {
    let [tab_bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    draw_tab_bar(frame, tab_bar, tab_index, palette);
    draw_table_view(frame, body, "billing blocks", EMPTY, view, palette);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::palette::registry::ThemeRegistry;
    use crate::view::table::Column;

    fn palette() -> Palette {
        ThemeRegistry::builtin()
            .get("aurora")
            .expect("aurora is always registered")
            .clone()
    }

    fn fixture() -> TableView {
        TableView {
            title: "Claude Code Token Usage Report - Session Blocks".to_owned(),
            columns: vec![Column::left("Block Start"), Column::right("Cost")],
            rows: vec![vec!["2026-09-01 09:00".to_owned(), "$3.00".to_owned()]],
            totals: None,
        }
    }

    #[test]
    fn a_loaded_report_shows_its_figures() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 4, Some(&fixture()), &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("2026-09-01 09:00"));
    }

    #[test]
    fn with_nothing_loaded_the_empty_message_is_shown() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 4, None, &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("no billing blocks"));
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 4, Some(&fixture()), &palette()))
            .expect("draw succeeds");
    }
}

//! The Daily tab: usage grouped by calendar day, over the whole corpus.
//!
//! This is the intentionally simplest possible TUI rendering of an
//! already-existing view model: [`crate::view::usage_view::table`] is the
//! same Two Step View `claude-stats daily` already prints from, folded by
//! [`crate::application::report_source::ReportSource::daily`] rather than
//! recomputed here. `App` is what decides *when* to call `daily()` and holds
//! the result -- see `crate::tui::app::App::ensure_reports_loaded` -- so this
//! module only ever draws a [`TableView`] it is handed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::tui::palette::Palette;
use crate::view::table::TableView;

use super::{draw_tab_bar, draw_table_view};

/// What this tab says while nothing has been loaded yet: no
/// [`crate::application::report_source::ReportSource`] was wired in (every
/// test in this crate, and any run of `claude-stats monitor` whose own
/// `FileSystemCatalog::from_home` failed), or the corpus really is empty.
const EMPTY: &str = "no daily usage data available yet";

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
    draw_table_view(frame, body, "daily usage", EMPTY, view, palette);
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
            title: "token usage by date".to_owned(),
            columns: vec![Column::left("Date"), Column::right("Cost (USD)")],
            rows: vec![vec!["2026-09-01".to_owned(), "$1.00".to_owned()]],
            totals: Some(vec!["Total".to_owned(), "$1.00".to_owned()]),
        }
    }

    #[test]
    fn a_loaded_report_shows_its_figures() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 1, Some(&fixture()), &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("2026-09-01"));
        assert!(screen.contains("Dashboard"), "the tab bar is drawn too");
    }

    #[test]
    fn with_nothing_loaded_the_empty_message_is_shown_instead_of_a_blank_table() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 1, None, &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("no daily usage data"));
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 1, Some(&fixture()), &palette()))
            .expect("draw succeeds");
    }
}

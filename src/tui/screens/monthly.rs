//! The Monthly tab. See [`super::daily`]'s module doc -- this is the same
//! rendering over
//! [`crate::application::report_source::ReportSource::monthly`] instead.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::tui::palette::Palette;
use crate::view::table::TableView;

use super::{draw_tab_bar, draw_table_view};

const EMPTY: &str = "no monthly usage data available yet";

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
    draw_table_view(frame, body, "monthly usage", EMPTY, view, palette);
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
            title: "token usage by month".to_owned(),
            columns: vec![Column::left("Month"), Column::right("Cost (USD)")],
            rows: vec![vec!["2026-09".to_owned(), "$42.00".to_owned()]],
            totals: Some(vec!["Total".to_owned(), "$42.00".to_owned()]),
        }
    }

    #[test]
    fn a_loaded_report_shows_its_figures() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 3, Some(&fixture()), &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("2026-09"));
    }

    #[test]
    fn with_nothing_loaded_the_empty_message_is_shown() {
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 3, None, &palette()))
            .expect("draw succeeds");
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(screen.contains("no monthly usage data"));
    }

    #[test]
    fn drawing_into_a_tiny_area_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(3, 2)).expect("test backend");
        terminal
            .draw(|frame| draw(frame, frame.area(), 3, Some(&fixture()), &palette()))
            .expect("draw succeeds");
    }
}

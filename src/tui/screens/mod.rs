//! The full-screen views the dashboard can show.
//!
//! Each one takes a `Rect` and the data it needs, and draws. None of them own
//! state -- what is selected, how far the log is scrolled, which view is
//! showing -- because that all belongs to [`crate::tui::app::App`], and
//! spreading it across the screens is how a terminal application ends up with
//! two sources of truth about where the cursor is.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Padding, Paragraph, Row, Table, Wrap};

use crate::tui::palette::Palette;
use crate::view::table::TableView;

pub mod blocks;
pub mod daily;
pub mod dashboard;
pub mod help;
pub mod layout_picker;
pub mod log;
pub mod monthly;
pub mod sessions;
pub mod theme_picker;
pub mod weekly;
pub mod which_key;

/// A `width` x `height` rectangle centred in `area`, never larger than it.
///
/// Shared by every overlay that pops a bordered panel over the rest of the
/// screen -- [`help::draw`], [`help::draw_searching`],
/// [`theme_picker::draw`], [`layout_picker::draw`] and now [`sessions::draw`]
/// -- rather than each one carrying its own copy. It started as `help`'s own
/// private helper; the moment a second overlay needed the identical centring
/// maths this became duplication worth removing (Fowler, *Refactoring*'s
/// "Extract Function", applied one level up once a third caller was about to
/// repeat it a third time).
pub(super) fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(row);
    cell
}

/// The six persistent content tabs `gt`/`gT`/`{count}gt` cycle through, in
/// display order.
///
/// A plain literal rather than something derived from
/// [`crate::tui::app::View`] itself: that enum lives in `app`, which already
/// imports this module for every screen's `draw` function, and reaching back
/// the other way for six strings would be the one thing in this crate's
/// module graph that could not be drawn as a tree. Keeping the two in step is
/// instead a test's job -- see `crate::tui::app`'s own
/// `the_tab_bar_lists_every_content_view_by_its_one_based_number` -- the same
/// division of labour [`crate::tui::keymap::defaults`]'s `key_label` tables
/// use against a *different* enum for the same reason.
pub(super) const TAB_LABELS: [&str; 6] =
    ["Dashboard", "Daily", "Weekly", "Monthly", "Blocks", "Log"];

/// Draws the one-line tab bar every content view reserves its top row for,
/// `current` (zero-based into [`TAB_LABELS`]) picked out with
/// `palette.border_active`, the rest in `palette.muted`.
///
/// A shared helper rather than six near-identical copies of the same
/// span-building loop, for the same reason [`centred`] became one: six
/// screens is well past the Rule of Three.
pub(super) fn draw_tab_bar(frame: &mut Frame<'_>, area: Rect, current: usize, palette: &Palette) {
    let mut spans = Vec::with_capacity(TAB_LABELS.len());
    for (index, label) in TAB_LABELS.iter().enumerate() {
        let style = if index == current {
            Style::default()
                .fg(palette.border_active.into())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.muted.into())
        };
        spans.push(Span::styled(format!(" {} {label}  ", index + 1), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(palette.surface.into())),
        area,
    );
}

/// Draws `view` -- a [`TableView`] already built by the matching
/// `crate::view::*_view` Two Step View -- as a plain bordered
/// `ratatui::widgets::Table`, or `empty_message` when there is nothing to
/// show yet (no [`crate::application::report_source::ReportSource`] wired
/// in, or the corpus really is empty).
///
/// Shared by [`daily`], [`weekly`], [`monthly`] and [`blocks`] rather than
/// each screen carrying its own copy of the column-width and totals-row
/// logic -- the four are, deliberately, "the same table drawn four times",
/// exactly as [`crate::view::usage_view::table`] and
/// [`crate::view::blocks_view::table`] are themselves "the same aggregate
/// folded four ways" (see those modules' own doc comments).
///
/// This is the intentionally simplest possible rendering of an
/// already-built [`TableView`]: a multi-line cell (the models column can
/// carry more than one model) is flattened to a single comma-joined line
/// rather than given its own row height, and every column is sized to its
/// widest cell capped at twenty-eight characters, rather than the
/// narrow-terminal column-dropping rule [`TableView::render`]'s text
/// rendering applies. Both are real, disclosed simplifications a later epic
/// is free to improve on; neither loses a figure, only some formatting.
pub(super) fn draw_table_view(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    empty_message: &str,
    view: Option<&TableView>,
    palette: &Palette,
) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette.border.into()))
        .style(Style::default().bg(palette.surface.into()))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            palette.title(palette.accent_primary.into()),
        ));

    let Some(view) = view else {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(empty_message)
                .style(Style::default().fg(palette.muted.into()))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    };

    // Multi-line cells (a bucket that used more than one model) are flattened
    // to one comma-joined line: nothing in the figure is lost, only the
    // one-model-per-line formatting `TableView::render`'s text output gives
    // it, which a fixed-height table row has nowhere honest to put.
    let flatten = |cell: &str| cell.replace('\n', ", ");

    let mut widths: Vec<usize> = view
        .columns
        .iter()
        .map(|column| column.header.chars().count())
        .collect();
    for row in view.rows.iter().chain(view.totals.iter()) {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(flatten(cell).chars().count());
            }
        }
    }
    let constraints: Vec<Constraint> = widths
        .iter()
        .map(|width| Constraint::Length((*width).min(28) as u16))
        .collect();

    let header = Row::new(
        view.columns
            .iter()
            .map(|column| Cell::from(column.header.clone())),
    )
    .style(
        Style::default()
            .fg(palette.muted.into())
            .add_modifier(Modifier::BOLD),
    );

    let mut rows: Vec<Row<'_>> = view
        .rows
        .iter()
        .map(|row| Row::new(row.iter().map(|cell| Cell::from(flatten(cell)))))
        .collect();
    if let Some(totals) = &view.totals {
        rows.push(
            Row::new(totals.iter().map(|cell| Cell::from(flatten(cell)))).style(
                Style::default()
                    .fg(palette.accent_primary.into())
                    .add_modifier(Modifier::BOLD),
            ),
        );
    }

    frame.render_widget(
        Table::new(rows, constraints).header(header).block(block),
        area,
    );
}

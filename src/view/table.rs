//! A table as data: headers, alignments, cells, and a total underneath.
//!
//! Fowler's Two Step View, and the reason for the two steps is the whole point
//! of this module. The first step turns a domain aggregate into this -- a
//! structure of plain strings that knows nothing about terminals, colour, JSON
//! or ratatui. The second step renders it. Because the first step is the one
//! that decides *what the table says*, and the second only decides what it
//! looks like, a text report and any future renderer cannot disagree about the
//! contents: there is one place the contents are decided.
//!
//! It also makes the interesting half testable. Asserting on a
//! [`TableView`] needs no terminal, no width probe and no escape-sequence
//! parsing, and asserting on [`TableView::render`] needs only a string.

use std::fmt::Write as _;

/// Which edge of its column a cell is pushed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Text: names, dates, labels.
    Left,
    /// Figures, so the digits line up and a column can be added by eye.
    Right,
}

/// One column's heading and how its cells sit in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The heading, printed as given.
    pub header: String,
    /// Which edge the cells are pushed against.
    pub align: Align,
}

impl Column {
    /// A left-aligned column of text.
    #[must_use]
    pub fn left(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            align: Align::Left,
        }
    }

    /// A right-aligned column of figures.
    #[must_use]
    pub fn right(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            align: Align::Right,
        }
    }
}

/// A whole table, ready to be rendered or inspected.
///
/// Cells are strings because by this point every number has already been
/// formatted; keeping them typed would mean the renderer had to know about
/// money and token counts, which is exactly the knowledge this structure
/// exists to have finished with.
///
/// A cell may contain newlines. That is how the models column shows several
/// models on one row without a nested table: the renderer treats a row as
/// however many lines its tallest cell needs and pads the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableView {
    /// The line printed above the table.
    pub title: String,
    /// The columns, left to right.
    pub columns: Vec<Column>,
    /// The body, one entry per row, each as wide as [`Self::columns`].
    pub rows: Vec<Vec<String>>,
    /// The row printed below the separator, when there is one.
    pub totals: Option<Vec<String>>,
}

impl TableView {
    /// The table as plain text.
    ///
    /// Every column is padded to its widest cell, columns are separated by two
    /// spaces, the header is underlined with dashes and the totals row sits
    /// below a second rule. Two spaces rather than a pipe or a box character
    /// because the output is routinely pasted into an issue or diffed against
    /// yesterday's, and both of those are easier when the table is only
    /// spaces.
    ///
    /// Trailing spaces are trimmed from every line. A left-aligned final
    /// column would otherwise pad every row out to the same width with
    /// invisible characters, which is noise in a diff and an editor's warning
    /// in a paste.
    #[must_use]
    pub fn render(&self) -> String {
        let widths = self.column_widths();
        let mut out = String::new();

        if !self.title.is_empty() {
            let _ = writeln!(out, "{}\n", self.title);
        }

        let headers: Vec<String> = self
            .columns
            .iter()
            .map(|column| column.header.clone())
            .collect();
        self.write_row(&mut out, &headers, &widths);
        write_rule(&mut out, &widths);
        for row in &self.rows {
            self.write_row(&mut out, row, &widths);
        }
        if let Some(totals) = &self.totals {
            write_rule(&mut out, &widths);
            self.write_row(&mut out, totals, &widths);
        }
        out
    }

    /// How wide each column has to be to hold its widest line.
    ///
    /// Measured in characters rather than bytes, because a project called
    /// `café` is four columns wide on screen and five bytes long, and padding
    /// by the second would leave the column beside it one space short.
    fn column_widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .map(|column| column.header.chars().count())
            .collect();
        let bodies = self.rows.iter().chain(self.totals.iter());
        for row in bodies {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(widest_line(cell));
                }
            }
        }
        widths
    }

    /// Writes one row, which may be several lines tall.
    fn write_row(&self, out: &mut String, cells: &[String], widths: &[usize]) {
        let height = cells.iter().map(|cell| cell.lines().count().max(1)).max();
        for line in 0..height.unwrap_or(1) {
            let mut rendered = String::new();
            for (index, width) in widths.iter().enumerate() {
                if index > 0 {
                    rendered.push_str(COLUMN_GAP);
                }
                let text = cells
                    .get(index)
                    .and_then(|cell| cell.lines().nth(line))
                    .unwrap_or("");
                let align = self
                    .columns
                    .get(index)
                    .map_or(Align::Left, |column| column.align);
                pad(&mut rendered, text, *width, align);
            }
            let _ = writeln!(out, "{}", rendered.trim_end());
        }
    }
}

/// What separates one column from the next.
const COLUMN_GAP: &str = "  ";

/// Writes the dashed rule under the headers and above the totals.
fn write_rule(out: &mut String, widths: &[usize]) {
    let rule: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    let _ = writeln!(out, "{}", rule.join(COLUMN_GAP));
}

/// The width of the longest line in a possibly multi-line cell.
fn widest_line(cell: &str) -> usize {
    cell.lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
}

/// Pushes `text` into `out`, padded to `width` on the side `align` asks for.
fn pad(out: &mut String, text: &str, width: usize, align: Align) {
    let spare = width.saturating_sub(text.chars().count());
    if align == Align::Right {
        for _ in 0..spare {
            out.push(' ');
        }
    }
    out.push_str(text);
    if align == Align::Left {
        for _ in 0..spare {
            out.push(' ');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> TableView {
        TableView {
            title: "token usage by date".to_owned(),
            columns: vec![Column::left("Date"), Column::right("Cost (USD)")],
            rows: vec![
                vec!["2026-09-01".to_owned(), "$1.00".to_owned()],
                vec!["2026-09-02".to_owned(), "$123.45".to_owned()],
            ],
            totals: Some(vec!["Total".to_owned(), "$124.45".to_owned()]),
        }
    }

    #[test]
    fn a_column_is_as_wide_as_its_widest_cell_including_the_header() {
        let rendered = table().render();
        assert_eq!(
            rendered,
            "token usage by date\n\
             \n\
             Date        Cost (USD)\n\
             ----------  ----------\n\
             2026-09-01       $1.00\n\
             2026-09-02     $123.45\n\
             ----------  ----------\n\
             Total          $124.45\n"
        );
    }

    #[test]
    fn a_cell_with_newlines_becomes_a_row_several_lines_tall() {
        let view = TableView {
            title: String::new(),
            columns: vec![Column::left("Models"), Column::right("Cost (USD)")],
            rows: vec![vec![
                "\u{2022} Opus 5\n\u{2022} Sonnet 5".to_owned(),
                "$9.00".to_owned(),
            ]],
            totals: None,
        };
        assert_eq!(
            view.render(),
            "Models      Cost (USD)\n\
             ----------  ----------\n\
             \u{2022} Opus 5         $9.00\n\
             \u{2022} Sonnet 5\n"
        );
    }

    #[test]
    fn a_table_with_no_totals_has_no_second_rule() {
        let mut view = table();
        view.totals = None;
        assert_eq!(
            view.render().matches("----------  ----------").count(),
            1,
            "only the header is underlined"
        );
    }

    #[test]
    fn a_wide_cell_is_measured_in_characters_rather_than_bytes() {
        // `café` is four columns on screen and five bytes long. Padding by
        // bytes would leave the next column a space short on every row that
        // held an accent.
        let view = TableView {
            title: String::new(),
            columns: vec![Column::left("P"), Column::left("N")],
            rows: vec![
                vec!["caf\u{e9}".to_owned(), "x".to_owned()],
                vec!["abcd".to_owned(), "y".to_owned()],
            ],
            totals: None,
        };
        let rendered = view.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[2].chars().count(), lines[3].chars().count());
    }
}

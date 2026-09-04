//! Turning a [`UsageReport`] into the table a period command prints.
//!
//! The first half of a Two Step View: this decides what the table *says* --
//! which columns, in which order, with which figures in them -- and
//! [`TableView::render`] decides what it looks like. Nothing here writes an
//! escape sequence or asks a terminal anything, which is what lets the whole
//! column layout, including the narrow-terminal rule, be asserted in a unit
//! test.

use crate::domain::model::{ModelCatalog, ModelId};
use crate::domain::report::{UsageReport, UsageRow};

use super::format;
use super::table::{Align, Column, TableView};

/// Below this many columns the cache figures are dropped.
///
/// The full table needs a little over a hundred characters once every count is
/// printed in full with separators. Wrapping it instead would be worse than
/// dropping columns: a wrapped table stops lining up, and a table that does
/// not line up cannot be read down a column, which is the only way anybody
/// reads one.
///
/// It is a parameter of [`table`] rather than something measured here on
/// purpose. Asking crossterm how wide the terminal is would make the layout
/// untestable and would make the function useless to anything that is not a
/// terminal -- a file, a pipe, a fixed-width paste into an issue.
pub const COMPACT_BELOW_COLUMNS: usize = 100;

/// The table for `report`.
///
/// `first_column` names the leftmost column -- `Date`, `Week`, `Month`,
/// `Project`, `Session` -- because the shape of the table is identical for all
/// of them and only the heading differs. `breakdown` adds a sub-row per model
/// beneath each row. `width` is how many columns the output has room for; see
/// [`COMPACT_BELOW_COLUMNS`].
///
/// The columns are, in order: the first column, `Models`, `Input`, `Output`,
/// `Cache Create`, `Cache Read`, `Total Tokens`, `Cost (USD)`, and a trailing
/// `Last Activity` when any row has one. `Cache Create` is the two cache
/// leases added together: they are priced sixty per cent apart, but they
/// occupy the same space and this column is about size, so splitting it here
/// would put a pricing fact in a column of sizes.
#[must_use]
pub fn table(report: &UsageReport, first_column: &str, breakdown: bool, width: usize) -> TableView {
    let compact = width < COMPACT_BELOW_COLUMNS;
    let with_activity = report.rows.iter().any(|row| row.last_activity_at.is_some());

    let mut columns = vec![Column::left(first_column), Column::left("Models")];
    columns.push(Column::right("Input"));
    columns.push(Column::right("Output"));
    if !compact {
        columns.push(Column::right("Cache Create"));
        columns.push(Column::right("Cache Read"));
        columns.push(Column::right("Total Tokens"));
    }
    columns.push(Column::right("Cost (USD)"));
    if with_activity {
        columns.push(Column {
            header: "Last Activity".to_owned(),
            align: Align::Left,
        });
    }

    let mut rows = Vec::with_capacity(report.rows.len());
    for row in &report.rows {
        rows.push(cells(
            &label(row),
            &models_cell(&row.models),
            row,
            compact,
            with_activity,
        ));
        if breakdown {
            for share in &row.breakdown {
                let mut sub = cells(
                    &format!("{SUB_ROW_PREFIX}{}", display_name(&share.model)),
                    "",
                    row,
                    compact,
                    with_activity,
                );
                overwrite_figures(&mut sub, share.tokens, share.cost, compact);
                // The sub-row is built from its parent, so it arrives carrying
                // the parent's activity stamp, and that stamp is not a fact
                // about this model. The aggregate records first and last
                // activity per *bucket*; a session that used Sonnet in the
                // morning and Opus all evening has one last-activity time, and
                // printing it against the Sonnet sub-row would state outright
                // that Sonnet was still running at midnight. An empty cell
                // says the only true thing there is to say here.
                if with_activity {
                    if let Some(activity) = sub.last_mut() {
                        activity.clear();
                    }
                }
                rows.push(sub);
            }
        }
    }

    // The totals row deliberately leaves the models cell empty. The union of
    // every model used is already visible in the column above it, and putting
    // it here would make the last row of a month's table a dozen lines tall
    // for information the reader has just scrolled past.
    let totals = cells("Total", "", &report.totals, compact, with_activity);

    TableView {
        title: format!("token usage by {}", first_column.to_lowercase()),
        columns,
        rows,
        totals: Some(totals),
    }
}

/// What a breakdown sub-row's first cell begins with.
///
/// Indented and drawn with a corner so the eye reads it as belonging to the
/// row above rather than as another bucket.
const SUB_ROW_PREFIX: &str = "  \u{2514}\u{2500} ";

/// The cells of one row, before any breakdown figures are substituted in.
fn cells(
    label: &str,
    models: &str,
    row: &UsageRow,
    compact: bool,
    with_activity: bool,
) -> Vec<String> {
    let mut cells = vec![label.to_owned(), models.to_owned()];
    cells.push(format::grouped(row.tokens.input));
    cells.push(format::grouped(row.tokens.output));
    if !compact {
        cells.push(format::grouped(row.tokens.cache_creation()));
        cells.push(format::grouped(row.tokens.cache_read));
        cells.push(format::grouped(row.tokens.total()));
    }
    cells.push(format::money(row.cost));
    if with_activity {
        cells.push(
            row.last_activity_at
                .map_or_else(String::new, |at| at.format(ACTIVITY_STAMP).to_string()),
        );
    }
    cells
}

/// How a last-activity stamp is written.
///
/// In UTC, because this function is given no zone. That is a deliberate
/// limitation rather than an oversight: the column exists so that two sessions
/// can be told apart and ordered, which a single consistent zone does
/// perfectly well, and threading a zone in only to render one column would put
/// a calendar decision in the renderer where the aggregate has already made
/// it.
const ACTIVITY_STAMP: &str = "%Y-%m-%d %H:%M";

/// Replaces a sub-row's figures with one model's share of them.
///
/// The sub-row is built from the parent so that the column count and the
/// compact rule cannot drift apart; only the numbers differ, and this is where
/// they are put in. Indices rather than named fields because the column list
/// is data, and a second list of names here would be a second thing to keep in
/// step with it.
fn overwrite_figures(
    cells: &mut [String],
    tokens: crate::domain::tokens::TokenUsage,
    cost: crate::domain::money::Usd,
    compact: bool,
) {
    let mut figures = vec![
        format::grouped(tokens.input),
        format::grouped(tokens.output),
    ];
    if !compact {
        figures.push(format::grouped(tokens.cache_creation()));
        figures.push(format::grouped(tokens.cache_read));
        figures.push(format::grouped(tokens.total()));
    }
    figures.push(format::money(cost));
    for (offset, figure) in figures.into_iter().enumerate() {
        if let Some(cell) = cells.get_mut(FIRST_FIGURE_COLUMN + offset) {
            *cell = figure;
        }
    }
}

/// Where the numeric columns begin: after the label and the models cell.
const FIRST_FIGURE_COLUMN: usize = 2;

/// What goes in a row's first cell.
///
/// Whichever parts of the row's identity are present, in the order a reader
/// narrows down: when it happened, where, and in which conversation. A daily
/// report has only the first, a session report only the last, and a
/// daily-per-project report has two -- and all three come out of the same
/// rule rather than out of three special cases.
fn label(row: &UsageRow) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !row.key.is_none() {
        parts.push(row.key.to_string());
    }
    if let Some(project) = &row.project {
        parts.push(project.display_name().to_owned());
    }
    if let Some(session) = &row.session {
        parts.push(format::session_id(session.as_str()).to_owned());
    }
    if parts.is_empty() {
        // A report with no grouping at all is one row covering everything.
        return "All".to_owned();
    }
    parts.join(" ")
}

/// The models cell: one model per line, in the order they were first seen.
fn models_cell(models: &[ModelId]) -> String {
    models
        .iter()
        .map(|model| format!("\u{2022} {}", display_name(model)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A model's short name, or its raw id when the catalogue has never heard of
/// it.
///
/// Read from [`ModelCatalog`] rather than from the run's price sheet because
/// this function is not given one. A user's override file can change what a
/// model *costs*, which is what the figures beside this cell are made of; it
/// changing what a model is *called* is a cosmetic difference that is not
/// worth widening this signature for.
fn display_name(model: &ModelId) -> String {
    ModelCatalog::display_name_for(model.as_str())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::period::{AggregationPeriod, GroupingSpec, Zone};
    use crate::domain::pricing::{CostMode, PriceSheet};
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn entry(id: &str, when: &str, model: &str, tokens: TokenUsage) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new("session-a"),
            },
            at: at(when),
            model: ModelId::new(model),
            tokens,
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// Two fixed days of traffic, chosen so every figure can be checked by
    /// hand: Opus 5 charges $5 per million input, $25 per million output,
    /// $0.50 per million cache read and $6.25 per million five-minute writes.
    fn fixture() -> UsageReport {
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-opus-5",
                TokenUsage {
                    input: 1_000_000,
                    cache_read: 2_000_000,
                    cache_write_5m: 400_000,
                    cache_write_1h: 0,
                    output: 200_000,
                },
            ),
            entry(
                "b",
                "2026-09-02T09:00:00Z",
                "claude-sonnet-5",
                TokenUsage {
                    input: 500_000,
                    cache_read: 1_000_000,
                    cache_write_5m: 100_000,
                    cache_write_1h: 0,
                    output: 50_000,
                },
            ),
        ];
        UsageReport::build(
            &entries,
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                ..GroupingSpec::default()
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
    }

    #[test]
    fn the_daily_table_renders_the_documented_columns_in_order() {
        let rendered = table(&fixture(), "Date", false, 200).render();
        assert_eq!(rendered, GOLDEN_DAILY_TABLE);
    }

    /// The whole table, spelled out.
    ///
    /// A golden literal rather than a handful of `contains` assertions because
    /// the column *order* and the alignment are the parts most likely to be
    /// broken by accident, and a substring check can see neither.
    ///
    /// Every figure in it can be checked by hand against the published rates.
    /// Opus 5: a million input at $5, two million cache reads at $0.50, four
    /// hundred thousand five-minute writes at $6.25 and two hundred thousand
    /// output at $25 come to $5.00 + $1.00 + $2.50 + $5.00 = $13.50. Sonnet 5
    /// at $2/$10 with a fifth of the traffic comes to $1.00 + $0.20 + $0.25 +
    /// $0.50 = $1.95. The two together are $15.45, which is what the totals
    /// row says.
    const GOLDEN_DAILY_TABLE: &str = r"token usage by date

Date        Models          Input   Output  Cache Create  Cache Read  Total Tokens  Cost (USD)
----------  ----------  ---------  -------  ------------  ----------  ------------  ----------
2026-09-01  • Opus 5    1,000,000  200,000       400,000   2,000,000     3,600,000      $13.50
2026-09-02  • Sonnet 5    500,000   50,000       100,000   1,000,000     1,650,000       $1.95
----------  ----------  ---------  -------  ------------  ----------  ------------  ----------
Total                   1,500,000  250,000       500,000   3,000,000     5,250,000      $15.45
";

    #[test]
    fn a_narrow_terminal_drops_the_cache_columns_rather_than_wrapping_the_table() {
        let wide = table(&fixture(), "Date", false, 200);
        let narrow = table(&fixture(), "Date", false, 80);

        let headers = |view: &TableView| -> Vec<String> {
            view.columns
                .iter()
                .map(|column| column.header.clone())
                .collect()
        };
        assert_eq!(
            headers(&wide),
            vec![
                "Date",
                "Models",
                "Input",
                "Output",
                "Cache Create",
                "Cache Read",
                "Total Tokens",
                "Cost (USD)"
            ]
        );
        assert_eq!(
            headers(&narrow),
            vec!["Date", "Models", "Input", "Output", "Cost (USD)"]
        );
        for line in narrow.render().lines() {
            assert!(
                line.chars().count() <= 80,
                "{line:?} is wider than the terminal"
            );
        }
        assert!(
            wide.render().contains("Cache Read"),
            "the wide table keeps them"
        );
    }

    #[test]
    fn the_compact_threshold_is_a_parameter_rather_than_a_terminal_probe() {
        // Exactly at the threshold the full table is still drawn; one column
        // narrower and it is not. Pinned because the rule is off-by-one prone
        // and because a test that cannot set the width would have to run in a
        // terminal.
        assert_eq!(
            table(&fixture(), "Date", false, COMPACT_BELOW_COLUMNS)
                .columns
                .len(),
            8
        );
        assert_eq!(
            table(&fixture(), "Date", false, COMPACT_BELOW_COLUMNS - 1)
                .columns
                .len(),
            5
        );
    }

    #[test]
    fn a_breakdown_row_is_an_ordinary_row_with_a_marked_first_cell() {
        let mixed = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-sonnet-5",
                TokenUsage {
                    input: 2_000_000,
                    ..TokenUsage::ZERO
                },
            ),
            entry(
                "b",
                "2026-09-01T10:00:00Z",
                "claude-opus-5",
                TokenUsage {
                    input: 1_000_000,
                    ..TokenUsage::ZERO
                },
            ),
        ];
        let report = UsageReport::build(
            &mixed,
            &GroupingSpec {
                period: Some(AggregationPeriod::Day),
                ..GroupingSpec::default()
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        let view = table(&report, "Date", true, 200);

        assert_eq!(view.rows.len(), 3, "the day plus one row per model");
        assert_eq!(
            view.rows[1][0], "  \u{2514}\u{2500} Opus 5",
            "dearest first"
        );
        assert_eq!(view.rows[2][0], "  \u{2514}\u{2500} Sonnet 5");
        assert_eq!(
            view.rows[1][1], "",
            "a sub-row leaves the models cell empty"
        );
        assert_eq!(view.rows[1][2], "1,000,000", "and carries its own figures");
        assert_eq!(view.rows[2][2], "2,000,000");
    }

    #[test]
    fn a_session_table_gains_a_last_activity_column() {
        let entries = [entry(
            "a",
            "2026-09-01T17:30:00Z",
            "claude-opus-5",
            TokenUsage {
                input: 1_000,
                ..TokenUsage::ZERO
            },
        )];
        let report = UsageReport::build(
            &entries,
            &GroupingSpec {
                period: None,
                by_project: false,
                by_session: true,
                order: crate::domain::period::Order::Ascending,
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        let view = table(&report, "Session", false, 200);

        assert_eq!(
            view.columns.last().map(|column| column.header.as_str()),
            Some("Last Activity")
        );
        assert_eq!(
            view.rows[0][0], "api session-",
            "the directory the conversation was started in, then its id \
             shortened to eight characters"
        );
        assert_eq!(
            view.rows[0].last().map(String::as_str),
            Some("2026-09-01 17:30")
        );
    }

    #[test]
    fn a_breakdown_sub_row_claims_no_last_activity_of_its_own() {
        // The session used Sonnet in the morning and Opus in the evening. The
        // aggregate knows when the *session* was last busy and nothing about
        // when either model was, so a stamp on the Sonnet sub-row would say
        // Sonnet was still running at a quarter to six, which it was not.
        let entries = [
            entry(
                "a",
                "2026-09-01T09:00:00Z",
                "claude-sonnet-5",
                TokenUsage {
                    input: 2_000_000,
                    ..TokenUsage::ZERO
                },
            ),
            entry(
                "b",
                "2026-09-01T17:45:00Z",
                "claude-opus-5",
                TokenUsage {
                    input: 1_000_000,
                    ..TokenUsage::ZERO
                },
            ),
        ];
        let report = UsageReport::build(
            &entries,
            &GroupingSpec {
                period: None,
                by_project: false,
                by_session: true,
                order: crate::domain::period::Order::Ascending,
            },
            &Zone::Utc,
            CostMode::Calculate,
            &PriceSheet::builtin(),
        );
        let view = table(&report, "Session", true, 200);

        assert_eq!(
            view.rows[0].last().map(String::as_str),
            Some("2026-09-01 17:45"),
            "the session row still carries the stamp"
        );
        for sub in &view.rows[1..] {
            assert_eq!(
                sub.last().map(String::as_str),
                Some(""),
                "{:?} must not borrow the session's stamp",
                sub[0]
            );
        }
    }

    #[test]
    fn the_models_cell_shows_one_model_per_line_in_first_seen_order() {
        let cell = models_cell(&[
            ModelId::new("claude-sonnet-5"),
            ModelId::new("claude-opus-5"),
        ]);
        assert_eq!(cell, "\u{2022} Sonnet 5\n\u{2022} Opus 5");
    }
}

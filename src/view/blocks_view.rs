//! Turning billing blocks into the table `claude-stats blocks` prints.
//!
//! The first half of a Two Step View, exactly as [`super::usage_view`] is:
//! this decides what the table *says* and [`TableView::render`] decides what
//! it looks like. Nothing here asks a terminal or a host clock anything -- the
//! width and the zone both arrive as arguments -- which is what lets the whole
//! layout, including the narrow rule and the two rows that hang under a
//! running block, be asserted in a unit test that reads the same on every
//! machine.

use crate::application::blocks_report::BlockRow;
use crate::domain::blocks::{BillingBlock, BlockKind};
use crate::domain::model::{ModelCatalog, ModelId};
use crate::domain::period::Zone;

use super::format;
use super::table::{Column, TableView};
use super::usage_view::COMPACT_BELOW_COLUMNS;

/// The heading the table carries.
///
/// Spelled here rather than assembled by the caller because, unlike the period
/// tables, there is only ever one blocks report and its name is not a
/// parameter of anything.
const TITLE: &str = "Claude Code Token Usage Report - Session Blocks";

/// The table for `rows`, with block starts written on `zone`'s clock.
///
/// The columns are, in order: `Block Start`, `Duration/Status`, `Models`,
/// `Tokens`, `[%]` and `Cost`. The percentage column appears only when a token
/// limit was resolved, because a percentage of nothing is not a figure. Below
/// [`COMPACT_BELOW_COLUMNS`] the `Tokens` and `[%]` columns are dropped rather
/// than the table being wrapped: a wrapped table stops lining up, and a table
/// that does not line up cannot be read down a column.
///
/// A running block is followed by two rows that belong to it rather than to
/// any window of their own -- what is left of the allowance, and where the
/// current rate is heading -- which is why they carry their assumption in the
/// first cell instead of a start time.
#[must_use]
pub fn table(rows: &[BlockRow], zone: &Zone, width: usize) -> TableView {
    let compact = width < COMPACT_BELOW_COLUMNS;
    // Taken from the rows rather than passed in: every row of one report
    // carries the same ceiling, and a column that appeared or vanished
    // depending on which row was looked at would be a table with two shapes.
    let limit = rows.iter().find_map(|row| row.limit);
    let layout = Layout {
        compact,
        with_percent: limit.is_some(),
    };

    let mut columns = vec![
        Column::left("Block Start"),
        Column::left("Duration/Status"),
        Column::left("Models"),
    ];
    if !compact {
        columns.push(Column::right("Tokens"));
        if layout.with_percent {
            columns.push(Column::right("[%]"));
        }
    }
    columns.push(Column::right("Cost"));

    let mut body = Vec::with_capacity(rows.len());
    for row in rows {
        body.push(block_cells(&row.block, zone, limit, layout));
        if row.block.kind == BlockKind::Active {
            if let Some(limit) = limit {
                // The remaining row is a token count and a percentage and
                // nothing else, so the narrow layout, which has dropped both
                // columns, has nowhere to put it. Emitted anyway it would be a
                // thirty-four character label announcing a figure that never
                // arrives -- and, because the first column is as wide as its
                // widest cell, it would push every block start on the table
                // sideways to do it, on the one terminal short of room. The
                // projected row keeps its place because it still carries a
                // cost.
                if !layout.compact {
                    body.push(remaining_cells(&row.block, limit, layout));
                }
                if let Some(projection) = row.projection {
                    body.push(projected_cells(&projection, limit, layout));
                }
            }
        }
    }

    TableView {
        title: TITLE.to_owned(),
        columns,
        rows: body,
        // No totals row, deliberately. Two of the three kinds of row under
        // this heading are not windows of work at all -- a gap holds nothing
        // and a projection has not happened yet -- so a column of figures
        // added up here would be a number that ties out to nothing a reader
        // could point at.
        totals: None,
    }
}

/// Which of the optional columns this table has.
///
/// Passed around as one value rather than as two booleans so that a cell
/// builder cannot be handed them the wrong way round.
#[derive(Debug, Clone, Copy)]
struct Layout {
    compact: bool,
    with_percent: bool,
}

impl Layout {
    /// Appends the figure columns a row ends with, in the order [`table`]
    /// declared them.
    ///
    /// One place decides which figures are present, so a row can never be
    /// built with a column the header does not have.
    fn figures(self, cells: &mut Vec<String>, tokens: &str, percent: &str, cost: &str) {
        if !self.compact {
            cells.push(tokens.to_owned());
            if self.with_percent {
                cells.push(percent.to_owned());
            }
        }
        cells.push(cost.to_owned());
    }
}

/// What a cell holds when there is nothing true to put in it.
const NOTHING: &str = "-";

/// One block's own row.
fn block_cells(
    block: &BillingBlock,
    zone: &Zone,
    limit: Option<u64>,
    layout: Layout,
) -> Vec<String> {
    if block.kind == BlockKind::Gap {
        // A gap has no start worth printing, no models, no tokens and no cost.
        // Every cell but the status says so outright rather than being left
        // blank, because a blank cell reads as a figure that failed to render.
        let mut cells = vec![
            NOTHING.to_owned(),
            "(inactive)".to_owned(),
            NOTHING.to_owned(),
        ];
        layout.figures(&mut cells, NOTHING, NOTHING, NOTHING);
        return cells;
    }

    let mut cells = vec![
        start_stamp(block, zone),
        status(block),
        models_cell(&block.models),
    ];
    layout.figures(
        &mut cells,
        &format::grouped(block.tokens.total()),
        &share(block.tokens.total(), limit),
        &format::money(block.cost),
    );
    cells
}

/// What is left of the allowance, as a row under the running block.
fn remaining_cells(block: &BillingBlock, limit: u64, layout: Layout) -> Vec<String> {
    let left = limit.saturating_sub(block.tokens.total());
    let mut cells = vec![
        format!("(assuming {} token limit)", format::grouped(limit)),
        "REMAINING".to_owned(),
        String::new(),
    ];
    layout.figures(
        &mut cells,
        &format::grouped(left),
        &share(left, Some(limit)),
        // The allowance is measured in tokens; what the rest of it will cost
        // depends on which model spends it, and this row is not entitled to
        // guess.
        "",
    );
    cells
}

/// Where the current rate lands, as a row under the running block.
fn projected_cells(
    projection: &crate::domain::blocks::Projection,
    limit: u64,
    layout: Layout,
) -> Vec<String> {
    let mut cells = vec![
        "(assuming current burn rate)".to_owned(),
        "PROJECTED".to_owned(),
        String::new(),
    ];
    layout.figures(
        &mut cells,
        &format::grouped(projection.total_tokens),
        &share(projection.total_tokens, Some(limit)),
        &format::money(projection.cost),
    );
    cells
}

/// A count as a percentage of the ceiling, or nothing when there is none.
fn share(tokens: u64, limit: Option<u64>) -> String {
    limit.map_or_else(String::new, |limit| {
        format::percent(tokens as f64 / limit as f64)
    })
}

/// What goes in the status cell of a real block.
///
/// A finished block reports how long it ran, measured from the hour it was
/// anchored to rather than from its first response: that hour is what the
/// allowance was consumed from, and it is the figure the start beside it
/// already names.
fn status(block: &BillingBlock) -> String {
    if block.kind == BlockKind::Active {
        return "ACTIVE".to_owned();
    }
    block.last_activity_at.map_or_else(
        || NOTHING.to_owned(),
        |last| format::duration(last - block.started_at),
    )
}

/// When a block opened, on the clock the report is being measured against.
///
/// Written on the reporting zone rather than in UTC, which is the opposite of
/// the choice [`super::usage_view`] makes for its activity column, and
/// deliberately so. A billing block is a wall-clock window: it is anchored to
/// the top of an hour precisely because that is when the allowance resets, and
/// somebody reading "when does this reset" needs the answer on the clock they
/// are looking at. A daily table's stamp, by contrast, only has to order two
/// sessions, which any single consistent zone does.
///
/// The zone is a parameter rather than [`chrono::Local`] read here, for the
/// reason [`crate::domain::period`] gives at length: only the composition root
/// is allowed to know that an environment exists. Reading the host's offset in
/// this function would also make `blocks --timezone Asia/Tokyo` bound its
/// entries on Tokyo days and then print their start times on the machine's own
/// clock, which is a table quietly measured on two calendars at once.
fn start_stamp(block: &BillingBlock, zone: &Zone) -> String {
    zone.wall_clock(block.started_at)
        .format(BLOCK_STAMP)
        .to_string()
}

/// How a block's start is written: minutes, no seconds, no zone suffix.
const BLOCK_STAMP: &str = "%Y-%m-%d %H:%M";

/// The models cell: one model per line, in the order they were first seen.
///
/// Named from [`ModelCatalog`] rather than from the run's price sheet, exactly
/// as [`super::usage_view`] does and for the same reason: an override file
/// changes what a model *costs*, which is what the figures beside this cell are
/// made of, and what it is *called* is a cosmetic difference not worth widening
/// this signature for.
fn models_cell(models: &[ModelId]) -> String {
    models
        .iter()
        .map(|model| {
            format!(
                "\u{2022} {}",
                ModelCatalog::display_name_for(model.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, Utc};

    use super::*;
    use crate::application::blocks_report::BlockRow;
    use crate::domain::blocks::{self, DEFAULT_SPAN_HOURS};
    use crate::domain::entry::{Entry, EntryId};
    use crate::domain::pricing::{CostMode, PriceSheet};
    use crate::domain::project::{Project, SessionId};
    use crate::domain::tokens::TokenUsage;

    fn at(stamp: &str) -> DateTime<Utc> {
        stamp.parse().expect("a valid timestamp")
    }

    fn entry(id: &str, when: &str, input: u64) -> Entry {
        Entry {
            id: EntryId {
                message_id: id.to_owned(),
                request_id: Some(format!("req_{id}")),
                session: SessionId::new("session-a"),
            },
            at: at(when),
            model: ModelId::new("claude-opus-5"),
            tokens: TokenUsage {
                input,
                ..TokenUsage::ZERO
            },
            recorded_cost: None,
            session: SessionId::new("session-a"),
            project: Project::new("/home/ada/api"),
            is_sidechain: false,
        }
    }

    /// The instant the fixture is read at: an hour into the second block's
    /// window, which runs from 09:00 to 14:00.
    fn now() -> DateTime<Utc> {
        at("2026-09-02T10:00:00Z")
    }

    /// A finished block, a long silence and a running one, chosen so that
    /// every figure below can be checked by hand.
    ///
    /// The first block holds two responses of five hundred thousand Opus 5
    /// input tokens: a million tokens at $5 a million, so $5.00, running from
    /// 09:00 to its last response at 11:30. The silence that follows is longer
    /// than five hours, so it becomes a gap. The second block holds two
    /// responses of a hundred thousand tokens twenty minutes apart: 200,000
    /// tokens for $1.00, a rate of 10,000 tokens a minute and $3.00 an hour.
    /// At 10:00 it has four hours of window left, so it projects to 200,000 +
    /// 10,000 x 240 = 2,600,000 tokens and $1.00 + $12.00 = $13.00.
    fn rows(limit: Option<u64>) -> Vec<BlockRow> {
        let entries = [
            entry("a", "2026-09-01T09:30:00Z", 500_000),
            entry("b", "2026-09-01T11:30:00Z", 500_000),
            entry("c", "2026-09-02T09:20:00Z", 100_000),
            entry("d", "2026-09-02T09:40:00Z", 100_000),
        ];
        blocks::identify(
            &entries,
            Duration::hours(DEFAULT_SPAN_HOURS),
            now(),
            CostMode::Calculate,
            &PriceSheet::builtin(),
        )
        .into_iter()
        .map(|block| BlockRow::of(block, now(), limit))
        .collect()
    }

    #[test]
    fn a_blocks_table_marks_the_active_block_and_its_projection() {
        let rendered = table(&rows(Some(5_000_000)), &Zone::Utc, 200).render();
        let expected = "Claude Code Token Usage Report - Session Blocks\n\
             \n\
             Block Start                       Duration/Status  Models       Tokens  [%]    Cost\n\
             --------------------------------  ---------------  --------  ---------  ---  ------\n\
             2026-09-01 09:00                  2h30m            • Opus 5  1,000,000  20%   $5.00\n\
             -                                 (inactive)       -                 -    -       -\n\
             2026-09-02 09:00                  ACTIVE           • Opus 5    200,000   4%   $1.00\n\
             (assuming 5,000,000 token limit)  REMAINING                  4,800,000  96%\n\
             (assuming current burn rate)      PROJECTED                  2,600,000  52%  $13.00\n";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn a_block_start_is_written_on_the_zone_the_report_was_asked_for() {
        // The one column on this table that answers "when does my allowance
        // reset", so it has to agree with the calendar `--since` and `--until`
        // were read on. Reading the host's own offset here instead would put
        // two calendars on one page, and nothing on the page would say so.
        let tokyo = Zone::parse("Asia/Tokyo").expect("Tokyo is a real zone");
        let view = table(&rows(None), &tokyo, 200);

        assert_eq!(
            view.rows[0][0], "2026-09-01 18:00",
            "09:00 UTC is six in the evening in Tokyo"
        );
        assert_eq!(
            table(&rows(None), &Zone::Utc, 200).rows[0][0],
            "2026-09-01 09:00"
        );
    }

    #[test]
    fn a_gap_block_renders_as_inactive_with_no_figures() {
        // Without a ceiling the percentage column is absent altogether, which
        // is the other shape this table has, so the golden pins both.
        let rendered = table(&rows(None), &Zone::Utc, 200).render();
        let expected = "Claude Code Token Usage Report - Session Blocks\n\
             \n\
             Block Start       Duration/Status  Models       Tokens   Cost\n\
             ----------------  ---------------  --------  ---------  -----\n\
             2026-09-01 09:00  2h30m            • Opus 5  1,000,000  $5.00\n\
             -                 (inactive)       -                 -      -\n\
             2026-09-02 09:00  ACTIVE           • Opus 5    200,000  $1.00\n";
        assert_eq!(rendered, expected);

        let view = table(&rows(None), &Zone::Utc, 200);
        assert_eq!(
            view.rows[1],
            vec!["-", "(inactive)", "-", "-", "-"],
            "a gap states its emptiness in every cell rather than leaving blanks"
        );
    }

    #[test]
    fn a_narrow_terminal_drops_the_token_columns_rather_than_wrapping_the_table() {
        let wide = table(&rows(Some(5_000_000)), &Zone::Utc, 200);
        let narrow = table(
            &rows(Some(5_000_000)),
            &Zone::Utc,
            COMPACT_BELOW_COLUMNS - 1,
        );

        let headers = |view: &TableView| -> Vec<String> {
            view.columns
                .iter()
                .map(|column| column.header.clone())
                .collect()
        };
        assert_eq!(
            headers(&wide),
            vec![
                "Block Start",
                "Duration/Status",
                "Models",
                "Tokens",
                "[%]",
                "Cost"
            ]
        );
        assert_eq!(
            headers(&narrow),
            vec!["Block Start", "Duration/Status", "Models", "Cost"]
        );
        for row in &narrow.rows {
            assert_eq!(
                row.len(),
                narrow.columns.len(),
                "{row:?} has a cell the header cannot name"
            );
        }

        // The remaining row is a token count and a percentage and nothing
        // else, so it goes with the columns that carried it. Kept, it would
        // spend thirty-four characters of the narrowest table on a label with
        // no figure after it, and widen the first column of every other row to
        // do so. The projected row stays, because it still has a cost to show.
        let rendered = narrow.render();
        assert!(
            !rendered.contains("REMAINING"),
            "nowhere left to print what remains: {rendered}"
        );
        assert!(rendered.contains("PROJECTED"), "{rendered}");
        assert!(
            wide.render().contains("REMAINING"),
            "the wide table still shows it"
        );
    }

    #[test]
    fn without_a_limit_the_running_block_carries_no_rows_beneath_it() {
        // The two extra rows exist to say how much of an allowance is left.
        // With no allowance there is nothing for them to say, and a
        // `REMAINING` row of blanks would look like a figure that failed.
        let view = table(&rows(None), &Zone::Utc, 200);
        assert_eq!(view.rows.len(), 3, "two blocks and the gap between them");
        assert!(
            !view.render().contains("REMAINING"),
            "nothing to remain out of: {}",
            view.render()
        );
    }

    #[test]
    fn a_finished_block_reports_how_long_it_ran_rather_than_its_status() {
        // The first block was anchored to 09:00 and last answered at 11:30.
        let view = table(&rows(Some(5_000_000)), &Zone::Utc, 200);
        assert_eq!(view.rows[0][1], "2h30m");
        assert_eq!(view.rows[2][1], "ACTIVE");
    }
}

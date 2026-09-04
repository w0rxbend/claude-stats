//! The composition root.
//!
//! This is the only place in the project where a concrete adapter meets an
//! abstract port. Everything above it -- the domain, the use cases, the
//! widgets -- is written against traits, so swapping the filesystem catalogue
//! for something else would be a change to this file and nothing else.

use std::io::Read as _;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc, Weekday};
use clap::Parser;
use claude_stats::application::blocks_report::BlocksReport;
use claude_stats::application::monitor::Monitor;
use claude_stats::application::period_report::PeriodReport;
use claude_stats::application::ports::{
    AccountUsageReader, PriceSheetSource, SessionFilter, SessionReader, SessionSelector,
    SystemClock, TranscriptCatalog, TranscriptRef,
};
use claude_stats::application::report_source::ReportSource;
use claude_stats::application::statusline::StatuslineReport;
use claude_stats::application::usage::UsageTracker;
use claude_stats::cli::{BlockFlags, Cli, Command, ReportOptions, StatuslineFlags};
use claude_stats::domain::period::{AggregationPeriod, GroupingSpec, Zone};
use claude_stats::domain::pricing::{CostMode, PriceSheet};
use claude_stats::infrastructure::config::{self, ConfigGateway};
use claude_stats::infrastructure::pricing::source::BuiltinPriceSource;
use claude_stats::infrastructure::reports::FileSystemReportSource;
use claude_stats::infrastructure::statusline::cache::{
    StatuslineCache, resolve as resolve_statusline,
};
use claude_stats::infrastructure::statusline::hook::StatuslineHook;
use claude_stats::infrastructure::statusline::transcript_tail::FileSystemTranscriptTail;
use claude_stats::infrastructure::transcript::corpus::FileSystemUsageRepository;
use claude_stats::infrastructure::transcript::locator::FileSystemCatalog;
use claude_stats::infrastructure::transcript::parser::TranscriptParser;
use claude_stats::infrastructure::transcript::usage::IncrementalUsageScanner;
use claude_stats::infrastructure::transcript::watcher::FileSystemWatchFactory;
use claude_stats::report;
use claude_stats::tui::palette::registry::ThemeRegistry;
use claude_stats::tui::runtime;
use claude_stats::view::statusline::{BurnDisplay, render as render_statusline};
use claude_stats::view::usage_view;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let selector = cli.selection.selector();
    let catalog = FileSystemCatalog::from_home()?;

    // One source for the whole run, asked for its sheet only by the commands
    // that price something. The source composes that sheet once and remembers
    // it, so every figure in one invocation is costed by the same rates --
    // two sheets in one run is precisely the confusion the sheet's provenance
    // exists to prevent. Asking it here rather than inside each command would
    // instead make `claude-stats sessions`, which prices nothing, refuse to
    // run over a mistake in a file it never needed to read.
    //
    // Where it *is* asked, a malformed override file stops the run rather than
    // being ignored: somebody wrote that file on purpose.
    let mut prices = BuiltinPriceSource::from_config_dir()?;

    match cli.command.unwrap_or(Command::Monitor) {
        Command::Monitor => monitor(catalog, selector, prices.sheet()?),
        Command::Stats { json } => stats(&catalog, &selector, json, &prices.sheet()?),
        Command::Sessions { limit } => sessions(&catalog, limit),
        Command::Models => {
            print!("{}", report::models(&prices.sheet()?));
            Ok(())
        }
        Command::Daily {
            instances,
            last,
            options,
        } => report_command(
            catalog,
            &options,
            &ReportShape::daily(&options, instances),
            &mut prices,
            last.last,
        ),
        Command::Weekly {
            start_of_week,
            last,
            options,
        } => report_command(
            catalog,
            &options,
            &ReportShape::weekly(&options, start_of_week),
            &mut prices,
            last.last,
        ),
        Command::Monthly { last, options } => report_command(
            catalog,
            &options,
            &ReportShape::monthly(&options),
            &mut prices,
            last.last,
        ),
        Command::Session { id, options } => report_command(
            catalog,
            &options,
            &ReportShape::by_session(&options, id),
            &mut prices,
            None,
        ),
        Command::Blocks { blocks, options } => {
            blocks_command(catalog, &options, &blocks, &mut prices)
        }
        Command::Statusline { flags } => statusline_command(&catalog, &flags, &mut prices),
    }
}

/// What distinguishes one period command from the other three.
///
/// Four commands, one implementation. The only things that genuinely differ
/// between `daily`, `weekly`, `monthly` and `session` are the four values
/// below; everything else -- the flags, the bounds, the deduplication, the
/// pricing, the rendering -- is shared, and four copies of it would be four
/// chances for one to drift away from the rest.
struct ReportShape {
    /// The second half of the table's heading.
    title: &'static str,
    /// The leftmost column's own heading.
    first_column: &'static str,
    /// The key the JSON document puts its rows under.
    json_root: &'static str,
    /// How the entries are piled up.
    spec: GroupingSpec,
    /// Which sessions count, which only `session --id` narrows.
    sessions: SessionFilter,
}

impl ReportShape {
    /// `claude-stats daily`, optionally split by project as well as by day.
    fn daily(options: &ReportOptions, instances: bool) -> Self {
        Self {
            title: "Daily",
            first_column: "Date",
            json_root: report::json_root::DAILY,
            spec: GroupingSpec {
                period: Some(AggregationPeriod::Day),
                by_project: instances,
                by_session: false,
                order: options.order.into(),
            },
            sessions: SessionFilter::All,
        }
    }

    /// `claude-stats weekly`, on weeks beginning on whichever day was named.
    fn weekly(options: &ReportOptions, start_of_week: Weekday) -> Self {
        Self {
            title: "Weekly",
            first_column: "Week",
            json_root: report::json_root::WEEKLY,
            spec: GroupingSpec {
                period: Some(AggregationPeriod::Week {
                    starts_on: start_of_week,
                }),
                by_project: false,
                by_session: false,
                order: options.order.into(),
            },
            sessions: SessionFilter::All,
        }
    }

    /// `claude-stats monthly`.
    fn monthly(options: &ReportOptions) -> Self {
        Self {
            title: "Monthly",
            first_column: "Month",
            json_root: report::json_root::MONTHLY,
            spec: GroupingSpec {
                period: Some(AggregationPeriod::Month),
                by_project: false,
                by_session: false,
                order: options.order.into(),
            },
            sessions: SessionFilter::All,
        }
    }

    /// `claude-stats session`, ranked by spend rather than by calendar.
    ///
    /// Grouped by conversation and by nothing else. Splitting by directory as
    /// well would look harmless -- a session row has to name a project, and
    /// that is one way to give it one -- but a session does not stay in one
    /// directory: Claude Code runs a workflow in a git worktree of its own, so
    /// a single conversation on this machine recorded sixty-five working
    /// directories. Split by them it became sixty-five rows sharing one
    /// `sessionId`, each holding a slice of the spend, and the ranking the
    /// command exists to produce put the dearest conversation fourth at half
    /// its real cost. The directory a row names now comes from
    /// [`UsageRow::project`], which a per-session report fills in without
    /// splitting anything.
    ///
    /// The `order` flag is carried even though a session report always comes
    /// out dearest first: the aggregate overrides it deliberately, and dropping
    /// it here would hide that decision behind a value nobody passed.
    ///
    /// [`UsageRow::project`]: claude_stats::domain::report::UsageRow::project
    fn by_session(options: &ReportOptions, id: Option<String>) -> Self {
        Self {
            title: "By Session",
            first_column: "Session",
            json_root: report::json_root::SESSIONS,
            spec: GroupingSpec {
                period: None,
                by_project: false,
                by_session: true,
                order: options.order.into(),
            },
            sessions: id.map_or(SessionFilter::All, SessionFilter::Id),
        }
    }
}

/// Runs one of the four period reports and prints it.
///
/// This is the whole of the wiring, and it is here rather than deeper in the
/// tree because every line of it is a decision only the composition root is
/// allowed to make: which concrete catalogue, which price source, how wide the
/// terminal is, and which of stdout and stderr a given sentence belongs on.
///
/// `last` is `Some` only for `daily`, `weekly` and `monthly` -- `session`
/// passes `None` because "the last N sessions" is not a calendar question
/// [`AggregationPeriod::last_n_bounds`] can answer, and `--last` is refused by
/// the parser wherever `ReportShape::spec.period` could be absent. Reading
/// `Utc::now()` here, rather than threading a clock through, matches every
/// other place in this file that needs the current instant: the composition
/// root is the one place allowed to know an environment exists.
fn report_command(
    catalog: FileSystemCatalog,
    options: &ReportOptions,
    shape: &ReportShape,
    prices: &mut BuiltinPriceSource,
    last: Option<u32>,
) -> Result<()> {
    // Refused before anything is read. Someone who asked for live prices needs
    // to be told there are none, not handed a table they will reasonably
    // believe was priced from the network.
    options.ensure_offline()?;

    let zone = options.zone()?;
    let mut query = options.query(&zone);
    query.sessions = shape.sessions.clone();
    if let Some(count) = last {
        let period = shape
            .spec
            .period
            .expect("--last is only offered on daily, weekly and monthly, which all group by a calendar period");
        let today = zone.local_date(Utc::now());
        let (since, until) = period.last_n_bounds(count, today);
        query.since = Some(zone.day_bounds(since).0);
        query.until = Some(zone.day_bounds(until).1 - chrono::Duration::nanoseconds(1));
    }

    // The catalogue arrives from the caller rather than being asked for a
    // second time, so that "where do transcripts live" is answered once per
    // run. Narrowing it to one directory is an optimisation on top of the
    // filter already in the query, never a replacement for it; see
    // `FileSystemCatalog::narrowed_to_project`.
    let catalog = catalog.narrowed_to_project(options.project.as_deref());
    let mut service = PeriodReport::new(FileSystemUsageRepository::new(catalog), prices.sheet()?);
    let report = service.run(&query, &shape.spec, &zone, options.mode.into())?;

    if options.json {
        println!("{}", report::usage_json(&report, shape.json_root));
        return Ok(());
    }

    let width = layout_width(options.compact);
    // Only when the *terminal* forced it. Somebody who passed `--compact`
    // chose the narrow layout and does not need to be told they have it.
    if width < usage_view::COMPACT_BELOW_COLUMNS && !options.compact {
        eprint!("{}", report::compact_notice(width));
    }
    print!(
        "{}",
        report::usage_table(
            &report,
            shape.title,
            shape.first_column,
            options.breakdown,
            width,
            options.mode.into(),
            service.sheet(),
        )
    );
    Ok(())
}

/// Runs the five-hour blocks report and prints it.
///
/// A sibling of [`report_command`] rather than a fifth shape passed through it.
/// The two share their flags and their wiring, but nothing else: a blocks
/// report is not a [`GroupingSpec`] over an aggregate, it has no first column
/// to name and no JSON root to choose, and it needs the one thing a period
/// report never asks for -- the current instant, because which window is
/// running and how much of it is left are both answers about now.
///
/// That instant is read here, at the composition root, and passed down. The
/// use case takes it as an argument so that every rule below it can be pinned
/// by a fixture rather than by waiting.
fn blocks_command(
    catalog: FileSystemCatalog,
    options: &ReportOptions,
    flags: &BlockFlags,
    prices: &mut BuiltinPriceSource,
) -> Result<()> {
    options.ensure_offline()?;

    let zone = options.zone()?;
    let query = options.query(&zone);
    let catalog = catalog.narrowed_to_project(options.project.as_deref());
    let mut service = BlocksReport::new(FileSystemUsageRepository::new(catalog), prices.sheet()?);
    let rows = service.run(&query, &flags.options(), Utc::now(), options.mode.into())?;

    if options.json {
        println!("{}", report::blocks_json(&rows));
        return Ok(());
    }

    let width = layout_width(options.compact);
    if width < usage_view::COMPACT_BELOW_COLUMNS && !options.compact {
        eprint!("{}", report::blocks_compact_notice(width));
    }
    print!(
        "{}",
        report::blocks_table(&rows, &zone, width, options.mode.into(), service.sheet())
    );
    Ok(())
}

/// Prints one line for the Claude Code prompt.
///
/// This is the only command in the crate that reads stdin, and the read
/// happens here and nowhere deeper, for the reason every other piece of I/O
/// in this file does: nothing below the composition root is allowed to know
/// stdin exists. It is also the only command with a fallback for its own
/// failure rather than a bare exit code -- see
/// [`claude_stats::infrastructure::statusline::cache::resolve`] for why a
/// statusline degrades to a stale or placeholder line instead of an error a
/// prompt has no room to show.
///
/// A malformed hook payload is the one failure this function does *not* try
/// to paper over. Without a payload there is no session to identify and
/// nothing to render even a stale line for, so `?` is allowed to reach `main`
/// here the way it does for every other command; everything past that point
/// -- the corpus scan, the price sheet, the transcript fallback -- goes
/// through [`resolve_statusline`] instead.
fn statusline_command(
    catalog: &FileSystemCatalog,
    flags: &StatuslineFlags,
    prices: &mut BuiltinPriceSource,
) -> Result<()> {
    flags.ensure_context_thresholds_are_ordered()?;
    let zone = flags.zone()?;
    let now = Utc::now();

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("cannot read the statusline hook payload from stdin")?;
    let request = StatuslineHook::parse(&input)?.into_request();

    // Read once, here, so the render closure below -- which may run zero or
    // one times depending on whether the cache is already fresh -- does not
    // have to touch the filesystem a second time to answer the same question.
    let transcript_modified_at = request
        .transcript_path
        .as_deref()
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .map(DateTime::<Utc>::from);

    // No session id means no file this run could own, so caching is quietly
    // unavailable rather than refused: every redraw simply renders fresh.
    let cache = if flags.cache_enabled() {
        request
            .session_id
            .as_ref()
            .and_then(|id| StatuslineCache::for_session(id.as_str()).ok())
    } else {
        None
    };

    let visual = BurnDisplay::from(flags.visual_burn_rate);
    let render = || -> Result<String> {
        // The price sheet is composed inside the closure, not above it, so a
        // user's malformed override file degrades to the cached or
        // placeholder line the same way a missing transcript does, rather
        // than crashing a prompt that has nowhere to show the failure.
        let sheet = prices.sheet()?;
        let repository = FileSystemUsageRepository::new(catalog.clone());
        let mut service = StatuslineReport::new(repository, FileSystemTranscriptTail, sheet);
        let line = service.run(&request, &zone, now, CostMode::Auto)?;
        Ok(render_statusline(&line, visual))
    };

    let output = resolve_statusline(
        cache.as_ref(),
        flags.refresh_interval(),
        transcript_modified_at,
        now,
        render,
    );
    print!("{output}");
    Ok(())
}

/// How many columns a table has to lay itself out in.
///
/// The width is measured here and passed down, so that nothing in the view or
/// the domain has to ask a terminal anything -- which is what keeps the whole
/// column layout assertable in a unit test.
///
/// When stdout is not a terminal the answer is deliberately *not* some small
/// safe number. A piped or redirected table is going into a file, an issue or
/// another program, none of which has a width at all, and dropping three
/// columns from it would lose data for a constraint that does not exist.
/// [`PIPED_WIDTH`] is wide enough to keep every column.
///
/// Stdout is asked whether it is a terminal *before* crossterm is asked how
/// wide it is, and the order matters more than it looks. `terminal::size`
/// answers from `/dev/tty` when it can, so it happily reports the width of the
/// window a command was launched from even when the table is on its way into a
/// file -- which would silently truncate every redirected report on a narrow
/// terminal, the exact failure this function exists to avoid.
fn layout_width(compact: bool) -> usize {
    use std::io::IsTerminal as _;

    if compact {
        return COMPACT_WIDTH;
    }
    if !std::io::stdout().is_terminal() {
        return PIPED_WIDTH;
    }
    crossterm::terminal::size().map_or(PIPED_WIDTH, |(columns, _)| usize::from(columns))
}

/// The width assumed when there is no terminal to measure.
const PIPED_WIDTH: usize = usage_view::COMPACT_BELOW_COLUMNS;

/// The width `--compact` pretends to have: one column short of the threshold.
const COMPACT_WIDTH: usize = usage_view::COMPACT_BELOW_COLUMNS - 1;

/// Runs the live dashboard.
fn monitor(
    catalog: FileSystemCatalog,
    selector: SessionSelector,
    prices: PriceSheet,
) -> Result<()> {
    // The usage tracker gets a catalogue of its own rather than sharing the
    // monitor's. They ask different questions of it on different clocks, and
    // a second one costs nothing: a catalogue is a path and no state.
    let usage = UsageTracker::new(
        Box::new(IncrementalUsageScanner::new(
            FileSystemCatalog::from_home()?,
            prices.clone(),
            Zone::Local,
        )),
        Box::new(SystemClock),
    );
    // Refusing early with a clear message beats emitting a screenful of escape
    // sequences into whatever the output was redirected to.
    anyhow::ensure!(
        runtime::is_interactive(),
        "the dashboard needs a terminal; try `claude-stats stats` to print a report instead"
    );

    // Config is loaded and resolved here, ahead of `runtime::run`, rather
    // than inside it: `runtime::run` calls `ratatui::try_init` as close to
    // immediately as it can, which switches the terminal to the alternate
    // screen, and a malformed `config.json`'s warning has to reach `stderr`
    // before that happens or nobody sees it. A missing file -- the ordinary
    // case -- costs nothing here either way.
    let (loaded, load_warning) = ConfigGateway::from_config_dir()?.load_or_default();
    let (resolved, resolve_warnings) = config::resolve(loaded, ThemeRegistry::builtin());
    let config_warning = load_warning.into_iter().chain(resolve_warnings).next();

    // The Daily/Weekly/Monthly/Blocks tabs get a source of their own for the
    // same reason `usage` above does: a fresh `FileSystemCatalog` costs
    // nothing (it is a path and no state), and a machine whose home
    // directory cannot be found already fails the `usage` construction a few
    // lines up in a way this run cannot recover from either -- but should
    // that lookup somehow succeed once and not the second time, degrading
    // those four tabs to their own "nothing loaded yet" message is still
    // better than refusing to start the dashboard at all.
    let report_source: Option<Box<dyn ReportSource>> =
        FileSystemCatalog::from_home().ok().map(|catalog| {
            Box::new(FileSystemReportSource::new(
                catalog,
                prices.clone(),
                Zone::Local,
            )) as Box<dyn ReportSource>
        });

    runtime::run(
        Monitor::new(
            catalog,
            TranscriptParser::new(prices),
            FileSystemWatchFactory,
            selector,
        ),
        usage,
        &resolved,
        config_warning,
        report_source,
    )
}

/// Prints a one-shot report.
fn stats(
    catalog: &FileSystemCatalog,
    selector: &SessionSelector,
    as_json: bool,
    prices: &PriceSheet,
) -> Result<()> {
    let transcript = require_session(catalog, selector)?;
    let snapshot = TranscriptParser::new(prices.clone()).read(&transcript)?;

    // Account-wide usage is a bonus here, not the point of the command: a
    // report about one session is still worth printing when the scan of every
    // other session fails, so a failure is dropped rather than propagated.
    let usage = FileSystemCatalog::from_home()
        .map(|catalog| IncrementalUsageScanner::new(catalog, prices.clone(), Zone::Local))
        .and_then(|mut scanner| scanner.usage(Utc::now()))
        .ok();

    if as_json {
        println!("{}", report::json(&snapshot, usage.as_ref()));
    } else {
        print!("{}", report::text(&snapshot, usage.as_ref()));
    }
    Ok(())
}

/// Lists the sessions on this machine.
fn sessions(catalog: &FileSystemCatalog, limit: usize) -> Result<()> {
    print!("{}", report::sessions(&catalog.list()?, limit));
    Ok(())
}

/// Resolves a selector, turning "nothing matched" into an actionable message.
fn require_session(
    catalog: &FileSystemCatalog,
    selector: &SessionSelector,
) -> Result<TranscriptRef> {
    catalog.resolve(selector)?.with_context(|| match selector {
        SessionSelector::Active => {
            "no active session found; start Claude Code, or pass --session".to_owned()
        }
        SessionSelector::Id(prefix) => format!("no session whose id starts with {prefix:?}"),
        SessionSelector::Project(dir) => format!("no sessions for {}", dir.display()),
        SessionSelector::Path(path) => format!("cannot read {}", path.display()),
    })
}

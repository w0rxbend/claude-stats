//! Renders the real dashboard into an off-screen buffer, exactly the way
//! `src/tui/screens/dashboard.rs`'s own tests do, and prints it as plain
//! text. This is how the two ASCII dashboards in `README.md` are produced --
//! from the genuine widget tree under a genuine theme, not hand-typed art
//! that can silently drift from what the tool actually draws.
//!
//! Run with: `cargo run --example readme_dashboard -- <theme-name>`
//! (defaults to `aurora`). Figures are synthetic but shaped like a real
//! week of use: several projects, two models, a running billing block.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use claude_stats::domain::activity::{ToolEvent, ToolKind};
use claude_stats::domain::entry::{Entry, EntryId};
use claude_stats::domain::limits::AccountUsage;
use claude_stats::domain::model::ModelId;
use claude_stats::domain::period::Zone;
use claude_stats::domain::pricing::PriceSheet;
use claude_stats::domain::project::{Project, SessionId};
use claude_stats::domain::session::{ResponseSample, SessionPhase, SessionSnapshot, TurnCounters};
use claude_stats::domain::tokens::TokenUsage;
use claude_stats::tui::palette::registry::ThemeRegistry;
use claude_stats::tui::screens::dashboard::{DashboardInputs, DollarPulseInputs, draw};

fn entry(session: &str, when: &str, project: &str, model: &str, input: u64, out: u64) -> Entry {
    let at: DateTime<Utc> = when.parse().expect("valid timestamp");
    Entry {
        id: EntryId {
            message_id: format!("msg-{session}-{when}"),
            request_id: None,
            session: SessionId::new(session),
        },
        at,
        model: ModelId::new(model),
        tokens: TokenUsage {
            input,
            cache_read: input * 40,
            output: out,
            ..TokenUsage::ZERO
        },
        recorded_cost: None,
        session: SessionId::new(session),
        project: Project::new(project),
        is_sidechain: false,
    }
}

fn sample_snapshot() -> SessionSnapshot {
    let mut s = SessionSnapshot::empty(
        "/home/ada/code/payments-api/session.jsonl".into(),
        "a1b2c3d4".to_owned(),
    );
    "claude-opus-5".clone_into(&mut s.model_id);
    s.project_dir = Some("/home/ada/code/payments-api".to_owned());
    s.git_branch = Some("feat/webhooks".to_owned());
    s.phase = SessionPhase::Thinking;
    s.turns = 12;
    s.totals.input = 22_000;
    s.totals.cache_read = 22_970_000;
    s.totals.output = 41_000;
    s.cost_accrued = claude_stats::domain::money::Usd::new(18.37);
    s.samples.push(ResponseSample {
        turn: 12,
        prompt_tokens: 714_200,
        output_tokens: 1_530,
        at: Utc::now(),
    });
    s.turns_since_compaction = 4;
    s.compactions
        .push(claude_stats::domain::session::CompactionEvent {
            turn: 6,
            context_before: 950_000,
            context_after: 120_000,
            turns_in_segment: 6,
            at: Utc::now(),
        });

    let calls = [
        (ToolKind::Write, "Edit", "webhook_handler.rs"),
        (ToolKind::Shell, "Bash", "cargo test --lib"),
        (ToolKind::Search, "Grep", "verify_signature"),
        (ToolKind::Read, "Read", "webhook_handler.rs"),
        (ToolKind::Shell, "Bash", "cargo build"),
        (ToolKind::Agent, "Task", "audit the retry path"),
        (ToolKind::Read, "Read", "retry.rs"),
        (ToolKind::Shell, "Bash", "git status --short"),
    ];
    for (i, (kind, name, subject)) in calls.iter().enumerate() {
        s.recent_tools.push_back(ToolEvent {
            at: Utc::now(),
            name: (*name).to_owned(),
            kind: *kind,
            subject: (*subject).to_owned(),
            failed: *name == "Bash" && *subject == "cargo build",
            id: format!("call-{i}"),
        });
    }
    s.tool_counts = BTreeMap::from([
        ("Edit".to_owned(), 12),
        ("Bash".to_owned(), 9),
        ("Read".to_owned(), 7),
        ("Grep".to_owned(), 3),
        ("Task".to_owned(), 1),
    ]);
    s.files_read = BTreeMap::from([
        ("webhook_handler.rs".to_owned(), 3),
        ("retry.rs".to_owned(), 2),
    ]);
    s.files_edited = BTreeMap::from([("webhook_handler.rs".to_owned(), 4)]);
    s.tool_errors = 1;
    s.last_error = Some("Exit code 101".to_owned());
    s.thinking_blocks = 4;
    s.subagents = 1;
    s.turn = TurnCounters {
        tools: BTreeMap::from([("Edit".to_owned(), 12)]),
        tool_errors: 1,
        files_read: BTreeMap::new(),
        files_edited: BTreeMap::new(),
        thinking_blocks: 4,
        agents_spawned: 1,
        agents_running: 1,
        active_skill: None,
    };
    s
}

fn account_usage() -> AccountUsage {
    let now: DateTime<Utc> = "2026-09-03T18:00:00Z".parse().expect("valid timestamp");
    let mut entries = Vec::new();
    let projects = ["payments-api", "web-app", "infra-tools", "docs-site"];
    for day in 0..7 {
        for (i, project) in projects.iter().enumerate() {
            let i = i64::try_from(i).expect("small fixture index fits in i64");
            let when = now - chrono::Duration::days(day) - chrono::Duration::hours(i);
            let model = if (day + i) % 3 == 0 {
                "claude-sonnet-5"
            } else {
                "claude-opus-5"
            };
            entries.push(entry(
                &format!("s{day}-{i}"),
                &when.to_rfc3339(),
                project,
                model,
                50_000 + u64::try_from(i).expect("small fixture index fits in u64") * 20_000,
                8_000 + u64::try_from(i).expect("small fixture index fits in u64") * 1_000,
            ));
        }
    }
    // A response inside the last five hours, so the running block and the
    // session window both have something to show.
    entries.push(entry(
        "active",
        &(now - chrono::Duration::minutes(20)).to_rfc3339(),
        "payments-api",
        "claude-opus-5",
        400_000,
        9_000,
    ));

    AccountUsage::measure(
        now,
        &entries,
        Vec::new(),
        &PriceSheet::builtin(),
        &Zone::Utc,
    )
}

fn main() {
    let theme = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "aurora".to_owned());
    let registry = ThemeRegistry::builtin();
    let palette = registry
        .get(&theme)
        .unwrap_or_else(|| panic!("unknown theme {theme}, see ThemeRegistry::builtin().names()"))
        .clone();

    let snapshot = sample_snapshot();
    let usage = account_usage();

    let mut terminal = Terminal::new(TestBackend::new(102, 40)).expect("test terminal");
    terminal
        .draw(|frame| {
            draw(
                frame,
                frame.area(),
                &snapshot,
                0,
                Some((&usage, true)),
                &palette,
                DashboardInputs {
                    pulse: DollarPulseInputs {
                        frames_since_increment: None,
                        off: false,
                    },
                    active_preset: "live",
                    tab_index: 0,
                },
            );
        })
        .expect("draw");

    let buffer = terminal.backend().buffer().clone();
    let (width, height) = (buffer.area.width, buffer.area.height);
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        println!("{}", line.trim_end());
    }
}

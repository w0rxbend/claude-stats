# claude-stats

[![Release](https://img.shields.io/github/v/release/w0rxbend/claude-stats)](https://github.com/w0rxbend/claude-stats/releases/latest)
[![CI](https://github.com/w0rxbend/claude-stats/actions/workflows/ci.yml/badge.svg)](https://github.com/w0rxbend/claude-stats/actions/workflows/ci.yml)
[![Licence](https://img.shields.io/github/license/w0rxbend/claude-stats)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org)

A live terminal dashboard for [Claude Code](https://claude.com/claude-code) sessions,
written in Rust with [ratatui](https://ratatui.rs).

Open it in a second terminal next to a running session. It finds the active session,
follows its transcript file, and redraws when it changes — showing context fill, cost,
cache hit ratio, distance to the next automatic compaction, and a live feed of the tools
Claude is calling right now.

This is a real render — off-screen `TestBackend` output from the `aurora` theme, dumped
by `cargo run --example readme_dashboard`, then anonymised (project and branch names
swapped for fakes) and rounded. It is not hand-drawn art, so it cannot drift from what
the tool actually puts on screen; regenerate it after any layout change with the same
command and paste the output back in. The tab bar above the header (`1 Dashboard`,
`2 Daily`, …) and the footer status line (`? help`, the current key chord, a config
warning) are both real, App-drawn chrome this dump doesn't include, since
`dashboard::draw` only owns the body between them.

```
 1 Dashboard   2 Daily   3 Weekly   4 Monthly   5 Blocks   6 Log
 claude-stats  ⠁ working  │ ◇ Opus 5  │ ▤ /home/ada/code/payments-api  │ ⎇ feat/webhooks
╭ ◴ CONTEXT ────╮╭ ¤ COST ───────╮╭ ⧉ CACHE ──────╮╭ ↯ COMPACTION ─╮╭ ↺ TURNS ──────╮╭ ⚠ ERRORS ─────╮
│ 71.4%         ││ $18.37        ││ 99.9%         ││ —             ││ 12            ││ 1             │
│ 714.2k / 1.0… ││ $1.53/turn    ││ 22.97M read   ││ 1 so far      ││ 32 tools      ││ 2 files       │
╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯
╭ context window ────────────────────────────────────────────────────────────────────────────────────╮
│ ██████████████████████████████████████████████████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░↓░░ │
│ ◇ used 714.2k   ◴ free 285.8k   ↯ until compaction 252.8k   ↗ growth/turn 0                        │
╰────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ account usage ────────────────────────────────────────────╮╭ spend ────────────────────────────────╮
│ ◷ 5h      ░░░░░░░░░░░░░░ 29.57M                           ││ ¤ today   $16.49   29.57M tokens   5  │
│            $16.49  │ 5 sessions  │ no busier window on re ││ ◷ block   $16.49   started 15:00   2h │
│                                                           ││   projected $27.48                    │
│ ◷ 7d      ░░░░░░░░░░░░░░ 108.52M                          ││                                       │
│            $59.45  │ 29 sessions  │ no busier window on r ││ payments-api                  $17.77  │
│ ¤ September$31.70  │ 55.88M  │ August $27.74 (52.63M)     ││ docs-site                     $15.73  │
│                                                           ││ infra-tools                   $14.50  │
│                                                           ││ web-app                       $11.46  │
│                                                           ││                                       │
╰───────────────────────────────────────────────────────────╯╰───────────────────────────────────────╯
╭ output per response ──────────────────────────────╮╭ live tool activity ───────────────────────────╮
│                                                 █ ││ ⠁ bash git status --short                     │
│                                                 █ ││ ◈ read retry.rs                               │
│                                                 █ ││ ✦ task audit the retry path                   │
│                                                 █ ││ ⚠ bash cargo build                            │
│ cache      ████████████████████████ 99.9%         ││ ◈ read webhook_handler.rs                     │
│ efficiency ███████████████████████▉ 99%           ││ ⌕ grep verify_signature                       │
│ context    █████████████████▏░░░░░░ 71.4%         ││ ▶ bash cargo test --lib                       │
╰───────────────────────────────────────────────────╯│ ✎ edit webhook_handler.rs                     │
╭ token mix ────────────────────────────────────────╮│                                               │
│                      ⢀⣀⣀⣄⣀⣀                       │╰───────────────────────────────────────────────╯
│                    ⣠⣾⣿⣿⣿⣿⣿⣿⣿⣦⡀                    │╭ this turn ────────────────────────────────────╮
│                  ⢀⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣆                   ││ ◇ tools 12   ∿ thinking 4   ⚠ errors 1        │
│                  ⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿                   ││ ‣ 1 sub-agent(s) running                      │
│                  ⢹⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁                  ││ ⚠ Exit code 101                               │
│                  ⠘⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡟                   ││                                               │
│                   ⠈⠻⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋                    ││                                               │
│                     ⠈⠙⠛⠛⠟⠛⠛⠉                      ││                                               │
╰───────────────────────────────────────────────────╯╰───────────────────────────────────────────────╯
```

---

## What it shows, and why each number is there

**Context fill.** How much of the model's window the conversation currently occupies.
Read from the latest response's prompt tokens, not summed over the session — the window
holds the *current* conversation, and summing would count the same cached prefix once per
turn.

**Compaction distance.** How many turns are left before Claude Code automatically compacts
the conversation. This is the number the tool exists for. Compaction is not free: it throws
away a conversation you have already paid for and pays again for a summary of it. Knowing
it is four turns away is the difference between finishing your thought and having it
summarised out from under you.

The distance is measured against the ~33k of head-room Claude Code keeps free, **not**
against the end of the window, so it predicts the event that actually fires. The violet tick
on the context bar marks that threshold.

**Cost.** Priced per token kind at Anthropic's published rates — fresh input, cache reads,
cache writes and output are all charged differently, and a session's bill is mostly decided
by which of them it is spending.

**Cache hit ratio.** The share of prompt tokens served from the prompt cache rather than
re-sent. This is the one tile where a *low* number is the alarming one, and its colour scale
runs backwards from every other tile for that reason: below about half, the conversation
prefix is being re-sent every turn, and that is where an unexpected bill comes from.

**Live tool activity.** What Claude is doing right now, newest first. Failed calls stay red
however far down they scroll.

**Today's spend, the running billing block and the top projects.** The dashboard's account
panel also shows three figures that only make sense once you know Anthropic bills in
five-hour windows (see [Billing blocks](#billing-blocks-blocks) below): what today has cost
so far, on your own calendar rather than UTC; the block that is running right now, with a
`fast` marker beside its projection when the burn rate is high enough that the projection
is worth a second look; and the five projects that have cost the most over the last week, so
you can see where a bill is coming from without leaving the dashboard.

---

## Install

### One line

```bash
curl -fsSL https://raw.githubusercontent.com/w0rxbend/claude-stats/main/install.sh | sh
```

Downloads a prebuilt binary for your machine, checks it against the release's published
SHA-256, and installs it to `~/.local/bin`. Nothing is compiled and no Rust toolchain is
needed. It will tell you if that directory is not on your `PATH`.

Two optional knobs:

```bash
CLAUDE_STATS_VERSION=v0.3.0 curl -fsSL .../install.sh | sh   # pin a version
CLAUDE_STATS_INSTALL_DIR=/usr/local/bin curl -fsSL .../install.sh | sh
```

The installer never escalates to `sudo` on its own. If you want it somewhere privileged,
say so with `CLAUDE_STATS_INSTALL_DIR` and run it under `sudo` yourself.

### Prebuilt binaries

Every [release](https://github.com/w0rxbend/claude-stats/releases) attaches an archive per
platform, plus a `SHA256SUMS` covering all of them:

| Platform | Archive |
|---|---|
| macOS, Apple silicon | `claude-stats-<version>-aarch64-apple-darwin.tar.gz` |
| macOS, Intel | `claude-stats-<version>-x86_64-apple-darwin.tar.gz` |
| Linux, x86-64 | `claude-stats-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux, ARM64 | `claude-stats-<version>-aarch64-unknown-linux-musl.tar.gz` |

The Linux builds are statically linked against musl, so they run on any distribution
regardless of its glibc version — including Alpine and distroless containers. The crate has
no C dependencies, so nothing is given up for that.

Windows is not supported directly; use [WSL 2](https://learn.microsoft.com/windows/wsl/install)
and the Linux instructions.

### From source

Requires Rust 1.85 or newer.

```bash
cargo install --git https://github.com/w0rxbend/claude-stats
```

### Uninstall

```bash
rm ~/.local/bin/claude-stats
```

The tool writes no configuration and no state — it only ever reads Claude Code's own
transcript files — so removing the binary removes all of it.

---

## Use

```bash
claude-stats                  # the live dashboard, following the active session
claude-stats monitor          # the same thing, spelled out
```

By default it attaches to the newest session belonging to the directory you launched it
from — so opening it in a second terminal inside your project attaches to the session in the
first one. If that directory has no sessions it falls back to the newest session anywhere,
and it re-checks every few seconds, so starting Claude Code after the dashboard still works.

### Looking at a different session

```bash
claude-stats --session a1b2c3d4          # by session id prefix
claude-stats --project ~/code/other-app  # newest session for another directory
claude-stats --path ~/.claude/projects/-home-me-app/abc.jsonl
```

Or press `o` inside the dashboard to pick one from a list.

### One-shot reports

```bash
claude-stats stats            # a formatted report, printed once
claude-stats stats --json     # raw numbers, for jq
claude-stats sessions         # every session on this machine, newest first
claude-stats models           # context windows and prices per million tokens
```

### Usage reports: `daily`, `weekly`, `monthly`, `session`

These four total every billable response across **every project on the machine**, not just
one session — the same corpus the dashboard's account panel draws on, but grouped and
filtered on the command line instead of watched live. Every figure they print is deduplicated
by `(message id, request id, session)` first, so a sub-agent transcript and its parent, or a
response replayed across two files, are counted once rather than twice — see
["How it works"](#how-it-works) below for why that matters.

```bash
claude-stats daily                        # every day, oldest first
claude-stats daily --last 3               # just the last three days
claude-stats weekly -w monday             # weeks starting Monday instead of Sunday
claude-stats monthly --json | jq .totals  # this and every other month's total, as JSON
claude-stats session                      # every conversation, dearest first
claude-stats session --id a0acbd4e        # one conversation, by the prefix `sessions` prints
```

`daily --last 3`, run against this machine's real history and with project names
anonymised:

```
Claude Code Token Usage Report - Daily

Date        Models         Input     Output  Cache Create     Cache Read   Total Tokens  Cost (USD)
----------  -----------  -------  ---------  ------------  -------------  -------------  ----------
2026-09-01  • Opus 5      25,818  4,196,290    45,423,007  1,685,360,546  1,735,005,661    $1240.12
2026-09-02  • Opus 5     316,064  1,867,514    39,172,730    764,150,770    805,507,078     $759.42
            • Fable 5.1
2026-09-03  • Fable 5.1   68,106    553,200    16,235,185    689,393,936    706,250,427     $405.74
            • Opus 5
            • Sonnet 5
----------  -----------  -------  ---------  ------------  -------------  -------------  ----------
Total                    409,988  6,617,004   100,830,922  3,138,905,252  3,246,763,166    $2405.28

priced with the built-in price sheet, mode: auto
```

The **Models** column lists every model that contributed to a row, first-seen order, so a
week where a new release showed up on Wednesday reads differently from one where it was
there from Monday. Pass `-b`/`--breakdown` to add a sub-row per model underneath, showing
each one's own token counts and cost rather than only the row's total.

Every one of these four commands, and `blocks` below, shares the same set of flags:

| Flag | Does |
|---|---|
| `-s`/`--since`, `-u`/`--until` | `YYYYMMDD`, inclusive, read on the reporting time zone |
| `--last <N>` | the most recent N periods instead of a date range — mutually exclusive with `--since`/`--until` |
| `-z`/`--timezone` | `local` (default), `utc`, or an IANA name such as `Asia/Tokyo` — a response stamped 23:30 UTC files under a different calendar day depending on which you pick |
| `-p`/`--project` | narrow to one project, matched against the full working directory or its last path segment |
| `-m`/`--mode` | `auto` (recorded cost where the transcript has one, else priced from the sheet), `calculate` (always priced from the sheet), or `display` (only ever show a recorded cost) — this transcript format never records a per-message cost, so `auto` and `calculate` agree everywhere today |
| `-o`/`--order` | `asc` (oldest first, default) or `desc` |
| `-b`/`--breakdown` | a sub-row per model under every row |
| `-j`/`--json` | machine-readable output instead of a table |
| `--compact` | the narrow layout (fewer columns) whatever the terminal is wide enough for |
| `-O`/`--offline` | the default and the only supported behaviour — `--online` exists only to be refused with an explanation, because prices are compiled in and nothing here ever opens a socket |

A table dropped below 100 columns wide (or given `--compact`) loses the Cache Create, Cache
Read and Total Tokens columns to stay readable; a note to that effect goes to stderr, never
to stdout, so it cannot end up inside piped JSON or a redirected table.

### Billing blocks: `blocks`

Anthropic doesn't meter usage response by response — it bills in **five-hour rolling
windows**. The first response after an idle stretch opens a window (floored to the top of
its hour); everything within five hours of that response's window belongs to it; the next
response after a five-hour gap opens the next one. `blocks` reconstructs those windows from
the same corpus `daily`/`weekly`/`monthly`/`session` read, and is the only command that
shows where the *currently running* window is heading.

```bash
claude-stats blocks --recent                       # the last three days of windows, plus the running one
claude-stats blocks --active                        # only the window that is running right now
claude-stats blocks --active --token-limit max       # ...judged against your own busiest window so far
```

```
Claude Code Token Usage Report - Session Blocks

Block Start                         Duration/Status  Models           Tokens  [%]     Cost
----------------------------------  ---------------  ----------  -----------  ---  -------
2026-09-03 15:00                    ACTIVE           • Opus 5    354,740,429  36%  $140.52
                                                       • Sonnet 5
(assuming 982,666,777 token limit)  REMAINING                    627,926,348  64%
(assuming current burn rate)        PROJECTED                    512,010,300  52%  $202.81

priced with the built-in price sheet, mode: auto
```

The **burn rate** behind `PROJECTED` is measured in tokens per minute, but the figure used
to decide how alarming it is deliberately **excludes cache traffic** — a window that is 99%
cache reads can show a burn rate in the millions of tokens per minute and still be doing
almost nothing new, so counting cache tokens toward the indicator would cry wolf on exactly
the traffic that costs the least. `--token-limit` needs a number to project against; there
is no API that hands one back, because the real allowance lives on Anthropic's side and is
never written to disk, so `max` is a stand-in meaning "the busiest window you've already
finished" — the only ceiling this tool can honestly discover on its own.

Rows always print oldest first, whatever `-o`/`--order` says: a `REMAINING`/`PROJECTED`
pair only makes sense hanging underneath the block it projects, and a gap row only means
anything between the two windows it separates, so re-sorting the table would break both.

### `statusline`

Prints one line, meant to sit in Claude Code's own prompt rather than be run by hand. Claude
Code sends it the current session's state as JSON on stdin (model, cost, context usage);
`statusline` combines that with the same corpus everything else in this tool reads — today's
spend across every project, the billing block that is currently running, and its burn rate —
and prints exactly one line to stdout, whatever goes wrong while producing it.

```bash
echo '{"session_id":"abc123","transcript_path":"...","model":{"id":"claude-opus-5","display_name":"Opus 5"},"cost":{"total_cost_usd":4.21},"context_window":{"total_input_tokens":94000,"context_window_size":200000}}' \
  | claude-stats statusline
```

```
🤖 Opus 5 | 💰 $4.21 session / $406.76 today / $140.59 block (1h 31m left) | 🔥 $40.57/hr | 🧠 94,000 (47%)
```

`-B/--visual-burn-rate emoji` appends a 🟢/⚠️/🚨 to the burn-rate figure according to how
fast the running block is spending (`text` appends a bracketed word instead, `emoji-text`
both, `off` — the default — neither, because a statusline is embedded in somebody else's
prompt and the quietest option is the least likely to fight it). To wire it into Claude
Code, add a `statusLine` entry to your Claude Code settings that runs `claude-stats
statusline`; see [Claude Code's own statusline
documentation](https://docs.anthropic.com/claude/docs/claude-code) for the settings shape.

A redraw doesn't rescan the whole corpus every time: the rendered line is cached per session
id under `${XDG_CACHE_HOME:-~/.cache}/claude-stats/statusline/`, and is considered fresh
for `--refresh-interval` seconds (default 1) as long as the transcript hasn't changed since
it was cached. `--no-cache` always renders fresh.

### Correcting prices

The published per-token rates are compiled into the binary, so no report ever needs a
network connection — `--online` is refused rather than silently ignored, on the grounds that
a flag that looks like it does something and doesn't is worse than one that says so. If
Anthropic changes a rate before a new release of this tool catches up, write an override to
`${XDG_CONFIG_HOME:-~/.config}/claude-stats/prices.json`:

```json
{
  "models": {
    "claude-opus-5": { "input": 5.0, "output": 25.0 }
  }
}
```

Only `input` and `output` are required — the cache-read and cache-write rates default to the
same multiples of the input rate the built-in catalogue row for that id already uses, so
correcting Opus 5's headline price doesn't also have to spell out its four cache rates by
hand. `models` prints a `rates: overridden from <path>` footer whenever this file is in
effect, so it's always visible which sheet a report was priced from. A malformed file
(a typo'd key, a value of the wrong type) aborts every command with a message naming the
file and the field, rather than silently falling back to the built-in rates — a price a user
wrote by hand and got wrong should never disappear quietly.

### Reading from more than one Claude Code install

By default the tool reads `~/.claude/projects`, merged with
`${XDG_CONFIG_HOME:-~/.config}/claude/projects` when that also exists (some installs use
one, some the other, and a few machines genuinely have sessions under both). Set
`CLAUDE_CONFIG_DIR` to point somewhere else, or to several places at once, comma-separated:

```bash
CLAUDE_CONFIG_DIR=/mnt/backup/.claude,~/.claude claude-stats sessions
```

Each entry may name either a Claude config directory or its `projects/` subdirectory
directly. A value that resolves to nothing real is a hard error naming the variable, rather
than a report that quietly looks empty.

### Account usage and rate limits

The dashboard also tracks what you have spent **across every session on the
machine**, not just the one it is following, because that is the shape of
Claude Code's limits: a five-hour "session limit" and a seven-day "weekly
limit" are consumed by everything you run, in every terminal and every project.

The `account usage` panel shows, for the last five hours and the last seven
days: tokens, cost, and how many sessions contributed. The same figures appear
under `stats`, and in `stats --json` beneath an `account` key.

**It does not show a percentage of your limit, because that number cannot be
known here.** Limits are enforced on Anthropic's side and the live figure is
never written to disk -- `/usage` inside Claude Code fetches it from the API.
Rather than draw a convincing gauge from a guessed ceiling, the bar is drawn
against your *own* busiest comparable window and labelled `vs peak`.

One thing is exact. When the API actually refuses a request it records when the
limit lifts, so if you are rate limited right now the panel says so and counts
down to the reset using the server's own answer.

`stats --json` emits unabbreviated token counts and dollar amounts, so it can be summed
across sessions:

```bash
claude-stats sessions --limit 100 | tail -n +2 | awk '{print $1}' |
  xargs -I{} claude-stats stats --json --session {} |
  jq -s 'map(.cost_usd) | add'
```

### Themes

Twenty-seven built-in themes ship in the binary, each a
[`Palette`](src/tui/palette.rs) value -- eighteen colour roles (ground,
borders, text, five accents, a four-step pressure ramp) checked, for every
theme, against WCAG contrast floors (body text 4.5:1, every accent role
3:1) and against reusing a pressure colour for something that is not under
pressure. Nothing here is picked by eye and left untested.

Press `t` to open the theme picker at whatever theme is current; pressing
`t` again -- and every time after -- steps to the next theme in the list and
applies it immediately, wrapping back to the first once it reaches the last.
There is no preview-then-confirm step: the picker *is* the live dashboard,
which is the fastest way to actually compare two themes against your own
data rather than a screenshot. The theme you land on is written to
`config.json` (see [Config file](#config-file) below) the moment you stop
cycling, so it is still selected the next time you start `claude-stats`.

You can also set a theme directly, without opening the picker:

```bash
echo '{ "theme": "gruvbox-light" }' > ~/.config/claude-stats/config.json
```

or, once the dashboard is running, `:theme gruvbox-light` in command mode.

All twenty-seven names:

```
aurora            catppuccin-mocha   catppuccin-macchiato  catppuccin-frappe
catppuccin-latte  tokyo-night        tokyo-night-storm      tokyo-night-day
gruvbox-dark      gruvbox-light      nord                    dracula
solarized-dark    solarized-light    rose-pine               rose-pine-moon
rose-pine-dawn    everforest-dark    kanagawa                one-dark
monokai           ayu-dark           ayu-mirage              ayu-light
high-contrast     solstice           terracotta
```

`aurora` -- a cool indigo ground with cyan and violet accents -- is the
default and the one every render elsewhere in this README uses. Here is the
same dashboard again, under `gruvbox-light`, a warm cream ground with olive
and ochre accents at the opposite end of the palette:

```
 1 Dashboard   2 Daily   3 Weekly   4 Monthly   5 Blocks   6 Log
 claude-stats  ⠁ working  │ ◇ Opus 5  │ ▤ /home/ada/code/payments-api  │ ⎇ feat/webhooks
╭ ◴ CONTEXT ────╮╭ ¤ COST ───────╮╭ ⧉ CACHE ──────╮╭ ↯ COMPACTION ─╮╭ ↺ TURNS ──────╮╭ ⚠ ERRORS ─────╮
│ 71.4%         ││ $18.37        ││ 99.9%         ││ —             ││ 12            ││ 1             │
│ 714.2k / 1.0… ││ $1.53/turn    ││ 22.97M read   ││ 1 so far      ││ 32 tools      ││ 2 files       │
╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯╰───────────────╯
╭ context window ────────────────────────────────────────────────────────────────────────────────────╮
│ ██████████████████████████████████████████████████████████████████████░░░░░░░░░░░░░░░░░░░░░░░░░↓░░ │
│ ◇ used 714.2k   ◴ free 285.8k   ↯ until compaction 252.8k   ↗ growth/turn 0                        │
╰────────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ account usage ────────────────────────────────────────────╮╭ spend ────────────────────────────────╮
│ ◷ 5h      ░░░░░░░░░░░░░░ 29.57M                           ││ ¤ today   $16.49   29.57M tokens   5  │
│            $16.49  │ 5 sessions  │ no busier window on re ││ ◷ block   $16.49   started 15:00   2h │
│                                                           ││   projected $27.48                    │
│ ◷ 7d      ░░░░░░░░░░░░░░ 108.52M                          ││                                       │
│            $59.45  │ 29 sessions  │ no busier window on r ││ payments-api                  $17.77  │
│ ¤ September$31.70  │ 55.88M  │ August $27.74 (52.63M)     ││ docs-site                     $15.73  │
│                                                           ││ infra-tools                   $14.50  │
│                                                           ││ web-app                       $11.46  │
│                                                           ││                                       │
╰───────────────────────────────────────────────────────────╯╰───────────────────────────────────────╯
╭ output per response ──────────────────────────────╮╭ live tool activity ───────────────────────────╮
│                                                 █ ││ ⠁ bash git status --short                     │
│                                                 █ ││ ◈ read retry.rs                               │
│                                                 █ ││ ✦ task audit the retry path                   │
│                                                 █ ││ ⚠ bash cargo build                            │
│ cache      ████████████████████████ 99.9%         ││ ◈ read webhook_handler.rs                     │
│ efficiency ███████████████████████▉ 99%           ││ ⌕ grep verify_signature                       │
│ context    █████████████████▏░░░░░░ 71.4%         ││ ▶ bash cargo test --lib                       │
╰───────────────────────────────────────────────────╯│ ✎ edit webhook_handler.rs                     │
╭ token mix ────────────────────────────────────────╮│                                               │
│                      ⢀⣀⣀⣄⣀⣀                       │╰───────────────────────────────────────────────╯
│                    ⣠⣾⣿⣿⣿⣿⣿⣿⣿⣦⡀                    │╭ this turn ────────────────────────────────────╮
│                  ⢀⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣆                   ││ ◇ tools 12   ∿ thinking 4   ⚠ errors 1        │
│                  ⢸⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿                   ││ ‣ 1 sub-agent(s) running                      │
│                  ⢹⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⠁                  ││ ⚠ Exit code 101                               │
│                  ⠘⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡟                   ││                                               │
│                   ⠈⠻⣿⣿⣿⣿⣿⣿⣿⣿⡿⠋                    ││                                               │
│                     ⠈⠙⠛⠛⠟⠛⠛⠉                      ││                                               │
╰───────────────────────────────────────────────────╯╰───────────────────────────────────────────────╯
```

The two renders above read identically as plain text, because
[`ratatui`'s `TestBackend`](https://docs.rs/ratatui) -- the off-screen
buffer both were dumped from -- captures which character is in each cell,
not its colour, and `README.md` cannot carry ANSI colour codes either. In a
real terminal the two look nothing alike: run `claude-stats` and press `t`
a few times to see the difference this file cannot show.

**Defining your own theme** means adding a function to
[`src/tui/palette/builtins.rs`](src/tui/palette/builtins.rs) and rebuilding
-- there is no way yet to hand the dashboard a theme through `config.json`
that was not already compiled in. `ThemeRegistry::register` (the method
every one of the twenty-seven built-ins is loaded through) is `pub`, so
anything embedding this crate as a library can register one at runtime, but
the shipped binary has no config-file path that reaches it. If you want a
theme this crate does not have, the built-in unit tests
(`every_built_in_theme_clears_its_wcag_contrast_floor`,
`no_theme_reuses_a_pressure_hex_for_a_non_pressure_role`) are exactly the
two checks a new one needs to pass before it is worth shipping.

### Keymap

Every key below comes from one literal table --
[`src/tui/keymap/defaults.rs`](src/tui/keymap/defaults.rs) -- and nothing
else: the help overlay (`?`), the which-key popup that appears while `g` is
held awaiting its second key, and this README section all read the same
data, so none of them can drift from what the dashboard actually does. A
key marked **pinned** cannot be shadowed or removed even by a future
keybinding-override feature -- see [Config file](#config-file) below for
where user overrides are heading.

| Key | Action | Group |
|---|---|---|
| `q` | quit | global |
| `Esc` | dismiss the help, then leave the current view, then quit | global (pinned) |
| `Enter` | attach to the selected session / confirm | global (pinned) |
| `?`, `F1` | toggle this help | global |
| `r` | re-read the transcript and re-measure usage | global |
| `o` | open the session picker | views |
| `j`, `↓` | move down | motion |
| `k`, `↑` | move up | motion |
| `h`, `←` | pan left | motion |
| `l`, `→` | pan right | motion |
| `Ctrl-d` | scroll down half a page | motion |
| `Ctrl-u` | scroll up half a page | motion |
| `Ctrl-f`, `PageDown` | scroll down a page | motion |
| `Ctrl-b`, `PageUp` | scroll up a page | motion |
| `{` | jump to the previous section | motion |
| `}` | jump to the next section | motion |
| `0`, `Home` | jump to the start of the line | motion |
| `$`, `End` | jump to the end of the line | motion |
| `gg` | jump to the top | jumps |
| `G` | jump to the bottom | jumps |
| `Ctrl-o` | jump back to the position before the last jump | jumps |
| `gt` | next tab | views |
| `gT` | previous tab | views |
| `Tab` / `Shift-Tab` | focus the next / previous pane | panes |
| `t` | cycle the theme | appearance |
| `L` | choose a layout | appearance |
| `/` | search | search |
| `n` / `N` | repeat the last search, forward / reversed | search |
| `:` | command | command |

Vim motions carry over exactly: `h`/`j`/`k`/`l` and the arrow keys are the
same binding under different names (an arrow key is `pinned`, its letter
equivalent is not, so a future rebind of `j` can never make the dashboard
unnavigable). `d` (dashboard) and bare `l` opening the log are both
deliberately retired -- the six content tabs (`1`-`6` in the tab bar) are
reached with `gt`/`gT`/`{n}gt` instead, described below, and `l` is a pure
motion key now.

**Counts.** Typing digits before a key multiplies it, exactly as in Vim:
`5j` moves down five rows, `10G` jumps to row ten from the top rather than
the very bottom. Counts saturate at `9999` rather than overflowing on a
mistyped extra digit. Two bindings are count-aware in a more specific way:
`{n}gg` (or `{n}G`) jumps to row `n` instead of the very top/bottom, and
`{n}gt` jumps directly to the `n`th tab -- `3gt` goes straight to Weekly
(the tab bar's own `3`) without stepping through Daily first. A bare `0`
with no count already typed is its own binding (`LineStart`), not the start
of a count -- the same rule Vim itself uses to let `0` mean two different
things depending on what came before it.

**Chords.** `g` alone opens a two-key chord: the which-key popup shows the
`g`-prefixed keys available (`gg`, `gt`, `gT`) while it is held, and any key
that is not one of them cancels the chord silently rather than doing
something unexpected. `Ctrl-C` aborts whatever is pending (a half-typed
count, an open chord) without quitting, and only quits outright when
nothing is pending and no overlay is open -- so `5<Ctrl-C>` clears the `5`
rather than closing the dashboard.

**Search mode** (`/`) is a real line editor, not a single-shot prompt:
printable characters append, `Backspace` deletes (and exits search on an
empty buffer, restoring exactly where you were), `Ctrl-u` clears the whole
line, `Ctrl-w` deletes the last word, `Esc` cancels and restores your
position, and `Enter` runs a case-insensitive substring search over the
current view (log entry text, or a session's id and project directory in
the session picker) and jumps to the first match at or after the current
position, wrapping around if nothing is found before the end. `n`/`N`
repeat that search forward/backward from wherever you are now, always
moving to a *different* row than the one you are on.

**Command mode** (`:`) accepts:

| Command | Does |
|---|---|
| `:q`, `:quit` | quit |
| `:help` | open the help overlay |
| `:view <name>` | jump to `dashboard`, `daily`, `weekly`, `monthly`, `blocks` or `log` |
| `:theme <name>` | switch to a theme by its registry name and persist it |

Anything else sets a footer notice naming what was typed rather than
silently doing nothing, on the same principle `--online` being refused
follows elsewhere in this tool: a command that looks like it did something
and did not is worse than one that says so.

### Config file

`claude-stats` writes and reads
`${XDG_CONFIG_HOME:-~/.config}/claude-stats/config.json` -- the same
directory `prices.json` (see [Correcting prices](#correcting-prices) above)
lives in, on both Linux and macOS. Nothing here is required: a dashboard
that has never seen this file behaves exactly like one reading
`{ }`.

Every field is optional and unrecognised fields are ignored, so a partially
hand-written file and one the theme/layout pickers wrote for you can both be
edited by hand without a round trip losing anything either side did not
touch:

```json
{
  "theme": "aurora",
  "layout": "live",
  "animation": "pulse",
  "keymap": {
    "bind": [
      { "keys": "gt", "action": "next-view" }
    ]
  },
  "layouts": {
    "my-layout": { "type": "panel", "panel": "panel.spend-panel" }
  }
}
```

* **`theme`** -- one of the twenty-seven names under [Themes](#themes).
  Written automatically the moment you stop cycling with `t`, or set by hand.
* **`layout`** -- `"live"` (the default), `"spend"`, `"minimal"`, `"wide"`,
  or a key of `layouts` below. Written automatically by the layout picker
  (`L`). See [Layouts](#layouts) for what each preset shows and how a custom
  one is shaped.
* **`animation`** -- `"pulse"` (the default), `"coin"` or `"off"`. See
  [The dollar animation](#the-dollar-animation) below.
* **`keymap.bind`** -- key binding overrides, in Vim notation (`"j"`,
  `"<C-d>"`, `"<Esc>"`, `"gt"`). The file **loads and validates** today, and
  a malformed `keys` string is caught and reported, but a binding here does
  not yet change what a key does inside the dashboard -- there is no table
  yet mapping an `action` string like `"next-view"` back onto the keymap's
  own `NormalAction` type. This is a known, disclosed gap rather than a
  silently broken feature: see the module doc on
  [`src/infrastructure/config/mod.rs`](src/infrastructure/config/mod.rs).
* **`layouts`** -- named custom layout trees, in the same shape
  [Layouts](#layouts) describes. They are loaded, listed in the layout
  picker, and persist correctly when chosen, but the live dashboard does
  not yet render a custom tree -- selecting one currently falls back to
  `"live"` until a future release wires the conversion in. Also disclosed,
  not silent: see `App::confirm_layout_picker`'s doc comment in
  [`src/tui/app.rs`](src/tui/app.rs).

**What happens when it is malformed.** Two different kinds of "wrong" are
handled two different ways, deliberately:

* **The file is not valid JSON at all** -- a stray comma, an unclosed
  brace. `claude-stats` prints one line to `stderr` naming the file, the
  underlying error, and the exact line and column, then starts the
  dashboard anyway with every setting at its default. A short warning also
  appears in the dashboard's own footer for the first key press, so it is
  not missed by someone who was not watching the terminal it started from.
  Nothing here ever aborts the dashboard over a config typo -- the file
  the composition root refuses to start over is `prices.json`, not this
  one, because a wrong theme name is cosmetic and a wrong price you
  believe is correct is not.
* **The JSON is well-formed but a value does not resolve to anything
  real** -- `"theme": "aurroa"`, `"layout": "not-a-real-preset"`. Each
  such field is individually downgraded to "use the default" with its own
  warning; a typo in `theme` does not also throw away a perfectly good
  `layout`.

A file that is missing outright is not a warning at all -- almost nobody
writes this file by hand, and treating its absence as noteworthy would make
the overwhelmingly common case noisy for no reason.

### Layouts

The dashboard is not one hand-drawn screen any more: it is a tree of named
panels ([`src/tui/panels.rs`](src/tui/panels.rs)), arranged by a small
layout engine ([`src/tui/layout.rs`](src/tui/layout.rs)) that solves for
"how big is each panel, and does it even fit" the same way a browser's flex
layout does -- each panel declares its own honest minimum size and whether
it can stretch, and panels are dropped, lowest-priority first, until
whatever survives fits the terminal you actually have. That is what makes
the resize behaviour in [What it shows](#what-it-shows-and-why-each-number-is-there)
possible: nothing here is a fixed pixel grid.

**The panel catalogue.** Every panel is looked up by a string id:

| Id | What it draws | Minimum size (cols × rows) |
|---|---|---|
| `tile.context`, `tile.cost`, `tile.cache`, `tile.compaction`, `tile.turns`, `tile.errors` | one headline stat tile each | 14 × 4 |
| `panel.tile-row` | all six tiles above, fused into one strip | 36 × 4 |
| `panel.context-gauge` | the context window bar | 40 × 4 |
| `panel.context-banner` | a compact context readout for a tight terminal | 20 × 4 |
| `panel.account-usage` | the 5h/7d/monthly account usage windows | 40 × 4 |
| `panel.spend-panel` | today's spend, the running block, top projects | 30 × 11 |
| `panel.output-trend` | the output sparkline and the cache/efficiency/context meters | 30 × 6 |
| `panel.token-mix` | the cache-read-vs-write pie chart | 20 × 8 |
| `panel.tool-feed` | the live tool activity feed | 30 × 5 |
| `panel.this-turn` | tool/thinking/error counters for the turn in progress | 24 × 4 |
| `panel.daily-spend-chart` | the last seven days' spend, as a bar chart | 40 × 8 |
| `panel.model-breakdown` | which models contributed to the last seven days | 24 × 6 |
| `panel.burn-rate-gauge` | tokens/minute on the running billing block | 30 × 5 |
| `panel.top-projects` | the busiest projects over the last seven days | 24 × 6 |
| `panel.dollar-pulse` | the animated "$" marker -- see [below](#the-dollar-animation) | 10 × 4 |

**The four shipped presets** (`src/tui/layout/presets.rs`):

* **`live`** (the default, shown throughout this README) -- the tile row,
  the context gauge, the account/spend row, and the output-trend/token-mix
  and tool-feed/this-turn columns beneath it.
* **`spend`** -- leads with the cost and compaction tiles, puts the dollar
  animation beside the burn-rate gauge, the spend panel beside the daily
  spend chart, and the top projects beside the model breakdown. The one
  preset that shows `panel.dollar-pulse` today.
* **`minimal`** -- three tiles (context, cost, compaction) side by side and
  nothing else, for a terminal too narrow to spare room for anything more
  than "am I about to run out of something".
* **`wide`** -- every panel this crate ships, spread across four columns,
  for an ultrawide terminal.

Switch presets with `L` (opens a picker listing all four plus any custom
ones from `config.json`'s `layouts`), or set `"layout"` in the config file
directly (see [Config file](#config-file) above) -- either way the choice
persists.

**Defining a custom layout** means writing a `Node` tree under
`config.json`'s `layouts` key: a leaf is `{ "type": "panel", "panel":
"<id from the table above>" }`, and a split is `{ "type": "split", "axis":
"row" | "column", "children": [ { "size": ..., "node": ... }, ... ] }`,
where each child's `size` is one of `{ "fixed": <rows-or-cols> }`,
`{ "weight": <share> }` (siblings' weights are compared to each other, not
to any absolute number) or `{ "min": <rows-or-cols> }`. A two-panel row,
spend panel on the left weighted wider than the token mix on the right:

```json
{
  "layouts": {
    "my-layout": {
      "type": "split",
      "axis": "row",
      "children": [
        { "size": { "weight": 60 }, "node": { "type": "panel", "panel": "panel.spend-panel" } },
        { "size": { "weight": 40 }, "node": { "type": "panel", "panel": "panel.token-mix" } }
      ]
    }
  },
  "layout": "my-layout"
}
```

An unknown panel id or a malformed axis is caught when the file is read,
with a warning naming the field -- not a panic partway through drawing a
frame. See [Config file](#config-file) above for the one honest gap here
today: a custom tree loads, validates, and is listed in the picker, but the
live dashboard does not yet render it -- selecting one falls back to `live`
until that wiring lands.

### The dollar animation

`panel.dollar-pulse` -- shown today only in the `spend` layout preset (see
[Layouts](#layouts) above) -- is a big animated "$" that reflects how close
today's spend is to the busiest day in your last seven, so a glance tells
you whether today is quiet or already your worst day this week without
reading a number. Three treatments, set by `"animation"` in `config.json`
(see [Config file](#config-file)) or `:` command mode is not wired to it
directly today -- edit the file, or use the layout picker's own theme-style
persistence once a future release exposes it there too:

* **`pulse`** (the default) -- fills and drains like a thermometer, colour
  drawn from the same gradient the context gauge uses.
* **`coin`** -- narrows to an edge-on sliver and back, like a spinning coin
  seen side-on.
* **`off`** -- one glyph, one colour, never moves.

**Turning it off** does not require editing a file. Set either environment
variable, and it wins outright regardless of what `config.json` says --
this is a hard switch, not a suggestion, for anyone who finds terminal
animation distracting or is running inside a recording/CI context where a
redrawing glyph is noise:

```bash
NO_ANIMATION=1 claude-stats
# or
CLAUDE_STATS_NO_ANIMATION=1 claude-stats
```

Both are read once, at startup, the same way any other environment-gated
behaviour in this tool is -- see [Reading from more than one Claude Code
install](#reading-from-more-than-one-claude-code-install) above for the
same "checked once at startup, not re-read every frame" convention applied
to `CLAUDE_CONFIG_DIR`.

---

## How it works

Claude Code writes every session to a [JSON Lines](https://jsonlines.org) file under
`~/.claude/projects/<encoded-project-dir>/<session-id>.jsonl`, appending one
self-describing object per entry as the session runs. `claude-stats` reads that file. It does
not talk to the Anthropic API, does not proxy anything, and does not need a key.

The format is neither versioned nor documented, so the parser treats every field as
optional, ignores unknown ones, and skips a malformed line rather than failing — the last
line of a live transcript is routinely half-written, and refusing to draw because of it
would break the tool exactly when it is most wanted.

Changes are noticed two ways at once: a filesystem watcher for instant wake-ups, and a
modification-time-plus-length fingerprint as a safety net for network filesystems,
exhausted inotify limits, and files replaced rather than appended to.

### Layout of the code

Dependencies point inwards only.

| Module | Holds | Knows about |
|---|---|---|
| `domain` | what a session, a token, a dollar and a context window *are* | nothing |
| `application` | the use cases, written against traits | `domain` |
| `infrastructure` | reading Claude Code's storage | `domain`, `application` |
| `tui` | the terminal presentation | `domain` |
| `main.rs` | the composition root — the only place a concrete adapter meets a port | everything |

The point of the split is that the interesting logic here is arithmetic on token counts,
and arithmetic deserves to be testable without starting a terminal. All 400+ tests run in a
few hundred milliseconds, including the ones that render whole screens into an off-screen
buffer and assert on the characters that came out.

---

## Widget stack

Built on [ratatui](https://ratatui.rs) 0.30, with:

- **[tui-big-text](https://github.com/joshka/tui-big-text)** — the oversized context
  percentage.
- **[tui-piechart](https://docs.rs/tui-piechart)** — the token-mix chart, at braille
  resolution.

Two crates originally planned for this — `ratatui-icons` and `ratatui-spinner` — turned out
to be **name reservations on crates.io**, both published at version 0.0.0 with no code in
them. The icon set and the spinner are implemented in this crate instead, in
`tui/icons.rs` and `tui/widgets/spinner.rs`.

Owning the icon set turned out to be worth doing anyway, because it allows one rule to be
enforced everywhere: **plain Unicode only, no Nerd Font**. Nerd Font glyphs live in the
private-use area and render as empty boxes for anyone without a patched font. There is a
test asserting no glyph strays in there, and another asserting every glyph is exactly one
character wide.

The context gauge, the sparkline, the meters and the stat tiles are all local widgets too,
each because the ratatui built-in could not do the one thing that made it worth drawing —
a gradient along the bar's length, a compaction boundary marked on the trace, sub-cell
precision so a bar does not jump 3% at a time.

---

## Prior art

This is a Rust reimplementation of the monitor from
[slima4/claude-tui](https://github.com/slima4/claude-tui), a Python toolkit for Claude Code
that also ships a statusline, an API sniffer and a session manager. The transcript format
and the metric vocabulary — context fill, cache ratio, compaction distance — come from
there. Go and look at it; it does considerably more than this does.

## Licence

MIT

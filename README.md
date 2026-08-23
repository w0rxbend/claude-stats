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

```
 claude-stats  ⠹ working  │ ◇ Opus 5  │ ▤ ~/code/payments-api  │ ⎇ feat/webhooks  │ ◷ 2h13m

╭ ◴ CONTEXT ────────────╮╭ ¤ COST ───────────────╮╭ ⧉ CACHE ──────────────╮╭ ↯ COMPACTION ─────────╮
│ 71.4%                 ││ $18.37                ││ 98.0%                 ││ ~4 turns              │
│ 714.2k / 1.00M        ││ $1.53/turn            ││ 22.97M read           ││ 1 so far              │
╰───────────────────────╯╰───────────────────────╯╰───────────────────────╯╰───────────────────────╯
╭ context window ──────────────────────────────────────────────────────────────────────────────────╮
│ ██████████████████████████████████████████████████████████▌░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░↓░░░░ │
│ ◇ used 714.2k   ◴ free 285.8k   ↯ until compaction 252.8k   ↗ growth/turn 61.4k                  │
╰──────────────────────────────────────────────────────────────────────────────────────────────────╯
╭ output per response ───────────────────────╮╭ live tool activity ────────────────────────────────╮
│              ▄█▀▀█▄  ▄█▀▀   ▄▄   ▄▄        ││ ⠸ edit webhook_handler.rs                          │
│                ▄▄█▀ ██▄▄▄   ▀▀ ▄█▀         ││ ▶ bash cargo test --lib                            │
│              ▄█▀ ▄▄ ██  ██   ▄█▀ ▄▄        ││ ⌕ grep verify_signature                            │
│              ▀▀▀▀▀▀  ▀▀▀▀   ▀▀   ▀▀        ││ ◈ read webhook_handler.rs                          │
│                  ██       ▂     ▆          ││ ⚠ bash cargo build                                 │
│         ▆▆       ██    ▃  █  ▃  █          ││ ✦ agent audit the retry path                       │
│    ▂▂█▇██ │▃███▆▇ █ ▃ █▅▃▄▄▄▁▂▂█▅▄█▂▇▅▅▃▁▇ ││ ◈ read retry.rs                                    │
│ cache      ███████████████████████▌  98.0% ││ ▶ bash git status --short                           │
│ efficiency ██████████████████░░░░░░     76%│╰────────────────────────────────────────────────────╯
│ context    █████████████████▏░░░░░░░ 71.4% │╭ this turn ─────────────────────────────────────────╮
╰────────────────────────────────────────────╯│ ◇ tools 12   ∿ thinking 4   ⚠ errors 1             │
╭ token mix ─────────────────────────────────╮│ ‣ 1 sub-agent(s) running                           │
│         ⣀⣴⣶⣿⣿⣿⣿⣿⣿⣷⣶⣄⡀    ■ cache read 97.1%││ ⚠ error: Exit code 101                             │
│      ⢀⣴⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣷⣄  ■ cache write 2.0%│╰────────────────────────────────────────────────────╯
╰────────────────────────────────────────────╯
 q quit   l log   o sessions   ? help
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
CLAUDETUI_VERSION=v0.1.0 curl -fsSL .../install.sh | sh   # pin a version
CLAUDETUI_INSTALL_DIR=/usr/local/bin curl -fsSL .../install.sh | sh
```

The installer never escalates to `sudo` on its own. If you want it somewhere privileged,
say so with `CLAUDETUI_INSTALL_DIR` and run it under `sudo` yourself.

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

`stats --json` emits unabbreviated token counts and dollar amounts, so it can be summed
across sessions:

```bash
claude-stats sessions --limit 100 | tail -n +2 | awk '{print $1}' |
  xargs -I{} claude-stats stats --json --session {} |
  jq -s 'map(.cost_usd) | add'
```

### Keys

| Key | Does |
|---|---|
| `q`, `Ctrl-C` | quit |
| `Esc` | close the help, then leave the view, then quit |
| `d` | dashboard |
| `l` | event log |
| `o` | session picker |
| `Enter` | attach to the selected session |
| `j` / `k` | move down / up |
| `g` / `G` | jump to the oldest / newest entry |
| `?` | help |

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
and arithmetic deserves to be testable without starting a terminal. All 117 tests run in
about twenty milliseconds, including the ones that render whole screens into an off-screen
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

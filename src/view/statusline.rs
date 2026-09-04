//! Rendering a [`StatuslineLine`] into the one line Claude Code prints in its
//! prompt.
//!
//! A Transform View in Fowler's sense: one pure function, no state of its
//! own, turning a value object into text. It is deliberately the only place
//! in the crate that decides what a statusline looks like, so the golden
//! format below is the single thing a change to the layout has to agree with.
//!
//! Nothing here carries ANSI colour. A statusline is embedded in somebody
//! else's prompt -- their own theme, their own escape sequences already in
//! flight -- and painting over any of that would be a much worse failure than
//! a plain line that reads correctly next to whatever else is there.

use crate::application::statusline::{BurnSegment, StatuslineLine};
use crate::domain::blocks::Intensity;
use crate::domain::context::ContextFill;
use crate::view::format;

/// How the burn segment's intensity is shown, beyond the bare rate.
///
/// The command-line spelling is `--visual-burn-rate <off|emoji|text|emoji-text>`;
/// this is that flag's meaning once translated out of the string clap parsed,
/// the same separation [`crate::cli::RowOrder`] keeps from
/// [`crate::domain::period::Order`]. Kept in the view layer rather than the
/// domain because which characters a rate is followed by is a presentation
/// choice with no bearing on what the rate *is* -- [`Intensity`] already says
/// that, and this only says how loudly to print it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BurnDisplay {
    /// The bare rate, and nothing else.
    #[default]
    Off,
    /// The rate followed by a coloured-circle emoji.
    Emoji,
    /// The rate followed by a bracketed word.
    Text,
    /// Both together.
    EmojiText,
}

/// Printed for a session with no figure to report a cost from.
///
/// Distinct from `$0.00` on purpose: the two mean different things, and
/// [`StatuslineLine::session_cost`] is `None` rather than [`Usd::ZERO`]
/// specifically so the view is not left to guess which one it has.
///
/// [`Usd::ZERO`]: crate::domain::money::Usd::ZERO
const NOT_AVAILABLE: &str = "N/A";

/// Printed when [`StatuslineLine::block`] is absent.
const NO_ACTIVE_BLOCK: &str = "No active block";

/// Renders `line` exactly as Claude Code's prompt will show it.
///
/// `burn_display` is a second parameter beyond the one the specification for
/// this command names, and deliberately so: the burn segment can be followed
/// by an intensity marker whose shape is chosen at the command line
/// (`--visual-burn-rate`), and a function that cannot be told which shape was
/// asked for cannot render it. Every other input the format needs already
/// lives on `line`.
#[must_use]
pub fn render(line: &StatuslineLine, burn_display: BurnDisplay) -> String {
    let model = model_label(line);
    let session = line
        .session_cost
        .map_or_else(|| NOT_AVAILABLE.to_owned(), format::money);
    let today = format::money(line.today_cost);
    let block = block_segment(line);
    let burn = line
        .burn
        .as_ref()
        .map_or_else(String::new, |burn| burn_segment(burn, burn_display));
    let context = context_segment(line.context);

    format!("🤖 {model} | 💰 {session} session / {today} today / {block}{burn} | 🧠 {context}")
}

/// The model name, with the effort level appended when the hook gave one.
fn model_label(line: &StatuslineLine) -> String {
    match &line.effort {
        Some(effort) => format!("{} ({effort})", line.model),
        None => line.model.clone(),
    }
}

/// `$0.45 block (2h 45m left)`, or the fixed sentence for no running window.
fn block_segment(line: &StatuslineLine) -> String {
    match &line.block {
        Some(block) => format!(
            "{} block ({} left)",
            format::money(block.cost),
            remaining(block.remaining)
        ),
        None => NO_ACTIVE_BLOCK.to_owned(),
    }
}

/// `2h 45m`, spaced rather than run together so it reads at a glance next to
/// the rest of a prompt that was not designed around this tool's own
/// formatting choices.
fn remaining(span: chrono::Duration) -> String {
    let minutes = span.num_minutes().max(0);
    format!("{}h {}m", minutes / 60, minutes % 60)
}

/// ` | 🔥 $0.12/hr`, optionally followed by an intensity marker.
fn burn_segment(burn: &BurnSegment, display: BurnDisplay) -> String {
    let rate = format::money(burn.cost_per_hour);
    let marker = match display {
        BurnDisplay::Off => String::new(),
        BurnDisplay::Emoji => format!(" {}", emoji_for(burn.intensity)),
        BurnDisplay::Text => format!(" ({})", word_for(burn.intensity)),
        BurnDisplay::EmojiText => {
            format!(
                " {} ({})",
                emoji_for(burn.intensity),
                word_for(burn.intensity)
            )
        }
    };
    format!(" | 🔥 {rate}/hr{marker}")
}

/// The coloured-circle marker for a burn intensity.
const fn emoji_for(intensity: Intensity) -> &'static str {
    match intensity {
        Intensity::Normal => "🟢",
        Intensity::Moderate => "⚠️",
        Intensity::High => "🚨",
    }
}

/// The plain-word marker for a burn intensity.
const fn word_for(intensity: Intensity) -> &'static str {
    match intensity {
        Intensity::Normal => "normal",
        Intensity::Moderate => "moderate",
        Intensity::High => "high",
    }
}

/// `25,000 (12%)`, or [`NOT_AVAILABLE`] when there is no reading at all.
fn context_segment(context: Option<ContextFill>) -> String {
    match context {
        Some(fill) => format!(
            "{} ({})",
            format::grouped(fill.used()),
            format::percent(fill.ratio())
        ),
        None => NOT_AVAILABLE.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::domain::money::Usd;

    fn full_line() -> StatuslineLine {
        StatuslineLine {
            model: "Opus 5".to_owned(),
            effort: Some("high".to_owned()),
            session_cost: Some(Usd::new(2.34)),
            today_cost: Usd::new(12.50),
            block: Some(crate::application::statusline::BlockSegment {
                cost: Usd::new(0.45),
                remaining: Duration::minutes(165),
            }),
            burn: Some(BurnSegment {
                cost_per_hour: Usd::new(0.12),
                intensity: Intensity::Normal,
            }),
            context: Some(ContextFill::new(24_000, 200_000)),
        }
    }

    #[test]
    fn the_status_line_matches_the_documented_format_exactly() {
        let rendered = render(&full_line(), BurnDisplay::Off);

        assert_eq!(
            rendered,
            "🤖 Opus 5 (high) | 💰 $2.34 session / $12.50 today / \
             $0.45 block (2h 45m left) | 🔥 $0.12/hr | 🧠 24,000 (12%)"
        );
    }

    #[test]
    fn a_session_with_no_active_block_says_so_and_omits_the_burn_segment() {
        let mut line = full_line();
        line.block = None;
        line.burn = None;

        let rendered = render(&line, BurnDisplay::Off);

        assert!(
            rendered.contains("No active block"),
            "must say plainly that nothing is running: {rendered}"
        );
        assert!(
            !rendered.contains('🔥'),
            "a rate with nothing to measure it across must not appear: {rendered}"
        );
    }

    #[test]
    fn a_missing_context_reading_renders_not_available_rather_than_zero_percent() {
        let mut line = full_line();
        line.context = None;

        let rendered = render(&line, BurnDisplay::Off);

        assert!(
            rendered.ends_with("🧠 N/A"),
            "an unmeasured window must not be claimed to be empty: {rendered}"
        );
        assert!(!rendered.contains("(0%)"));
    }

    #[test]
    fn a_missing_session_cost_renders_not_available_rather_than_zero_dollars() {
        let mut line = full_line();
        line.session_cost = None;

        let rendered = render(&line, BurnDisplay::Off);

        assert!(
            rendered.contains("N/A session"),
            "no session to measure must not be claimed to have cost nothing: {rendered}"
        );
    }

    #[test]
    fn the_visual_burn_rate_flag_chooses_what_follows_the_rate() {
        let line = full_line();

        assert!(render(&line, BurnDisplay::Off).ends_with("🧠 24,000 (12%)"));
        assert!(render(&line, BurnDisplay::Off).contains("$0.12/hr |"));

        let emoji = render(&line, BurnDisplay::Emoji);
        assert!(emoji.contains("$0.12/hr 🟢"), "{emoji}");

        let text = render(&line, BurnDisplay::Text);
        assert!(text.contains("$0.12/hr (normal)"), "{text}");

        let both = render(&line, BurnDisplay::EmojiText);
        assert!(both.contains("$0.12/hr 🟢 (normal)"), "{both}");
    }

    #[test]
    fn a_model_with_no_effort_level_is_printed_alone() {
        let mut line = full_line();
        line.effort = None;

        let rendered = render(&line, BurnDisplay::Off);

        assert!(rendered.starts_with("🤖 Opus 5 |"), "{rendered}");
    }
}

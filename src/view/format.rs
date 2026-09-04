//! Turning numbers into the strings a report or a dashboard has room for.
//!
//! Most of these trade precision for width on purpose. A context figure of
//! `953429` is six characters of which only the first two carry any meaning at
//! a glance; `953.4k` says the same thing in the same space and reads
//! instantly. The full values are still in the domain for anything that needs
//! them.
//!
//! [`grouped`] and [`money`] are the exceptions, and they are here for the
//! other audience. A dashboard is watched and wants the shape of a number; a
//! table is read once, checked against an invoice, and pasted into an issue,
//! and it wants the number itself. Both live in the same module because a
//! crate with two formatting modules ends up with two spellings of the same
//! figure.
//!
//! This is the view layer rather than the terminal one. Nothing here knows
//! about ratatui, crossterm or a screen, which is what lets the text reports
//! use it without importing the dashboard.

use chrono::Duration;

use crate::domain::money::Usd;

/// Renders a token count compactly: `1234` -> `1.2k`, `1234567` -> `1.23M`.
///
/// Counts below a thousand are printed exactly, because at that scale the
/// individual digits still mean something.
///
/// The billions step matters once counts are summed across sessions rather
/// than taken from one: a week of work runs to thousands of millions, and
/// `2564.06M` is a number nobody can read at a glance.
#[must_use]
pub fn tokens(count: u64) -> String {
    // These bounds are rounding boundaries, not unit boundaries, and the
    // difference matters. 999_950 tokens divided by a thousand is 999.95,
    // which `{:.1}` rounds to "1000.0" -- five digits where the whole point of
    // the unit was to show four. So a count promotes to the next unit as soon
    // as it would *round* into it, not when it reaches it. Do not "tidy" these
    // back to 1_000_000 and 1_000_000_000.
    match count {
        n if n < 1_000 => n.to_string(),
        n if n < 999_950 => format!("{:.1}k", n as f64 / 1_000.0),
        n if n < 999_995_000 => format!("{:.2}M", n as f64 / 1_000_000.0),
        n => format!("{:.2}B", n as f64 / 1_000_000_000.0),
    }
}

/// Renders a count in full, with thousands separators: `1234567` ->
/// `1,234,567`.
///
/// The counterpart to [`tokens`], for the tables rather than the tiles. A
/// daily report is read against an invoice or pasted into an issue, and
/// `1.23M` cannot be checked against anything -- it stands for any of ten
/// thousand different counts. The separators are what make a seven-digit
/// figure scannable without them.
///
/// Grouping is done by walking the digits from the right rather than by a
/// locale library, because the crate has no locale to consult and inventing
/// one would mean a report that reads differently on two machines that ran the
/// same command.
#[must_use]
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Renders an amount as `$12.34`.
///
/// Always two decimals, unlike [`Usd`]'s own `Display`, which widens to four
/// for sub-cent amounts so that the first few turns of a live session do not
/// read `$0.00`. A table has no such problem and a positive reason not to: a
/// column where most cells are `$12.34` and one is `$0.0007` does not line up
/// on the decimal point, and a column of money that does not line up is a
/// column nobody can add up by eye.
#[must_use]
pub fn money(amount: Usd) -> String {
    format!("${:.2}", amount.dollars())
}

/// Renders a ratio in `0.0..=1.0` as a whole percentage: `0.984` -> `98%`.
#[must_use]
pub fn percent(ratio: f64) -> String {
    format!("{:.0}%", ratio * 100.0)
}

/// Renders a ratio with one decimal: `0.9838` -> `98.4%`.
///
/// Used where the figure moves slowly and the tenth is the interesting part --
/// a cache ratio creeping from 98.4% to 98.1% is a real signal that whole
/// percentages would hide.
#[must_use]
pub fn percent_precise(ratio: f64) -> String {
    format!("{:.1}%", ratio * 100.0)
}

/// Renders an elapsed duration as `45s`, `12m`, `2h13m` or `3d4h`.
///
/// Each unit pair is chosen so the string never exceeds six characters, which
/// is what the header has room for. Six rather than five because a session can
/// legitimately run to `23h59m`, and dropping the minutes past ten hours to
/// save one column would lose real information from the longest sessions --
/// exactly the ones whose elapsed time is worth reading.
#[must_use]
pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.num_seconds().max(0);
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h{:02}m", s / 3_600, (s % 3_600) / 60),
        s => format!("{}d{}h", s / 86_400, (s % 86_400) / 3_600),
    }
}

/// Shortens a session id to the eight characters the listings have room for.
///
/// Cuts on a character boundary rather than a byte one. A session id is not a
/// validated UUID -- it is the file stem of any `*.jsonl` under the projects
/// directory, read lossily -- so a byte cut can land mid-character and panic,
/// taking the running dashboard down mid-render.
///
/// Not [`fit`], which would spend one of the eight columns on an ellipsis.
#[must_use]
pub fn session_id(id: &str) -> &str {
    match id.char_indices().nth(SESSION_ID_CHARS) {
        Some((byte, _)) => &id[..byte],
        None => id,
    }
}

/// How much of a session id the listings show.
const SESSION_ID_CHARS: usize = 8;

/// Shortens `text` to `width` characters, ending in an ellipsis if it was cut.
///
/// Truncates from the *left* when `keep_end` is set, which is what paths want:
/// the tail of `/very/long/path/to/money.rs` is the informative half.
#[must_use]
pub fn fit(text: &str, width: usize, keep_end: bool) -> String {
    let length = text.chars().count();
    if length <= width {
        return text.to_owned();
    }
    if width <= 1 {
        return "\u{2026}".to_owned();
    }
    if keep_end {
        let tail: String = text.chars().skip(length - (width - 1)).collect();
        format!("\u{2026}{tail}")
    } else {
        let head: String = text.chars().take(width - 1).collect();
        format!("{head}\u{2026}")
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_session_id_is_cut_on_a_character_boundary_not_a_byte_one() {
        assert_eq!(session_id("0f3a9c21-1b2c-4d5e"), "0f3a9c21");
        assert_eq!(
            session_id("short"),
            "short",
            "shorter than eight is left alone"
        );
        // A transcript file name is not a validated UUID. Cutting this by
        // bytes would land mid-character and panic.
        assert_eq!(session_id("sesión-de-prueba").chars().count(), 8);
    }
    use super::*;

    #[test]
    fn token_counts_switch_units_at_a_thousand_and_a_million() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_234), "1.2k");
        assert_eq!(tokens(953_429), "953.4k");
        assert_eq!(tokens(1_234_567), "1.23M");
        assert_eq!(
            tokens(999_949),
            "999.9k",
            "the largest count that really is thousands"
        );
        assert_eq!(
            tokens(999_950),
            "1.00M",
            "rounds up into millions rather than printing 1000.0k"
        );
        assert_eq!(
            tokens(999_994_999),
            "999.99M",
            "the largest count that really is millions"
        );
        assert_eq!(
            tokens(999_999_999),
            "1.00B",
            "rounds up into billions rather than printing 1000.00M"
        );
        assert_eq!(
            tokens(2_564_060_000),
            "2.56B",
            "a week's worth of tokens stays readable"
        );
    }

    #[test]
    fn token_counts_stay_within_seven_characters() {
        // The tiles that show these have a fixed column width, so a count
        // that renders one character too wide is a layout bug, not a cosmetic
        // one. The boundary values are the ones that used to overflow.
        for count in [
            0,
            999,
            1_000,
            999_949,
            999_950,
            1_234_567,
            999_994_999,
            999_999_999,
            2_564_060_000,
            // Billions is the last unit, so the guarantee stops just below
            // the trillion that would round to "1000.00B". No account has
            // ever come within three orders of magnitude of that.
            999_994_999_999,
        ] {
            let text = tokens(count);
            assert!(text.len() <= 7, "{text:?} is too wide for its column");
        }
    }

    #[test]
    fn durations_stay_within_six_characters() {
        for seconds in [0, 59, 60, 3_599, 3_600, 86_399, 86_400, 900_000] {
            let text = duration(Duration::seconds(seconds));
            assert!(text.len() <= 6, "{text:?} is too wide");
        }
    }

    #[test]
    fn a_path_is_truncated_from_the_left_so_the_file_name_survives() {
        assert_eq!(
            fit("/very/long/path/money.rs", 12, true),
            "\u{2026}th/money.rs"
        );
    }

    #[test]
    fn text_that_already_fits_is_left_alone() {
        assert_eq!(fit("short", 10, false), "short");
    }

    #[test]
    fn fitting_into_no_space_yields_an_ellipsis_rather_than_panicking() {
        assert_eq!(fit("anything", 1, false), "\u{2026}");
    }

    #[test]
    fn multibyte_text_is_cut_on_a_character_boundary() {
        let text = "\u{e4}\u{f6}\u{fc}\u{e4}\u{f6}\u{fc}";
        assert_eq!(fit(text, 4, false).chars().count(), 4);
    }

    #[test]
    fn a_grouped_count_is_exact_where_an_abbreviated_one_is_not() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(1_000_000_000), "1,000,000,000");
        assert_eq!(
            tokens(1_234_567),
            "1.23M",
            "the abbreviated form stands for any of ten thousand counts"
        );
    }

    #[test]
    fn money_is_always_two_decimals_so_a_column_lines_up() {
        assert_eq!(money(Usd::new(12.34)), "$12.34");
        assert_eq!(money(Usd::ZERO), "$0.00");
        // The live dashboard widens this one to `$0.0007`; a table must not,
        // or the decimal points stop lining up down the column.
        assert_eq!(money(Usd::new(0.000_7)), "$0.00");
        assert_eq!(money(Usd::new(1_234.5)), "$1234.50");
    }
}

//! Turning numbers into the short strings a dense dashboard has room for.
//!
//! Every one of these trades precision for width on purpose. A context figure
//! of `953429` is six characters of which only the first two carry any meaning
//! at a glance; `953.4k` says the same thing in the same space and reads
//! instantly. The full values are still in the domain for anything that needs
//! them.

use chrono::Duration;

/// Renders a token count compactly: `1234` -> `1.2k`, `1234567` -> `1.23M`.
///
/// Counts below a thousand are printed exactly, because at that scale the
/// individual digits still mean something.
#[must_use]
pub fn tokens(count: u64) -> String {
    match count {
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => format!("{:.2}M", n as f64 / 1_000_000.0),
    }
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
    use super::*;

    #[test]
    fn token_counts_switch_units_at_a_thousand_and_a_million() {
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_234), "1.2k");
        assert_eq!(tokens(953_429), "953.4k");
        assert_eq!(tokens(1_234_567), "1.23M");
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
        assert_eq!(fit("/very/long/path/money.rs", 12, true), "\u{2026}th/money.rs");
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
}

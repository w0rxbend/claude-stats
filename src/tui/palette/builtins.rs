//! The twenty-seven palettes shipped with the crate.
//!
//! One function per theme, each returning a literal [`Palette`] -- there is
//! no cleverness to find here on purpose. A theme is somebody's aesthetic
//! judgement written down as seventeen hex values, and the only thing this
//! module is trusted to do with that judgement is transcribe it faithfully
//! and hand it to the [`super::registry::ThemeRegistry`]. Anything more
//! "helpful" -- generating one channel from another, say -- would make a
//! theme's colours a function of this code instead of a fact about the theme,
//! and the first bug report about a palette looking wrong would have to
//! start by ruling out the generator.
//!
//! [`Palette::name`](super::Palette::name) is the registry key
//! [`super::registry::ThemeRegistry::register`] files each theme under, so it
//! is kebab-case throughout: lookups, saved preferences and a future theme
//! picker's list all want one stable, typeable spelling per theme rather than
//! the display capitalisation used in prose.

use super::{Palette, Rgb};

/// Parses a `#rrggbb` (or `rrggbb`) literal into an [`Rgb`].
///
/// Panics on malformed input, which is deliberately the same trade-off
/// [`str::parse`] makes for a numeric literal: every hex value below is a
/// compile-time-known constant written by hand, so a malformed one is a typo
/// in this file, and the very first test run turns it into a loud panic
/// rather than a silently wrong colour shipped to every user of that theme.
fn hex(s: &str) -> Rgb {
    let s = s.strip_prefix('#').unwrap_or(s);
    assert!(s.len() == 6, "{s:?} is not a 6-digit hex colour");
    let channel = |from: usize| {
        u8::from_str_radix(&s[from..from + 2], 16)
            .unwrap_or_else(|_| panic!("{s:?} is not a valid hex colour"))
    };
    Rgb(channel(0), channel(2), channel(4))
}

/// Every built-in theme, in the order they are defined below.
#[must_use]
pub fn all() -> Vec<Palette> {
    vec![
        aurora(),
        catppuccin_mocha(),
        catppuccin_macchiato(),
        catppuccin_frappe(),
        catppuccin_latte(),
        tokyo_night(),
        tokyo_night_storm(),
        tokyo_night_day(),
        gruvbox_dark(),
        gruvbox_light(),
        nord(),
        dracula(),
        solarized_dark(),
        solarized_light(),
        rose_pine(),
        rose_pine_moon(),
        rose_pine_dawn(),
        everforest_dark(),
        kanagawa(),
        one_dark(),
        monokai(),
        ayu_dark(),
        ayu_mirage(),
        ayu_light(),
        high_contrast(),
        solstice(),
        terracotta(),
    ]
}

/// The dashboard's original palette: a cool indigo ground with cyan and
/// violet accents, and the warm amber-to-crimson ramp held back for pressure
/// alone. Every field here reproduces the RGB triple the old `Theme` unit
/// struct carried as a constant, so switching the registry's default to this
/// theme changes nothing about how the dashboard already looked.
fn aurora() -> Palette {
    Palette {
        name: "aurora".to_owned(),
        background: hex("#0b0d1a"),
        surface: hex("#131628"),
        overlay: hex("#05060d"),
        border: hex("#30365c"),
        border_active: hex("#7aa2ff"),
        text: hex("#dee4ff"),
        muted: hex("#7884b2"),
        faint: hex("#48527a"),
        inverted_text: hex("#0b0d1a"),
        accent_primary: hex("#56e2e8"),
        accent_secondary: hex("#a78bfa"),
        accent_success: hex("#5eeaa8"),
        accent_info: hex("#60a5fa"),
        accent_special: hex("#f472b6"),
        pressure_low: hex("#fbbf24"),
        pressure_mid: hex("#fb923c"),
        pressure_high: hex("#f87171"),
    }
}

fn catppuccin_mocha() -> Palette {
    Palette {
        name: "catppuccin-mocha".to_owned(),
        background: hex("#1e1e2e"),
        surface: hex("#313244"),
        overlay: hex("#11111b"),
        border: hex("#585b70"),
        border_active: hex("#b4befe"),
        text: hex("#cdd6f4"),
        muted: hex("#a6adc8"),
        faint: hex("#6c7086"),
        inverted_text: hex("#1e1e2e"),
        accent_primary: hex("#74c7ec"),
        accent_secondary: hex("#b4befe"),
        accent_success: hex("#a6e3a1"),
        accent_info: hex("#89b4fa"),
        accent_special: hex("#f5c2e7"),
        pressure_low: hex("#f9e2af"),
        pressure_mid: hex("#fab387"),
        pressure_high: hex("#f38ba8"),
    }
}

fn catppuccin_macchiato() -> Palette {
    Palette {
        name: "catppuccin-macchiato".to_owned(),
        background: hex("#24273a"),
        surface: hex("#363a4f"),
        overlay: hex("#181926"),
        border: hex("#5b6078"),
        border_active: hex("#b7bdf8"),
        text: hex("#cad3f5"),
        muted: hex("#a5adcb"),
        faint: hex("#6e738d"),
        inverted_text: hex("#24273a"),
        accent_primary: hex("#7dc4e4"),
        accent_secondary: hex("#b7bdf8"),
        accent_success: hex("#a6da95"),
        accent_info: hex("#8aadf4"),
        accent_special: hex("#f5bde6"),
        pressure_low: hex("#eed49f"),
        pressure_mid: hex("#f5a97f"),
        pressure_high: hex("#ed8796"),
    }
}

fn catppuccin_frappe() -> Palette {
    Palette {
        name: "catppuccin-frappe".to_owned(),
        background: hex("#303446"),
        surface: hex("#414559"),
        overlay: hex("#232634"),
        border: hex("#626880"),
        border_active: hex("#babbf1"),
        text: hex("#c6d0f5"),
        muted: hex("#a5adce"),
        faint: hex("#737994"),
        inverted_text: hex("#303446"),
        accent_primary: hex("#85c1dc"),
        accent_secondary: hex("#babbf1"),
        accent_success: hex("#a6d189"),
        accent_info: hex("#8caaee"),
        accent_special: hex("#f4b8e4"),
        pressure_low: hex("#e5c890"),
        pressure_mid: hex("#ef9f76"),
        pressure_high: hex("#e78284"),
    }
}

fn catppuccin_latte() -> Palette {
    Palette {
        name: "catppuccin-latte".to_owned(),
        background: hex("#eff1f5"),
        surface: hex("#e6e9ef"),
        overlay: hex("#ffffff"),
        border: hex("#acb0be"),
        border_active: hex("#687efd"),
        text: hex("#4c4f69"),
        muted: hex("#5c5f77"),
        faint: hex("#8c8fa1"),
        inverted_text: hex("#eff1f5"),
        accent_primary: hex("#1e97ac"),
        accent_secondary: hex("#687efd"),
        accent_success: hex("#3e9c2a"),
        accent_info: hex("#1e66f5"),
        accent_special: hex("#e44ebc"),
        pressure_low: hex("#bf7a19"),
        pressure_mid: hex("#ef5801"),
        pressure_high: hex("#d20f39"),
    }
}

fn tokyo_night() -> Palette {
    Palette {
        name: "tokyo-night".to_owned(),
        background: hex("#1a1b26"),
        surface: hex("#24283b"),
        overlay: hex("#16161e"),
        border: hex("#414868"),
        border_active: hex("#7aa2f7"),
        text: hex("#c0caf5"),
        muted: hex("#9aa5ce"),
        faint: hex("#565f89"),
        inverted_text: hex("#1a1b26"),
        accent_primary: hex("#7dcfff"),
        accent_secondary: hex("#bb9af7"),
        accent_success: hex("#9ece6a"),
        accent_info: hex("#7aa2f7"),
        accent_special: hex("#c678dd"),
        pressure_low: hex("#e0af68"),
        pressure_mid: hex("#ff9e64"),
        pressure_high: hex("#f7768e"),
    }
}

fn tokyo_night_storm() -> Palette {
    Palette {
        name: "tokyo-night-storm".to_owned(),
        background: hex("#24283b"),
        surface: hex("#2f334d"),
        overlay: hex("#1f2335"),
        border: hex("#414868"),
        border_active: hex("#7aa2f7"),
        text: hex("#c0caf5"),
        muted: hex("#a9b1d6"),
        faint: hex("#565f89"),
        inverted_text: hex("#24283b"),
        accent_primary: hex("#7dcfff"),
        accent_secondary: hex("#bb9af7"),
        accent_success: hex("#9ece6a"),
        accent_info: hex("#7aa2f7"),
        accent_special: hex("#c678dd"),
        pressure_low: hex("#e0af68"),
        pressure_mid: hex("#ff9e64"),
        pressure_high: hex("#f7768e"),
    }
}

fn tokyo_night_day() -> Palette {
    Palette {
        name: "tokyo-night-day".to_owned(),
        background: hex("#e1e2e7"),
        surface: hex("#d6d8e0"),
        overlay: hex("#ffffff"),
        border: hex("#a8aecb"),
        border_active: hex("#34548a"),
        text: hex("#3760bf"),
        muted: hex("#565a6e"),
        faint: hex("#7a80a0"),
        inverted_text: hex("#e1e2e7"),
        accent_primary: hex("#166775"),
        accent_secondary: hex("#5a4a78"),
        accent_success: hex("#385f0d"),
        accent_info: hex("#34548a"),
        accent_special: hex("#7c3fa3"),
        pressure_low: hex("#8f5e15"),
        pressure_mid: hex("#b34e11"),
        pressure_high: hex("#c64343"),
    }
}

/// `border_active` deliberately points at the aqua accent rather than
/// Gruvbox's stock orange -- that orange is `pressure_mid`, and reusing it for
/// a focused panel's border would make an ordinary state (something has
/// focus) look identical to a warning (something is running hot).
fn gruvbox_dark() -> Palette {
    Palette {
        name: "gruvbox-dark".to_owned(),
        background: hex("#282828"),
        surface: hex("#3c3836"),
        overlay: hex("#1d2021"),
        border: hex("#504945"),
        border_active: hex("#83a598"),
        text: hex("#ebdbb2"),
        muted: hex("#a89984"),
        faint: hex("#665c54"),
        inverted_text: hex("#282828"),
        accent_primary: hex("#83a598"),
        accent_secondary: hex("#d3869b"),
        accent_success: hex("#b8bb26"),
        accent_info: hex("#83a598"),
        accent_special: hex("#d3869b"),
        pressure_low: hex("#fabd2f"),
        pressure_mid: hex("#fe8019"),
        pressure_high: hex("#fb4934"),
    }
}

/// Same `border_active` fix as [`gruvbox_dark`], mirrored to the light theme's
/// aqua/blue.
fn gruvbox_light() -> Palette {
    Palette {
        name: "gruvbox-light".to_owned(),
        background: hex("#fbf1c7"),
        surface: hex("#ebdbb2"),
        overlay: hex("#f9f5d7"),
        border: hex("#d5c4a1"),
        border_active: hex("#076678"),
        text: hex("#3c3836"),
        muted: hex("#504945"),
        faint: hex("#7c6f64"),
        inverted_text: hex("#fbf1c7"),
        accent_primary: hex("#076678"),
        accent_secondary: hex("#8f3f71"),
        accent_success: hex("#79740e"),
        accent_info: hex("#076678"),
        accent_special: hex("#8f3f71"),
        pressure_low: hex("#b57614"),
        pressure_mid: hex("#af3a03"),
        pressure_high: hex("#9d0006"),
    }
}

fn nord() -> Palette {
    Palette {
        name: "nord".to_owned(),
        background: hex("#2e3440"),
        surface: hex("#3b4252"),
        overlay: hex("#242933"),
        border: hex("#4c566a"),
        border_active: hex("#88c0d0"),
        text: hex("#e5e9f0"),
        muted: hex("#9fadc4"),
        faint: hex("#616e88"),
        inverted_text: hex("#2e3440"),
        accent_primary: hex("#88c0d0"),
        accent_secondary: hex("#b48ead"),
        accent_success: hex("#a3be8c"),
        accent_info: hex("#81a1c1"),
        accent_special: hex("#b48ead"),
        pressure_low: hex("#ebcb8b"),
        pressure_mid: hex("#d08770"),
        pressure_high: hex("#bf616a"),
    }
}

fn dracula() -> Palette {
    Palette {
        name: "dracula".to_owned(),
        background: hex("#282a36"),
        surface: hex("#343746"),
        overlay: hex("#1e1f29"),
        border: hex("#44475a"),
        border_active: hex("#bd93f9"),
        text: hex("#f8f8f2"),
        muted: hex("#b6bdd1"),
        faint: hex("#6272a4"),
        inverted_text: hex("#282a36"),
        accent_primary: hex("#8be9fd"),
        accent_secondary: hex("#bd93f9"),
        accent_success: hex("#50fa7b"),
        accent_info: hex("#8be9fd"),
        accent_special: hex("#ff79c6"),
        pressure_low: hex("#f1fa8c"),
        pressure_mid: hex("#ffb86c"),
        pressure_high: hex("#ff5555"),
    }
}

fn solarized_dark() -> Palette {
    Palette {
        name: "solarized-dark".to_owned(),
        background: hex("#002b36"),
        surface: hex("#073642"),
        overlay: hex("#00212b"),
        border: hex("#0f5261"),
        border_active: hex("#268bd2"),
        text: hex("#93a1a1"),
        muted: hex("#839496"),
        faint: hex("#586e75"),
        inverted_text: hex("#fdf6e3"),
        accent_primary: hex("#2aa198"),
        accent_secondary: hex("#6c71c4"),
        accent_success: hex("#859900"),
        accent_info: hex("#268bd2"),
        accent_special: hex("#d33682"),
        pressure_low: hex("#b58900"),
        pressure_mid: hex("#cb4b16"),
        pressure_high: hex("#dc322f"),
    }
}

fn solarized_light() -> Palette {
    Palette {
        name: "solarized-light".to_owned(),
        background: hex("#fdf6e3"),
        surface: hex("#eee8d5"),
        overlay: hex("#fffbf0"),
        border: hex("#ab9f80"),
        border_active: hex("#268bd2"),
        text: hex("#073642"),
        muted: hex("#586e75"),
        faint: hex("#93a1a1"),
        inverted_text: hex("#fdf6e3"),
        accent_primary: hex("#299d94"),
        accent_secondary: hex("#6c71c4"),
        accent_success: hex("#819400"),
        accent_info: hex("#268bd2"),
        accent_special: hex("#d33682"),
        pressure_low: hex("#b08500"),
        pressure_mid: hex("#cb4b16"),
        pressure_high: hex("#dc322f"),
    }
}

fn rose_pine() -> Palette {
    Palette {
        name: "rose-pine".to_owned(),
        background: hex("#191724"),
        surface: hex("#1f1d2e"),
        overlay: hex("#0f0e17"),
        border: hex("#403d52"),
        border_active: hex("#c4a7e7"),
        text: hex("#e0def4"),
        muted: hex("#908caa"),
        faint: hex("#6e6a86"),
        inverted_text: hex("#191724"),
        accent_primary: hex("#9ccfd8"),
        accent_secondary: hex("#c4a7e7"),
        accent_success: hex("#3e8fb0"),
        accent_info: hex("#9ccfd8"),
        accent_special: hex("#ebbcba"),
        pressure_low: hex("#f6c177"),
        pressure_mid: hex("#ea9a97"),
        pressure_high: hex("#eb6f92"),
    }
}

/// `accent_special` was deliberately moved off the official "rose" swatch,
/// which collided with `pressure_mid`: `#ebbcba` is a distinct hex from
/// `#ea9a97`.
fn rose_pine_moon() -> Palette {
    Palette {
        name: "rose-pine-moon".to_owned(),
        background: hex("#232136"),
        surface: hex("#2a273f"),
        overlay: hex("#1b192a"),
        border: hex("#44415a"),
        border_active: hex("#c4a7e7"),
        text: hex("#e0def4"),
        muted: hex("#908caa"),
        faint: hex("#6e6a86"),
        inverted_text: hex("#232136"),
        accent_primary: hex("#9ccfd8"),
        accent_secondary: hex("#c4a7e7"),
        accent_success: hex("#3e8fb0"),
        accent_info: hex("#9ccfd8"),
        accent_special: hex("#ebbcba"),
        pressure_low: hex("#f6c177"),
        pressure_mid: hex("#ea9a97"),
        pressure_high: hex("#eb6f92"),
    }
}

/// `accent_special` (`#c05992`) is a newly tuned colour, distinct from
/// `pressure_mid` (`#d16f6b`), fixing the same collision as
/// [`rose_pine_moon`].
fn rose_pine_dawn() -> Palette {
    Palette {
        name: "rose-pine-dawn".to_owned(),
        background: hex("#faf4ed"),
        surface: hex("#fffaf3"),
        overlay: hex("#f2e9e1"),
        border: hex("#dfdad9"),
        border_active: hex("#907aa9"),
        text: hex("#575279"),
        muted: hex("#716d8c"),
        faint: hex("#9893a5"),
        inverted_text: hex("#faf4ed"),
        accent_primary: hex("#286983"),
        accent_secondary: hex("#907aa9"),
        accent_success: hex("#56949f"),
        accent_info: hex("#286983"),
        accent_special: hex("#c05992"),
        pressure_low: hex("#c77c15"),
        pressure_mid: hex("#d16f6b"),
        pressure_high: hex("#b4637a"),
    }
}

fn everforest_dark() -> Palette {
    Palette {
        name: "everforest-dark".to_owned(),
        background: hex("#2d353b"),
        surface: hex("#343f44"),
        overlay: hex("#232a2e"),
        border: hex("#4f5b58"),
        border_active: hex("#83c092"),
        text: hex("#d3c6aa"),
        muted: hex("#9da9a0"),
        faint: hex("#7a8478"),
        inverted_text: hex("#2d353b"),
        accent_primary: hex("#7fbbb3"),
        accent_secondary: hex("#d699b6"),
        accent_success: hex("#a7c080"),
        accent_info: hex("#7fbbb3"),
        accent_special: hex("#d699b6"),
        pressure_low: hex("#dbbc7f"),
        pressure_mid: hex("#e69875"),
        pressure_high: hex("#e67e80"),
    }
}

fn kanagawa() -> Palette {
    Palette {
        name: "kanagawa".to_owned(),
        background: hex("#1f1f28"),
        surface: hex("#2a2a37"),
        overlay: hex("#16161d"),
        border: hex("#54546d"),
        border_active: hex("#7e9cd8"),
        text: hex("#dcd7ba"),
        muted: hex("#9299b0"),
        faint: hex("#727169"),
        inverted_text: hex("#1f1f28"),
        accent_primary: hex("#7fb4ca"),
        accent_secondary: hex("#957fb8"),
        accent_success: hex("#98bb6c"),
        accent_info: hex("#7e9cd8"),
        accent_special: hex("#d27e99"),
        pressure_low: hex("#dca561"),
        pressure_mid: hex("#ffa066"),
        pressure_high: hex("#e46876"),
    }
}

fn one_dark() -> Palette {
    Palette {
        name: "one-dark".to_owned(),
        background: hex("#282c34"),
        surface: hex("#323844"),
        overlay: hex("#21252b"),
        border: hex("#4b5263"),
        border_active: hex("#61afef"),
        text: hex("#abb2bf"),
        muted: hex("#8f96a3"),
        faint: hex("#5c6370"),
        inverted_text: hex("#282c34"),
        accent_primary: hex("#56b6c2"),
        accent_secondary: hex("#c678dd"),
        accent_success: hex("#98c379"),
        accent_info: hex("#61afef"),
        accent_special: hex("#c678dd"),
        pressure_low: hex("#e5c07b"),
        pressure_mid: hex("#d19a66"),
        pressure_high: hex("#e06c75"),
    }
}

/// `border_active` is the cyan, not the pink/orange family, so no ramp
/// collision.
fn monokai() -> Palette {
    Palette {
        name: "monokai".to_owned(),
        background: hex("#272822"),
        surface: hex("#33342c"),
        overlay: hex("#1e1f1c"),
        border: hex("#5b5c53"),
        border_active: hex("#66d9ef"),
        text: hex("#f8f8f2"),
        muted: hex("#c2c2a8"),
        faint: hex("#75715e"),
        inverted_text: hex("#272822"),
        accent_primary: hex("#66d9ef"),
        accent_secondary: hex("#ae81ff"),
        accent_success: hex("#a6e22e"),
        accent_info: hex("#66d9ef"),
        accent_special: hex("#fd5ff0"),
        pressure_low: hex("#e6db74"),
        pressure_mid: hex("#fd971f"),
        pressure_high: hex("#f92672"),
    }
}

fn ayu_dark() -> Palette {
    Palette {
        name: "ayu-dark".to_owned(),
        background: hex("#0a0e14"),
        surface: hex("#131721"),
        overlay: hex("#060a10"),
        border: hex("#232937"),
        border_active: hex("#39bae6"),
        text: hex("#bfbdb6"),
        muted: hex("#757d8b"),
        faint: hex("#4a5361"),
        inverted_text: hex("#0a0e14"),
        accent_primary: hex("#39bae6"),
        accent_secondary: hex("#d2a6ff"),
        accent_success: hex("#7fd962"),
        accent_info: hex("#59c2ff"),
        accent_special: hex("#d2a6ff"),
        pressure_low: hex("#ffb454"),
        pressure_mid: hex("#ff8f40"),
        pressure_high: hex("#f07178"),
    }
}

fn ayu_mirage() -> Palette {
    Palette {
        name: "ayu-mirage".to_owned(),
        background: hex("#1f2430"),
        surface: hex("#232834"),
        overlay: hex("#171b24"),
        border: hex("#3d4451"),
        border_active: hex("#5ccfe6"),
        text: hex("#cbccc6"),
        muted: hex("#848c9c"),
        faint: hex("#535c68"),
        inverted_text: hex("#1f2430"),
        accent_primary: hex("#5ccfe6"),
        accent_secondary: hex("#d4bfff"),
        accent_success: hex("#87d96c"),
        accent_info: hex("#73d0ff"),
        accent_special: hex("#d4bfff"),
        pressure_low: hex("#ffd580"),
        pressure_mid: hex("#ffa759"),
        pressure_high: hex("#f28779"),
    }
}

fn ayu_light() -> Palette {
    Palette {
        name: "ayu-light".to_owned(),
        background: hex("#fafafa"),
        surface: hex("#f0f0f0"),
        overlay: hex("#ffffff"),
        border: hex("#c7c7c7"),
        border_active: hex("#2b97e4"),
        text: hex("#5c6166"),
        muted: hex("#6b7075"),
        faint: hex("#8a919b"),
        inverted_text: hex("#fafafa"),
        accent_primary: hex("#2b97e4"),
        accent_secondary: hex("#a37acc"),
        accent_success: hex("#5aa237"),
        accent_info: hex("#2b97e4"),
        accent_special: hex("#a37acc"),
        pressure_low: hex("#a35a1f"),
        pressure_mid: hex("#bd5c00"),
        pressure_high: hex("#d13838"),
    }
}

fn high_contrast() -> Palette {
    Palette {
        name: "high-contrast".to_owned(),
        background: hex("#000000"),
        surface: hex("#121212"),
        overlay: hex("#000000"),
        border: hex("#6e6e6e"),
        border_active: hex("#ffffff"),
        text: hex("#ffffff"),
        muted: hex("#c9c9c9"),
        faint: hex("#8a8a8a"),
        inverted_text: hex("#000000"),
        accent_primary: hex("#00e5ff"),
        accent_secondary: hex("#c792ff"),
        accent_success: hex("#3ddc84"),
        accent_info: hex("#5ac8ff"),
        accent_special: hex("#ff6ec7"),
        pressure_low: hex("#ffd400"),
        pressure_mid: hex("#ff9100"),
        pressure_high: hex("#ff3b3b"),
    }
}

fn solstice() -> Palette {
    Palette {
        name: "solstice".to_owned(),
        background: hex("#101820"),
        surface: hex("#182432"),
        overlay: hex("#0a1017"),
        border: hex("#2f4a63"),
        border_active: hex("#5ec8f2"),
        text: hex("#e6f1fb"),
        muted: hex("#93b0c9"),
        faint: hex("#4f6478"),
        inverted_text: hex("#101820"),
        accent_primary: hex("#5ec8f2"),
        accent_secondary: hex("#9d8cf7"),
        accent_success: hex("#4fd1a5"),
        accent_info: hex("#5b9df9"),
        accent_special: hex("#e8709c"),
        pressure_low: hex("#f0c14b"),
        pressure_mid: hex("#f28a4b"),
        pressure_high: hex("#f2554b"),
    }
}

/// `border_active` is the teal `accent_primary`, not the theme's warm amber
/// -- the same fix pattern as [`gruvbox_dark`].
fn terracotta() -> Palette {
    Palette {
        name: "terracotta".to_owned(),
        background: hex("#241c19"),
        surface: hex("#302521"),
        overlay: hex("#180f0d"),
        border: hex("#544138"),
        border_active: hex("#5fb0a8"),
        text: hex("#f1e6dc"),
        muted: hex("#c2a99a"),
        faint: hex("#7a6355"),
        inverted_text: hex("#241c19"),
        accent_primary: hex("#5fb0a8"),
        accent_secondary: hex("#b591d9"),
        accent_success: hex("#8fbf6e"),
        accent_info: hex("#6fa8d6"),
        accent_special: hex("#d97fae"),
        pressure_low: hex("#e9c14a"),
        pressure_mid: hex("#e9a24a"),
        pressure_high: hex("#e3573f"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_a_leading_hash_or_its_absence_the_same_way() {
        assert_eq!(hex("#ff0080"), hex("ff0080"));
        assert_eq!(hex("#ff0080"), Rgb(255, 0, 128));
    }

    #[test]
    fn there_are_exactly_twenty_seven_built_in_themes() {
        assert_eq!(all().len(), 27);
    }

    #[test]
    fn every_theme_has_a_distinct_kebab_case_name() {
        let mut names: Vec<String> = all().into_iter().map(|p| p.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 27, "a name collided: {names:?}");
        for name in names {
            assert_eq!(
                name.to_lowercase(),
                name,
                "{name:?} must be lowercase to be a stable lookup key"
            );
        }
    }

    #[test]
    fn aurora_reproduces_the_original_themes_exact_colours() {
        let aurora = aurora();
        assert_eq!(aurora.background, Rgb(11, 13, 26));
        assert_eq!(aurora.accent_primary, Rgb(86, 226, 232));
        assert_eq!(aurora.pressure_high, Rgb(248, 113, 113));
    }
}

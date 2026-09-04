//! A small, hand-rolled parser for vim-notation key specs, e.g. `"j"`,
//! `"<C-d>"`, `"<Esc>"` or `"gt"`.
//!
//! This exists so a config file's `keymap.bind[].keys` string can become a
//! [`crate::tui::keymap::KeySeq`] -- the same trigger type
//! [`crate::tui::keymap::Binding`] is built around -- without pulling in a
//! parser-combinator crate for a grammar this small: one optional `<...>`
//! token, then either nothing else (a single-key binding, `KeySeq::One`) or
//! exactly one more token (a two-key chord, `KeySeq::Two`, `"gt"` being the
//! obvious example). Wiring these bindings onto the live
//! [`crate::tui::keymap::Keymap`] is left to a later epic; this module's
//! whole job is turning the *string* into the *type*, and rejecting a string
//! that cannot mean a key at all rather than guessing.
//!
//! # Why `parse_key_spec` returns a [`KeySeq`], not a single [`Key`]
//!
//! A config-file binding's trigger can be one key (`"j"`) or two
//! (`"gt"`) -- [`KeySeq`] already has exactly the two variants for that, from
//! the epic that introduced the keymap itself -- so parsing the *whole*
//! spec string one binding names has to be able to produce either. The
//! single-key half of that (`"j"`, `"<C-d>"`, `"<Esc>"`, ...) is [`parse_one`]
//! below, kept private: nothing outside this module ever needs a bare `Key`
//! out of a config file, only a complete trigger for a binding.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::keymap::{Key, KeySeq};

/// Why a key spec string could not be turned into a [`KeySeq`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySpecError {
    /// A `<...>` token was opened but never closed.
    UnterminatedToken(String),
    /// A token -- either a bare character or a `<...>` name -- is not one
    /// this parser recognises.
    UnknownToken(String),
    /// The spec tokenised to some number of keys other than one or two,
    /// which is every count [`KeySeq`] has no variant for.
    WrongKeyCount(usize),
}

impl std::fmt::Display for KeySpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnterminatedToken(token) => {
                write!(f, "{token:?} opens a '<' with no matching '>'")
            }
            Self::UnknownToken(token) => write!(f, "{token:?} is not a key this crate knows"),
            Self::WrongKeyCount(count) => {
                write!(f, "a binding needs one or two keys, not {count}")
            }
        }
    }
}

impl std::error::Error for KeySpecError {}

/// Parses a whole binding spec -- `"j"`, `"<C-d>"`, `"gt"` -- into the
/// [`KeySeq`] a [`crate::tui::keymap::Binding`] is triggered by.
///
/// # Errors
///
/// Returns [`KeySpecError`] if the string does not tokenise into exactly one
/// or two recognised keys -- see [`tokenize`] and [`parse_one`] for what
/// counts as a token and what counts as recognised.
pub fn parse_key_spec(s: &str) -> Result<KeySeq, KeySpecError> {
    let tokens = tokenize(s)?;
    match tokens.as_slice() {
        [one] => Ok(KeySeq::One(parse_one(one)?)),
        [first, second] => Ok(KeySeq::Two(parse_one(first)?, parse_one(second)?)),
        _ => Err(KeySpecError::WrongKeyCount(tokens.len())),
    }
}

/// Splits a spec string into key tokens: each `<...>` run counts as one
/// token regardless of how many characters it spans, and every other
/// character is its own token.
///
/// This is what lets `"gt"` (two one-character tokens) and `"<C-d>"` (one
/// five-character token) both mean "one or two keys" to [`parse_key_spec`]
/// without it having to special-case which. Working in byte slices of the
/// original `&str` rather than collecting owned `String`s keeps this
/// allocation-free; the only place further down this module allocates is
/// [`KeySpecError`]'s own message, on the failure path.
fn tokenize(s: &str) -> Result<Vec<&str>, KeySpecError> {
    let mut tokens = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(after_open) = rest.strip_prefix('<') {
            let Some(close) = after_open.find('>') else {
                return Err(KeySpecError::UnterminatedToken(rest.to_owned()));
            };
            let token_len = 1 + close + 1; // '<' + inner + '>'
            tokens.push(&rest[..token_len]);
            rest = &rest[token_len..];
        } else {
            let char_len = rest
                .chars()
                .next()
                .expect("rest is non-empty inside this loop")
                .len_utf8();
            tokens.push(&rest[..char_len]);
            rest = &rest[char_len..];
        }
    }
    Ok(tokens)
}

/// Parses one token -- a bare character, or a `<...>` name -- into the
/// [`Key`] it names.
fn parse_one(token: &str) -> Result<Key, KeySpecError> {
    if let Some(inner) = token.strip_prefix('<').and_then(|t| t.strip_suffix('>')) {
        return named_key(inner).ok_or_else(|| KeySpecError::UnknownToken(token.to_owned()));
    }

    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(Key {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        }),
        _ => Err(KeySpecError::UnknownToken(token.to_owned())),
    }
}

/// The [`Key`] a `<...>` token's inner name refers to, e.g. `"Esc"` or
/// `"C-d"` -- `None` for anything this parser does not recognise.
///
/// A literal table of names rather than something derived from
/// `KeyCode`/`KeyModifiers`, for the same reason
/// `crate::tui::keymap::key_label`'s own table is literal: every name here
/// is a judgement call about which vim-notation spellings this crate
/// accepts (`"CR"` and `"Enter"` both work; `"Return"` deliberately does
/// not), and a generic formatter would silently make that call for whatever
/// name gets typed next rather than forcing it to be looked at.
fn named_key(inner: &str) -> Option<Key> {
    if let Some(rest) = inner.strip_prefix("C-") {
        let mut chars = rest.chars();
        return match (chars.next(), chars.next()) {
            (Some(c), None) => Some(Key {
                code: KeyCode::Char(c),
                mods: KeyModifiers::CONTROL,
            }),
            _ => None,
        };
    }

    let code = match inner {
        "Esc" => KeyCode::Esc,
        "CR" | "Enter" => KeyCode::Enter,
        "Tab" => KeyCode::Tab,
        "S-Tab" => KeyCode::BackTab,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "F1" => KeyCode::F(1),
        _ => return None,
    };
    Some(Key {
        code,
        mods: KeyModifiers::NONE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        }
    }

    #[test]
    fn a_bare_letter_is_a_single_unmodified_key() {
        assert_eq!(parse_key_spec("j"), Ok(KeySeq::One(plain('j'))));
    }

    #[test]
    fn angle_bracket_c_dash_is_a_control_modified_key() {
        assert_eq!(
            parse_key_spec("<C-d>"),
            Ok(KeySeq::One(Key {
                code: KeyCode::Char('d'),
                mods: KeyModifiers::CONTROL,
            }))
        );
    }

    #[test]
    fn esc_is_a_named_special() {
        assert_eq!(
            parse_key_spec("<Esc>"),
            Ok(KeySeq::One(Key {
                code: KeyCode::Esc,
                mods: KeyModifiers::NONE,
            }))
        );
    }

    #[test]
    fn cr_and_enter_both_name_the_enter_key() {
        let expect = Ok(KeySeq::One(Key {
            code: KeyCode::Enter,
            mods: KeyModifiers::NONE,
        }));
        assert_eq!(parse_key_spec("<CR>"), expect);
        assert_eq!(parse_key_spec("<Enter>"), expect);
    }

    #[test]
    fn gt_parses_to_a_two_key_chord() {
        assert_eq!(
            parse_key_spec("gt"),
            Ok(KeySeq::Two(plain('g'), plain('t')))
        );
    }

    #[test]
    fn a_named_special_still_counts_as_a_single_token_inside_a_two_key_spec() {
        // Not a binding this crate ships today, but the tokenizer must not
        // see "<Esc>j" as more than two keys just because "<Esc>" spans five
        // characters.
        assert_eq!(
            parse_key_spec("<Esc>j"),
            Ok(KeySeq::Two(
                Key {
                    code: KeyCode::Esc,
                    mods: KeyModifiers::NONE,
                },
                plain('j'),
            ))
        );
    }

    #[test]
    fn an_unknown_named_token_is_refused_rather_than_guessed_at() {
        let error = parse_key_spec("<NotReal>").expect_err("no such named key exists");
        assert_eq!(error, KeySpecError::UnknownToken("<NotReal>".to_owned()));
    }

    #[test]
    fn an_unterminated_token_is_refused() {
        let error = parse_key_spec("<C-d").expect_err("the '<' is never closed");
        assert_eq!(error, KeySpecError::UnterminatedToken("<C-d".to_owned()));
    }

    #[test]
    fn three_keys_is_refused_since_keyseq_only_goes_up_to_two() {
        let error = parse_key_spec("abc").expect_err("KeySeq has no three-key variant");
        assert_eq!(error, KeySpecError::WrongKeyCount(3));
    }

    #[test]
    fn an_empty_spec_is_refused() {
        let error = parse_key_spec("").expect_err("zero keys cannot trigger anything");
        assert_eq!(error, KeySpecError::WrongKeyCount(0));
    }
}

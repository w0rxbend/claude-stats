//! Mapping a key press to what it means, as data rather than as a hardcoded
//! `match`.
//!
//! [`Keymap`] is a Registry (Fowler, *`PoEAA`*): one well-known table that
//! every part of the UI -- the event loop, the footer hint, the help screen --
//! asks "what does this key do" of, rather than each of them keeping its own
//! copy of the answer. Before this module existed the answer lived as a
//! single `match` in `App`'s old `Action::from_key`, with the help overlay's
//! `KEYS` constant a hand-written summary of the same facts kept in step by
//! hand. Now there is exactly one table, in [`defaults`], and everything
//! else -- [`Keymap::help_rows`], the event loop's [`resolve`] call -- reads
//! it rather than repeating it.
//!
//! This module imports `crossterm` for its key types and nothing from
//! `crate::domain`: which key does what is a presentation concern, not a
//! business rule, so it belongs in `tui` and nowhere deeper. It is modelled
//! on the sibling Registry in [`crate::tui::palette`], which answers "what
//! colour is this" the same way this one answers "what does this key do".

use std::collections::HashMap;
use std::fmt;

use crossterm::event::{KeyCode, KeyModifiers};

mod defaults;

/// One physical key press: a code and the modifiers held down with it.
///
/// Kept as a small `Copy` struct of our own, rather than matching on
/// `crossterm::event::KeyEvent` directly, because a `KeyEvent` also carries a
/// press/release `kind` and a terminal `state` field this module has no use
/// for. Filtering those out here means the rest of the keymap only ever has
/// to reason about the two fields that actually distinguish one binding from
/// another; the event loop is the one place that still has to know a
/// `KeyEvent` exists at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// A binding's trigger: either a single key, or a two-key chord such as `gg`.
///
/// Modelling a chord as its own variant, rather than as a `Vec<Key>`, is what
/// lets [`Keymap`] be a plain `HashMap` keyed directly by this type, and what
/// lets [`Keymap::validate`]'s prefix-free check be a simple equality test
/// between a `One` and the first key of a `Two`, rather than a general (and
/// here unneeded) trie-prefix search over arbitrary-length sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeySeq {
    One(Key),
    Two(Key, Key),
}

/// Something the user asked for in normal mode, independent of which key (or
/// count, or chord) produced it.
///
/// Search and command entry gained full behaviour in the epic that added
/// [`crate::tui::app::App::handle_line_edit`] -- `EnterSearch`/`EnterCommand`
/// only flip `App` into [`crate::tui::app::InputMode::Search`]/`Command`,
/// after which every further key bypasses this module entirely (see
/// `handle_line_edit`'s own doc comment for why resolving key-by-key text
/// entry through a `Keymap` built for single actions makes no sense).
///
/// `RepeatSearch` is this same epic's own small addendum to the table, not
/// something epic 2 left half-built: nothing before this epic could produce
/// a `last_search` for `n`/`N` to repeat, so there was nothing for epic 2 to
/// wire a variant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalAction {
    Quit,
    Back,
    ToggleHelp,
    Refresh,
    OpenSessions,
    Confirm,
    MoveDown,
    MoveUp,
    PanLeft,
    PanRight,
    JumpToRow(RowTarget),
    HalfPage(Dir),
    Page(Dir),
    PrevSection,
    NextSection,
    LineStart,
    LineEnd,
    NextView,
    PrevView,
    GotoView(u32),
    FocusNext,
    FocusPrev,
    JumpBack,
    EnterSearch,
    EnterCommand,
    /// Repeats the last confirmed search: `true` in the same direction it
    /// originally ran (`n`), `false` reversed (`N`).
    RepeatSearch(bool),
    /// Opens the theme picker, or -- when it is already open -- advances it
    /// to the next entry in [`crate::tui::palette::registry::ThemeRegistry::builtin`]'s
    /// own order and applies it immediately. See
    /// [`crate::tui::app::App::handle`]'s own doc comment on why a single
    /// action covers both "open" and "cycle": binding a second key to
    /// "advance while open" would need the keymap to know the picker's own
    /// state, which is exactly the kind of presentation state a pure
    /// key-to-action table has no business holding.
    CycleTheme,
    /// Opens the layout picker.
    OpenLayoutPicker,
}

/// Where a jump lands: the first row, the last row, or a specific one-based
/// row number supplied as a count (`5gg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowTarget {
    Top,
    Bottom,
    Row(u32),
}

/// Which way a scroll or a jump moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
}

/// Which section of the help overlay a binding is listed under.
///
/// This exists purely to make the help screen and the (future) which-key
/// popup readable -- it plays no part in how a key resolves to an action, so
/// it does not need `Hash` or `Ord`: [`Keymap::help_rows`] sorts by
/// [`Group::rank`] rather than by deriving one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Global,
    Motion,
    Views,
    Search,
    Command,
    Panes,
    Jumps,
    /// Choosing a theme or a layout -- distinct from `Views`, which is about
    /// *what* is on screen (the dashboard, the log), not *how* it is drawn.
    Appearance,
}

impl Group {
    /// Display order for the help overlay: broad, always-available keys
    /// first, the two not-yet-implemented entry points last.
    const fn rank(self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Motion => 1,
            Self::Jumps => 2,
            Self::Views => 3,
            Self::Panes => 4,
            Self::Appearance => 5,
            Self::Search => 6,
            Self::Command => 7,
        }
    }

    /// A short, lower-case heading for this group, used to break the help
    /// overlay's key list into sections.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Motion => "motion",
            Self::Jumps => "jumps",
            Self::Views => "views",
            Self::Panes => "panes",
            Self::Appearance => "appearance",
            Self::Search => "search",
            Self::Command => "command",
        }
    }
}

/// One row of the keymap: a trigger, what it does, and how it is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub seq: KeySeq,
    pub action: NormalAction,
    pub description: &'static str,
    pub group: Group,
    /// Whether this binding is load-bearing enough that it must always
    /// resolve to an action -- the arrow keys, Home/End, Page Up/Down, Esc
    /// and Enter. A future rebinding feature is free to let a user add or
    /// remove ordinary bindings, but [`Keymap::validate`] refuses a table
    /// that leaves one of these unreachable, because a terminal application
    /// that does not answer Esc or the arrow keys is not usable by touch.
    pub pinned: bool,
}

/// The catalogue of every key binding the dashboard understands in normal
/// mode.
///
/// See the module documentation for why this is a Registry rather than a
/// scattered `match`. The table itself lives in [`defaults`]; this type is
/// only the behaviour around it -- looking a sequence up, listing it for the
/// help screen, and checking it is internally consistent.
#[derive(Debug)]
pub struct Keymap {
    bindings: HashMap<KeySeq, Binding>,
}

/// Why a keymap failed [`Keymap::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeymapError {
    /// A `KeySeq::One` binding exists for a key that is also the first key
    /// of some `KeySeq::Two` chord. The moment that key is pressed there is
    /// no way to tell whether the user meant the single-key binding or the
    /// start of the chord, so the table is ambiguous rather than merely
    /// surprising.
    ShadowsPrefixKey(Key),
    /// A binding marked [`Binding::pinned`] has no entry left in the table
    /// mapping to its action, so the key it promises never actually fires.
    PinnedActionUnreachable(NormalAction),
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShadowsPrefixKey(key) => {
                write!(f, "{key:?} is bound on its own but also opens a chord")
            }
            Self::PinnedActionUnreachable(action) => {
                write!(f, "{action:?} is pinned but no binding reaches it")
            }
        }
    }
}

impl std::error::Error for KeymapError {}

impl Keymap {
    /// The keymap every dashboard starts with.
    ///
    /// # Panics
    ///
    /// Never in practice: [`defaults::bindings`] is a fixed literal table
    /// written by hand in this crate, so if it were ever ambiguous or missing
    /// a pinned action that would be a bug in this crate caught by this
    /// module's own tests, not a condition a caller could hit at runtime. The
    /// `expect` says that plainly rather than threading a `Result` through
    /// every caller for an error state the shipped table cannot produce.
    #[must_use]
    pub fn default_bindings() -> Self {
        let bindings = defaults::bindings()
            .into_iter()
            .map(|binding| (binding.seq, binding))
            .collect();
        let keymap = Self { bindings };
        keymap
            .validate()
            .expect("the built-in keymap is internally consistent");
        keymap
    }

    /// The binding for an exact key sequence, if one is bound.
    #[must_use]
    pub fn lookup(&self, seq: KeySeq) -> Option<&Binding> {
        self.bindings.get(&seq)
    }

    /// Whether `key` opens a chord -- that is, whether it is the first key of
    /// some bound `KeySeq::Two`.
    ///
    /// Written as a search over the table rather than a hardcoded `key ==
    /// 'g'` check so that a future binding that opens a second chord (a
    /// leader key, say) needs no change here: adding the `KeySeq::Two` to
    /// [`defaults::bindings`] is enough to make its first key a prefix key
    /// too. Today the shipped table only ever chords on `g`, which is what
    /// makes this true only for plain `g`.
    #[must_use]
    pub fn is_prefix_key(&self, key: Key) -> bool {
        self.bindings
            .keys()
            .any(|seq| matches!(seq, KeySeq::Two(first, _) if *first == key))
    }

    /// Every binding, as `(group, key label, description)`, sorted for
    /// display.
    ///
    /// This is the single source the help overlay, the footer hint and a
    /// future which-key popup all read from -- see the module documentation
    /// for why duplicating this table by hand in three places was the thing
    /// worth removing. The sort is by [`Group::rank`] and then by label,
    /// which exists only so the result is deterministic: `self.bindings` is a
    /// `HashMap`, and without an explicit order the help screen would
    /// reshuffle itself on every restart.
    #[must_use]
    pub fn help_rows(&self) -> Vec<(Group, &str, &str)> {
        let mut rows: Vec<(Group, &str, &str)> = self
            .bindings
            .values()
            .map(|binding| (binding.group, key_label(binding.seq), binding.description))
            .collect();
        rows.sort_by_key(|(group, label, _)| (group.rank(), *label));
        rows
    }

    /// Checks the table is internally consistent.
    ///
    /// Two things are checked, in order:
    ///
    /// 1. **Prefix-free.** No `KeySeq::One(k)` may coexist with a
    ///    `KeySeq::Two(k, _)`: the instant `k` is pressed there would be no
    ///    way to tell a completed single-key binding from the opening of a
    ///    chord.
    /// 2. **Pinned bindings stay reachable.** Every binding marked
    ///    [`Binding::pinned`] must have some entry in the table -- itself,
    ///    typically -- whose action matches its own. This is a tautology for
    ///    the table [`Keymap::default_bindings`] builds today, because
    ///    nothing here can remove a binding once inserted; it earns its keep
    ///    the day a later epic lets a user override or unbind entries; at
    ///    that point this is the check standing between "the arrow keys
    ///    still work" and a keymap a user's own config quietly broke.
    fn validate(&self) -> Result<(), KeymapError> {
        for seq in self.bindings.keys() {
            if let KeySeq::One(key) = seq {
                if self.is_prefix_key(*key) {
                    return Err(KeymapError::ShadowsPrefixKey(*key));
                }
            }
        }

        for binding in self.bindings.values().filter(|binding| binding.pinned) {
            let reachable = self
                .bindings
                .values()
                .any(|other| other.action == binding.action);
            if !reachable {
                return Err(KeymapError::PinnedActionUnreachable(binding.action));
            }
        }

        Ok(())
    }
}

/// `(key, label)` for every single-key binding shipped today, read by
/// [`key_label`] below. A `const` table rather than a `match` over every
/// `KeySeq::One`, so this stays data -- one line per key -- rather than a
/// function long enough for `cargo clippy`'s `too_many_lines` to object to on
/// its own behalf. See [`key_label`]'s doc comment for why this is a literal
/// table rather than something generated from `KeyCode`/`KeyModifiers`.
const SINGLE_KEY_LABELS: &[(Key, &str)] = &[
    (plain('q'), "q"),
    (
        Key {
            code: KeyCode::Esc,
            mods: KeyModifiers::NONE,
        },
        "Esc",
    ),
    (
        Key {
            code: KeyCode::Enter,
            mods: KeyModifiers::NONE,
        },
        "Enter",
    ),
    (plain('?'), "?"),
    (
        Key {
            code: KeyCode::F(1),
            mods: KeyModifiers::NONE,
        },
        "F1",
    ),
    (plain('r'), "r"),
    (plain('o'), "o"),
    (plain('j'), "j"),
    (
        Key {
            code: KeyCode::Down,
            mods: KeyModifiers::NONE,
        },
        "Down",
    ),
    (plain('k'), "k"),
    (
        Key {
            code: KeyCode::Up,
            mods: KeyModifiers::NONE,
        },
        "Up",
    ),
    (plain('h'), "h"),
    (
        Key {
            code: KeyCode::Left,
            mods: KeyModifiers::NONE,
        },
        "Left",
    ),
    (plain('l'), "l"),
    (
        Key {
            code: KeyCode::Right,
            mods: KeyModifiers::NONE,
        },
        "Right",
    ),
    (plain('G'), "G"),
    (ctrl('d'), "Ctrl-d"),
    (ctrl('u'), "Ctrl-u"),
    (ctrl('f'), "Ctrl-f"),
    (
        Key {
            code: KeyCode::PageDown,
            mods: KeyModifiers::NONE,
        },
        "PageDown",
    ),
    (ctrl('b'), "Ctrl-b"),
    (
        Key {
            code: KeyCode::PageUp,
            mods: KeyModifiers::NONE,
        },
        "PageUp",
    ),
    (plain('{'), "{"),
    (plain('}'), "}"),
    (plain('0'), "0"),
    (
        Key {
            code: KeyCode::Home,
            mods: KeyModifiers::NONE,
        },
        "Home",
    ),
    (plain('$'), "$"),
    (
        Key {
            code: KeyCode::End,
            mods: KeyModifiers::NONE,
        },
        "End",
    ),
    (
        Key {
            code: KeyCode::Tab,
            mods: KeyModifiers::NONE,
        },
        "Tab",
    ),
    (
        Key {
            code: KeyCode::BackTab,
            mods: KeyModifiers::NONE,
        },
        "Shift-Tab",
    ),
    (ctrl('o'), "Ctrl-o"),
    (plain('/'), "/"),
    (plain(':'), ":"),
    (plain('n'), "n"),
    (plain('N'), "N"),
    (plain('t'), "t"),
    (plain('L'), "L"),
];

/// `(first, second, label)` for every chord shipped today. Kept apart from
/// [`SINGLE_KEY_LABELS`] because a chord's label is not simply its two keys'
/// labels concatenated -- `key_label` would have to know that, which is
/// exactly the kind of formatting judgement call this module's doc comment
/// on [`key_label`] says belongs in a literal, not a formatter.
const CHORD_LABELS: &[(Key, Key, &str)] = &[
    (plain('g'), plain('g'), "gg"),
    (plain('g'), plain('t'), "gt"),
    (plain('g'), plain('T'), "gT"),
];

/// A key with no modifiers held, for the literal tables above.
const fn plain(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::NONE,
    }
}

/// A key held with Control, for the literal tables above.
const fn ctrl(c: char) -> Key {
    Key {
        code: KeyCode::Char(c),
        mods: KeyModifiers::CONTROL,
    }
}

/// A short, human-readable label for one key or chord, e.g. `"Ctrl-d"` or
/// `"gg"`.
///
/// Looking the label up in [`SINGLE_KEY_LABELS`]/[`CHORD_LABELS`] rather than
/// building one from `KeyCode`/`KeyModifiers` is deliberate, for the same
/// reason the palette's built-in themes are a flat table of hex literals in
/// `palette::builtins` rather than something generated: every label is a
/// judgement call about what reads well in a terminal (`"Ctrl-d"`, not
/// `"C-d"` or `"^D"`), and a generic formatter would silently make that call
/// for whatever key gets bound next rather than forcing it to be looked at.
/// `every_default_binding_has_a_readable_label` below exists precisely to
/// catch a new binding in [`defaults`] that these tables were not updated to
/// match.
fn key_label(seq: KeySeq) -> &'static str {
    const UNLABELLED: &str = "unlabelled key";
    match seq {
        KeySeq::One(key) => SINGLE_KEY_LABELS
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map_or(UNLABELLED, |(_, label)| label),
        KeySeq::Two(first, second) => CHORD_LABELS
            .iter()
            .find(|(a, b, _)| *a == first && *b == second)
            .map_or(UNLABELLED, |(_, _, label)| label),
    }
}

/// How many chevrons of state a count-then-chord input has accumulated so
/// far. `Idle` is "nothing pending"; `AwaitingG` means a `g` has been pressed
/// and the next key either completes a `Two`-key binding or cancels back to
/// `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChordState {
    #[default]
    Idle,
    AwaitingG,
}

/// What the count/chord state machine has accumulated between key presses:
/// a numeric prefix (`5` in `5gg`) and whether a chord is half-typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pending {
    pub count: Option<u32>,
    pub chord: ChordState,
}

/// Applies a count typed before a chord to the action the chord resolved to,
/// where the action has a variant able to carry it.
///
/// Only two substitutions exist because only two actions have a
/// count-shaped counterpart: `NextView` becomes `GotoView(n)` (`3gt` means
/// "the third view", not "the next view, thrice"), and either end of
/// `JumpToRow` becomes the specific row `Row(n)` (`5gg` means "row five", not
/// "the top, thrice"). Every other action that nominally "accepts a count" --
/// `MoveDown`, `MoveUp`, `PanLeft`, `PanRight`, `HalfPage` and `Page` -- has
/// no data-carrying counterpart to substitute into, because repeating a
/// motion and jumping to an absolute position are different shapes of
/// meaning; see the doc comment on [`resolve`] for how those six are meant to
/// be handled instead.
const fn substitute_count(action: NormalAction, count: Option<u32>) -> NormalAction {
    match (action, count) {
        (NormalAction::NextView, Some(n)) => NormalAction::GotoView(n),
        (NormalAction::JumpToRow(RowTarget::Top | RowTarget::Bottom), Some(n)) => {
            NormalAction::JumpToRow(RowTarget::Row(n))
        }
        _ => action,
    }
}

/// Fires `action` (after applying [`substitute_count`]) and resets pending
/// state, which is what every path in [`resolve`] that actually produces an
/// action needs to do.
const fn fire(count: Option<u32>, action: NormalAction) -> (Pending, Option<NormalAction>) {
    (Pending::new(), Some(substitute_count(action, count)))
}

impl Pending {
    /// A `const` equivalent of [`Pending::default`], needed because `fire`
    /// above is `const fn` and the derived `Default` impl is not.
    const fn new() -> Self {
        Self {
            count: None,
            chord: ChordState::Idle,
        }
    }
}

/// Feeds one key press through the count/chord state machine and returns the
/// state to carry forward together with the action that press resolved to,
/// if any.
///
/// The rules, checked in this order:
///
/// 1. A digit `1`-`9` (no modifiers) accumulates into `pending.count`: the
///    first digit sets it, each digit after multiplies the running total by
///    ten and adds the new one, saturating at `9999` so a mistyped `99999g`
///    cannot be used to jump somewhere absurd. The chord state is left
///    exactly as it was -- a count can be typed either before a chord opens
///    (`5gg`) or, less commonly, in the middle of one (`g5g`) -- and no
///    action fires.
/// 2. A bare `0` is different from every other digit: with no count typed
///    yet it is not the start of one, it is its own binding (`LineStart`) and
///    fires immediately. Only once a count has started does `0` behave like
///    any other digit and extend it.
/// 3. Anything else is dispatched against the keymap. While idle, a key that
///    opens a chord (today, only `g`) moves to `AwaitingG` without firing
///    anything; once a chord is open, the next key either completes a bound
///    `KeySeq::Two` (firing its action) or does not, which cancels the chord
///    with no action rather than falling back to treating the key as a fresh
///    single-key press -- typing `gx` does not also try `x` on its own.
///    While idle and not opening a chord, a bound `KeySeq::One` fires
///    directly; a key that is not bound at all leaves `pending` untouched
///    rather than resetting it, so a run of unbound keys cannot swallow a
///    count that has not found its motion yet (see the
///    `an_unbound_key_is_ignored_without_disturbing_an_accumulating_count`
///    test).
/// 4. Whenever an action actually fires, or a chord is cancelled by a
///    non-matching key, `Pending` resets to [`Pending::default`] -- the count
///    and the chord were both consumed by that one resolution and neither
///    should leak into the next key press.
///
/// **Counts on plain motions.** `MoveDown`, `MoveUp`, `PanLeft`, `PanRight`,
/// `HalfPage` and `Page` all read as accepting a count (`10j` should move
/// down ten lines), but `NormalAction` gives none of them a field to carry
/// one -- unlike `GotoView`/`JumpToRow(Row(_))`, "move down" has no natural
/// absolute-position reading for a count to turn it into. `resolve` therefore
/// fires the action once, the same as with no count, and does not attempt to
/// return it several times: its signature returns one `Option<NormalAction>`,
/// not a list. Repetition for these six is left to the caller, which by the
/// time this function returns has already lost the count (`pending` has been
/// reset to `Pending::default`) -- so a caller that wants `10j` to actually
/// move ten lines has to read `pending.count` *before* calling `resolve` and
/// interpret the returned action as "repeat this `pending.count.unwrap_or(1)`
/// times" itself. `App::handle` in this epic does not do that yet; it treats
/// every action as firing once, which is the same behaviour a count on one of
/// these six actions had before this module existed (there was no way to
/// type a count at all). Wiring the repeat up is a small, purely
/// `tui::app`-side change future work can make without touching this
/// function.
#[must_use]
pub fn resolve(keymap: &Keymap, pending: Pending, key: Key) -> (Pending, Option<NormalAction>) {
    if let KeyCode::Char(c) = key.code {
        if key.mods == KeyModifiers::NONE && c.is_ascii_digit() {
            if c == '0' && pending.count.is_none() {
                return match keymap.lookup(KeySeq::One(key)) {
                    Some(binding) => fire(None, binding.action),
                    None => (Pending::new(), None),
                };
            }
            let digit = u32::from(c as u8 - b'0');
            let count = Some(match pending.count {
                None => digit,
                Some(existing) => existing.saturating_mul(10).saturating_add(digit).min(9999),
            });
            return (
                Pending {
                    count,
                    chord: pending.chord,
                },
                None,
            );
        }
    }

    match pending.chord {
        ChordState::Idle => {
            if keymap.is_prefix_key(key) {
                return (
                    Pending {
                        count: pending.count,
                        chord: ChordState::AwaitingG,
                    },
                    None,
                );
            }
            match keymap.lookup(KeySeq::One(key)) {
                Some(binding) => fire(pending.count, binding.action),
                None => (pending, None),
            }
        }
        ChordState::AwaitingG => {
            let opener = Key {
                code: KeyCode::Char('g'),
                mods: KeyModifiers::NONE,
            };
            match keymap.lookup(KeySeq::Two(opener, key)) {
                Some(binding) => fire(pending.count, binding.action),
                None => (Pending::new(), None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(code: KeyCode) -> Key {
        Key {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    fn ctrl(code: KeyCode) -> Key {
        Key {
            code,
            mods: KeyModifiers::CONTROL,
        }
    }

    fn resolve_all(keymap: &Keymap, keys: &[Key]) -> (Pending, Option<NormalAction>) {
        let mut pending = Pending::default();
        let mut action = None;
        for key in keys {
            let (next_pending, next_action) = resolve(keymap, pending, *key);
            pending = next_pending;
            action = next_action;
        }
        (pending, action)
    }

    #[test]
    fn the_built_in_keymap_validates_on_construction() {
        // `default_bindings` panics on a failed `validate`, so simply
        // building one is the assertion.
        let _ = Keymap::default_bindings();
    }

    #[test]
    fn a_binding_that_shadows_a_prefix_key_is_refused() {
        let mut bindings = defaults::bindings();
        bindings.push(Binding {
            seq: KeySeq::One(plain(KeyCode::Char('g'))),
            action: NormalAction::Refresh,
            description: "a deliberately ambiguous binding",
            group: Group::Global,
            pinned: false,
        });
        let keymap = Keymap {
            bindings: bindings.into_iter().map(|b| (b.seq, b)).collect(),
        };

        assert_eq!(
            keymap.validate(),
            Err(KeymapError::ShadowsPrefixKey(plain(KeyCode::Char('g')))),
            "a bare 'g' can no longer be told apart from the start of 'gg'/'gt'/'gT'"
        );
    }

    #[test]
    fn a_pinned_binding_cannot_be_removed() {
        let keymap = Keymap::default_bindings();
        let pinned_keys = [
            plain(KeyCode::Esc),
            plain(KeyCode::Up),
            plain(KeyCode::Down),
            plain(KeyCode::Left),
            plain(KeyCode::Right),
            plain(KeyCode::Home),
            plain(KeyCode::End),
            plain(KeyCode::PageUp),
            plain(KeyCode::PageDown),
            plain(KeyCode::Enter),
        ];

        for key in pinned_keys {
            let binding = keymap
                .lookup(KeySeq::One(key))
                .unwrap_or_else(|| panic!("{key:?} is meant to be pinned but is not bound"));
            assert!(binding.pinned, "{key:?} is bound but not marked pinned");

            let (_, action) = resolve(&keymap, Pending::default(), key);
            assert!(
                action.is_some(),
                "{key:?} is pinned but resolve() produced no action from a fresh Keymap"
            );
        }
    }

    #[test]
    fn every_default_binding_has_a_readable_label() {
        for (_, label, description) in Keymap::default_bindings().help_rows() {
            assert_ne!(
                label, "unlabelled key",
                "key_label does not know this binding yet: {description:?}"
            );
        }
    }

    #[test]
    fn a_count_before_a_jump_goes_to_that_row() {
        let keymap = Keymap::default_bindings();
        let keys = [
            plain(KeyCode::Char('5')),
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('g')),
        ];
        let (pending, action) = resolve_all(&keymap, &keys);

        assert_eq!(action, Some(NormalAction::JumpToRow(RowTarget::Row(5))));
        assert_eq!(
            pending,
            Pending::default(),
            "the count is consumed by the jump"
        );
    }

    #[test]
    fn an_unbound_key_is_ignored_without_disturbing_an_accumulating_count() {
        let keymap = Keymap::default_bindings();

        // 'd' was retired outright by this epic and binds to nothing.
        let (after_d, action) = resolve(&keymap, Pending::default(), plain(KeyCode::Char('d')));
        assert_eq!(action, None);
        assert_eq!(
            after_d,
            Pending::default(),
            "nothing was pending to disturb"
        );

        let (after_5, _) = resolve(&keymap, after_d, plain(KeyCode::Char('5')));
        assert_eq!(after_5.count, Some(5));

        let (after_j, action) = resolve(&keymap, after_5, plain(KeyCode::Down));
        assert_eq!(action, Some(NormalAction::MoveDown));
        assert_eq!(after_j, Pending::default());
    }

    #[test]
    fn a_bare_zero_moves_to_the_start_of_the_line_rather_than_starting_a_count() {
        let keymap = Keymap::default_bindings();
        let (pending, action) = resolve(&keymap, Pending::default(), plain(KeyCode::Char('0')));

        assert_eq!(action, Some(NormalAction::LineStart));
        assert_eq!(pending, Pending::default());
    }

    #[test]
    fn a_leading_one_then_a_zero_extends_the_count_to_ten() {
        let keymap = Keymap::default_bindings();
        let keys = [plain(KeyCode::Char('1')), plain(KeyCode::Char('0'))];
        let mut pending = Pending::default();
        for key in keys {
            let (next, action) = resolve(&keymap, pending, key);
            assert_eq!(action, None, "still just accumulating a count");
            pending = next;
        }

        assert_eq!(pending.count, Some(10));

        let (after_j, action) = resolve(&keymap, pending, plain(KeyCode::Char('j')));
        assert_eq!(action, Some(NormalAction::MoveDown));
        assert_eq!(after_j, Pending::default());
    }

    #[test]
    fn opening_a_chord_and_then_pressing_an_unrelated_key_cancels_it_with_no_action() {
        let keymap = Keymap::default_bindings();
        let (awaiting, action) = resolve(&keymap, Pending::default(), plain(KeyCode::Char('g')));
        assert_eq!(action, None);
        assert_eq!(awaiting.chord, ChordState::AwaitingG);

        let (after_esc, action) = resolve(&keymap, awaiting, plain(KeyCode::Esc));
        assert_eq!(
            action, None,
            "Esc is not the second key of any 'g' chord, so it cancels rather than firing Back"
        );
        assert_eq!(after_esc, Pending::default());
    }

    #[test]
    fn gt_fires_the_instant_it_resolves_and_a_fresh_g_opens_a_new_chord() {
        let keymap = Keymap::default_bindings();
        let keys = [plain(KeyCode::Char('g')), plain(KeyCode::Char('t'))];
        let (after_gt, action) = resolve_all(&keymap, &keys);
        assert_eq!(action, Some(NormalAction::NextView));
        assert_eq!(after_gt, Pending::default());

        let (after_second_g, action) = resolve(&keymap, after_gt, plain(KeyCode::Char('g')));
        assert_eq!(action, None);
        assert_eq!(after_second_g.chord, ChordState::AwaitingG);
    }

    #[test]
    fn a_count_before_gt_goes_to_that_view_instead_of_the_next_one() {
        let keymap = Keymap::default_bindings();
        let keys = [
            plain(KeyCode::Char('3')),
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('t')),
        ];
        let (pending, action) = resolve_all(&keymap, &keys);

        assert_eq!(action, Some(NormalAction::GotoView(3)));
        assert_eq!(pending, Pending::default());
    }

    #[test]
    fn ctrl_d_and_ctrl_u_scroll_by_half_a_page() {
        let keymap = Keymap::default_bindings();
        let (_, down) = resolve(&keymap, Pending::default(), ctrl(KeyCode::Char('d')));
        let (_, up) = resolve(&keymap, Pending::default(), ctrl(KeyCode::Char('u')));

        assert_eq!(down, Some(NormalAction::HalfPage(Dir::Down)));
        assert_eq!(up, Some(NormalAction::HalfPage(Dir::Up)));
    }
}

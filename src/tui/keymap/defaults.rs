//! The keymap's actual data: the table of `key -> action` bindings shipped
//! with the dashboard.
//!
//! Kept apart from `keymap.rs` for the same reason `palette/builtins.rs` sits
//! apart from `palette.rs`: the parent module's types and behaviour are the
//! interesting code, and this file is a flat table of literal bindings. Every
//! row here is a judgement call about which key should do what, and mixing
//! that judgement into the same file as the state machine that resolves it
//! would bury one under the other.
//!
//! `d` and `l` as bare, unmodified keys are deliberately absent from this
//! table. `l` is not gone -- it is `PanRight`'s binding now, alongside
//! `Right` -- but the binding this table used to give it, "go to the Log
//! view", has no replacement key of its own: the log is reached by cycling
//! views with `gt`/`gT` instead. `d`, which used to mean "go to the
//! Dashboard", is retired outright with nothing standing in for it. Both
//! retirements are the one deliberately observable behaviour change this
//! epic makes, which is why it ships as its own commit rather than folded
//! into a colour-only change.

use crossterm::event::{KeyCode, KeyModifiers};

use super::{Binding, Dir, Group, Key, KeySeq, NormalAction, RowTarget};

/// A key pressed with no modifiers -- the common case for almost every
/// binding below.
const fn plain(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::NONE,
    }
}

/// A key pressed while holding Control.
const fn ctrl(code: KeyCode) -> Key {
    Key {
        code,
        mods: KeyModifiers::CONTROL,
    }
}

/// A single-key binding.
const fn one(
    key: Key,
    action: NormalAction,
    description: &'static str,
    group: Group,
    pinned: bool,
) -> Binding {
    Binding {
        seq: KeySeq::One(key),
        action,
        description,
        group,
        pinned,
    }
}

/// A two-key chord binding. No chord shipped today is pinned: a chord always
/// has `is_prefix_key`'s escape hatch (pressing anything else cancels it) in
/// a way a single key does not, so nothing forces one to be load-bearing.
const fn two(
    first: Key,
    second: Key,
    action: NormalAction,
    description: &'static str,
    group: Group,
) -> Binding {
    Binding {
        seq: KeySeq::Two(first, second),
        action,
        description,
        group,
        pinned: false,
    }
}

/// Every binding the dashboard ships with, in normal mode.
///
/// Split into one function per [`Group`] below, the same way
/// `palette::builtins::all` is one function per theme rather than one
/// enormous literal: each group's function comfortably fits on a screen on
/// its own, and `cargo clippy`'s `too_many_lines` lint agrees that a single
/// two-hundred-line function would not have.
pub(super) fn bindings() -> Vec<Binding> {
    let mut all = global_bindings();
    all.extend(motion_bindings());
    all.extend(scroll_bindings());
    all.extend(line_bindings());
    all.extend(jump_bindings());
    all.extend(view_bindings());
    all.extend(pane_bindings());
    all.extend(search_and_command_bindings());
    all.extend(appearance_bindings());
    all
}

/// Quit, help, refresh, and the other keys that mean the same thing
/// regardless of which view is showing.
fn global_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Char('q')),
            NormalAction::Quit,
            "quit",
            Group::Global,
            false,
        ),
        one(
            plain(KeyCode::Esc),
            NormalAction::Back,
            "dismiss the help, or leave the current view",
            Group::Global,
            true,
        ),
        one(
            plain(KeyCode::Enter),
            NormalAction::Confirm,
            "attach to the selected session",
            Group::Global,
            true,
        ),
        one(
            plain(KeyCode::Char('?')),
            NormalAction::ToggleHelp,
            "show this help",
            Group::Global,
            false,
        ),
        one(
            plain(KeyCode::F(1)),
            NormalAction::ToggleHelp,
            "show this help",
            Group::Global,
            false,
        ),
        one(
            plain(KeyCode::Char('r')),
            NormalAction::Refresh,
            "re-read the transcript and re-measure usage",
            Group::Global,
            false,
        ),
        one(
            plain(KeyCode::Char('o')),
            NormalAction::OpenSessions,
            "open the session picker",
            Group::Views,
            false,
        ),
    ]
}

/// Moving the selection or panning the viewport by one step: `j`/`k`/`h`/`l`
/// and the arrow keys.
fn motion_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Char('j')),
            NormalAction::MoveDown,
            "move down",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Down),
            NormalAction::MoveDown,
            "move down",
            Group::Motion,
            true,
        ),
        one(
            plain(KeyCode::Char('k')),
            NormalAction::MoveUp,
            "move up",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Up),
            NormalAction::MoveUp,
            "move up",
            Group::Motion,
            true,
        ),
        one(
            plain(KeyCode::Char('h')),
            NormalAction::PanLeft,
            "pan left",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Left),
            NormalAction::PanLeft,
            "pan left",
            Group::Motion,
            true,
        ),
        one(
            plain(KeyCode::Char('l')),
            NormalAction::PanRight,
            "pan right",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Right),
            NormalAction::PanRight,
            "pan right",
            Group::Motion,
            true,
        ),
    ]
}

/// Scrolling by a half or full page: `Ctrl-d`/`Ctrl-u`, `Ctrl-f`/`Ctrl-b` and
/// `PageDown`/`PageUp`.
fn scroll_bindings() -> Vec<Binding> {
    vec![
        one(
            ctrl(KeyCode::Char('d')),
            NormalAction::HalfPage(Dir::Down),
            "scroll down half a page",
            Group::Motion,
            false,
        ),
        one(
            ctrl(KeyCode::Char('u')),
            NormalAction::HalfPage(Dir::Up),
            "scroll up half a page",
            Group::Motion,
            false,
        ),
        one(
            ctrl(KeyCode::Char('f')),
            NormalAction::Page(Dir::Down),
            "scroll down a page",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::PageDown),
            NormalAction::Page(Dir::Down),
            "scroll down a page",
            Group::Motion,
            true,
        ),
        one(
            ctrl(KeyCode::Char('b')),
            NormalAction::Page(Dir::Up),
            "scroll up a page",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::PageUp),
            NormalAction::Page(Dir::Up),
            "scroll up a page",
            Group::Motion,
            true,
        ),
    ]
}

/// Jumping within the current line or section: `{`/`}` and `0`/`Home`/`$`/
/// `End`.
fn line_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Char('{')),
            NormalAction::PrevSection,
            "jump to the previous section",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Char('}')),
            NormalAction::NextSection,
            "jump to the next section",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Char('0')),
            NormalAction::LineStart,
            "jump to the start of the line",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::Home),
            NormalAction::LineStart,
            "jump to the start of the line",
            Group::Motion,
            true,
        ),
        one(
            plain(KeyCode::Char('$')),
            NormalAction::LineEnd,
            "jump to the end of the line",
            Group::Motion,
            false,
        ),
        one(
            plain(KeyCode::End),
            NormalAction::LineEnd,
            "jump to the end of the line",
            Group::Motion,
            true,
        ),
    ]
}

/// Jumping to an absolute position: the top or bottom of a list, or back to
/// wherever the last jump came from.
fn jump_bindings() -> Vec<Binding> {
    vec![
        two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('g')),
            NormalAction::JumpToRow(RowTarget::Top),
            "jump to the top",
            Group::Jumps,
        ),
        one(
            plain(KeyCode::Char('G')),
            NormalAction::JumpToRow(RowTarget::Bottom),
            "jump to the bottom",
            Group::Jumps,
            false,
        ),
        one(
            ctrl(KeyCode::Char('o')),
            NormalAction::JumpBack,
            "jump back to the previous position",
            Group::Jumps,
            false,
        ),
    ]
}

/// Cycling between the dashboard and the log.
fn view_bindings() -> Vec<Binding> {
    vec![
        two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('t')),
            NormalAction::NextView,
            "next view",
            Group::Views,
        ),
        two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('T')),
            NormalAction::PrevView,
            "previous view",
            Group::Views,
        ),
    ]
}

/// Moving focus between panes within a view.
fn pane_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Tab),
            NormalAction::FocusNext,
            "focus the next pane",
            Group::Panes,
            false,
        ),
        one(
            plain(KeyCode::BackTab),
            NormalAction::FocusPrev,
            "focus the previous pane",
            Group::Panes,
            false,
        ),
    ]
}

/// Entering search or command mode, and repeating the last search.
fn search_and_command_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Char('/')),
            NormalAction::EnterSearch,
            "search",
            Group::Search,
            false,
        ),
        one(
            plain(KeyCode::Char(':')),
            NormalAction::EnterCommand,
            "command",
            Group::Command,
            false,
        ),
        one(
            plain(KeyCode::Char('n')),
            NormalAction::RepeatSearch(true),
            "repeat the last search",
            Group::Search,
            false,
        ),
        one(
            plain(KeyCode::Char('N')),
            NormalAction::RepeatSearch(false),
            "repeat the last search, reversed",
            Group::Search,
            false,
        ),
    ]
}

/// Choosing a theme or a layout at runtime.
///
/// `L` for the layout picker, rather than the epic's own literal
/// `Key { code: Char('L'), mods: SHIFT }`: every capital letter already bound
/// in this table (`G` for `JumpToRow(Bottom)`, `T` in the `gT` chord) is
/// looked up as `Char('L')` with **no** modifier, because a real terminal
/// reports a shifted letter as the capitalised `char` itself, not as a
/// lower-case letter plus a `SHIFT` bit -- `crossterm` only sets `SHIFT`
/// alongside a *keyboard-enhancement-protocol* event, which this crate does
/// not opt into. Binding `Key { code: Char('L'), mods: SHIFT }` literally
/// would compile, validate, and then never once fire from a real keypress;
/// matching this table's own established convention for capital letters is
/// what actually makes `L` reachable.
fn appearance_bindings() -> Vec<Binding> {
    vec![
        one(
            plain(KeyCode::Char('t')),
            NormalAction::CycleTheme,
            "cycle the theme",
            Group::Appearance,
            false,
        ),
        one(
            plain(KeyCode::Char('L')),
            NormalAction::OpenLayoutPicker,
            "choose a layout",
            Group::Appearance,
            false,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_this_epic_specifies_is_present() {
        let bindings = bindings();
        let has = |seq: KeySeq| bindings.iter().any(|binding| binding.seq == seq);

        assert!(has(KeySeq::One(plain(KeyCode::Char('q')))));
        assert!(has(KeySeq::Two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('g'))
        )));
        assert!(has(KeySeq::Two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('t'))
        )));
        assert!(has(KeySeq::Two(
            plain(KeyCode::Char('g')),
            plain(KeyCode::Char('T'))
        )));
        assert!(has(KeySeq::One(plain(KeyCode::Tab))));
        assert!(has(KeySeq::One(plain(KeyCode::BackTab))));
        assert!(has(KeySeq::One(plain(KeyCode::Char('/')))));
        assert!(has(KeySeq::One(plain(KeyCode::Char(':')))));
    }

    #[test]
    fn bare_d_no_longer_switches_to_the_dashboard() {
        let bindings = bindings();
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.seq == KeySeq::One(plain(KeyCode::Char('d')))),
            "'d' used to mean 'go to the dashboard' and is retired outright"
        );
    }

    #[test]
    fn bare_l_now_pans_right_instead_of_opening_the_log_view() {
        let bindings = bindings();
        let l_key = KeySeq::One(plain(KeyCode::Char('l')));
        let binding = bindings
            .iter()
            .find(|binding| binding.seq == l_key)
            .expect("'l' is still bound -- just to something else now");

        assert_eq!(
            binding.action,
            NormalAction::PanRight,
            "'l' used to open the log view; it is a motion key now, paired with Right"
        );
    }
}

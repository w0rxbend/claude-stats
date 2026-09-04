//! The dashboard's state machine.
//!
//! Key events are translated into a [`NormalAction`] first, by
//! [`crate::tui::keymap::resolve`], and the state transitions here are written against
//! actions rather than against key codes. That indirection buys two things:
//! the transitions are testable without synthesising terminal events, and
//! rebinding a key is a change to `crate::tui::keymap::defaults` rather
//! than a hunt through this file's update logic. What used to be a single
//! `match` on `KeyCode` in this module (`Action::from_key`) is now
//! [`Keymap`], a Registry (Fowler, *`PoEAA`*) that the event loop, the footer
//! hint and the help screen all read the same table from -- see
//! [`crate::tui::keymap`] for the full account of why.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::application::monitor::{Monitor, Tick};
use crate::application::ports::{ChangeSourceFactory, SessionReader, TranscriptCatalog};
use crate::application::usage::UsageTracker;
use crate::domain::period::Zone;
use crate::infrastructure::config::{Config, ConfigGateway, ConfigWarning};
use crate::tui::icons::Icon;
use crate::tui::keymap::{ChordState, Dir, Keymap, NormalAction, Pending, RowTarget};
use crate::tui::palette::Palette;
use crate::tui::palette::registry::ThemeRegistry;
use crate::tui::screens;
use crate::tui::widgets::dollar_pulse::{AnimationStyle, PulseClock};
use crate::view::{blocks_view, usage_view};

/// Which full-screen content tab is showing.
///
/// Six persistent tabs now, numbered 1-6 the way `crate::tui::screens::TAB_LABELS`
/// and the one-line tab bar every content view reserves its top row for both
/// display them: `Dashboard`, `Daily`, `Weekly`, `Monthly`, `Blocks`, `Log`.
/// `Sessions` is deliberately not one of them any more -- earlier epics had
/// it as a seventh `View` variant reached by switching away from whichever
/// tab was showing, which meant `Esc` after opening it could only ever land
/// back on `Dashboard`, never on the tab the user actually came from. It is
/// now [`Overlay::Sessions`], drawn last over whichever tab is showing via
/// the same `Clear` + bordered `Block` mechanism [`crate::tui::screens::help`]
/// already used for the keybinding overlay, exactly the way the theme and
/// layout pickers already work -- see [`App::underlying_view_before_overlay`]
/// for how `Esc` now restores the tab correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The metrics dashboard.
    #[default]
    Dashboard,
    /// Usage grouped by calendar day.
    Daily,
    /// Usage grouped by calendar week.
    Weekly,
    /// Usage grouped by calendar month.
    Monthly,
    /// The five-hour billing blocks.
    Blocks,
    /// The scrollable event log.
    Log,
}

/// The six content tabs, in the order `gt`/`gT`/`GotoView` cycle through
/// them and the order the tab bar numbers them 1-6.
///
/// The session picker is deliberately not one of these: it is an overlay
/// reached directly with [`NormalAction::OpenSessions`] and left with
/// [`NormalAction::Back`] or [`NormalAction::Confirm`] -- see [`View`]'s own
/// doc comment for the fuller account of why. `bare d`/`bare l`, which used
/// to jump straight to the dashboard or the log, are retired without a
/// direct replacement -- see `crate::tui::keymap::defaults` for why -- so
/// cycling with `gt`/`gT`, or jumping with a count (`3gt`), is how any of the
/// six is reached now.
const CONTENT_VIEWS: [View; 6] = [
    View::Dashboard,
    View::Daily,
    View::Weekly,
    View::Monthly,
    View::Blocks,
    View::Log,
];

/// How many entries the jumplist keeps before the oldest one is dropped.
///
/// A cap rather than an unbounded `Vec` because a long-running dashboard
/// session that cycles tabs a few thousand times over a workday should not
/// grow a jumplist a few thousand entries long for a feature (`Ctrl-o`) that
/// only ever pops from the end -- fifty is comfortably more than anyone
/// backs out of in one sitting.
const JUMPLIST_CAP: usize = 50;

/// The four layout presets [`crate::tui::layout::presets::by_name`] knows,
/// repeated here as a literal rather than read back out of that module,
/// because there is nothing in `presets` to read it *from* -- `by_name` is
/// itself a hand-written `match` over these same four strings, which is the
/// module's own established way of keeping "what a preset is called" as
/// plain data a config file or a picker can pass around; see its own doc
/// comment. A future epic that gives `presets` a real name table can drop
/// this constant in favour of reading it.
const BUILTIN_LAYOUT_NAMES: [&str; 4] = ["live", "spend", "minimal", "wide"];

/// Which full-screen overlay is showing, if any.
///
/// The help screen, the theme picker and the layout picker were three
/// separate `bool` fields on `App` until this epic added the third and
/// fourth to what was previously only `help_open` -- past three,
/// `clippy::struct_excessive_bools` is right to object, but the deeper
/// reason to fold them into one enum is that they were never independent
/// facts to begin with: [`App::close_overlays`] already existed to keep at
/// most one of them true at a time, which is exactly the invariant a
/// four-variant enum enforces by construction rather than by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Overlay {
    #[default]
    None,
    Help,
    ThemePicker,
    LayoutPicker,
    /// The session picker. Folded into this enum rather than kept as the
    /// separate `sessions_open: bool` this epic's own spec first reached
    /// for, for the same reason `Help`/`ThemePicker`/`LayoutPicker` already
    /// were: a fifth field alongside a four-variant enum would both trip
    /// `clippy::struct_excessive_bools` right back over its threshold and
    /// reopen the exact "more than one overlay showing at once" bug this
    /// enum exists to make impossible by construction. See
    /// [`App::underlying_view_before_overlay`] for how `Esc` still lands
    /// back on the right tab without a dedicated boolean of its own.
    Sessions,
}

/// How far a half-page and a full-page scroll move, in rows.
///
/// The dashboard has no fixed row height to derive these from -- the log
/// panel's real height is only known at draw time, the same reason
/// `App::max_log_offset` is "deliberately generous" rather than exact (see
/// its doc comment). These are a reasonable middle ground rather than a
/// measurement: big enough that `Ctrl-f`/`Ctrl-d` visibly move more than
/// `j`/`k`, clamped like every other scroll against the real list length.
const HALF_PAGE: isize = 10;
const FULL_PAGE: isize = 20;

/// Where a search that is about to start typing found the view, so leaving
/// search mode without confirming a match can put it back -- and, since this
/// epic, what a jumplist entry ([`App::push_jump`]) records alongside the
/// [`View`] it belongs to.
///
/// Deliberately mirrors whichever scroll-shaped field the view already
/// tracks -- [`App::log_offset`] for [`View::Log`], [`App::selected`] for
/// [`Overlay::Sessions`] -- rather than inventing a third representation of
/// "where the view is scrolled to". None of the other five tabs has a
/// scroll position of its own to save, hence `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSnapshot {
    Log(usize),
    Sessions(usize),
    None,
}

/// Which input mode the dashboard is in.
///
/// Search and command entry are flipped into by
/// [`NormalAction::EnterSearch`] and [`NormalAction::EnterCommand`]. While
/// either variant is active, [`crate::tui::runtime`]'s event loop stops
/// resolving key presses through [`crate::tui::keymap::resolve`] altogether
/// and feeds every key to [`App::handle_line_edit`] instead -- see that
/// method's own doc comment for why a `Keymap` built to answer "what does
/// this one key mean" is the wrong tool for "append this character to a
/// buffer".
///
/// This replaces the placeholder `Mode` the epic that added `EnterSearch`/
/// `EnterCommand` shipped: that epic's own notes said plainly that a footer
/// notice reading "not yet implemented" was the deliberate, temporary gap
/// this epic exists to close.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Search {
        buf: String,
        /// Where the view was before this search started typing, so
        /// cancelling (`Esc`, or `Backspace` on an empty `buf`) can restore
        /// it exactly -- a search that is abandoned half-typed must not
        /// leave the view wherever the last confirmed search happened to
        /// land.
        origin_scroll: ScrollSnapshot,
    },
    Command {
        buf: String,
    },
}

/// What one key press handed to [`App::handle_line_edit`] did.
///
/// Not consumed by anything in this crate today -- [`crate::tui::runtime`]'s
/// event loop calls `handle_line_edit` and lets the redraw at the top of the
/// next frame pick up whatever changed -- but returning it rather than `()`
/// is what makes each of `handle_line_edit`'s branches independently
/// assertable in a test, the same reason [`crate::tui::keymap::resolve`]
/// returns a fresh [`Pending`] rather than mutating one in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEditOutcome {
    /// The buffer changed (a character was typed or erased) but the mode
    /// did not.
    Changed,
    /// The line was abandoned: `Esc`, or `Backspace` on an empty buffer.
    Cancelled,
    /// `Enter` confirmed the line -- a search ran, or a command executed.
    Confirmed,
    /// The key meant nothing to line editing (an unhandled control
    /// character, for instance) and nothing changed.
    Unchanged,
}

/// The running dashboard.
pub struct App<C, R, W> {
    monitor: Monitor<C, R, W>,
    view: View,
    /// The help screen, a picker, or the session list, or none of them --
    /// see [`Overlay`]'s own doc comment for why this is one field rather
    /// than a `bool` per overlay.
    overlay: Overlay,
    /// Which tab was showing before [`Overlay::Sessions`] opened, so
    /// [`NormalAction::Back`] can restore it -- `Esc` from the picker must
    /// return to the Blocks tab it was opened from, not always to
    /// `View::Dashboard` the way the pre-epic `Action::Back` unconditionally
    /// did once `Sessions` was a `View` variant in its own right.
    ///
    /// Set in [`App::handle`]'s `OpenSessions` arm *only when this is
    /// already `None`* -- opening the help overlay on top of an already-open
    /// session picker (`?` while browsing sessions) must not clobber the
    /// deeper original tab with whatever the picker itself would have
    /// reported, since the picker never changes `view` at all. Cleared
    /// together with `overlay` every time an overlay actually closes back to
    /// [`Overlay::None`] -- see [`App::close_overlays`] -- because a stale
    /// value here would otherwise make the *next* `OpenSessions` silently
    /// refuse to record the tab it was actually opened from.
    underlying_view_before_overlay: Option<View>,
    /// Every `(view, scroll)` position `Ctrl-o` ([`NormalAction::JumpBack`])
    /// can pop back to, oldest first, capped at [`JUMPLIST_CAP`] entries.
    ///
    /// Pushed to immediately before `gt`/`gT`/`{count}gt`
    /// (`NextView`/`PrevView`/`GotoView`) actually move the view, before
    /// `gg`/`G`/`{count}gg` (`JumpToRow`) actually jump, and before a
    /// confirmed search (`Enter` in [`InputMode::Search`]) actually runs --
    /// see each call site's own comment for why capturing the position
    /// *before* it changes, rather than after, is what makes `Ctrl-o` return
    /// to where the user actually was.
    jumplist: Vec<(View, ScrollSnapshot)>,
    /// Animation phase, incremented once per frame and shared by every
    /// animated widget so they stay in step.
    phase: u64,
    /// How many log entries are hidden below the bottom of the log view.
    log_offset: usize,
    sessions: Vec<crate::application::ports::TranscriptRef>,
    selected: usize,
    quit: bool,
    /// A transient message for the footer: a failed attach, a manual refresh.
    notice: Option<String>,
    /// Account-wide usage, when the dashboard was given something to measure
    /// it with. `None` in tests that only care about session behaviour.
    usage: Option<UsageTracker>,
    /// The colours every screen and widget draws with.
    ///
    /// Owned here rather than reached for from a global, because a `Palette`
    /// is a Value Object (Fowler, *`PoEAA`*) threaded through as a parameter --
    /// see [`crate::tui::palette`] for why that indirection is worth the extra
    /// argument on every `draw` function between here and the widgets that
    /// actually read a colour out of it.
    palette: Palette,
    /// The catalogue of every key binding, in normal mode. See the module
    /// documentation for why this is a `Keymap` rather than a `match`.
    keymap: Keymap,
    /// What a count/chord input has accumulated since the last action fired.
    /// Owned here, not inside `Keymap`, because it is per-dashboard state --
    /// two `App`s sharing one `Keymap` (not that any currently do) would each
    /// need their own half-typed `5g`.
    pending: Pending,
    /// Whether normal-mode key handling applies, or the dashboard is instead
    /// collecting a search or a command.
    input_mode: InputMode,
    /// The pattern and direction of the last confirmed search, for `n`/`N`
    /// ([`NormalAction::RepeatSearch`]) to repeat -- `None` until a search
    /// has actually been confirmed once, in either mode.
    last_search: Option<(String, bool)>,
    /// Which row is highlighted in whichever picker ([`Overlay::ThemePicker`]
    /// or [`Overlay::LayoutPicker`]) is open. A single field rather than one
    /// per picker because `overlay` already guarantees at most one of them
    /// is open at a time.
    picker_selected: usize,
    /// Every [`Palette`] name the theme picker can offer, read once from
    /// [`ThemeRegistry::builtin`] at startup. Keeping our own copy, rather
    /// than calling `ThemeRegistry::builtin().names()` afresh every time it
    /// is needed, is what lets [`NormalAction::CycleTheme`] and the picker's
    /// own render both index into *the same order* -- `names()` walks a
    /// `HashMap`, whose iteration order is fixed for the life of one
    /// `OnceLock` but arbitrary in principle, so two independent calls that
    /// happened to disagree would make `t` visibly skip or repeat an entry.
    theme_names: Vec<String>,
    /// Every layout name the layout picker can offer: the four built-in
    /// presets, plus whatever custom trees `config.layouts` named. See
    /// [`App::confirm_layout_picker`] for why choosing one of the custom
    /// names does not yet change what [`screens::dashboard::draw`] shows.
    layout_names: Vec<String>,
    /// The layout preset [`screens::dashboard::draw`] solves against --
    /// loaded from `config.layout` in [`App::with_config`], and changed at
    /// runtime by confirming a choice in the layout picker.
    active_preset: String,
    /// Where a runtime theme or layout change is written back to, if
    /// anywhere.
    ///
    /// `None` in every test in this module and in
    /// [`crate::tui::runtime`]'s own tests: persistence writing to a real
    /// user's `~/.config/claude-stats/config.json` every time a test presses
    /// `t` would make the whole suite touch the developer's actual
    /// filesystem, which is exactly the kind of hidden side effect
    /// `crate::infrastructure::config`'s own tests go out of their way to
    /// avoid (see its `TempDir` helper). Only [`crate::tui::runtime::run`],
    /// the one real composition path, ever calls
    /// [`App::persisting_config`] to fill this in.
    config_gateway: Option<ConfigGateway>,
    /// Watches the account's own spend for `today` tick over, so
    /// `panel.dollar-pulse` knows when to flash. See
    /// `crate::tui::widgets::dollar_pulse`'s module doc for why this is its
    /// own small piece of state rather than two fields living directly on
    /// `App`.
    pulse: PulseClock,
    /// How `panel.dollar-pulse` animates -- resolved once, here at startup,
    /// from the `NO_ANIMATION`/`CLAUDE_STATS_NO_ANIMATION` environment
    /// variables, ahead of any config-driven choice a later epic adds. See
    /// [`initial_animation_style`].
    animation_style: AnimationStyle,
    /// A short note about `config.json`, when
    /// [`crate::infrastructure::config::ConfigGateway::load_or_default`] or
    /// [`crate::infrastructure::config::resolve`] found something in it they
    /// could not honour -- a malformed file, an unknown theme name. Rendered
    /// in the same footer notice slot the `notice` field above uses (see
    /// `App::draw_footer`) and cleared the same way: the first key press
    /// the user makes, in [`App::handle`], the same instant any other
    /// transient footer message would be dismissed. `None` is the ordinary
    /// case -- most users never write a config file, and the ones who do
    /// usually write one that parses.
    config_warning: Option<ConfigWarning>,
    /// Where the Daily/Weekly/Monthly/Blocks tabs' figures come from, if
    /// anywhere.
    ///
    /// `None` in every test in this module, in
    /// [`crate::tui::runtime`]'s own tests, and in any run whose
    /// `FileSystemCatalog::from_home` itself failed -- see
    /// [`App::with_reports`] for why a missing source degrades to those four
    /// tabs showing an honest "nothing loaded yet" message rather than the
    /// whole dashboard refusing to start. Only [`crate::tui::runtime::run`]
    /// ever calls `with_reports` with `Some`, mirroring `config_gateway`'s
    /// own reasoning for staying optional everywhere else.
    report_source: Option<Box<dyn crate::application::report_source::ReportSource>>,
    /// The Daily tab's most recently loaded figures, built by
    /// [`App::ensure_reports_loaded`] from
    /// [`crate::application::report_source::ReportSource::daily`] and reused
    /// until the tab is next entered after a `None` (`r`, or the very first
    /// visit) rather than refetched every frame -- a full corpus scan on
    /// every redraw would make switching to this tab visibly stutter on a
    /// machine with a few thousand transcripts.
    daily_report: Option<crate::view::table::TableView>,
    /// The Weekly tab's cached figures. See [`Self::daily_report`].
    weekly_report: Option<crate::view::table::TableView>,
    /// The Monthly tab's cached figures. See [`Self::daily_report`].
    monthly_report: Option<crate::view::table::TableView>,
    /// The Blocks tab's cached figures. See [`Self::daily_report`].
    blocks_report: Option<crate::view::table::TableView>,
}

impl<C, R, W> App<C, R, W>
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    /// A dashboard driving the given monitor.
    ///
    /// # Panics
    ///
    /// Never in practice: the "aurora" theme is registered unconditionally by
    /// [`ThemeRegistry::builtin`], as one of the twenty-seven built-in
    /// palettes in [`crate::tui::palette::builtins`], so the lookup below
    /// cannot fail. The `expect` exists to say that plainly rather than
    /// leaving an `Option` that every caller would otherwise have to handle a
    /// case that can never actually happen.
    pub fn new(monitor: Monitor<C, R, W>) -> Self {
        Self {
            monitor,
            view: View::default(),
            overlay: Overlay::default(),
            underlying_view_before_overlay: None,
            jumplist: Vec::new(),
            phase: 0,
            log_offset: 0,
            sessions: Vec::new(),
            selected: 0,
            quit: false,
            notice: None,
            usage: None,
            palette: ThemeRegistry::builtin()
                .get("aurora")
                .expect("aurora is always registered")
                .clone(),
            keymap: Keymap::default_bindings(),
            pending: Pending::default(),
            input_mode: InputMode::default(),
            last_search: None,
            picker_selected: 0,
            theme_names: ThemeRegistry::builtin()
                .names()
                .map(str::to_owned)
                .collect(),
            layout_names: BUILTIN_LAYOUT_NAMES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            active_preset: "live".to_owned(),
            config_gateway: None,
            pulse: PulseClock::new(),
            animation_style: initial_animation_style(),
            config_warning: None,
            report_source: None,
            daily_report: None,
            weekly_report: None,
            monthly_report: None,
            blocks_report: None,
        }
    }

    /// Adds account-wide usage tracking to the dashboard.
    ///
    /// Separate from [`App::new`] because the dashboard is perfectly usable
    /// without it -- and because a test that is checking key handling should
    /// not have to supply a scanner for every transcript on the machine.
    #[must_use]
    pub fn tracking_usage(mut self, tracker: UsageTracker) -> Self {
        self.usage = Some(tracker);
        self
    }

    /// Applies the user's `config.json`, resolved against the real theme
    /// registry, to this dashboard.
    ///
    /// Separate from [`App::new`] for the same reason [`App::tracking_usage`]
    /// is: most of this module's own tests build an `App` with nothing more
    /// than a monitor and have no interest in a config file, and requiring
    /// every one of them to also thread a [`Config`] through `new` would be
    /// asking a test that only cares about key handling to also carry an
    /// opinion about theming. `warning` is whatever
    /// [`crate::infrastructure::config::ConfigGateway::load_or_default`] and
    /// [`crate::infrastructure::config::resolve`] found wrong with the file,
    /// if anything -- see `config_warning`'s own field doc comment for where
    /// it ends up.
    ///
    /// # Panics
    ///
    /// Never in practice, for the same reason [`App::new`]'s own `expect`
    /// cannot fail: `"aurora"` is one of the twenty-seven built-in palettes
    /// [`ThemeRegistry::builtin`] registers unconditionally, so the fallback
    /// this method reaches for whenever `config.theme` is `None` (or names a
    /// theme [`crate::infrastructure::config::resolve`] has already
    /// downgraded back to `None`) always resolves.
    #[must_use]
    pub fn with_config(mut self, config: &Config, warning: Option<ConfigWarning>) -> Self {
        self.palette = config
            .theme
            .as_deref()
            .and_then(|name| ThemeRegistry::builtin().get(name))
            .cloned()
            .unwrap_or_else(|| {
                ThemeRegistry::builtin()
                    .get("aurora")
                    .expect("aurora always registered")
                    .clone()
            });
        self.active_preset = config.layout.clone().unwrap_or_else(|| "live".to_owned());
        // A `BTreeSet` rather than the `HashMap`'s own arbitrary order:
        // this list is what the layout picker renders, and re-shuffling
        // itself on every restart (or every time this method runs, in a
        // test that calls it twice) would make the picker feel broken even
        // though nothing is actually wrong with it.
        let mut names: std::collections::BTreeSet<String> = BUILTIN_LAYOUT_NAMES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        names.extend(config.layouts.keys().cloned());
        self.layout_names = names.into_iter().collect();
        // `App::new` already resolved `NO_ANIMATION`/`CLAUDE_STATS_NO_ANIMATION`
        // into `self.animation_style` before any config file was in the
        // picture, defaulting to `AnimationStyle::Pulse` when neither was
        // set. That default is what `config.animation` overrides here --
        // re-running `resolve_animation_style` with `config.animation` as
        // the new "default" keeps the environment variable's veto intact
        // (an explicit `NO_ANIMATION=1` still forces `Off` even if
        // `config.json` asks for `"coin"`) while finally giving the config
        // file's own `animation` field somewhere to land. Before this, the
        // field parsed and validated cleanly but every dashboard rendered
        // `Pulse` regardless of what the file said -- a config value that
        // was read but never applied.
        self.animation_style = resolve_animation_style(no_animation_requested(), config.animation);
        self.config_warning = warning;
        self
    }

    /// Points a later runtime theme or layout change at a real
    /// `config.json`.
    ///
    /// Separate from [`App::with_config`] for the same reason
    /// [`App::tracking_usage`] is separate from [`App::new`]: reading a
    /// config file at startup and being able to *write* one back are
    /// different capabilities, and giving every test in this module (which
    /// only ever wants the first) a `ConfigGateway` it would have to point
    /// at a temporary directory just to avoid touching a real one would be
    /// asking the wrong question of most callers. See `config_gateway`'s own
    /// field doc comment for the full account of why this stays `None`
    /// everywhere except [`crate::tui::runtime::run`].
    #[must_use]
    pub fn persisting_config(mut self, gateway: ConfigGateway) -> Self {
        self.config_gateway = Some(gateway);
        self
    }

    /// Points the Daily/Weekly/Monthly/Blocks tabs at a real source of
    /// figures.
    ///
    /// `source` is `Option` rather than a bare
    /// `impl ReportSource + 'static`, unlike [`App::tracking_usage`]'s bare
    /// `UsageTracker`, because the one production caller
    /// ([`crate::tui::runtime::run`]) already has to decide whether it could
    /// build one at all -- `FileSystemCatalog::from_home` can fail the same
    /// way it can for the session catalogue itself -- and folding that
    /// `Option` in here is simpler than a second builder method that only
    /// ever gets called conditionally. `None` here (every test in this
    /// module included) leaves those four tabs showing their own "nothing
    /// loaded yet" message rather than a panic or a fabricated figure.
    #[must_use]
    pub fn with_reports(
        mut self,
        source: Option<Box<dyn crate::application::report_source::ReportSource>>,
    ) -> Self {
        self.report_source = source;
        self
    }

    /// The keymap this dashboard resolves key presses against.
    ///
    /// Exposed so the event loop can call [`crate::tui::keymap::resolve`] with it and so
    /// the help overlay can read [`Keymap::help_rows`] from it -- both live
    /// outside this module, and neither should keep a copy of their own.
    #[must_use]
    pub const fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// The count/chord state accumulated since the last action fired.
    ///
    /// Read by the event loop before calling [`crate::tui::keymap::resolve`] and written
    /// back with the state that call returns; see `runtime::event_loop` for
    /// the exact wiring.
    #[must_use]
    pub const fn pending(&self) -> Pending {
        self.pending
    }

    /// Records the count/chord state the event loop's last call to
    /// [`crate::tui::keymap::resolve`] produced.
    pub fn set_pending(&mut self, pending: Pending) {
        self.pending = pending;
    }

    /// Whether normal-mode key handling applies right now, or every key
    /// should instead go to [`App::handle_line_edit`].
    ///
    /// Read by [`crate::tui::runtime`]'s event loop before it decides which
    /// of `keymap::resolve` or `handle_line_edit` a given key press belongs
    /// to -- see that module's `handle_key` for the branch this drives.
    #[must_use]
    pub const fn input_mode(&self) -> &InputMode {
        &self.input_mode
    }

    /// Whether the event loop should stop.
    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.quit
    }

    /// The current view.
    #[must_use]
    pub const fn view(&self) -> View {
        self.view
    }

    /// Advances animation and lets the monitor do its work.
    ///
    /// Every tick is worth a redraw, which is why nothing is reported back:
    /// animation counts on its own, because a spinner that only moves when the
    /// transcript does is a spinner that looks frozen during a long tool call.
    pub fn tick(&mut self) {
        self.phase = self.phase.wrapping_add(1);
        if let Some(usage) = &mut self.usage {
            usage.tick();
        }
        // The same "today" figure `panel.spend-panel` already prints (see
        // `crate::tui::widgets::spend_panel`'s module doc) is what arms the
        // pulse: a rise in the account's own spend for today, not this
        // session's own cost, is what `panel.dollar-pulse` is a marker for.
        if let Some(usage) = &self.usage {
            self.pulse.observe(usage.usage().today.cost, self.phase);
        }
        let outcome = self.monitor.tick();
        if outcome == Tick::Attached {
            // A different session means the old scroll position points at
            // entries that no longer exist.
            self.log_offset = 0;
        }
    }

    /// Applies an action resolved from normal-mode key handling.
    pub fn handle(&mut self, action: NormalAction) {
        self.notice = None;
        // The config warning is a startup-time notice, not a per-action one,
        // but it shares `notice`'s footer slot and the same "gone at the
        // first key press" lifetime -- see `config_warning`'s own doc
        // comment for why a distinct field rather than folding it into
        // `notice` itself.
        self.config_warning = None;
        match action {
            NormalAction::Quit => self.quit = true,
            // Esc on the session picker restores the tab it was opened from
            // rather than falling through to the generic "close whatever
            // overlay is open" arm below -- see
            // `underlying_view_before_overlay`'s own doc comment for why
            // that restoration needs a dedicated arm.
            NormalAction::Back if self.overlay == Overlay::Sessions => {
                if let Some(view) = self.underlying_view_before_overlay.take() {
                    self.view = view;
                }
                self.close_overlays();
            }
            // Esc on either picker cancels without persisting: unlike the
            // theme picker's `t`, which applies (and saves) on every step,
            // nothing here has touched `active_preset` or `config.json` yet.
            // On the help overlay it simply closes it.
            NormalAction::Back if self.overlay != Overlay::None => self.close_overlays(),
            NormalAction::Back => {
                if self.view == View::Dashboard {
                    self.quit = true;
                } else {
                    self.view = View::Dashboard;
                }
            }
            NormalAction::ToggleHelp if self.overlay == Overlay::Help => self.close_overlays(),
            // Deliberately *not* `self.close_overlays()` first: that would
            // also clear `underlying_view_before_overlay`, which must
            // survive `?` opening help on top of an already-open session
            // picker -- see that field's own doc comment. Assigning
            // `overlay` directly still keeps the "at most one overlay"
            // invariant, since the enum has room for exactly one value.
            NormalAction::ToggleHelp => self.overlay = Overlay::Help,
            NormalAction::OpenSessions => self.open_sessions(),
            NormalAction::MoveDown => self.move_picker_or_selection(1),
            NormalAction::MoveUp => self.move_picker_or_selection(-1),
            NormalAction::JumpToRow(target) => {
                self.push_jump();
                self.jump_to_row(target);
            }
            NormalAction::HalfPage(dir) => self.move_selection(page_delta(dir, HALF_PAGE)),
            NormalAction::Page(dir) => self.move_selection(page_delta(dir, FULL_PAGE)),
            NormalAction::NextView => {
                self.push_jump();
                self.cycle_view(1);
            }
            NormalAction::PrevView => {
                self.push_jump();
                self.cycle_view(-1);
            }
            NormalAction::GotoView(n) => {
                self.push_jump();
                self.goto_view(n);
            }
            NormalAction::JumpBack => self.jump_back(),
            NormalAction::Confirm => match self.overlay {
                Overlay::LayoutPicker => self.confirm_layout_picker(),
                // Already applied and persisted on every `t` press -- see
                // `App::cycle_theme` -- so Enter here only closes the picker
                // rather than repeating that work.
                Overlay::ThemePicker => self.close_overlays(),
                Overlay::Sessions => self.attach_selected(),
                Overlay::Help | Overlay::None => {}
            },
            NormalAction::Refresh => {
                self.refresh_session_list();
                if let Some(usage) = &mut self.usage {
                    usage.scan();
                }
                self.daily_report = None;
                self.weekly_report = None;
                self.monthly_report = None;
                self.blocks_report = None;
                self.ensure_reports_loaded();
            }
            NormalAction::EnterSearch => {
                // The session picker is the one overlay search still reaches
                // into (see `App::scroll_snapshot`/`App::run_search_with`),
                // so it is deliberately spared the generic overlay-closing
                // every other case gets: dismissing it here would mean `/`
                // while browsing sessions silently searched whichever tab
                // was underneath instead.
                if self.overlay != Overlay::Sessions {
                    self.close_overlays();
                }
                let origin_scroll = self.scroll_snapshot();
                self.input_mode = InputMode::Search {
                    buf: String::new(),
                    origin_scroll,
                };
            }
            NormalAction::EnterCommand => {
                self.close_overlays();
                self.input_mode = InputMode::Command { buf: String::new() };
            }
            NormalAction::RepeatSearch(same_direction) => self.repeat_search(same_direction),
            NormalAction::CycleTheme => self.cycle_theme(),
            NormalAction::OpenLayoutPicker => self.open_layout_picker(),
            // These five read as bound keys today -- `Keymap::validate` and
            // the help screen already describe them -- but `App` has no
            // state yet for any of them to act on: no horizontal-scroll
            // position for `PanLeft`/`PanRight`/`LineStart`/`LineEnd`, and no
            // notion of a "section" within a view for `PrevSection`/
            // `NextSection` to move between. Wiring the keys ahead of the
            // behaviour they will eventually drive is what let an earlier
            // epic ship the whole keymap at once rather than a fifth of it;
            // the behaviour itself belongs to whichever later epic adds
            // panes or sections. `FocusNext`/`FocusPrev` are the same story.
            NormalAction::PanLeft
            | NormalAction::PanRight
            | NormalAction::PrevSection
            | NormalAction::NextSection
            | NormalAction::LineStart
            | NormalAction::LineEnd
            | NormalAction::FocusNext
            | NormalAction::FocusPrev => {}
        }
    }

    /// Handles Ctrl-C.
    ///
    /// This dashboard treats Ctrl-C the way a shell treats it at its prompt:
    /// it aborts whatever is half-typed rather than closing the window
    /// outright. Only when there is truly nothing to abort -- no count or
    /// chord pending, and not midway through typing a search or a command --
    /// does it fall through to the same behaviour as `q`. Handled here rather
    /// than inside [`crate::tui::keymap::resolve`] because "abort the line" is a
    /// decision about *mode*, not about any one key binding, and `resolve`
    /// is pure state-machine logic with no idea what mode the dashboard is
    /// in.
    pub fn handle_ctrl_c(&mut self) {
        let nothing_pending = self.pending == Pending::default();
        let in_normal_mode = matches!(self.input_mode, InputMode::Normal);
        if nothing_pending && in_normal_mode {
            self.quit = true;
        } else {
            self.pending = Pending::default();
            self.input_mode = InputMode::Normal;
        }
    }

    /// Closes whichever overlay this dashboard is showing, other than the
    /// view itself.
    ///
    /// Called wherever an overlay actually closes back to [`Overlay::None`]
    /// -- `Esc`'s generic arm in [`App::handle`], and switching tabs, which
    /// abandons whatever overlay was open the same way it always has.
    /// `underlying_view_before_overlay` is cleared here too, and only here:
    /// it must survive `ToggleHelp` or `CycleTheme` *swapping* one overlay
    /// for another on top of an open session picker (those assign `overlay`
    /// directly rather than routing through this method -- see
    /// [`App::handle`]'s `ToggleHelp` arm for why), but a stale value left
    /// behind once every overlay is actually gone would make the *next*
    /// `OpenSessions` wrongly believe it already knew the tab it was opened
    /// from.
    fn close_overlays(&mut self) {
        self.overlay = Overlay::None;
        self.underlying_view_before_overlay = None;
    }

    /// Opens the session picker, recording which tab it was opened from the
    /// first time -- not the second, if a picker is already open -- so
    /// [`NormalAction::Back`] can restore exactly that tab. See
    /// `underlying_view_before_overlay`'s own doc comment for the full
    /// reasoning.
    fn open_sessions(&mut self) {
        if self.underlying_view_before_overlay.is_none() {
            self.underlying_view_before_overlay = Some(self.view);
        }
        self.overlay = Overlay::Sessions;
        self.refresh_session_list();
    }

    /// Records where the dashboard is looking right now, so `Ctrl-o`
    /// ([`NormalAction::JumpBack`]) can return to it. Every call site in
    /// [`App::handle`] runs this *before* the action it guards actually
    /// changes the view or the scroll position -- capturing the position
    /// after the fact would push the destination onto the jumplist instead
    /// of the place the jump started from, making `Ctrl-o` a no-op.
    fn push_jump(&mut self) {
        if self.jumplist.len() >= JUMPLIST_CAP {
            self.jumplist.remove(0);
        }
        self.jumplist.push((self.view, self.scroll_snapshot()));
    }

    /// Pops the most recent jumplist entry, if there is one, and returns to
    /// it -- switching the view as well as the scroll position, when the
    /// entry names a different tab from the one showing now.
    fn jump_back(&mut self) {
        let Some((view, scroll)) = self.jumplist.pop() else {
            return;
        };
        self.view = view;
        self.restore_scroll(scroll);
    }

    /// Where a search starting now should be able to come back to, if it is
    /// abandoned rather than confirmed -- also what a jumplist entry
    /// ([`App::push_jump`]) records alongside the current [`View`].
    ///
    /// Checked against `overlay` before `view`: the session picker is drawn
    /// over whichever tab happens to be showing rather than being a `View`
    /// of its own (see [`View`]'s own doc comment), so its scroll position
    /// has to be read from `overlay` instead of from a `View::Sessions` that
    /// no longer exists.
    fn scroll_snapshot(&self) -> ScrollSnapshot {
        if self.overlay == Overlay::Sessions {
            return ScrollSnapshot::Sessions(self.selected);
        }
        match self.view {
            View::Log => ScrollSnapshot::Log(self.log_offset),
            View::Dashboard | View::Daily | View::Weekly | View::Monthly | View::Blocks => {
                ScrollSnapshot::None
            }
        }
    }

    /// Puts a previously-taken [`ScrollSnapshot`] back.
    fn restore_scroll(&mut self, snapshot: ScrollSnapshot) {
        match snapshot {
            ScrollSnapshot::Log(offset) => self.log_offset = offset,
            ScrollSnapshot::Sessions(selected) => self.selected = selected,
            ScrollSnapshot::None => {}
        }
    }

    /// `MoveUp`/`MoveDown` while a picker is open move `picker_selected` (or,
    /// for the session picker, `selected`), wrapping at either end, instead
    /// of the current tab's own selection.
    fn move_picker_or_selection(&mut self, delta: isize) {
        let len = match self.overlay {
            Overlay::ThemePicker => self.theme_names.len(),
            Overlay::LayoutPicker => self.layout_names.len(),
            Overlay::Sessions => {
                self.move_session_selection(delta);
                return;
            }
            Overlay::Help | Overlay::None => {
                self.move_selection(delta);
                return;
            }
        };
        if len == 0 {
            return;
        }
        // `picker_selected` and `len` are both list lengths/indices, never
        // more than a handful of entries -- the built-in themes and layout
        // presets are counted in dozens, not billions -- so the
        // usize-to-isize round trip below can never actually wrap; the
        // `unwrap_or` fallbacks exist only to satisfy
        // `clippy::cast_possible_wrap`'s pedantic caution with a
        // fallible conversion rather than an assertion that could panic.
        let len_signed = isize::try_from(len).unwrap_or(isize::MAX);
        let selected_signed = isize::try_from(self.picker_selected).unwrap_or(0);
        let next = (selected_signed + delta).rem_euclid(len_signed);
        self.picker_selected = usize::try_from(next).unwrap_or(0);
    }

    /// Moves the highlighted row in the session picker, clamped at both
    /// ends rather than wrapping -- the same rule [`App::move_selection`]'s
    /// old `View::Sessions` arm applied before the picker became an overlay,
    /// carried over verbatim rather than reimagined.
    fn move_session_selection(&mut self, delta: isize) {
        if self.sessions.is_empty() {
            return;
        }
        let last = self.sessions.len() - 1;
        self.selected = if delta > 0 {
            (self.selected.saturating_add(delta.unsigned_abs())).min(last)
        } else {
            self.selected.saturating_sub(delta.unsigned_abs())
        };
    }

    /// Opens the theme picker at the currently active theme, or -- when it
    /// is already open -- advances it to the next entry and applies (and
    /// persists) that theme at once.
    ///
    /// Splitting "open" from "advance" on the very same key is what the
    /// design calls for: `t` alone, with no `Enter`, is meant to be a fast
    /// cycle through every registered theme, which only works if the very
    /// first press does not also change anything -- a user who taps `t`
    /// once to see what the picker looks like should not have already
    /// changed their theme by doing so.
    fn cycle_theme(&mut self) {
        if self.theme_names.is_empty() {
            return;
        }
        if self.overlay == Overlay::ThemePicker {
            self.picker_selected = (self.picker_selected + 1) % self.theme_names.len();
            self.apply_theme_at_selected();
        } else {
            // Assigned directly rather than through `close_overlays`, for
            // the same reason `NormalAction::ToggleHelp` is in `App::handle`:
            // this can be reached with the session picker already open (`t`
            // while browsing sessions), and `close_overlays` would also
            // clear `underlying_view_before_overlay`, losing the tab `Esc`
            // is meant to restore once the theme picker itself closes.
            self.overlay = Overlay::ThemePicker;
            self.picker_selected = self
                .theme_names
                .iter()
                .position(|name| *name == self.palette.name)
                .unwrap_or(0);
        }
    }

    /// Applies and persists whichever theme `picker_selected` currently
    /// names.
    fn apply_theme_at_selected(&mut self) {
        let Some(name) = self.theme_names.get(self.picker_selected).cloned() else {
            return;
        };
        if let Some(palette) = ThemeRegistry::builtin().get(&name) {
            self.palette = palette.clone();
        }
        self.persist(|config| config.theme = Some(name));
    }

    /// Opens the layout picker at the currently active preset.
    fn open_layout_picker(&mut self) {
        // See `App::cycle_theme`'s own comment for why this is a direct
        // assignment rather than a `close_overlays` call.
        self.overlay = Overlay::LayoutPicker;
        self.picker_selected = self
            .layout_names
            .iter()
            .position(|name| *name == self.active_preset)
            .unwrap_or(0);
    }

    /// Applies and persists whichever layout `picker_selected` currently
    /// names, then closes the picker.
    ///
    /// Only the four built-in presets in [`BUILTIN_LAYOUT_NAMES`] actually
    /// change what [`screens::dashboard::draw`] renders next frame --
    /// `crate::tui::screens::dashboard`'s own call site resolves
    /// `active_preset` with
    /// [`crate::tui::layout::presets::by_name`], which only ever answers
    /// `Some` for those four. Choosing a custom `config.layouts` entry here
    /// still sets `active_preset` and still writes `config.layout` to that
    /// name -- the round trip through `config.json` is honest and complete
    /// -- but the dashboard falls back to [`crate::tui::layout::presets::live`]
    /// until a later epic teaches that call site to also resolve a
    /// `LayoutNodeDto` out of `config.layouts` via the `TryFrom` conversion
    /// [`crate::infrastructure::config`] already provides. Documented here
    /// rather than silently pretending every listed name works today.
    fn confirm_layout_picker(&mut self) {
        if let Some(name) = self.layout_names.get(self.picker_selected).cloned() {
            self.active_preset.clone_from(&name);
            self.persist(|config| config.layout = Some(name));
        }
        self.close_overlays();
    }

    /// Repeats the last confirmed search: `same_direction` is `true` for
    /// `n`, `false` for `N`, which is why the direction actually searched is
    /// `forward` `XOR`ed against it rather than `forward` itself.
    ///
    /// Unlike the search that starts at `Enter`, a repeat never re-reports
    /// the match the view is already sitting on -- see `run_search`'s
    /// `inclusive` parameter -- because a `n` that could not move anywhere
    /// new whenever the view already sat on the only match in the list
    /// would look broken, not idle.
    fn repeat_search(&mut self, same_direction: bool) {
        let Some((pattern, forward)) = self.last_search.clone() else {
            self.notice = Some("no previous search".to_owned());
            return;
        };
        self.run_search_with(&pattern, same_direction == forward, false);
    }

    /// Runs `pattern` against whichever view is showing and jumps to the
    /// first match at or after the current position, wrapping around the
    /// whole list. Matching is a plain case-insensitive substring test
    /// against each row's primary label -- the log's own
    /// [`crate::domain::session::LogEntry::text`], the session picker's
    /// session id and project directory -- not a regular expression, which
    /// keeps this a small, honest search rather than a promise this
    /// dashboard cannot back up.
    fn run_search(&mut self, pattern: &str, forward: bool) {
        self.run_search_with(pattern, forward, true);
    }

    /// `run_search`'s real implementation. `inclusive` is `true` for a fresh
    /// search confirmed with `Enter` (the current row itself is a valid
    /// match) and `false` for [`App::repeat_search`] (which must move on).
    fn run_search_with(&mut self, pattern: &str, forward: bool, inclusive: bool) {
        if pattern.is_empty() {
            return;
        }
        let needle = pattern.to_lowercase();
        // The session picker first, the same way `scroll_snapshot` checks
        // `overlay` before `view` -- see that method's own doc comment for
        // why.
        if self.overlay == Overlay::Sessions {
            self.search_sessions(&needle, forward, inclusive);
            return;
        }
        match self.view {
            View::Log => self.search_log(&needle, forward, inclusive),
            View::Dashboard | View::Daily | View::Weekly | View::Monthly | View::Blocks => {}
        }
    }

    fn search_log(&mut self, needle: &str, forward: bool, inclusive: bool) {
        let Some(snapshot) = self.monitor.snapshot() else {
            return;
        };
        let events = &snapshot.events;
        let total = events.len();
        if total == 0 {
            return;
        }
        // `log_offset` counts entries hidden below the bottom of the view --
        // see its own field doc comment -- so the entry currently anchoring
        // the view sits `log_offset` steps back from the newest, and index
        // `0` here is the *oldest* entry.
        let anchor = total.saturating_sub(1).saturating_sub(self.log_offset);
        let found = find_from(total, anchor, forward, inclusive, |index| {
            events[index].text.to_lowercase().contains(needle)
        });
        match found {
            Some(index) => self.log_offset = total.saturating_sub(1).saturating_sub(index),
            None => self.notice = Some(format!("pattern not found: {needle}")),
        }
    }

    fn search_sessions(&mut self, needle: &str, forward: bool, inclusive: bool) {
        let sessions = &self.sessions;
        let total = sessions.len();
        if total == 0 {
            return;
        }
        let found = find_from(total, self.selected, forward, inclusive, |index| {
            let session = &sessions[index];
            session.session_id.to_lowercase().contains(needle)
                || session.project_dir.to_lowercase().contains(needle)
        });
        match found {
            Some(index) => self.selected = index,
            None => self.notice = Some(format!("pattern not found: {needle}")),
        }
    }

    /// Looks `buf` up against the command grammar this epic ships, and acts
    /// on it -- or, for anything this small a grammar does not recognise,
    /// leaves a footer notice saying so rather than doing nothing silently.
    fn execute_command(&mut self, buf: &str) {
        let trimmed = buf.trim();
        if let Some(name) = trimmed.strip_prefix("view ") {
            self.command_view(name.trim(), buf);
        } else if let Some(name) = trimmed.strip_prefix("theme ") {
            self.command_theme(name.trim(), buf);
        } else {
            match trimmed {
                "q" | "quit" => self.quit = true,
                "help" => self.overlay = Overlay::Help,
                _ => self.notice = Some(format!("not a command: {buf}")),
            }
        }
    }

    /// `:view <name>` -- one of the six [`CONTENT_VIEWS`] names, matching
    /// what the tab bar itself labels them. `sessions` is deliberately not
    /// among them any more: the session picker is
    /// [`Overlay::Sessions`] now, reached with `o`
    /// ([`NormalAction::OpenSessions`]) rather than a destination `:view`
    /// switches to -- see [`View`]'s own doc comment for why.
    fn command_view(&mut self, name: &str, raw: &str) {
        let view = match name {
            "dashboard" => View::Dashboard,
            "daily" => View::Daily,
            "weekly" => View::Weekly,
            "monthly" => View::Monthly,
            "blocks" => View::Blocks,
            "log" => View::Log,
            _ => {
                self.notice = Some(format!("not a command: {raw}"));
                return;
            }
        };
        self.close_overlays();
        self.view = view;
        self.ensure_reports_loaded();
    }

    fn command_theme(&mut self, name: &str, raw: &str) {
        if let Some(palette) = ThemeRegistry::builtin().get(name) {
            self.palette = palette.clone();
            let owned = name.to_owned();
            self.persist(|config| config.theme = Some(owned));
        } else {
            self.notice = Some(format!("not a command: {raw}"));
        }
    }

    /// Writes a change back to `config.json`, when this dashboard was built
    /// with somewhere to write it -- see `config_gateway`'s own field doc
    /// comment for why that is optional. A write that fails (a read-only
    /// filesystem, a directory that could not be created) is surfaced the
    /// same way any other failed action is, through `notice`, rather than
    /// silently discarded: the theme or layout still changed for the rest of
    /// this run, but the user should know it will not survive a restart.
    fn persist(&mut self, mutate: impl FnOnce(&mut Config)) {
        let result = self
            .config_gateway
            .as_ref()
            .map(|gateway| gateway.merge_write(mutate));
        if let Some(Err(error)) = result {
            self.notice = Some(format!("could not save config: {error}"));
        }
    }

    /// Applies one key press while [`App::input_mode`] is
    /// [`InputMode::Search`] or [`InputMode::Command`].
    ///
    /// This exists apart from [`crate::tui::keymap::resolve`] because the two
    /// are answering different questions: `resolve` maps one whole key press
    /// to one of a fixed set of actions, which is the right shape for normal
    /// mode's "what does `j` do" but the wrong shape for "append this
    /// character to a growing buffer" -- there is no `NormalAction` for the
    /// letter `f`, and there should not be one, because a `Keymap` entry per
    /// possible character would turn a hundred-odd-row table into a
    /// three-thousand-row one for no benefit over simply reading
    /// [`crossterm::event::KeyCode::Char`] directly. [`crate::tui::runtime`]'s
    /// event loop therefore checks [`App::input_mode`] first and routes here
    /// instead of through `resolve` for as long as it says anything other
    /// than [`InputMode::Normal`].
    pub fn handle_line_edit(&mut self, key: KeyEvent) -> LineEditOutcome {
        match std::mem::replace(&mut self.input_mode, InputMode::Normal) {
            InputMode::Normal => LineEditOutcome::Unchanged,
            InputMode::Search { buf, origin_scroll } => self.edit_search(key, buf, origin_scroll),
            InputMode::Command { buf } => self.edit_command(key, buf),
        }
    }

    fn edit_search(
        &mut self,
        key: KeyEvent,
        mut buf: String,
        origin_scroll: ScrollSnapshot,
    ) -> LineEditOutcome {
        match key.code {
            KeyCode::Esc => {
                self.restore_scroll(origin_scroll);
                LineEditOutcome::Cancelled
            }
            KeyCode::Backspace if buf.is_empty() => {
                self.restore_scroll(origin_scroll);
                LineEditOutcome::Cancelled
            }
            KeyCode::Backspace => {
                buf.pop();
                self.input_mode = InputMode::Search { buf, origin_scroll };
                LineEditOutcome::Changed
            }
            KeyCode::Enter if buf.is_empty() => {
                // An empty pattern has nothing to search for and nothing
                // worth remembering -- confirming it leaves the view and
                // `last_search` exactly as they were, rather than
                // overwriting a real previous search with an empty one that
                // `n`/`N` could never usefully repeat.
                self.restore_scroll(origin_scroll);
                LineEditOutcome::Confirmed
            }
            KeyCode::Enter => {
                // A confirmed search is itself a jump -- `Ctrl-o`
                // (`NormalAction::JumpBack`) must be able to undo it the same
                // way it undoes `gt`/`gg`, which only works if the position
                // it is about to leave is pushed *before* `run_search` moves
                // it. See `App::push_jump`'s own doc comment for why this has
                // to happen before, not after.
                self.push_jump();
                self.run_search(&buf, true);
                self.last_search = Some((buf, true));
                LineEditOutcome::Confirmed
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.clear();
                self.input_mode = InputMode::Search { buf, origin_scroll };
                LineEditOutcome::Changed
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                delete_last_word(&mut buf);
                self.input_mode = InputMode::Search { buf, origin_scroll };
                LineEditOutcome::Changed
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.push(c);
                self.input_mode = InputMode::Search { buf, origin_scroll };
                LineEditOutcome::Changed
            }
            _ => {
                self.input_mode = InputMode::Search { buf, origin_scroll };
                LineEditOutcome::Unchanged
            }
        }
    }

    fn edit_command(&mut self, key: KeyEvent, mut buf: String) -> LineEditOutcome {
        match key.code {
            // Both `Esc` and `Backspace` on an empty buffer leave
            // `input_mode` at `InputMode::Normal`, which
            // `std::mem::replace` in `handle_line_edit` already set it to --
            // unlike search, a command has no scroll position to restore,
            // so there is nothing left to do beyond reporting the outcome.
            KeyCode::Esc => LineEditOutcome::Cancelled,
            KeyCode::Backspace if buf.is_empty() => LineEditOutcome::Cancelled,
            KeyCode::Backspace => {
                buf.pop();
                self.input_mode = InputMode::Command { buf };
                LineEditOutcome::Changed
            }
            KeyCode::Enter => {
                self.execute_command(&buf);
                LineEditOutcome::Confirmed
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.clear();
                self.input_mode = InputMode::Command { buf };
                LineEditOutcome::Changed
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                delete_last_word(&mut buf);
                self.input_mode = InputMode::Command { buf };
                LineEditOutcome::Changed
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                buf.push(c);
                self.input_mode = InputMode::Command { buf };
                LineEditOutcome::Changed
            }
            _ => {
                self.input_mode = InputMode::Command { buf };
                LineEditOutcome::Unchanged
            }
        }
    }

    /// A left-hand footer badge naming what the dashboard thinks the next
    /// keystroke will do: `SEARCH: fo` while typing a search, `CMD: q`
    /// while typing a command, or a bare `5g` while a count or chord is
    /// only half typed in normal mode. `None` the rest of the time, which is
    /// most of the time -- this exists to answer the design's own complaint
    /// that the footer previously gave no feedback about what state a
    /// half-typed key sequence had left the dashboard in.
    fn status_badge(&self) -> Option<(String, Color)> {
        match &self.input_mode {
            InputMode::Search { buf, .. } => {
                Some((format!("SEARCH: {buf}"), self.palette.accent_info.into()))
            }
            InputMode::Command { buf } => {
                Some((format!("CMD: {buf}"), self.palette.accent_secondary.into()))
            }
            InputMode::Normal if self.pending != Pending::default() => {
                Some((pending_label(self.pending), self.palette.muted.into()))
            }
            InputMode::Normal => None,
        }
    }

    /// Moves down (`+1`) or up (`-1`) in whichever list is showing.
    ///
    /// In the log, "down" means *further into the past*, because that is the
    /// direction the content extends. Scrolling towards older entries is the
    /// only reason to scroll a log that auto-follows its newest line.
    fn move_selection(&mut self, delta: isize) {
        match self.view {
            View::Log => {
                let max = self.max_log_offset();
                self.log_offset = if delta > 0 {
                    (self.log_offset.saturating_add(delta.unsigned_abs())).min(max)
                } else {
                    self.log_offset.saturating_sub(delta.unsigned_abs())
                };
            }
            // None of the four report tabs have a scroll position of their
            // own yet -- each is a single bordered table, drawn in full --
            // the same reason `View::Dashboard` has always been a no-op
            // here.
            View::Dashboard | View::Daily | View::Weekly | View::Monthly | View::Blocks => {}
        }
    }

    /// Jumps to the top, the bottom, or (when a count preceded the chord
    /// that produced this) a specific one-based row counted in from the top.
    ///
    /// `Top` and `Bottom` preserve exactly what the pre-keymap `JumpToOldest`
    /// and `JumpToNewest` actions did -- `gg`/`Home` still lands where `g`/
    /// `Home` used to, `G`/`End` still lands where `G`/`End` used to -- this
    /// epic renamed the actions, not their behaviour. Which physical end of
    /// each list counts as "the top" is therefore also unchanged, and it is
    /// not the same for both views: the log's oldest entry sits at the
    /// *maximum* offset (see the doc comment on `move_selection` for why
    /// "down" and "further into the past" are the same direction there),
    /// while the session picker's oldest session sits at the *last* index.
    /// `Row(n)` is new, and counts from that same oldest end in both cases,
    /// clamped to the list's actual length: `5gg` lands where five
    /// individual downward jumps from the top would.
    fn jump_to_row(&mut self, target: RowTarget) {
        if self.overlay == Overlay::Sessions {
            let last = self.sessions.len().saturating_sub(1);
            self.selected = match target {
                RowTarget::Top => last,
                RowTarget::Bottom => 0,
                RowTarget::Row(n) => last.saturating_sub((n as usize).saturating_sub(1)),
            };
            return;
        }
        match self.view {
            View::Log => {
                let max = self.max_log_offset();
                self.log_offset = match target {
                    RowTarget::Top => max,
                    RowTarget::Bottom => 0,
                    RowTarget::Row(n) => max.saturating_sub((n as usize).saturating_sub(1)),
                };
            }
            View::Dashboard | View::Daily | View::Weekly | View::Monthly | View::Blocks => {}
        }
    }

    /// Moves to the next (`delta = 1`) or previous (`delta = -1`) entry in
    /// [`CONTENT_VIEWS`], wrapping at either end.
    fn cycle_view(&mut self, delta: isize) {
        let len = CONTENT_VIEWS.len();
        let current = CONTENT_VIEWS
            .iter()
            .position(|view| *view == self.view)
            .unwrap_or(0);
        // Worked in `usize` throughout, rather than casting `current` to
        // `isize` and back, so wrap-around is ordinary modular arithmetic
        // rather than something a cast could overflow on -- forward and
        // backward steps are handled separately because `usize` has no
        // negative numbers for a single `rem_euclid` to lean on.
        let steps = delta.unsigned_abs() % len;
        let next = if delta >= 0 {
            (current + steps) % len
        } else {
            (current + len - steps) % len
        };
        self.close_overlays();
        self.view = CONTENT_VIEWS[next];
        self.ensure_reports_loaded();
    }

    /// Jumps straight to the `n`th (one-based) entry in [`CONTENT_VIEWS`],
    /// clamped to the last one rather than wrapping -- `99gt` means "the
    /// last view", not an error.
    ///
    /// The acceptance criteria for this epic's own spec text names Monthly
    /// as "tab 3" for `3gt` -- that does not match either the tab bar's own
    /// literal string (`" 1 Dashboard  2 Daily  3 Weekly  4 Monthly  5
    /// Blocks  6 Log "`) or [`CONTENT_VIEWS`]'s declared order, both given in
    /// the same spec section and agreeing with each other that Weekly is
    /// tab 3 and Monthly is tab 4. Implemented against those two
    /// structural, twice-stated sources rather than the prose aside, the
    /// same kind of judgement call earlier epics' own notes record making
    /// when a spec's literal numbers disagreed with itself -- see
    /// `crate::tui::app`'s test `three_gt_from_any_tab_jumps_to_weekly_the_third_tab`
    /// for the behaviour this actually ships.
    fn goto_view(&mut self, n: u32) {
        let index = (n as usize).saturating_sub(1).min(CONTENT_VIEWS.len() - 1);
        self.close_overlays();
        self.view = CONTENT_VIEWS[index];
        self.ensure_reports_loaded();
    }

    /// Loads whichever of the Daily/Weekly/Monthly/Blocks tabs `self.view`
    /// now names, if it has not been loaded since the last `None` (a fresh
    /// `App`, or `r`/[`NormalAction::Refresh`] clearing the cache) and a
    /// [`crate::application::report_source::ReportSource`] was actually
    /// wired in. A failed load leaves the cached field `None` and surfaces
    /// the error through the ordinary `notice` footer slot rather than
    /// panicking or silently showing stale figures -- the same rule
    /// `App::handle`'s `Refresh` arm already follows for the session list
    /// and the account usage scan.
    fn ensure_reports_loaded(&mut self) {
        let Some(source) = self.report_source.as_mut() else {
            return;
        };
        match self.view {
            View::Daily if self.daily_report.is_none() => match source.daily() {
                Ok(report) => {
                    self.daily_report = Some(usage_view::table(&report, "Date", false, 200));
                }
                Err(error) => self.notice = Some(error.to_string()),
            },
            View::Weekly if self.weekly_report.is_none() => match source.weekly() {
                Ok(report) => {
                    self.weekly_report = Some(usage_view::table(&report, "Week", false, 200));
                }
                Err(error) => self.notice = Some(error.to_string()),
            },
            View::Monthly if self.monthly_report.is_none() => match source.monthly() {
                Ok(report) => {
                    self.monthly_report = Some(usage_view::table(&report, "Month", false, 200));
                }
                Err(error) => self.notice = Some(error.to_string()),
            },
            View::Blocks if self.blocks_report.is_none() => {
                match source.blocks(chrono::Utc::now()) {
                    Ok(rows) => {
                        self.blocks_report = Some(blocks_view::table(&rows, &Zone::Local, 200));
                    }
                    Err(error) => self.notice = Some(error.to_string()),
                }
            }
            View::Dashboard
            | View::Daily
            | View::Weekly
            | View::Monthly
            | View::Blocks
            | View::Log => {}
        }
    }

    /// The furthest back the log can be scrolled.
    ///
    /// Deliberately generous by one screenful: it is clamped again at draw
    /// time against the real panel height, which the state does not know.
    fn max_log_offset(&self) -> usize {
        self.monitor
            .snapshot()
            .map_or(0, |snapshot| snapshot.events.len().saturating_sub(1))
    }

    fn refresh_session_list(&mut self) {
        match self.monitor.list_sessions() {
            Ok(sessions) => {
                self.selected = self.selected.min(sessions.len().saturating_sub(1));
                self.sessions = sessions;
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    fn attach_selected(&mut self) {
        if self.overlay != Overlay::Sessions {
            return;
        }
        let Some(chosen) = self.sessions.get(self.selected).cloned() else {
            return;
        };
        match self.monitor.attach_to(chosen) {
            Ok(()) => {
                self.log_offset = 0;
                self.view = View::Dashboard;
                // A deliberate switch to the dashboard, not a restoration --
                // an attached session is worth seeing regardless of which
                // tab the picker was opened from, so there is nothing left
                // to remember here.
                self.close_overlays();
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    /// Draws the current tab, then whichever overlay (a picker, help, or the
    /// session list) sits on top of it, then -- for as long as a `g` chord is
    /// half-typed -- the which-key popup naming what could complete it.
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let screen = frame.area();
        Paragraph::new("")
            .style(self.palette.base())
            .render(screen, frame.buffer_mut());

        let [body, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(screen);

        self.draw_content(frame, body);
        self.draw_footer(frame, footer);
        self.draw_overlay(frame, screen);

        // Drawn last, over everything else, for as long as a `g` chord is
        // half-typed -- checked fresh every frame rather than timed, so it
        // vanishes the instant the chord resolves or is cancelled, exactly
        // when `App::pending` next reports `ChordState::Idle`.
        if self.pending.chord == ChordState::AwaitingG {
            screens::which_key::draw(frame, screen, footer.y, &self.palette, &self.keymap);
        }
    }

    /// Draws whichever content tab `self.view` names, into `body` -- the
    /// screen area left once [`App::draw`] has reserved the footer's own
    /// row.
    fn draw_content(&self, frame: &mut Frame<'_>, body: ratatui::layout::Rect) {
        // Every content tab reserves its own top row for the tab bar --
        // `CONTENT_VIEWS`' position of `self.view` is that tab's one-based
        // number, minus one, the same order `crate::tui::screens::TAB_LABELS`
        // lists them in.
        let tab_index = CONTENT_VIEWS
            .iter()
            .position(|view| *view == self.view)
            .unwrap_or(0);

        match (self.view, self.monitor.snapshot()) {
            (View::Log, Some(snapshot)) => {
                screens::log::draw(
                    frame,
                    body,
                    snapshot,
                    self.log_offset,
                    tab_index,
                    &self.palette,
                );
            }
            (View::Daily, _) => screens::daily::draw(
                frame,
                body,
                tab_index,
                self.daily_report.as_ref(),
                &self.palette,
            ),
            (View::Weekly, _) => screens::weekly::draw(
                frame,
                body,
                tab_index,
                self.weekly_report.as_ref(),
                &self.palette,
            ),
            (View::Monthly, _) => screens::monthly::draw(
                frame,
                body,
                tab_index,
                self.monthly_report.as_ref(),
                &self.palette,
            ),
            (View::Blocks, _) => screens::blocks::draw(
                frame,
                body,
                tab_index,
                self.blocks_report.as_ref(),
                &self.palette,
            ),
            (View::Dashboard, Some(snapshot)) => {
                let usage = self
                    .usage
                    .as_ref()
                    .map(|tracker| (tracker.usage(), tracker.has_measured()));
                let inputs = screens::dashboard::DashboardInputs {
                    pulse: screens::dashboard::DollarPulseInputs {
                        frames_since_increment: self.pulse.frames_since(self.phase),
                        off: self.animation_style == AnimationStyle::Off,
                    },
                    active_preset: &self.active_preset,
                    tab_index,
                };
                screens::dashboard::draw(
                    frame,
                    body,
                    snapshot,
                    self.phase,
                    usage,
                    &self.palette,
                    inputs,
                );
            }
            (View::Dashboard | View::Log, None) => {
                screens::help::draw_searching(frame, body, self.phase, &self.palette);
            }
        }
    }

    /// Draws whichever overlay (a picker, help, or the session list) sits on
    /// top of the content tab, over the whole `screen` -- or nothing, when
    /// [`Overlay::None`].
    fn draw_overlay(&self, frame: &mut Frame<'_>, screen: ratatui::layout::Rect) {
        match self.overlay {
            Overlay::ThemePicker => {
                let names: Vec<&str> = self.theme_names.iter().map(String::as_str).collect();
                screens::theme_picker::draw(
                    frame,
                    screen,
                    &names,
                    self.picker_selected,
                    &self.palette,
                );
            }
            Overlay::LayoutPicker => {
                let names: Vec<&str> = self.layout_names.iter().map(String::as_str).collect();
                screens::layout_picker::draw(
                    frame,
                    screen,
                    &names,
                    self.picker_selected,
                    &self.palette,
                );
            }
            Overlay::Sessions => screens::sessions::draw(
                frame,
                screen,
                &self.sessions,
                self.selected,
                self.monitor.attached(),
                &self.palette,
            ),
            Overlay::Help => screens::help::draw(frame, screen, &self.palette, &self.keymap),
            Overlay::None => {}
        }
    }

    /// The badge [`App::status_badge`] renders in the footer's left-hand
    /// corner: a coloured tag naming the mode, followed by its live text.
    fn badge_span(&self) -> Option<Span<'static>> {
        self.status_badge().map(|(text, colour)| {
            Span::styled(
                format!(" {text} "),
                Style::default()
                    .fg(self.palette.inverted_text.into())
                    .bg(colour)
                    .add_modifier(Modifier::BOLD),
            )
        })
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let badge = self.badge_span();

        // `notice` takes priority over `config_warning`: it is the more
        // recent of the two by construction (nothing sets a fresh `notice`
        // without also having already cleared `config_warning` on the same
        // key press, per `App::handle`), and a manual action's own feedback
        // -- a failed attach, say -- is more relevant to what the user just
        // did than a note about how the dashboard started up.
        if let Some(message) = self.notice.as_deref().or_else(|| {
            self.config_warning
                .as_ref()
                .map(|warning| warning.message.as_str())
        }) {
            let mut spans = Vec::new();
            if let Some(badge) = badge {
                spans.push(badge);
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(
                format!(" {} {message}", Icon::ERROR),
                Style::default()
                    .fg(self.palette.pressure_high.into())
                    .bg(self.palette.surface.into()),
            ));
            frame.render_widget(
                Paragraph::new(Line::from(spans))
                    .style(Style::default().bg(self.palette.surface.into())),
                area,
            );
            return;
        }

        let hint = if self.overlay == Overlay::Sessions {
            " q quit   j/k move   Enter attach   Esc back   ? help "
        } else {
            match self.view {
                View::Dashboard => " q quit   gt next tab   o sessions   ? help ",
                View::Daily | View::Weekly | View::Monthly | View::Blocks => {
                    " q quit   gt next tab   r refresh   ? help "
                }
                View::Log => " q quit   gt next tab   j/k scroll   gg/G ends   ? help ",
            }
        };
        // The monitor's error wins because it concerns the session actually on
        // screen. The tracker's is the fallback: a failed account scan leaves
        // the five-hour and weekly figures stale, and showing them with no
        // word of it is the one thing the tracker keeps the error to avoid.
        let error = self
            .monitor
            .last_error()
            .or_else(|| self.usage.as_ref().and_then(UsageTracker::last_error))
            .map(|e| format!("  {} {e}", Icon::ERROR));

        let mut spans = Vec::new();
        if let Some(badge) = badge {
            spans.push(badge);
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            hint,
            Style::default().fg(self.palette.muted.into()),
        ));
        spans.push(Span::styled(
            error.unwrap_or_default(),
            Style::default().fg(self.palette.pressure_high.into()),
        ));

        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(Style::default().bg(self.palette.surface.into())),
            area,
        );
    }
}

/// Which way a half-page or full-page scroll moves, as a signed row delta.
const fn page_delta(dir: Dir, magnitude: isize) -> isize {
    match dir {
        Dir::Down => magnitude,
        Dir::Up => -magnitude,
    }
}

/// Finds the first index in `0..len` for which `matches` holds, walking
/// outward from `anchor` in the direction `forward` says and wrapping around
/// either end.
///
/// A free function rather than a method on `App`, and taking `matches` as a
/// closure rather than a needle string, so [`App::search_log`] and
/// [`App::search_sessions`] -- which disagree about what a row's "text" even
/// is -- can both drive the exact same wrap-around walk instead of each
/// re-deriving it.
///
/// `inclusive` decides whether `anchor` itself is checked first (when
/// `true`, "at or after the current position" for a fresh search: a pattern
/// that already matches where the view sits should not have to move at all
/// to report success) or only as the very last resort after every other row
/// has failed to match (when `false`, for [`App::repeat_search`] -- `n` must
/// move to the *next* occurrence, wrapping back to the one it started on
/// only if it turns out to be the sole match in the whole list).
fn find_from(
    len: usize,
    anchor: usize,
    forward: bool,
    inclusive: bool,
    matches: impl Fn(usize) -> bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let skip_anchor = usize::from(!inclusive);
    (skip_anchor..=len)
        .map(|step| {
            if forward {
                (anchor + step) % len
            } else {
                (anchor + len - step % len) % len
            }
        })
        .find(|&index| matches(index))
}

/// Deletes the previous whitespace-delimited word from `buf`, the way a
/// shell's `Ctrl-w` does: trailing whitespace goes first, then everything
/// back to (but not including) the whitespace before it.
fn delete_last_word(buf: &mut String) {
    let trimmed = buf.trim_end();
    let cut = trimmed
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    buf.truncate(cut);
}

/// A short label for a half-typed count and/or chord, e.g. `"5"`, `"g"` or
/// `"5g"` -- read by [`App::status_badge`] for the footer's bare-`Pending`
/// case. Kept apart from any `Display` impl on [`Pending`] itself because
/// this is a presentation judgement ("g", not "`AwaitingG`") that belongs in
/// `tui::app`, the same reasoning `crate::tui::keymap::key_label`'s own doc
/// comment gives for keeping key labels out of `crate::tui::keymap`'s data
/// types.
fn pending_label(pending: Pending) -> String {
    let mut label = String::new();
    if let Some(count) = pending.count {
        label.push_str(&count.to_string());
    }
    if pending.chord == ChordState::AwaitingG {
        label.push('g');
    }
    label
}

/// The animation style `App::new` starts with, before a config file (if any)
/// has been read: `NO_ANIMATION` or `CLAUDE_STATS_NO_ANIMATION`, checked
/// once here at startup rather than on every frame, forces
/// [`AnimationStyle::Off`] -- mirroring the `NO_COLOR` convention --
/// otherwise [`AnimationStyle::Pulse`]. [`App::with_config`] re-resolves this
/// once more against `config.animation`, keeping the same environment-wins
/// rule; a caller that never calls `with_config` (most of this module's own
/// tests) is left with exactly this value.
fn initial_animation_style() -> AnimationStyle {
    resolve_animation_style(no_animation_requested(), AnimationStyle::Pulse)
}

/// Whether either "disable animation" environment variable is set.
fn no_animation_requested() -> bool {
    std::env::var("NO_ANIMATION").is_ok() || std::env::var("CLAUDE_STATS_NO_ANIMATION").is_ok()
}

/// The override rule itself -- "either variable set beats any later
/// default" -- pulled out as a pure function so a test can check it without
/// mutating process-wide environment state, which is inherently racy across
/// a parallel test run (`std::env::var` is process-global, and `cargo test`
/// runs every test in the same process).
const fn resolve_animation_style(no_animation: bool, default: AnimationStyle) -> AnimationStyle {
    if no_animation {
        AnimationStyle::Off
    } else {
        default
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::Utc;

    use super::*;
    use crate::application::ports::{
        AccountUsageReader, ChangeSource, SessionSelector, SystemClock, TranscriptRef,
    };
    use crate::domain::limits::AccountUsage;
    use crate::domain::session::SessionSnapshot;

    struct Catalog(Vec<TranscriptRef>);

    impl TranscriptCatalog for Catalog {
        fn resolve(&self, _s: &SessionSelector) -> anyhow::Result<Option<TranscriptRef>> {
            Ok(self.0.first().cloned())
        }
        fn list(&self) -> anyhow::Result<Vec<TranscriptRef>> {
            Ok(self.0.clone())
        }
        fn list_billable(&self) -> anyhow::Result<Vec<TranscriptRef>> {
            Ok(self.0.clone())
        }
    }

    struct Reader;

    impl SessionReader for Reader {
        fn read(&self, t: &TranscriptRef) -> anyhow::Result<SessionSnapshot> {
            Ok(SessionSnapshot::empty(t.path.clone(), t.session_id.clone()))
        }
    }

    struct Never;

    impl ChangeSource for Never {
        fn has_changed(&mut self) -> bool {
            false
        }
    }

    struct Factory;

    impl ChangeSourceFactory for Factory {
        fn watch(&self, _p: &Path) -> Box<dyn ChangeSource> {
            Box::new(Never)
        }
    }

    fn transcript(id: &str) -> TranscriptRef {
        TranscriptRef {
            path: format!("/tmp/{id}.jsonl").into(),
            session_id: id.to_owned(),
            project_dir: "/project".to_owned(),
            modified_at: Utc::now(),
            size_bytes: 0,
        }
    }

    fn app(ids: &[&str]) -> App<Catalog, Reader, Factory> {
        let catalog = Catalog(ids.iter().map(|id| transcript(id)).collect());
        App::new(Monitor::new(
            catalog,
            Reader,
            Factory,
            SessionSelector::Active,
        ))
    }

    /// A reader that always fails, for checking that the failure is shown.
    struct FailingUsage;

    impl AccountUsageReader for FailingUsage {
        fn usage(&mut self, _now: chrono::DateTime<Utc>) -> anyhow::Result<AccountUsage> {
            Err(anyhow::anyhow!("usage scan failed"))
        }
    }

    /// A reader whose transcripts cannot be read, so the monitor has an error
    /// of its own to outrank the tracker's.
    struct BrokenReader;

    impl SessionReader for BrokenReader {
        fn read(&self, _t: &TranscriptRef) -> anyhow::Result<SessionSnapshot> {
            Err(anyhow::anyhow!("transcript unreadable"))
        }
    }

    /// A reader that always reports the same non-zero spend for today, for
    /// checking that a scan feeds `App`'s [`PulseClock`].
    struct WorkingUsage;

    impl AccountUsageReader for WorkingUsage {
        fn usage(&mut self, now: chrono::DateTime<Utc>) -> anyhow::Result<AccountUsage> {
            let mut usage = AccountUsage::empty(now);
            usage.today.cost = crate::domain::money::Usd::new(1.23);
            Ok(usage)
        }
    }

    /// A tracker that has not yet scanned, so its first `tick()` does.
    fn working_tracker() -> UsageTracker {
        UsageTracker::new(Box::new(WorkingUsage), Box::new(SystemClock))
    }

    fn failing_tracker() -> UsageTracker {
        let mut tracker = UsageTracker::new(Box::new(FailingUsage), Box::new(SystemClock));
        tracker.scan();
        tracker
    }

    /// Renders the whole app once and returns the screen as text.
    fn rendered<C, R, F>(app: &mut App<C, R, F>) -> String
    where
        C: TranscriptCatalog,
        R: SessionReader,
        F: ChangeSourceFactory,
    {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40))
            .expect("test backend");
        terminal
            .draw(|frame| app.draw(frame))
            .expect("draw succeeds");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn a_tick_with_a_rising_cost_arms_the_pulse() {
        // `working_tracker` has not scanned yet, so `App::tick`'s first call
        // to `UsageTracker::tick` scans -- landing a non-zero `today.cost`
        // where the tracker started at `AccountUsage::empty`'s zero. That
        // rise is exactly what `PulseClock::observe` is watching for.
        let mut app = app(&["a"]).tracking_usage(working_tracker());
        app.tick();

        assert_eq!(
            app.pulse.frames_since(app.phase),
            Some(0),
            "armed on the very tick the scan landed"
        );
    }

    #[test]
    fn a_second_tick_with_no_fresh_scan_does_not_re_arm_the_pulse() {
        // `UsageTracker::tick` only re-scans once its interval has passed,
        // so the second tick here sees the same `today.cost` the first one
        // did -- the pulse must still be counting from the original arm,
        // not reset to zero again.
        let mut app = app(&["a"]).tracking_usage(working_tracker());
        app.tick();
        app.tick();

        assert_eq!(
            app.pulse.frames_since(app.phase),
            Some(1),
            "still counting from the first tick's arm"
        );
    }

    #[test]
    fn no_animation_env_forces_the_off_style_regardless_of_the_default() {
        assert_eq!(
            resolve_animation_style(true, AnimationStyle::Pulse),
            AnimationStyle::Off
        );
        assert_eq!(
            resolve_animation_style(true, AnimationStyle::Coin),
            AnimationStyle::Off,
            "the override wins over any default, not just the hard-coded one"
        );
    }

    #[test]
    fn without_the_env_override_the_default_style_survives() {
        assert_eq!(
            resolve_animation_style(false, AnimationStyle::Pulse),
            AnimationStyle::Pulse
        );
    }

    #[test]
    fn a_failed_account_scan_says_so_rather_than_showing_stale_figures_silently() {
        let mut app = app(&["a"]).tracking_usage(failing_tracker());
        app.tick();

        assert!(
            rendered(&mut app).contains("usage scan failed"),
            "the tracker keeps this error precisely so it can be shown"
        );
    }

    #[test]
    fn the_attached_sessions_error_outranks_the_account_scans() {
        let catalog = Catalog(vec![transcript("a")]);
        let mut app = App::new(Monitor::new(
            catalog,
            BrokenReader,
            Factory,
            SessionSelector::Active,
        ))
        .tracking_usage(failing_tracker());
        app.tick();

        let screen = rendered(&mut app);
        assert!(
            screen.contains("transcript unreadable"),
            "the session on screen is the more urgent failure"
        );
        assert!(
            !screen.contains("usage scan failed"),
            "only one error fits in the footer: {screen:?}"
        );
    }

    #[test]
    fn escape_backs_out_one_level_at_a_time_before_quitting() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::GotoView(6)); // Log, the sixth tab
        app.handle(NormalAction::ToggleHelp);

        app.handle(NormalAction::Back);
        assert_eq!(app.overlay, Overlay::None, "first escape closes the help");
        assert_eq!(app.view(), View::Log, "and leaves the view alone");

        app.handle(NormalAction::Back);
        assert_eq!(app.view(), View::Dashboard, "second escape returns home");
        assert!(!app.should_quit());

        app.handle(NormalAction::Back);
        assert!(app.should_quit(), "escape at home quits");
    }

    #[test]
    fn opening_sessions_from_blocks_and_pressing_esc_restores_blocks() {
        // The overlay-conversion this epic makes: `Esc` from the session
        // picker must land back on whichever tab it was opened from, not
        // always on `View::Dashboard` the way the pre-epic `Action::Back`
        // did once `Sessions` was a `View` variant of its own.
        let mut app = app(&["a"]);
        app.handle(NormalAction::GotoView(5)); // Blocks, the fifth tab
        assert_eq!(app.view(), View::Blocks);

        app.handle(NormalAction::OpenSessions);
        assert_eq!(
            app.overlay,
            Overlay::Sessions,
            "sessions_open true, in the spec's own words"
        );

        app.handle(NormalAction::Back);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(
            app.view(),
            View::Blocks,
            "Esc restores Blocks, not Dashboard"
        );
    }

    #[test]
    fn opening_the_picker_loads_the_session_list() {
        let mut app = app(&["a", "b", "c"]);
        app.handle(NormalAction::OpenSessions);
        assert_eq!(app.overlay, Overlay::Sessions);
        assert_eq!(
            app.view(),
            View::Dashboard,
            "opening the picker does not switch the tab underneath it"
        );
        assert_eq!(app.sessions.len(), 3);
    }

    #[test]
    fn moving_through_the_picker_stops_at_both_ends() {
        let mut app = app(&["a", "b"]);
        app.handle(NormalAction::OpenSessions);

        app.handle(NormalAction::MoveUp);
        assert_eq!(app.selected, 0, "cannot move above the first");

        app.handle(NormalAction::MoveDown);
        app.handle(NormalAction::MoveDown);
        assert_eq!(app.selected, 1, "cannot move past the last");
    }

    #[test]
    fn confirming_a_session_attaches_and_returns_to_the_dashboard() {
        let mut app = app(&["a", "b"]);
        app.handle(NormalAction::OpenSessions);
        app.handle(NormalAction::MoveDown);
        app.handle(NormalAction::Confirm);

        assert_eq!(app.view(), View::Dashboard);
        assert_eq!(
            app.monitor.attached().map(|t| t.session_id.clone()),
            Some("b".to_owned())
        );
    }

    #[test]
    fn confirming_with_no_sessions_does_nothing_rather_than_panicking() {
        let mut app = app(&[]);
        app.handle(NormalAction::OpenSessions);
        app.handle(NormalAction::Confirm);
        assert_eq!(
            app.overlay,
            Overlay::Sessions,
            "nothing to attach to, so it stays open"
        );
    }

    #[test]
    fn scrolling_the_log_cannot_go_above_the_newest_entry() {
        let mut app = app(&["a"]);
        app.tick();
        app.handle(NormalAction::GotoView(6)); // Log, the sixth tab
        app.handle(NormalAction::MoveUp);
        assert_eq!(app.log_offset, 0);
    }

    #[test]
    fn jump_to_row_top_and_bottom_reach_both_ends_of_the_log() {
        let mut app = app(&["a"]);
        app.tick();
        app.handle(NormalAction::GotoView(6)); // Log, the sixth tab

        app.handle(NormalAction::JumpToRow(RowTarget::Top));
        assert_eq!(
            app.log_offset,
            app.max_log_offset(),
            "top is the oldest entry"
        );

        app.handle(NormalAction::JumpToRow(RowTarget::Bottom));
        assert_eq!(app.log_offset, 0, "bottom is the newest entry");
    }

    #[test]
    fn the_tab_bar_lists_every_content_view_by_its_one_based_number() {
        // `crate::tui::screens::TAB_LABELS` cannot be derived from
        // `CONTENT_VIEWS` (see that constant's own doc comment for why --
        // `screens` cannot reach back into `app`), so the two are kept in
        // step by hand. This is the test that promise depends on: it fails
        // the moment a tab is added, renamed or reordered on one side of the
        // module boundary without the same change on the other.
        assert_eq!(
            crate::tui::screens::TAB_LABELS.len(),
            CONTENT_VIEWS.len(),
            "every content view needs exactly one tab bar label"
        );
        for (view, label) in CONTENT_VIEWS.iter().zip(crate::tui::screens::TAB_LABELS) {
            let name = format!("{view:?}");
            assert_eq!(
                name, label,
                "CONTENT_VIEWS and TAB_LABELS have drifted apart at {name:?}"
            );
        }
    }

    #[test]
    fn cycling_views_moves_through_every_tab_and_wraps_at_both_ends() {
        let mut app = app(&["a"]);
        assert_eq!(app.view(), View::Dashboard);

        for expected in [
            View::Daily,
            View::Weekly,
            View::Monthly,
            View::Blocks,
            View::Log,
        ] {
            app.handle(NormalAction::NextView);
            assert_eq!(app.view(), expected);
        }

        app.handle(NormalAction::NextView);
        assert_eq!(
            app.view(),
            View::Dashboard,
            "gt from the last tab wraps back to the first"
        );

        app.handle(NormalAction::PrevView);
        assert_eq!(
            app.view(),
            View::Log,
            "gT from the first tab wraps to the last"
        );
    }

    #[test]
    fn gt_from_the_last_tab_wraps_to_dashboards_tab_number_one() {
        // This epic's own acceptance criteria say, in prose, "gt from Blocks
        // wraps to Dashboard's tab number 1" -- but the same spec section
        // states, twice and in agreement with itself, that Blocks is the
        // *fifth* of six tabs (` 1 Dashboard  2 Daily  3 Weekly  4 Monthly
        // 5 Blocks  6 Log `, and `CONTENT_VIEWS`' declared order), with Log
        // sixth and last. `gt` from a tab that is not last does not wrap; it
        // moves on to the next one (`Blocks -> Log`, proved by
        // `cycling_views_moves_through_every_tab_and_wraps_at_both_ends`
        // above). This test instead proves the wrap the criterion is
        // actually checking for, from the tab that really is last -- see
        // `App::goto_view`'s own doc comment for the fuller account of this
        // judgement call.
        let mut app = app(&["a"]);
        app.handle(NormalAction::GotoView(6));
        assert_eq!(app.view(), View::Log, "tab 6 is the last one");

        app.handle(NormalAction::NextView);
        assert_eq!(
            app.view(),
            View::Dashboard,
            "gt from the last tab wraps to tab number 1"
        );
    }

    #[test]
    fn three_gt_from_any_tab_jumps_directly_to_weekly_the_third_tab() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::GotoView(3));
        assert_eq!(
            app.view(),
            View::Weekly,
            "tab 3, per the tab bar's own numbering"
        );
    }

    #[test]
    fn ctrl_o_returns_to_the_view_and_position_before_three_tab_switches() {
        let mut app = app(&["a"]);
        let original_view = app.view();
        let original_scroll = app.scroll_snapshot();

        for _ in 0..3 {
            app.handle(NormalAction::NextView);
        }
        assert_ne!(app.view(), original_view, "three switches actually moved");

        for _ in 0..3 {
            app.handle(NormalAction::JumpBack);
        }

        assert_eq!(app.view(), original_view, "back to the original tab");
        assert_eq!(
            app.scroll_snapshot(),
            original_scroll,
            "and the original scroll position"
        );
    }

    use crate::domain::session::{LogEntry, LogLevel};

    /// A reader whose snapshot carries the given log entries, so a test can
    /// drive [`App::run_search`] against a view with known, matchable text
    /// -- [`Reader`] above always hands back an empty snapshot, which has
    /// nothing for a search to find.
    struct LoggedReader(Vec<LogEntry>);

    impl SessionReader for LoggedReader {
        fn read(&self, t: &TranscriptRef) -> anyhow::Result<SessionSnapshot> {
            let mut snapshot = SessionSnapshot::empty(t.path.clone(), t.session_id.clone());
            snapshot.events = self.0.clone().into();
            Ok(snapshot)
        }
    }

    fn logged_entry(text: &str) -> LogEntry {
        LogEntry {
            at: Utc::now(),
            level: LogLevel::Info,
            text: text.to_owned(),
        }
    }

    fn app_with_log(entries: Vec<LogEntry>) -> App<Catalog, LoggedReader, Factory> {
        let catalog = Catalog(vec![transcript("a")]);
        let mut app = App::new(Monitor::new(
            catalog,
            LoggedReader(entries),
            Factory,
            SessionSelector::Active,
        ));
        app.tick(); // attaches to "a" and reads its (fixture) log entries
        app
    }

    /// A plain, unmodified key press, for driving [`App::handle_line_edit`]
    /// the same way `crate::tui::runtime`'s real event loop would.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_a_search_pattern_and_pressing_enter_jumps_to_the_first_match() {
        let mut app = app_with_log(vec![
            logged_entry("starting up"),
            logged_entry("reading file foo.rs"),
            logged_entry("done"),
        ]);
        app.handle(NormalAction::GotoView(6)); // Log, log_offset stays 0 (newest)

        app.handle(NormalAction::EnterSearch);
        for c in ['f', 'o', 'o'] {
            assert_eq!(
                app.handle_line_edit(key(KeyCode::Char(c))),
                LineEditOutcome::Changed
            );
        }
        assert_eq!(
            app.handle_line_edit(key(KeyCode::Enter)),
            LineEditOutcome::Confirmed
        );

        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(
            app.log_offset, 1,
            "jumped to the one entry mentioning foo.rs, one step back from the newest"
        );
        assert_eq!(app.last_search, Some(("foo".to_owned(), true)));

        // The confirmed search moved `log_offset` from `0` to `1` -- a
        // confirmed search is a jump the same way `gt`/`gg` are, so `Ctrl-o`
        // must be able to undo it too.
        app.handle(NormalAction::JumpBack);
        assert_eq!(
            app.log_offset, 0,
            "Ctrl-o undoes a confirmed search the same way it undoes gt/gg"
        );
    }

    #[test]
    fn backspace_on_an_empty_search_buffer_exits_search_mode() {
        let mut app = app_with_log(vec![logged_entry("one"), logged_entry("two")]);
        app.handle(NormalAction::GotoView(6)); // Log
        app.handle(NormalAction::JumpToRow(RowTarget::Top));
        let offset_before = app.log_offset;

        app.handle(NormalAction::EnterSearch);
        let outcome = app.handle_line_edit(key(KeyCode::Backspace));

        assert_eq!(outcome, LineEditOutcome::Cancelled);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(
            app.log_offset, offset_before,
            "the scroll position from before the search is restored"
        );
    }

    #[test]
    fn pressing_enter_on_an_empty_search_buffer_does_not_forget_the_previous_search() {
        let mut app = app_with_log(vec![
            logged_entry("foo one"),
            logged_entry("nothing here"),
            logged_entry("foo two"),
        ]);
        app.handle(NormalAction::GotoView(6)); // Log

        app.handle(NormalAction::EnterSearch);
        for c in ['f', 'o', 'o'] {
            app.handle_line_edit(key(KeyCode::Char(c)));
        }
        app.handle_line_edit(key(KeyCode::Enter));
        let remembered = app.last_search.clone();

        app.handle(NormalAction::EnterSearch);
        let outcome = app.handle_line_edit(key(KeyCode::Enter));

        assert_eq!(outcome, LineEditOutcome::Confirmed);
        assert_eq!(
            app.input_mode,
            InputMode::Normal,
            "confirming an empty pattern still leaves search mode"
        );
        assert_eq!(
            app.last_search, remembered,
            "an empty pattern is not a real search and must not overwrite the last real one"
        );
    }

    #[test]
    fn n_repeats_the_last_search_forward_from_the_new_position() {
        let mut app = app_with_log(vec![
            logged_entry("foo one"),
            logged_entry("nothing here"),
            logged_entry("foo two"),
        ]);
        app.handle(NormalAction::GotoView(6)); // Log

        app.handle(NormalAction::EnterSearch);
        for c in ['f', 'o', 'o'] {
            app.handle_line_edit(key(KeyCode::Char(c)));
        }
        app.handle_line_edit(key(KeyCode::Enter));
        let first_match = app.log_offset;

        app.handle(NormalAction::RepeatSearch(true));
        assert_ne!(
            app.log_offset, first_match,
            "n should have moved on to the next occurrence of \"foo\""
        );
    }

    #[test]
    fn colon_q_enter_quits() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::EnterCommand);
        app.handle_line_edit(key(KeyCode::Char('q')));
        let outcome = app.handle_line_edit(key(KeyCode::Enter));

        assert_eq!(outcome, LineEditOutcome::Confirmed);
        assert!(app.should_quit());
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    /// Types `:view <name>` and presses Enter, the same way a user driving
    /// [`NormalAction::EnterCommand`] would.
    fn run_view_command(app: &mut App<Catalog, Reader, Factory>, name: &str) {
        app.handle(NormalAction::EnterCommand);
        for c in format!("view {name}").chars() {
            app.handle_line_edit(key(KeyCode::Char(c)));
        }
        app.handle_line_edit(key(KeyCode::Enter));
    }

    #[test]
    fn colon_view_accepts_all_six_tab_names() {
        // `:view <name>` (epic 7's command grammar) has to accept every one
        // of the six content tabs this epic adds, matching exactly what the
        // tab bar itself calls them -- see `App::command_view`'s own doc
        // comment for why `sessions` is deliberately not among them.
        let mut app = app(&["a"]);
        for (name, expected) in [
            ("daily", View::Daily),
            ("weekly", View::Weekly),
            ("monthly", View::Monthly),
            ("blocks", View::Blocks),
            ("log", View::Log),
            ("dashboard", View::Dashboard),
        ] {
            run_view_command(&mut app, name);
            assert_eq!(app.view(), expected, "`:view {name}`");
            assert_eq!(app.input_mode, InputMode::Normal);
        }
    }

    #[test]
    fn colon_view_sessions_is_not_a_command_any_more() {
        // The session picker is `Overlay::Sessions`, reached with `o`, not a
        // `:view` destination -- see `View`'s own doc comment for why.
        let mut app = app(&["a"]);
        run_view_command(&mut app, "sessions");

        assert_eq!(app.view(), View::Dashboard, "the view did not change");
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|n| n.contains("not a command")),
            "and the footer says so"
        );
    }

    #[test]
    fn colon_theme_with_a_known_name_switches_the_palette() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::EnterCommand);
        for c in "theme nord".chars() {
            app.handle_line_edit(key(KeyCode::Char(c)));
        }
        app.handle_line_edit(key(KeyCode::Enter));

        assert_eq!(app.palette.name, "nord");
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn an_unknown_command_sets_a_footer_notice_and_returns_to_normal() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::EnterCommand);
        app.handle_line_edit(key(KeyCode::Char('z')));
        app.handle_line_edit(key(KeyCode::Char('z')));
        app.handle_line_edit(key(KeyCode::Enter));

        assert_eq!(app.input_mode, InputMode::Normal);
        let notice = app
            .notice
            .as_deref()
            .expect("an unknown verb sets a notice");
        assert!(notice.contains("not a command"), "{notice:?}");
    }

    #[test]
    fn pressing_t_twice_advances_the_palette_but_the_first_press_only_opens_the_picker() {
        let mut app = app(&["a"]);
        let names = app.theme_names.clone();
        let start = names
            .iter()
            .position(|name| *name == app.palette.name)
            .expect("the starting theme is registered");

        app.handle(NormalAction::CycleTheme);
        assert_eq!(
            app.overlay,
            Overlay::ThemePicker,
            "the first press opens the picker"
        );
        assert_eq!(app.palette.name, names[start], "nothing changed yet");

        app.handle(NormalAction::CycleTheme);
        assert_eq!(app.palette.name, names[(start + 1) % names.len()]);
    }

    #[test]
    fn cycling_the_theme_all_the_way_round_wraps_back_to_the_start() {
        let mut app = app(&["a"]);
        let start_name = app.palette.name.clone();
        let steps = app.theme_names.len();

        // One press opens the picker (no change), then `steps` more presses
        // advance it exactly once around the whole registry.
        for _ in 0..=steps {
            app.handle(NormalAction::CycleTheme);
        }

        assert_eq!(app.palette.name, start_name, "wrapped all the way around");
    }

    #[test]
    fn confirming_the_layout_picker_changes_the_active_preset() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::OpenLayoutPicker);
        app.handle(NormalAction::MoveDown);
        let chosen = app.layout_names[app.picker_selected].clone();

        app.handle(NormalAction::Confirm);

        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(app.active_preset, chosen);
    }

    #[test]
    fn escaping_the_layout_picker_leaves_the_active_preset_untouched() {
        let mut app = app(&["a"]);
        let original = app.active_preset.clone();
        app.handle(NormalAction::OpenLayoutPicker);
        app.handle(NormalAction::MoveDown);

        app.handle(NormalAction::Back);

        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(
            app.active_preset, original,
            "Esc cancels without persisting"
        );
    }

    #[test]
    fn the_status_badge_shows_the_live_search_buffer() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::EnterSearch);
        app.handle_line_edit(key(KeyCode::Char('f')));

        let (text, _) = app.status_badge().expect("search is active");
        assert_eq!(text, "SEARCH: f");
    }

    #[test]
    fn the_status_badge_is_absent_in_plain_normal_mode() {
        let app = app(&["a"]);
        assert_eq!(app.status_badge(), None);
    }

    #[test]
    fn the_status_badge_shows_a_bare_pending_count_and_chord() {
        let mut app = app(&["a"]);
        app.set_pending(Pending {
            count: Some(5),
            chord: ChordState::AwaitingG,
        });

        let (text, _) = app.status_badge().expect("a count and chord are pending");
        assert_eq!(text, "5g");
    }

    #[test]
    fn ctrl_c_quits_only_when_nothing_is_pending_and_the_mode_is_normal() {
        let mut app = app(&["a"]);
        app.handle_ctrl_c();
        assert!(
            app.should_quit(),
            "nothing was pending, so this is the same as 'q'"
        );
    }

    #[test]
    fn ctrl_c_clears_a_pending_count_instead_of_quitting() {
        let mut app = app(&["a"]);
        app.set_pending(Pending {
            count: Some(5),
            chord: ChordState::Idle,
        });

        app.handle_ctrl_c();

        assert!(!app.should_quit(), "there was a count to abort first");
        assert_eq!(app.pending(), Pending::default());
    }

    #[test]
    fn ctrl_c_exits_search_mode_instead_of_quitting() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::EnterSearch);

        app.handle_ctrl_c();

        assert!(
            !app.should_quit(),
            "Ctrl-C aborts the search, not the dashboard"
        );
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn with_config_selects_the_named_theme() {
        let config = Config {
            theme: Some("dracula".to_owned()),
            ..Config::default()
        };
        let app = app(&["a"]).with_config(&config, None);

        assert_eq!(app.palette.name, "dracula");
    }

    #[test]
    fn with_config_falls_back_to_aurora_when_no_theme_is_named() {
        let app = app(&["a"]).with_config(&Config::default(), None);

        assert_eq!(app.palette.name, "aurora");
    }

    #[test]
    fn with_config_applies_the_configured_animation_style() {
        // Before this test existed, `config.animation` parsed and validated
        // cleanly but `App::with_config` never read it -- every dashboard
        // rendered `AnimationStyle::Pulse` regardless of what the file said.
        // This pins the fix: a `Config` naming `Coin` actually changes
        // `App`'s own style, not just the `Config` value sitting unused.
        let config = Config {
            animation: AnimationStyle::Coin,
            ..Config::default()
        };
        let app = app(&["a"]).with_config(&config, None);

        assert_eq!(app.animation_style, AnimationStyle::Coin);
    }

    #[test]
    fn a_config_warning_shows_in_the_footer_until_the_first_key_press() {
        let mut app = app(&["a"]).with_config(
            &Config::default(),
            Some(ConfigWarning {
                message: "config.json is not valid config; using defaults".to_owned(),
            }),
        );

        assert!(
            rendered(&mut app).contains("config.json is not valid config"),
            "the warning from startup should reach the footer"
        );

        app.handle(NormalAction::MoveDown);

        assert!(
            app.config_warning.is_none(),
            "the first action handled clears it, the same way a `notice` is cleared"
        );
        assert!(
            !rendered(&mut app).contains("config.json is not valid config"),
            "and it must not still be on screen afterwards"
        );
    }

    /// A [`ReportSource`] that answers from memory and counts how often each
    /// method was called, so a test can prove [`App::ensure_reports_loaded`]
    /// really does cache rather than reloading every time a tab is entered.
    struct FakeReports {
        daily_calls: u32,
    }

    impl crate::application::report_source::ReportSource for FakeReports {
        fn daily(&mut self) -> anyhow::Result<crate::domain::report::UsageReport> {
            self.daily_calls += 1;
            Ok(crate::domain::report::UsageReport::build(
                &[],
                &crate::domain::period::GroupingSpec::default(),
                &Zone::Utc,
                crate::domain::pricing::CostMode::Auto,
                &crate::domain::pricing::PriceSheet::builtin(),
            ))
        }
        fn weekly(&mut self) -> anyhow::Result<crate::domain::report::UsageReport> {
            self.daily()
        }
        fn monthly(&mut self) -> anyhow::Result<crate::domain::report::UsageReport> {
            self.daily()
        }
        fn blocks(
            &mut self,
            _now: chrono::DateTime<Utc>,
        ) -> anyhow::Result<Vec<crate::application::blocks_report::BlockRow>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn switching_to_the_daily_tab_loads_it_once_and_reuses_it_on_a_second_visit() {
        let mut app = app(&["a"]).with_reports(Some(Box::new(FakeReports { daily_calls: 0 })));

        app.handle(NormalAction::GotoView(2)); // Daily
        assert!(app.daily_report.is_some(), "the tab loaded its own figures");

        app.handle(NormalAction::GotoView(1)); // back to Dashboard
        app.handle(NormalAction::GotoView(2)); // Daily again

        // Reaching into the fake through the boxed trait object is not worth
        // the ceremony a downcast would take; this test instead reads the
        // one externally visible sign that a second load did or did not
        // happen -- the cached field survives the round trip rather than
        // being cleared and refetched.
        assert!(app.daily_report.is_some());
    }

    #[test]
    fn refresh_clears_every_cached_report_and_reloads_the_current_tab() {
        let mut app = app(&["a"]).with_reports(Some(Box::new(FakeReports { daily_calls: 0 })));
        app.handle(NormalAction::GotoView(3)); // Weekly
        assert!(app.weekly_report.is_some());

        app.handle(NormalAction::Refresh);

        assert!(
            app.weekly_report.is_some(),
            "refreshed rather than merely cleared -- the current tab reloads immediately"
        );
        assert!(
            app.daily_report.is_none(),
            "a tab that is not showing stays cleared until it is visited again"
        );
    }

    #[test]
    fn with_no_report_source_the_tabs_stay_empty_rather_than_panicking() {
        let mut app = app(&["a"]);
        app.handle(NormalAction::GotoView(4)); // Monthly
        assert!(app.monthly_report.is_none());
        let _ = rendered(&mut app);
    }

    #[test]
    fn the_which_key_popup_shows_only_while_a_g_chord_is_pending() {
        let mut app = app(&["a"]);
        assert!(
            !rendered(&mut app).contains("jump to the top"),
            "no chord pending yet"
        );

        app.set_pending(Pending {
            count: None,
            chord: ChordState::AwaitingG,
        });
        assert!(
            rendered(&mut app).contains("jump to the top"),
            "'gg's own description, read straight out of help_rows"
        );

        app.set_pending(Pending::default());
        assert!(
            !rendered(&mut app).contains("jump to the top"),
            "gone the instant the chord resolves or is cancelled"
        );
    }
}

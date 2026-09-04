//! Owning the terminal: setup, the event loop, and getting out cleanly.
//!
//! The one rule this module exists to enforce is that the terminal is always
//! restored. A dashboard that leaves raw mode on and the alternate screen
//! active after a panic leaves the user with a shell that does not echo what
//! they type -- and the usual reaction is to close the window, losing whatever
//! else was in it. The panic hook installed here makes that impossible.

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::application::monitor::Monitor;
use crate::application::ports::{ChangeSourceFactory, SessionReader, TranscriptCatalog};
use crate::application::report_source::ReportSource;
use crate::application::usage::UsageTracker;
use crate::infrastructure::config::{Config, ConfigGateway, ConfigWarning};
use crate::tui::app::{App, InputMode};
use crate::tui::keymap;

/// How long to wait for input before drawing another frame.
///
/// This is the animation frame budget, not a polling interval for the
/// transcript -- changes to that arrive through the filesystem watcher. At
/// roughly eight frames a second the spinners look alive without the process
/// showing up in `top`.
const FRAME_BUDGET: Duration = Duration::from_millis(125);

/// Runs the dashboard until the user quits.
///
/// `config` and `config_warning` are already-resolved by the time they
/// reach here -- `src/main.rs`'s `monitor` function calls
/// `crate::infrastructure::config::ConfigGateway::load_or_default` and
/// `crate::infrastructure::config::resolve` itself, *before* calling this
/// function, specifically so a malformed `config.json`'s warning is printed
/// to the real terminal while there still is one: by the time this function
/// gets as far as [`ratatui::try_init`] a few lines down, `stderr` is about
/// to disappear behind the alternate screen.
///
/// `reports` feeds the Daily/Weekly/Monthly/Blocks tabs, and is built at
/// the call site rather than reconstructed here the way `ConfigGateway` is a
/// few lines down: `src/main.rs`'s `monitor` function already has the
/// resolved [`crate::domain::pricing::PriceSheet`] this function has no
/// parameter for, in scope for the very similar `UsageTracker` it hands
/// `usage` in as. `None` leaves those four tabs showing their own "nothing
/// loaded yet" message -- see [`crate::tui::app::App::with_reports`].
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or drawn to.
pub fn run<C, R, W>(
    monitor: Monitor<C, R, W>,
    usage: UsageTracker,
    config: &Config,
    config_warning: Option<ConfigWarning>,
    reports: Option<Box<dyn ReportSource>>,
) -> Result<()>
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    install_panic_hook();
    let mut terminal = ratatui::try_init()?;
    let mut app = App::new(monitor)
        .tracking_usage(usage)
        .with_config(config, config_warning)
        .with_reports(reports);
    // Reconstructed here rather than threaded through as a parameter:
    // `src/main.rs`'s own `ConfigGateway::from_config_dir()` call, a few
    // lines before this function runs, already resolved the exact same path
    // to load `config` from -- doing it again costs nothing (it is a pure
    // function of the environment) and spares this function's signature a
    // fourth parameter for what is, at this call site, a `Result` that can
    // only fail the same way the first call already would have. A failure
    // here (an unreadable `$HOME`) simply means runtime theme/layout changes
    // do not persist for this run -- the dashboard still starts and runs
    // normally, which is why this is `if let`, not `?`.
    if let Ok(gateway) = ConfigGateway::from_config_dir() {
        app = app.persisting_config(gateway);
    }
    let outcome = event_loop(&mut terminal, app);
    ratatui::restore();
    outcome
}

fn event_loop<B, C, R, W>(terminal: &mut ratatui::Terminal<B>, mut app: App<C, R, W>) -> Result<()>
where
    B: ratatui::backend::Backend,
    B::Error: std::error::Error + Send + Sync + 'static,
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    while !app.should_quit() {
        terminal.draw(|frame| app.draw(frame))?;

        // Spend whatever is left of the frame budget waiting for input, so a
        // key press is acted on immediately rather than at the next frame
        // boundary, and an idle dashboard costs nothing.
        let deadline = Instant::now() + FRAME_BUDGET;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if !event::poll(remaining)? {
                break;
            }
            match event::read()? {
                Event::Key(key) => handle_key(&mut app, key),
                // A resize invalidates the whole layout, so stop waiting and
                // redraw at once rather than showing a torn frame for an
                // eighth of a second.
                Event::Resize(_, _) => break,
                _ => {}
            }
        }

        app.tick();
    }
    Ok(())
}

/// Applies one key event to `app`.
///
/// Ctrl-C is handled here rather than inside [`keymap::resolve`], ahead of
/// everything else: [`App::handle_ctrl_c`] decides whether it means "quit"
/// or "abort whatever is pending" by reading `app`'s mode, and `resolve` is
/// deliberately pure state-machine logic over a `Keymap` with no idea what
/// mode the dashboard is in.
///
/// Everything else forks on [`App::input_mode`]. While it is
/// [`InputMode::Search`] or [`InputMode::Command`], the key goes straight to
/// [`App::handle_line_edit`] -- see that method's own doc comment for why a
/// `Keymap` built to resolve one whole key press to one action is the wrong
/// tool for typing free text into a buffer. Otherwise the key is turned into
/// a [`keymap::Key`] and fed through `app`'s [`keymap::Keymap`] via
/// `keymap::resolve`, exactly as before this epic -- see `crate::tui::keymap`
/// for why that used to be a `match` in this module (`Action::from_key`)
/// instead.
///
/// Pulled out of `event_loop` so it can be exercised directly against a
/// `KeyEvent` in tests, without needing a real terminal to read one from.
fn handle_key<C, R, W>(app: &mut App<C, R, W>, key: KeyEvent)
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    // Terminals on Windows report a release for every press; without this
    // check every action would fire twice there.
    if key.kind != KeyEventKind::Press {
        return;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.handle_ctrl_c();
        return;
    }

    if !matches!(app.input_mode(), InputMode::Normal) {
        let _ = app.handle_line_edit(key);
        return;
    }

    let pressed = keymap::Key {
        code: key.code,
        mods: key.modifiers,
    };
    let (next_pending, action) = keymap::resolve(app.keymap(), app.pending(), pressed);
    app.set_pending(next_pending);
    if let Some(action) = action {
        app.handle(action);
    }
}

/// Restores the terminal before letting a panic through.
///
/// Without this, a panic in a widget leaves raw mode enabled and the alternate
/// screen active. The backtrace scrolls past unreadably and the shell is left
/// in a state most users respond to by closing the window.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        previous(info);
    }));
}

/// Whether the process is attached to a terminal that can host the dashboard.
///
/// Checked before starting, so that piping the command somewhere produces a
/// clear message rather than a screenful of escape sequences.
#[must_use]
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::Utc;

    use super::*;
    use crate::application::ports::{ChangeSource, SessionSelector, TranscriptRef};
    use crate::domain::session::SessionSnapshot;
    use crate::tui::app::{ScrollSnapshot, View};
    use crate::tui::keymap::{ChordState, NormalAction, Pending};

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

    fn app() -> App<Catalog, Reader, Factory> {
        let catalog = Catalog(vec![transcript("a")]);
        App::new(Monitor::new(
            catalog,
            Reader,
            Factory,
            SessionSelector::Active,
        ))
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn a_key_release_is_ignored_so_windows_does_not_fire_everything_twice() {
        let mut app = app();
        let mut release = key(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        handle_key(&mut app, release);

        assert!(!app.should_quit(), "a release is not a press");
    }

    #[test]
    fn pressing_q_quits_by_resolving_through_the_keymap() {
        let mut app = app();
        handle_key(&mut app, key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.should_quit());
    }

    #[test]
    fn ctrl_c_quits_when_nothing_is_pending() {
        let mut app = app();
        handle_key(&mut app, key(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(
            app.should_quit(),
            "nothing was pending, so this is the same as 'q'"
        );
    }

    #[test]
    fn ctrl_c_clears_a_pending_count_instead_of_quitting() {
        let mut app = app();
        app.set_pending(Pending {
            count: Some(5),
            chord: ChordState::Idle,
        });

        handle_key(&mut app, key(KeyCode::Char('c'), KeyModifiers::CONTROL));

        assert!(!app.should_quit(), "there was a count to abort first");
        assert_eq!(app.pending(), Pending::default());
    }

    #[test]
    fn ctrl_c_exits_search_mode_instead_of_quitting() {
        let mut app = app();
        app.handle(NormalAction::EnterSearch);

        handle_key(&mut app, key(KeyCode::Char('c'), KeyModifiers::CONTROL));

        assert!(
            !app.should_quit(),
            "Ctrl-C aborts the search, not the dashboard"
        );
    }

    #[test]
    fn slash_enters_search_mode_and_further_keys_bypass_the_keymap() {
        // Driven through `handle_key` itself, the same function the real
        // event loop calls -- proof that `/` really does flip the dispatch
        // this module does, not just that `App::handle_line_edit` works when
        // called directly.
        let mut app = app();
        handle_key(&mut app, key(KeyCode::Char('/'), KeyModifiers::NONE));
        assert_eq!(
            *app.input_mode(),
            InputMode::Search {
                buf: String::new(),
                origin_scroll: ScrollSnapshot::None,
            }
        );

        // 'q' would quit in normal mode; while typing a search it is just a
        // letter appended to the buffer.
        handle_key(&mut app, key(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(
            !app.should_quit(),
            "'q' was consumed by the search buffer, not the keymap"
        );

        handle_key(&mut app, key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(*app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn a_full_colon_command_typed_through_handle_key_quits() {
        let mut app = app();
        for code in [KeyCode::Char(':'), KeyCode::Char('q'), KeyCode::Enter] {
            handle_key(&mut app, key(code, KeyModifiers::NONE));
        }
        assert!(app.should_quit());
    }

    #[test]
    fn the_gt_chord_resolves_across_two_separate_key_events() {
        let mut app = app();
        app.tick();
        handle_key(&mut app, key(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(
            app.view(),
            View::Dashboard,
            "'g' alone only opens the chord"
        );

        handle_key(&mut app, key(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(
            app.view(),
            View::Daily,
            "'gt' moved from the dashboard to the next tab, Daily"
        );
    }
}

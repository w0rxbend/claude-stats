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
use ratatui::crossterm::event::{self, Event};

use crate::application::monitor::Monitor;
use crate::application::ports::{ChangeSourceFactory, SessionReader, TranscriptCatalog};
use crate::tui::app::{Action, App};

/// How long to wait for input before drawing another frame.
///
/// This is the animation frame budget, not a polling interval for the
/// transcript -- changes to that arrive through the filesystem watcher. At
/// roughly eight frames a second the spinners look alive without the process
/// showing up in `top`.
const FRAME_BUDGET: Duration = Duration::from_millis(125);

/// Runs the dashboard until the user quits.
///
/// # Errors
///
/// Returns an error if the terminal cannot be set up or drawn to.
pub fn run<C, R, W>(monitor: Monitor<C, R, W>) -> Result<()>
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    install_panic_hook();
    let mut terminal = ratatui::try_init()?;
    let outcome = event_loop(&mut terminal, App::new(monitor));
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
                Event::Key(key) => {
                    if let Some(action) = Action::from_key(key) {
                        app.handle(action);
                    }
                }
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

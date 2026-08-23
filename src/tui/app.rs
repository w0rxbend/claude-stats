//! The dashboard's state machine.
//!
//! Key events are translated into an [`Action`] first, and the state
//! transitions are written against actions rather than against key codes. That
//! indirection buys two things: the transitions are testable without
//! synthesising terminal events, and rebinding a key is a change in one match
//! arm rather than a hunt through the update logic.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::application::monitor::{Monitor, Tick};
use crate::application::ports::{ChangeSourceFactory, SessionReader, TranscriptCatalog};
use crate::application::usage::UsageTracker;
use crate::tui::icons::Icon;
use crate::tui::screens;
use crate::tui::theme::Theme;

/// Which full-screen view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// The metrics dashboard.
    #[default]
    Dashboard,
    /// The scrollable event log.
    Log,
    /// The session picker.
    Sessions,
}

/// Something the user asked for, independent of which key produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Show(View),
    /// Dismiss the help overlay, or leave the current view for the dashboard.
    Back,
    ToggleHelp,
    MoveDown,
    MoveUp,
    JumpToOldest,
    JumpToNewest,
    /// Attach to whatever the session picker has selected.
    Confirm,
    /// Re-read the transcript immediately.
    Refresh,
}

impl Action {
    /// Maps a key press to an action, or `None` if the key is not bound.
    ///
    /// Only key *presses* map to anything. Terminals on Windows report a
    /// release for every press, and without this check every action would fire
    /// twice there.
    #[must_use]
    pub fn from_key(key: KeyEvent) -> Option<Self> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        // Ctrl-C is a quit everywhere, regardless of what else is bound.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Self::Quit);
        }
        match key.code {
            KeyCode::Char('q') => Some(Self::Quit),
            KeyCode::Esc => Some(Self::Back),
            KeyCode::Char('d') => Some(Self::Show(View::Dashboard)),
            KeyCode::Char('l') => Some(Self::Show(View::Log)),
            KeyCode::Char('o') => Some(Self::Show(View::Sessions)),
            KeyCode::Char('?') | KeyCode::F(1) => Some(Self::ToggleHelp),
            KeyCode::Char('j') | KeyCode::Down => Some(Self::MoveDown),
            KeyCode::Char('k') | KeyCode::Up => Some(Self::MoveUp),
            KeyCode::Char('g') | KeyCode::Home => Some(Self::JumpToOldest),
            KeyCode::Char('G') | KeyCode::End => Some(Self::JumpToNewest),
            KeyCode::Enter => Some(Self::Confirm),
            KeyCode::Char('r') => Some(Self::Refresh),
            _ => None,
        }
    }
}

/// The running dashboard.
pub struct App<C, R, W> {
    monitor: Monitor<C, R, W>,
    view: View,
    help_open: bool,
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
}

impl<C, R, W> App<C, R, W>
where
    C: TranscriptCatalog,
    R: SessionReader,
    W: ChangeSourceFactory,
{
    /// A dashboard driving the given monitor.
    pub fn new(monitor: Monitor<C, R, W>) -> Self {
        Self {
            monitor,
            view: View::default(),
            help_open: false,
            phase: 0,
            log_offset: 0,
            sessions: Vec::new(),
            selected: 0,
            quit: false,
            notice: None,
            usage: None,
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
        let outcome = self.monitor.tick();
        if outcome == Tick::Attached {
            // A different session means the old scroll position points at
            // entries that no longer exist.
            self.log_offset = 0;
        }
    }

    /// Applies an action.
    pub fn handle(&mut self, action: Action) {
        self.notice = None;
        match action {
            Action::Quit => self.quit = true,
            Action::Back => {
                if self.help_open {
                    self.help_open = false;
                } else if self.view == View::Dashboard {
                    self.quit = true;
                } else {
                    self.view = View::Dashboard;
                }
            }
            Action::ToggleHelp => self.help_open = !self.help_open,
            Action::Show(view) => {
                self.help_open = false;
                if view == View::Sessions {
                    self.refresh_session_list();
                }
                self.view = view;
            }
            Action::MoveDown => self.move_selection(1),
            Action::MoveUp => self.move_selection(-1),
            Action::JumpToOldest => match self.view {
                View::Log => self.log_offset = self.max_log_offset(),
                View::Sessions => self.selected = self.sessions.len().saturating_sub(1),
                View::Dashboard => {}
            },
            Action::JumpToNewest => match self.view {
                View::Log => self.log_offset = 0,
                View::Sessions => self.selected = 0,
                View::Dashboard => {}
            },
            Action::Confirm => self.attach_selected(),
            Action::Refresh => {
                self.refresh_session_list();
                if let Some(usage) = &mut self.usage {
                    usage.scan();
                }
            }
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
                    (self.log_offset + 1).min(max)
                } else {
                    self.log_offset.saturating_sub(1)
                };
            }
            View::Sessions => {
                if self.sessions.is_empty() {
                    return;
                }
                let last = self.sessions.len() - 1;
                self.selected = if delta > 0 {
                    (self.selected + 1).min(last)
                } else {
                    self.selected.saturating_sub(1)
                };
            }
            View::Dashboard => {}
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
        if self.view != View::Sessions {
            return;
        }
        let Some(chosen) = self.sessions.get(self.selected).cloned() else {
            return;
        };
        match self.monitor.attach_to(chosen) {
            Ok(()) => {
                self.log_offset = 0;
                self.view = View::Dashboard;
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    /// Draws the current view.
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let screen = frame.area();
        Paragraph::new("")
            .style(Theme::base())
            .render(screen, frame.buffer_mut());

        let [body, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(screen);

        match (self.view, self.monitor.snapshot()) {
            (View::Sessions, _) => screens::sessions::draw(
                frame,
                body,
                &self.sessions,
                self.selected,
                self.monitor.attached(),
            ),
            (View::Log, Some(snapshot)) => {
                screens::log::draw(frame, body, snapshot, self.log_offset);
            }
            (_, Some(snapshot)) => {
                let usage = self
                    .usage
                    .as_ref()
                    .map(|tracker| (tracker.usage(), tracker.has_measured()));
                screens::dashboard::draw(frame, body, snapshot, self.phase, usage);
            }
            (_, None) => screens::help::draw_searching(frame, body, self.phase),
        }

        self.draw_footer(frame, footer);

        if self.help_open {
            screens::help::draw(frame, screen);
        }
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        if let Some(notice) = &self.notice {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {} {notice}", Icon::ERROR),
                    Style::default().fg(Theme::CRIMSON).bg(Theme::SURFACE),
                )))
                .style(Style::default().bg(Theme::SURFACE)),
                area,
            );
            return;
        }

        let hint = match self.view {
            View::Dashboard => " q quit   l log   o sessions   ? help ",
            View::Log => " q quit   d dashboard   j/k scroll   g/G ends   ? help ",
            View::Sessions => " q quit   j/k move   Enter attach   d dashboard   ? help ",
        };
        let error = self
            .monitor
            .last_error()
            .map(|e| format!("  {} {e}", Icon::ERROR));

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(hint, Style::default().fg(Theme::MUTED)),
                Span::styled(
                    error.unwrap_or_default(),
                    Style::default().fg(Theme::CRIMSON),
                ),
            ]))
            .style(Style::default().bg(Theme::SURFACE)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use chrono::Utc;

    use super::*;
    use crate::application::ports::{ChangeSource, SessionSelector, TranscriptRef};
    use crate::domain::session::SessionSnapshot;

    struct Catalog(Vec<TranscriptRef>);

    impl TranscriptCatalog for Catalog {
        fn resolve(&self, _s: &SessionSelector) -> anyhow::Result<Option<TranscriptRef>> {
            Ok(self.0.first().cloned())
        }
        fn list(&self) -> anyhow::Result<Vec<TranscriptRef>> {
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

    fn press(code: KeyCode) -> Action {
        Action::from_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("bound key")
    }

    #[test]
    fn a_key_release_is_ignored_so_windows_does_not_fire_everything_twice() {
        let mut key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(Action::from_key(key), None);
    }

    #[test]
    fn ctrl_c_quits_whatever_else_is_bound() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(Action::from_key(key), Some(Action::Quit));
    }

    #[test]
    fn escape_backs_out_one_level_at_a_time_before_quitting() {
        let mut app = app(&["a"]);
        app.handle(Action::Show(View::Log));
        app.handle(Action::ToggleHelp);

        app.handle(Action::Back);
        assert!(!app.help_open, "first escape closes the help");
        assert_eq!(app.view(), View::Log, "and leaves the view alone");

        app.handle(Action::Back);
        assert_eq!(app.view(), View::Dashboard, "second escape returns home");
        assert!(!app.should_quit());

        app.handle(Action::Back);
        assert!(app.should_quit(), "escape at home quits");
    }

    #[test]
    fn opening_the_picker_loads_the_session_list() {
        let mut app = app(&["a", "b", "c"]);
        app.handle(press(KeyCode::Char('o')));
        assert_eq!(app.view(), View::Sessions);
        assert_eq!(app.sessions.len(), 3);
    }

    #[test]
    fn moving_through_the_picker_stops_at_both_ends() {
        let mut app = app(&["a", "b"]);
        app.handle(Action::Show(View::Sessions));

        app.handle(Action::MoveUp);
        assert_eq!(app.selected, 0, "cannot move above the first");

        app.handle(Action::MoveDown);
        app.handle(Action::MoveDown);
        assert_eq!(app.selected, 1, "cannot move past the last");
    }

    #[test]
    fn confirming_a_session_attaches_and_returns_to_the_dashboard() {
        let mut app = app(&["a", "b"]);
        app.handle(Action::Show(View::Sessions));
        app.handle(Action::MoveDown);
        app.handle(Action::Confirm);

        assert_eq!(app.view(), View::Dashboard);
        assert_eq!(
            app.monitor.attached().map(|t| t.session_id.clone()),
            Some("b".to_owned())
        );
    }

    #[test]
    fn confirming_with_no_sessions_does_nothing_rather_than_panicking() {
        let mut app = app(&[]);
        app.handle(Action::Show(View::Sessions));
        app.handle(Action::Confirm);
        assert_eq!(app.view(), View::Sessions);
    }

    #[test]
    fn scrolling_the_log_cannot_go_above_the_newest_entry() {
        let mut app = app(&["a"]);
        app.tick();
        app.handle(Action::Show(View::Log));
        app.handle(Action::MoveUp);
        assert_eq!(app.log_offset, 0);
    }
}

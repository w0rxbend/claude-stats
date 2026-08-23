//! The command-line surface.
//!
//! Every subcommand answers the same question -- what is a Claude Code session
//! costing and how close is it to compacting -- at a different level of
//! ceremony. `monitor` is the live dashboard, `stats` is the same numbers
//! printed once and piped somewhere, `sessions` and `models` are the
//! supporting lookups.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::application::ports::SessionSelector;

/// A colourful terminal dashboard for Claude Code sessions.
#[derive(Debug, Parser)]
#[command(name = "claude-stats", version, about, long_about = None)]
pub struct Cli {
    /// What to run. Defaults to the live dashboard.
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub selection: Selection,
}

/// Which session to look at.
///
/// Flattened into every subcommand rather than repeated, so `--session` means
/// the same thing everywhere and gains new spellings in one place.
#[derive(Debug, Clone, clap::Args)]
pub struct Selection {
    /// Follow the session whose id starts with this prefix.
    #[arg(long, value_name = "PREFIX", global = true)]
    pub session: Option<String>,

    /// Follow the newest session belonging to this directory.
    #[arg(long, value_name = "DIR", global = true)]
    pub project: Option<PathBuf>,

    /// Follow this transcript file directly.
    #[arg(long, value_name = "FILE", global = true)]
    pub path: Option<PathBuf>,
}

impl Selection {
    /// Turns the flags into a selector.
    ///
    /// The most specific flag wins, and with none of them set the dashboard
    /// follows whatever session is currently active. Rather than rejecting
    /// combinations, the order is fixed and documented: someone who passes two
    /// gets the more specific one, which is what they almost certainly meant.
    #[must_use]
    pub fn selector(&self) -> SessionSelector {
        if let Some(path) = &self.path {
            return SessionSelector::Path(path.clone());
        }
        if let Some(prefix) = &self.session {
            return SessionSelector::Id(prefix.clone());
        }
        if let Some(project) = &self.project {
            return SessionSelector::Project(project.clone());
        }
        SessionSelector::Active
    }
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Watch a session live in a second terminal. This is the default.
    Monitor,

    /// Print a one-shot report for a session and exit.
    Stats {
        /// Emit JSON instead of a formatted report.
        #[arg(long)]
        json: bool,
    },

    /// List the sessions on this machine, newest first.
    Sessions {
        /// Show at most this many.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Print the model catalogue: context windows and prices.
    Models,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_definition_is_internally_consistent() {
        // Catches duplicated flags, bad defaults and conflicting short names
        // at test time rather than at the user's first run.
        Cli::command().debug_assert();
    }

    #[test]
    fn with_no_flags_the_active_session_is_followed() {
        let cli = Cli::parse_from(["claude-stats"]);
        assert_eq!(cli.selection.selector(), SessionSelector::Active);
        assert!(cli.command.is_none());
    }

    #[test]
    fn the_more_specific_selector_wins_over_the_broader_one() {
        let cli = Cli::parse_from(["claude-stats", "--session", "abc", "--project", "/tmp"]);
        assert_eq!(
            cli.selection.selector(),
            SessionSelector::Id("abc".to_owned())
        );
    }

    #[test]
    fn selection_flags_work_after_a_subcommand_too() {
        let cli = Cli::parse_from(["claude-stats", "stats", "--session", "abc"]);
        assert!(matches!(cli.command, Some(Command::Stats { json: false })));
        assert_eq!(
            cli.selection.selector(),
            SessionSelector::Id("abc".to_owned())
        );
    }
}

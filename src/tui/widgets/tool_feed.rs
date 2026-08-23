//! The live tool-activity feed.
//!
//! Newest first. That is the opposite of a log file, and it is deliberate:
//! this panel answers "what is Claude doing *now*", so the answer belongs on
//! the first line where the eye lands, not at the bottom of a list that has to
//! be scrolled or scanned.
//!
//! The newest entry is also the only one drawn at full brightness. Everything
//! below it fades towards the muted tone, which gives the panel a sense of
//! motion on a terminal that cannot animate, and stops a busy feed from being
//! a wall of equally-loud text.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::domain::activity::ToolEvent;
use crate::tui::format;
use crate::tui::icons::Icon;
use crate::tui::theme::Theme;
use crate::tui::widgets::spinner::{Spinner, SpinnerStyle};

/// A newest-first list of recent tool calls.
pub struct ToolFeed<'a> {
    events: &'a std::collections::VecDeque<ToolEvent>,
    /// Whether the assistant is mid-turn, which puts a spinner on the newest
    /// entry.
    running: bool,
    /// Animation phase, shared with every other animated widget.
    phase: u64,
}

impl<'a> ToolFeed<'a> {
    /// A feed over the given events, which are held oldest-first.
    #[must_use]
    pub const fn new(
        events: &'a std::collections::VecDeque<ToolEvent>,
        running: bool,
        phase: u64,
    ) -> Self {
        Self {
            events,
            running,
            phase,
        }
    }
}

impl Widget for ToolFeed<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        if self.events.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                "no tool activity this turn",
                Style::default().fg(Theme::FAINT),
            )))
            .render(area, buf);
            return;
        }

        let rows = area.height as usize;
        let lines: Vec<Line<'_>> = self
            .events
            .iter()
            .rev()
            .take(rows)
            .enumerate()
            .map(|(depth, event)| self.line_for(event, depth, area.width as usize))
            .collect();

        Paragraph::new(lines).render(area, buf);
    }
}

impl ToolFeed<'_> {
    fn line_for<'t>(&self, event: &'t ToolEvent, depth: usize, width: usize) -> Line<'t> {
        let newest = depth == 0;

        // A failed call is always crimson, whatever its kind and however far
        // down the list it has scrolled: an error that fades out of notice is
        // an error that gets missed.
        let colour = if event.failed {
            Theme::CRIMSON
        } else if newest {
            Theme::tool_kind(event.kind)
        } else if depth < 3 {
            Theme::TEXT
        } else {
            Theme::MUTED
        };

        let marker = if event.failed {
            Icon::ERROR
        } else if newest && self.running {
            Spinner::new(SpinnerStyle::Braille, self.phase).glyph()
        } else {
            event.kind.glyph()
        };

        let mut style = Style::default().fg(colour);
        if newest {
            style = style.add_modifier(Modifier::BOLD);
        }

        // Two columns go to the marker and its space; the label gets the rest.
        let label = format::fit(&event.label(), width.saturating_sub(2), true);
        Line::from(vec![
            Span::styled(format!("{marker} "), Style::default().fg(colour)),
            Span::styled(label, style),
        ])
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use chrono::Utc;

    use super::*;
    use crate::domain::activity::ToolKind;

    fn event(name: &str, subject: &str, failed: bool) -> ToolEvent {
        ToolEvent {
            at: Utc::now(),
            kind: ToolKind::classify(name),
            name: name.to_owned(),
            subject: subject.to_owned(),
            failed,
            id: name.to_owned(),
        }
    }

    fn render(events: Vec<ToolEvent>, width: u16, height: u16) -> Vec<String> {
        let queue: VecDeque<ToolEvent> = events.into();
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        ToolFeed::new(&queue, false, 0).render(area, &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_newest_call_is_on_the_first_line() {
        let rows = render(
            vec![
                event("Read", "old.rs", false),
                event("Edit", "new.rs", false),
            ],
            30,
            2,
        );
        assert!(rows[0].contains("new.rs"), "got {rows:?}");
        assert!(rows[1].contains("old.rs"), "got {rows:?}");
    }

    #[test]
    fn an_empty_feed_says_so_instead_of_leaving_a_blank_panel() {
        let rows = render(vec![], 40, 2);
        assert!(rows[0].contains("no tool activity"), "got {rows:?}");
    }

    #[test]
    fn a_failed_call_is_marked_with_the_warning_glyph() {
        let rows = render(vec![event("Bash", "false", true)], 30, 1);
        assert!(rows[0].contains(Icon::ERROR), "got {rows:?}");
    }

    #[test]
    fn a_long_label_is_truncated_from_the_left_so_the_file_name_survives() {
        let rows = render(
            vec![event("Read", "/a/very/long/path/to/money.rs", false)],
            20,
            1,
        );
        assert!(rows[0].contains("money.rs"), "got {rows:?}");
    }
}

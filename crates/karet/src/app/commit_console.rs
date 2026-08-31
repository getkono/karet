//! The live console a commit's hooks print into.
//!
//! A commit with hooks is the one repository operation that has something to say
//! while it runs. A formatter rewrites files, a linter objects, a test suite takes
//! half a minute — and until now none of it was visible: the output was discarded
//! and the panel simply sat there saying "committing…".
//!
//! # Why it opens on the first line, not on the commit
//!
//! Most commits have no hooks and print one summary line. Opening a console for
//! those would put a panel in the way of every commit for nothing, so the console
//! opens when there is something to show — which is also, conveniently, the moment
//! a commit turns out to be slow. That is the shared "avoid flashing a loading
//! state on a fast path" rule, met without needing a timer.
//!
//! # Why the log is not inside the overlay
//!
//! Lines keep arriving from the worker whether or not anyone is looking. Keeping
//! them here means dismissing the console cannot lose the rest of a hook's output,
//! and the log can be reopened after the commit has finished.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use karet_core::ThemeRole;
use karet_vcs::CommitOutputLine;
use karet_vcs::OutputStream;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use super::App;

/// Lines kept before the head is dropped.
///
/// A runaway hook can print without bound; the tail is what a reader wants, and
/// the head is replaced by a marker so a truncated log never pretends to be whole.
const MAX_LINES: usize = 5_000;

/// How a commit that produced output ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    /// The commit landed, with its short hash.
    Committed(String),
    /// The commit was refused, with the reason reported alongside it.
    Failed(String),
}

/// The console's log, viewport, and outcome.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitConsole {
    /// Every line received, oldest first, capped at [`MAX_LINES`].
    pub(crate) lines: Vec<CommitOutputLine>,
    /// Whether lines were dropped from the head.
    pub(crate) truncated: bool,
    /// Whether the console is on screen.
    pub(crate) open: bool,
    /// Whether the reader dismissed this commit's console. Output keeps arriving
    /// and is kept, but it does not reopen a view that was deliberately closed.
    dismissed: bool,
    /// First visible row.
    pub(crate) scroll: u16,
    /// First visible column, for a hook line wider than the console.
    pub(crate) column: u16,
    /// Whether the view follows the tail as lines arrive.
    pub(crate) following: bool,
    /// How the commit ended, once it has.
    pub(crate) outcome: Option<CommitOutcome>,
    /// Where the console was last painted, for hit-testing the wheel.
    pub(crate) rect: ratatui::layout::Rect,
}

impl CommitConsole {
    /// Whether there is a log worth reopening.
    pub(crate) fn has_log(&self) -> bool {
        !self.lines.is_empty()
    }

    /// Style each line for `theme`.
    ///
    /// The stream is not a severity — git routes a hook's own stdout to its
    /// stderr, so colouring by stream would paint every successful hook red. Only
    /// git's own summary, which arrives on stdout, is distinguished.
    pub(crate) fn styled(&self, theme: &karet_theme::Theme) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        if self.truncated {
            lines.push(Line::styled(
                format!("… earlier output dropped (over {MAX_LINES} lines)"),
                theme.style(ThemeRole::Muted),
            ));
        }
        lines.extend(self.lines.iter().map(|line| {
            let style = match line.stream {
                OutputStream::Stdout => theme.style(ThemeRole::Muted),
                OutputStream::Stderr => theme.style(ThemeRole::Foreground),
            };
            // Hooks colour their output; honour it, and fall back to the stream's
            // own style for the plain runs.
            let spans: Vec<Span<'static>> = karet_widgets::ansi::ansi_spans(&line.text)
                .into_iter()
                .map(|span| {
                    if span.style == Style::default() {
                        span.style(style)
                    } else {
                        span
                    }
                })
                .collect();
            Line::from(spans)
        }));
        lines
    }
}

impl App {
    /// Take one batch of a running commit's console output.
    pub(crate) fn on_commit_output(&mut self, lines: Vec<CommitOutputLine>) {
        if lines.is_empty() {
            return;
        }
        // The first line of a commit that has something to say is what opens the
        // console — see the module docs. Once, though: a reader who closed it is
        // not asking to be interrupted again by the same commit.
        if !self.commit_console.open
            && !self.commit_console.dismissed
            && self.commit_console.outcome.is_none()
        {
            self.commit_console.open = true;
            self.commit_console.following = true;
        }
        self.commit_console.lines.extend(lines);
        if self.commit_console.lines.len() > MAX_LINES {
            let excess = self.commit_console.lines.len() - MAX_LINES;
            self.commit_console.lines.drain(..excess);
            self.commit_console.truncated = true;
        }
    }

    /// Start a fresh console for a commit that is about to run.
    pub(crate) fn commit_console_reset(&mut self) {
        self.commit_console = CommitConsole::default();
    }

    /// Record how the commit ended.
    ///
    /// A commit that printed nothing never opened a console and does not open one
    /// now: there is nothing in it to read. A failure always shows, because the
    /// reason is the whole point.
    pub(crate) fn commit_console_finished(&mut self, outcome: CommitOutcome) {
        let failed = matches!(outcome, CommitOutcome::Failed(_));
        self.commit_console.outcome = Some(outcome);
        // A refusal overrides a dismissal: a commit that did not happen, and the
        // reason it did not, is not something to have quietly missed.
        if failed && self.commit_console.has_log() {
            self.commit_console.open = true;
        }
    }

    /// Show the last commit's console again, if there is one.
    pub(crate) fn commit_console_reopen(&mut self) {
        if self.commit_console.has_log() {
            self.commit_console.open = true;
            self.commit_console.dismissed = false;
        }
    }

    /// Dismiss the console, keeping its log for a later reopen.
    pub(crate) fn commit_console_dismiss(&mut self) {
        self.commit_console.open = false;
        self.commit_console.dismissed = true;
    }

    /// Move the console's viewport, releasing it from the tail.
    pub(crate) fn commit_console_scroll(&mut self, delta: i32) {
        let position = i64::from(self.commit_console.scroll).saturating_add(i64::from(delta));
        self.commit_console_scroll_to(usize::try_from(position.max(0)).unwrap_or(usize::MAX));
    }

    /// Land the console's viewport on an absolute row.
    pub(crate) fn commit_console_scroll_to(&mut self, position: usize) {
        self.commit_console.scroll = u16::try_from(position).unwrap_or(u16::MAX);
        // A deliberate scroll releases the tail; the console re-engages it only
        // when the reader scrolls back to the bottom, the way a terminal does.
        self.commit_console.following = false;
    }

    /// Land the console's viewport on an absolute column.
    pub(crate) fn commit_console_scroll_columns_to(&mut self, position: usize) {
        self.commit_console.column = u16::try_from(position).unwrap_or(u16::MAX);
    }
}

impl App {
    /// Move through the console with the keys a pager answers to.
    pub(super) fn commit_console_key(&mut self, key: KeyEvent) {
        let page = 10;
        match key.code {
            KeyCode::Up => self.commit_console_scroll(-1),
            KeyCode::Down => self.commit_console_scroll(1),
            KeyCode::PageUp => self.commit_console_scroll(-page),
            KeyCode::PageDown => self.commit_console_scroll(page),
            KeyCode::Home => self.commit_console_scroll_to(0),
            // The end of a running commit's log is wherever it has got to, so
            // following the tail is what "go to the end" means here.
            KeyCode::End => self.commit_console.following = true,
            KeyCode::Left => {
                self.commit_console.column = self.commit_console.column.saturating_sub(4);
            },
            KeyCode::Right => {
                self.commit_console.column = self.commit_console.column.saturating_add(4);
            },
            _ => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(stream: OutputStream, text: &str) -> CommitOutputLine {
        CommitOutputLine {
            stream,
            text: text.to_string(),
        }
    }

    fn fresh() -> App {
        crate::app::tests::support::app()
    }

    #[test]
    fn an_ordinary_commit_never_puts_a_console_in_the_way() {
        let mut app = fresh();
        app.commit_console_reset();
        assert!(!app.commit_console.open, "not on dispatch");
        // A hookless commit prints nothing worth showing.
        app.commit_console_finished(CommitOutcome::Committed("abc1234".to_string()));
        assert!(!app.commit_console.open);
        assert!(!app.commit_console.has_log());
    }

    #[test]
    fn the_first_line_a_hook_prints_is_what_opens_it() {
        let mut app = fresh();
        app.commit_console_reset();
        app.on_commit_output(Vec::new());
        assert!(!app.commit_console.open, "an empty batch is not output");
        app.on_commit_output(vec![line(OutputStream::Stderr, "running checks")]);
        assert!(app.commit_console.open);
        assert!(app.commit_console.following, "and it follows the tail");

        // It stays after the commit lands, with its outcome named.
        app.commit_console_finished(CommitOutcome::Committed("abc1234".to_string()));
        assert!(app.commit_console.open);
        assert_eq!(
            app.commit_console.outcome,
            Some(CommitOutcome::Committed("abc1234".to_string()))
        );
    }

    #[test]
    fn a_refusal_shows_its_log_even_if_the_console_was_dismissed() {
        let mut app = fresh();
        app.commit_console_reset();
        app.on_commit_output(vec![line(OutputStream::Stderr, "why it refused")]);
        app.commit_console_dismiss();
        assert!(!app.commit_console.open);
        app.commit_console_finished(CommitOutcome::Failed("hook failed".to_string()));
        assert!(app.commit_console.open, "the reason is the whole point");
    }

    #[test]
    fn dismissing_keeps_the_log_for_a_later_reopen() {
        let mut app = fresh();
        app.commit_console_reset();
        app.on_commit_output(vec![line(OutputStream::Stdout, "one")]);
        app.commit_console_dismiss();
        // Lines still arriving from the worker are not lost with the view.
        app.on_commit_output(vec![line(OutputStream::Stdout, "two")]);
        assert_eq!(app.commit_console.lines.len(), 2);
        assert!(!app.commit_console.open, "and it stays dismissed");
        app.commit_console_reopen();
        assert!(app.commit_console.open);

        // Nothing to reopen when nothing was logged.
        let mut app = fresh();
        app.commit_console_reset();
        app.commit_console_reopen();
        assert!(!app.commit_console.open);
    }

    #[test]
    fn a_runaway_hook_keeps_the_tail_and_admits_the_truncation() {
        let mut app = fresh();
        app.commit_console_reset();
        let flood: Vec<CommitOutputLine> = (0..MAX_LINES + 10)
            .map(|n| line(OutputStream::Stderr, &format!("line {n}")))
            .collect();
        app.on_commit_output(flood);
        assert_eq!(app.commit_console.lines.len(), MAX_LINES);
        assert!(app.commit_console.truncated);
        assert_eq!(
            app.commit_console
                .lines
                .first()
                .map(|line| line.text.clone()),
            Some("line 10".to_string()),
            "the head is dropped, not the tail"
        );
        let styled = app.commit_console.styled(&app.theme);
        assert!(
            styled
                .first()
                .is_some_and(|line| line.to_string().contains("dropped")),
            "and a truncated log says so"
        );
    }

    #[test]
    fn scrolling_releases_the_tail_and_the_end_key_takes_it_back() {
        let mut app = fresh();
        app.commit_console_reset();
        app.on_commit_output(vec![line(OutputStream::Stderr, "one")]);
        assert!(app.commit_console.following);
        app.commit_console_scroll(-1);
        assert!(!app.commit_console.following, "a deliberate scroll wins");
        app.commit_console_key(crossterm::event::KeyEvent::from(KeyCode::End));
        assert!(app.commit_console.following);
    }

    #[test]
    fn a_hooks_colours_survive_but_the_stream_is_not_a_severity() {
        let mut app = fresh();
        app.commit_console_reset();
        app.on_commit_output(vec![
            line(OutputStream::Stderr, "\u{1b}[31mred\u{1b}[0m plain"),
            line(OutputStream::Stdout, "summary"),
        ]);
        let styled = app.commit_console.styled(&app.theme);
        assert_eq!(styled.len(), 2);
        assert_eq!(styled[0].to_string(), "red plain", "the escapes are parsed");
        // Everything a hook prints arrives on stderr, so that stream carries the
        // ordinary foreground rather than an error colour.
        let error = app.theme.style(ThemeRole::DiagnosticError);
        assert!(
            styled[1].spans.iter().all(|span| span.style != error),
            "git's own summary is not an error either"
        );
    }
}

//! Tasks: the `tasks` settings table behind Tasks: Run Task.
//!
//! The picker lists every configured task by label, its command beside it.
//! Picking one runs its command line through the platform shell in a new
//! integrated terminal tab, in the task's working directory with its
//! environment. While it runs, and once more when it exits, its problem
//! matchers read the terminal's whole output (history included) and what they
//! find replaces that task's entries in the Problems panel; the entries stay
//! until the task runs again, or until settings no longer define it.
//!
//! A task's terminal is scanned whether or not any pane shows it: each scan
//! acknowledges the terminal's output itself, so a run in a background tab
//! keeps announcing new output, and output the throttle holds back is scanned
//! at a scheduled wake rather than waiting for more of it.
//!
//! Running a task whose last run is still going shows that run instead of
//! starting a second; a finished run's tab is closed and replaced, so repeated
//! runs of a build do not pile up tabs.

#[cfg(feature = "terminal")]
mod matcher;

use karet_core::NotificationKind;

use crate::app::App;
use crate::app::notifications::Report;
use crate::overlay::Overlay;
use crate::overlay::OverlayEvent;
use crate::overlay::Picker;
use crate::overlay::PickerKind;

/// The Run Task picker's title.
const RUN_TASK_TITLE: &str = "Run Task";

impl App {
    /// Tasks: Run Task: offer the configured tasks, or say how to add one.
    pub(super) fn open_task_picker(&mut self) {
        if self.settings.tasks.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "no tasks configured: add a `tasks` table to setting.jsonc",
            );
            return;
        }
        let items = self
            .settings
            .tasks
            .iter()
            .map(|(label, task)| {
                let event = OverlayEvent::AcceptTask {
                    label: label.clone(),
                    command: task.command.clone(),
                };
                (label.clone(), event)
            })
            .collect();
        self.overlay = Some(Overlay::picker(
            PickerKind::Tasks,
            Picker::new(RUN_TASK_TITLE, items),
        ));
    }

    /// A lean build has no integrated terminal for a task to run in.
    #[cfg(not(feature = "terminal"))]
    pub(super) fn run_task(&mut self, _label: &str) {
        self.notify(
            Report::Failure,
            NotificationKind::System,
            "tasks run in the integrated terminal, which this build lacks (built without the \
             `terminal` feature)",
        );
    }
}

#[cfg(feature = "terminal")]
pub(crate) use running::TaskRuns;

#[cfg(feature = "terminal")]
mod running {
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::Instant;

    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Column;
    use alacritty_terminal::index::Line;
    use alacritty_terminal::term::Term;
    use alacritty_terminal::term::cell::Flags;
    use karet_core::NotificationKind;
    use karet_session::config::schema::Task;
    use portable_pty::CommandBuilder;

    use super::matcher;
    use crate::app::App;
    use crate::app::notifications::Report;
    use crate::tab::Tab;
    use crate::tab::TabKind;
    use crate::terminal::TerminalId;

    /// The least time between two scans of a running task's output. Output
    /// arriving sooner schedules a scan for when the interval is up
    /// ([`App::task_next_wake`]), so the last word is never lost to the throttle.
    pub(super) const LIVE_SCAN_INTERVAL: Duration = Duration::from_millis(500);

    /// One task run, keyed by the terminal it runs in.
    #[derive(Debug)]
    pub(super) struct TaskRun {
        label: String,
        cwd: PathBuf,
        matchers: Vec<matcher::Matcher>,
        exited: bool,
        scanned: Option<Instant>,
        /// When output the throttle held back is to be scanned.
        due: Option<Instant>,
    }

    /// The task runs whose terminals may still be open.
    #[derive(Debug, Default)]
    pub(crate) struct TaskRuns {
        pub(super) runs: HashMap<TerminalId, TaskRun>,
    }

    impl App {
        /// Run the configured task `label` in a new terminal tab.
        pub(in crate::app) fn run_task(&mut self, label: &str) {
            let Some(task) = self.settings.tasks.get(label).cloned() else {
                self.notify(
                    Report::Refusal,
                    NotificationKind::System,
                    format!("no task named `{label}` (settings changed?)"),
                );
                return;
            };
            if let Err(error) = self.start_task(label, &task) {
                self.notify(
                    Report::Failure,
                    NotificationKind::System,
                    format!("task `{label}`: {error}"),
                );
            }
        }

        fn start_task(&mut self, label: &str, task: &Task) -> Result<(), String> {
            if task.command.trim().is_empty() {
                return Err("it has no `command`".to_owned());
            }
            let matchers = matcher::compile(&task.problem_matcher)?;
            let cwd = match task.cwd.trim() {
                "" => self.root.clone(),
                cwd => self.root.join(cwd),
            };
            if !cwd.is_dir() {
                return Err(format!("no directory {}", cwd.display()));
            }
            // Forget runs whose terminal was closed since.
            let open: Vec<TerminalId> = self
                .all_tabs()
                .filter_map(|tab| match &tab.kind {
                    TabKind::Terminal(pane) => Some(pane.id()),
                    _ => None,
                })
                .collect();
            self.tasks.runs.retain(|id, _| open.contains(id));
            let previous = self
                .tasks
                .runs
                .iter()
                .find(|(_, run)| run.label == label)
                .map(|(id, run)| (*id, run.exited));
            if let Some((id, false)) = previous
                && let Some((pane, index)) = self.terminal_tab(id)
            {
                self.focus_pane_switch(pane);
                self.select_tab(index);
                self.notify(
                    Report::Refusal,
                    NotificationKind::System,
                    format!("task `{label}` is already running (close its terminal to stop it)"),
                );
                return Ok(());
            }
            // Started before the finished run's tab goes, so a run that cannot
            // start leaves that output, and its problems, where they were.
            let (columns, rows) = self.terminal_grid_size();
            let mut pane =
                self.terminals
                    .spawn_command(shell_command(task, &cwd), columns, rows)?;
            pane.set_title(Some(&format!("Task: {label}")));
            if let Some((id, _)) = previous
                && let Some((pane, index)) = self.terminal_tab(id)
            {
                self.focus_pane_switch(pane);
                self.close_tab_at(index);
                self.tasks.runs.remove(&id);
            }
            self.set_task_problems(label, Vec::new());
            self.tasks.runs.insert(
                pane.id(),
                TaskRun {
                    label: label.to_owned(),
                    cwd,
                    matchers,
                    exited: false,
                    scanned: None,
                    due: None,
                },
            );
            self.push_tab(Tab::terminal(pane));
            Ok(())
        }

        /// New output from terminal `id`: rescan it when it is a task's, at most
        /// once per [`LIVE_SCAN_INTERVAL`] while it runs; output arriving sooner
        /// schedules the scan for when the interval is up. After the exit every
        /// batch rescans, since it is output the exit's own scan may have missed.
        ///
        /// Returns whether a scan ran and changed the task's problems, so the
        /// Problems panel and the status bar repaint even when no pane shows
        /// the terminal.
        #[must_use]
        pub(in crate::app) fn task_output(&mut self, id: TerminalId, now: Instant) -> bool {
            let Some(run) = self.tasks.runs.get_mut(&id) else {
                return false;
            };
            let next = run.scanned.map(|scanned| scanned + LIVE_SCAN_INTERVAL);
            match next {
                Some(next) if !run.exited && now < next => {
                    run.due.get_or_insert(next);
                    false
                },
                _ => self.scan_task(id, now) == Some(true),
            }
        }

        /// How long until the soonest scan the throttle scheduled, if any.
        pub(in crate::app) fn task_next_wake(&self, now: Instant) -> Option<Duration> {
            self.tasks
                .runs
                .values()
                .filter_map(|run| run.due)
                .min()
                .map(|due| due.saturating_duration_since(now))
        }

        /// Run every scan the throttle scheduled for `now` or earlier, and say
        /// whether any of them changed its task's problems (the Problems panel
        /// and status bar then need a repaint).
        #[must_use]
        pub(in crate::app) fn fire_task_scans(&mut self, now: Instant) -> bool {
            let due: Vec<TerminalId> = self
                .tasks
                .runs
                .iter()
                .filter(|(_, run)| run.due.is_some_and(|due| due <= now))
                .map(|(id, _)| *id)
                .collect();
            let mut changed = false;
            for id in due {
                changed |= self.scan_task(id, now) == Some(true);
            }
            changed
        }

        /// Settings were applied: forget the runs and problems of tasks they no
        /// longer define. A forgotten run's terminal stays open as a plain one.
        /// A `tasks` section that failed to load says nothing about which tasks
        /// exist, so a malformed save keeps every run and its problems.
        pub(in crate::app) fn forget_removed_tasks(&mut self) {
            if self.loaded_config.failed_sections.contains("tasks") {
                return;
            }
            let tasks = &self.settings.tasks;
            self.tasks
                .runs
                .retain(|_, run| tasks.contains_key(&run.label));
            let before = self.problems.task_problems.len();
            self.problems
                .task_problems
                .retain(|label, _| tasks.contains_key(label));
            if self.problems.task_problems.len() != before {
                self.refresh_problems();
            }
        }

        /// Terminal `id` exited with `status`: when it ran a task, scan its
        /// output a last time and say how it went.
        pub(in crate::app) fn task_exited(&mut self, id: TerminalId, status: Option<&str>) {
            let Some(run) = self.tasks.runs.get_mut(&id) else {
                return;
            };
            run.exited = true;
            let label = run.label.clone();
            if self.scan_task(id, Instant::now()).is_none() {
                return;
            }
            let count = self.problems.task_problems.get(&label).map_or(0, Vec::len);
            let status = status.unwrap_or("exited");
            let report = if status == "exited with code 0" {
                Report::Outcome
            } else {
                Report::Failure
            };
            let problems = match count {
                0 => String::new(),
                1 => ", 1 problem".to_owned(),
                n => format!(", {n} problems"),
            };
            self.notify(
                report,
                NotificationKind::System,
                format!("task `{label}` {status}{problems}"),
            );
        }

        /// Match task terminal `id`'s whole output and publish what was found.
        /// Returns whether the task's problems changed, or `None`, forgetting
        /// the run, once its terminal is closed.
        ///
        /// The scan acknowledges the terminal's output, as painting it would, so
        /// the terminal announces its next batch even when no pane shows it.
        fn scan_task(&mut self, id: TerminalId, now: Instant) -> Option<bool> {
            let lines = self.all_tabs().find_map(|tab| match &tab.kind {
                TabKind::Terminal(pane) if pane.id() == id => {
                    // Before reading, so output parsed after the read is announced.
                    pane.acknowledge_output();
                    Some(transcript(&pane.term()))
                },
                _ => None,
            });
            let Some(lines) = lines else {
                self.tasks.runs.remove(&id);
                return None;
            };
            let run = self.tasks.runs.get_mut(&id)?;
            run.scanned = Some(now);
            run.due = None;
            let context = matcher::ScanContext {
                cwd: &run.cwd,
                root: &self.root,
                source: &run.label,
            };
            let found = matcher::scan(&run.matchers, &lines, &context);
            let label = run.label.clone();
            let before = self.problems.task_problems.get(&label);
            let changed = before.map_or(&[][..], Vec::as_slice) != found.as_slice();
            self.set_task_problems(&label, found);
            Some(changed)
        }
    }

    /// `task`'s command line under the platform shell, in `cwd`, with its
    /// environment over the editor's.
    fn shell_command(task: &Task, cwd: &Path) -> CommandBuilder {
        #[cfg(windows)]
        let mut command = {
            let mut command = CommandBuilder::new("cmd.exe");
            command.args(["/C", &task.command]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.args(["-c", &task.command]);
            command
        };
        command.cwd(cwd);
        for (key, value) in &task.env {
            command.env(key, value);
        }
        command
    }

    /// Every line of `term`'s output, history first, with the rows a long line
    /// soft-wrapped onto joined back into it and trailing blanks dropped.
    pub(super) fn transcript<T>(term: &Term<T>) -> Vec<String> {
        let grid = term.grid();
        let columns = grid.columns();
        let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        let top = -i32::try_from(grid.history_size()).unwrap_or(i32::MAX);
        let bottom = i32::try_from(grid.screen_lines()).unwrap_or(i32::MAX);
        let mut lines = Vec::new();
        let mut current = String::new();
        for line in top..bottom {
            let row = &grid[Line(line)];
            for column in 0..columns {
                let cell = &row[Column(column)];
                if !cell.flags.intersects(spacer) {
                    current.push(cell.c);
                }
            }
            let wrapped = columns > 0 && row[Column(columns - 1)].flags.contains(Flags::WRAPLINE);
            if !wrapped {
                lines.push(current.trim_end().to_owned());
                current.clear();
            }
        }
        if !current.is_empty() {
            lines.push(current.trim_end().to_owned());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }
}

#[cfg(all(test, feature = "terminal"))]
use running::transcript;

#[cfg(all(test, feature = "terminal"))]
mod tests;

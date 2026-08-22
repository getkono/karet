//! Debugger UI glue: run controls, breakpoint toggling, and the `Debug*`
//! event handling (state segment, stop-location jump, output buffering).

use karet_session::Command as SessionCommand;
use karet_session::DebugBreakpoint;
use karet_session::DebugSessionState;

use super::*;

/// How many console lines the app buffers for the Debug panel.
const DEBUG_OUTPUT_CAP: usize = 500;

impl App {
    /// F5: start a session when idle, continue when stopped.
    pub(super) fn debug_start_or_continue(&mut self) {
        match self.debug_state {
            DebugSessionState::Stopped => self.debug_send(SessionCommand::DebugContinue),
            DebugSessionState::Idle => {
                self.debug_send(SessionCommand::DebugStart {
                    configuration: None,
                });
            },
            DebugSessionState::Starting | DebugSessionState::Running => {
                self.status = Some("debug session already running (Shift+F5 stops)".to_owned());
            },
            // Non-exhaustive: anything newer behaves like running.
            _ => {},
        }
    }

    /// A step control is only meaningful while stopped; nudge otherwise.
    pub(super) fn debug_step(&mut self, command: SessionCommand) {
        if self.debug_state == DebugSessionState::Stopped {
            self.debug_send(command);
        } else {
            self.status = Some("the debuggee is not stopped".to_owned());
        }
    }

    /// Send one debugger command to the backend.
    pub(super) fn debug_send(&mut self, command: SessionCommand) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        let _ = backend.send(id, command);
    }

    /// F9: toggle a breakpoint on the caret line of the active code tab.
    pub(super) fn debug_toggle_breakpoint(&mut self) {
        let Some((path, line)) = self.tabs.get(self.active).and_then(|tab| {
            let path = tab.path()?.to_path_buf();
            Some((path, tab.editor.cursor().line))
        }) else {
            self.status = Some("breakpoints need a file tab".to_owned());
            return;
        };
        self.debug_toggle_breakpoint_at(path, line);
    }

    /// Toggle `line` of `path` and push the file's full set to the backend
    /// (`setBreakpoints` is full-replace per file).
    pub(super) fn debug_toggle_breakpoint_at(&mut self, path: PathBuf, line: u32) {
        let file = self.breakpoints.entry(path.clone()).or_default();
        if file.remove(&line).is_none() {
            file.insert(line, false);
        }
        let lines: Vec<u32> = file.keys().copied().collect();
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        }
        self.debug_send(SessionCommand::DebugSetBreakpoints { path, lines });
    }

    /// `Event::DebugState`: mirror the lifecycle and surface transitions.
    pub(super) fn on_debug_state(&mut self, state: DebugSessionState, detail: String) {
        let was = self.debug_state;
        self.debug_state = state;
        match state {
            DebugSessionState::Idle if !detail.is_empty() => {
                self.status = Some(format!("debug: {detail}"));
            },
            DebugSessionState::Running if was == DebugSessionState::Starting => {
                self.status = Some(format!("debugging {detail}"));
            },
            _ => {},
        }
        self.debug_detail = detail;
    }

    /// `Event::DebugStopped`: remember the thread context and jump to the
    /// stop location when the adapter reported one.
    pub(super) fn on_debug_stopped(
        &mut self,
        reason: &str,
        path: Option<PathBuf>,
        line: Option<u32>,
    ) {
        self.status = Some(format!("stopped: {reason}"));
        if let (Some(path), Some(line)) = (path, line) {
            self.jump_to_location(&path, LineCol::new(line, 0));
        }
    }

    /// `Event::DebugOutput`: buffer for the Debug panel, capped.
    pub(super) fn on_debug_output(&mut self, category: String, text: String) {
        for line in text.lines() {
            self.debug_output
                .push_back((category.clone(), line.to_owned()));
        }
        while self.debug_output.len() > DEBUG_OUTPUT_CAP {
            self.debug_output.pop_front();
        }
    }

    /// `Event::DebugBreakpoints`: adopt the adapter's acknowledgement. A full
    /// answer replaces the file's set; a late single-entry verification event
    /// merges by line.
    pub(super) fn on_debug_breakpoints(&mut self, path: PathBuf, acked: &[DebugBreakpoint]) {
        let file = self.breakpoints.entry(path.clone()).or_default();
        if acked.len() >= file.len() {
            file.clear();
        }
        for bp in acked {
            file.insert(bp.line, bp.verified);
        }
        if file.is_empty() {
            self.breakpoints.remove(&path);
        }
    }

    /// The status-bar debug segment, `None` while idle.
    pub(crate) fn debug_status_segment(&self) -> Option<String> {
        match self.debug_state {
            DebugSessionState::Idle => None,
            DebugSessionState::Starting => Some("⏳ debug".to_owned()),
            DebugSessionState::Running => Some("▶ debug".to_owned()),
            DebugSessionState::Stopped => Some(format!("⏸ {}", self.debug_detail)),
            _ => None,
        }
    }
}

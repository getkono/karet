//! Debugger UI glue: run controls, breakpoint toggling, and the `Debug*`
//! event handling (state segment, stop-location jump, output buffering).

use karet_session::Command as SessionCommand;
use karet_session::DebugBreakpoint;
use karet_session::DebugFrame;
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
        // Leaving the stopped state invalidates every inspection artifact.
        if state != DebugSessionState::Stopped && was == DebugSessionState::Stopped {
            self.debug_stopped = None;
            self.debug_panel.clear_inspection(self.debug_output.len());
        }
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
        self.debug_panel.clear_inspection(self.debug_output.len());
        self.debug_stopped = path.clone().zip(line);
        if let (Some(path), Some(line)) = (path, line) {
            self.jump_to_location(&path, LineCol::new(line, 0));
        }
        // Populate the panel for this stop.
        self.debug_request(SessionCommand::DebugStackTrace);
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
        self.debug_panel.rebuild_rows(self.debug_output.len());
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

    /// Move the Debug panel cursor.
    pub(super) fn debug_select(&mut self, delta: i32) {
        self.debug_panel.selection.move_by(delta);
    }

    /// Route a click inside the Debug panel to its row (select + activate).
    pub(super) fn debug_click(&mut self, col: u16, row_y: u16) {
        if !rect_contains(self.debug_ui.results_rect, (col, row_y)) {
            return;
        }
        let index = self.debug_ui.offset + (row_y - self.debug_ui.results_rect.y) as usize;
        if index < self.debug_panel.rows.len() {
            self.debug_panel.selection.move_to(index);
            self.debug_activate_row();
        }
    }

    /// Enter/click on a Debug panel row: frames select-and-jump, expandable
    /// nodes toggle (fetching children lazily on first expand).
    pub(super) fn debug_activate_row(&mut self) {
        let Some(&row) = self
            .debug_panel
            .rows
            .get(self.debug_panel.selection.cursor())
        else {
            return;
        };
        match row {
            DebugRow::Frame(index) => {
                let Some(frame) = self.debug_panel.stack.get(index) else {
                    return;
                };
                let (id, line, path) = (frame.id, frame.line, frame.path.clone());
                self.debug_panel.selected_frame = Some(id);
                self.debug_panel.scopes.clear();
                self.debug_panel.variables.clear();
                self.debug_panel.expanded.clear();
                if let Some(path) = path {
                    self.jump_to_location(&path, LineCol::new(line, 0));
                }
                self.debug_request(SessionCommand::DebugScopes { frame: id });
                self.debug_panel.rebuild_rows(self.debug_output.len());
            },
            DebugRow::Scope(index) => {
                let Some(reference) = self
                    .debug_panel
                    .scopes
                    .get(index)
                    .map(|scope| scope.reference)
                else {
                    return;
                };
                self.debug_toggle_expand(reference);
            },
            DebugRow::Variable { parent, index, .. } => {
                let reference = self
                    .debug_panel
                    .variables
                    .get(&parent)
                    .and_then(|children| children.get(index))
                    .map_or(0, |child| child.reference);
                if reference > 0 {
                    self.debug_toggle_expand(reference);
                }
            },
            _ => {},
        }
    }

    /// Toggle one reference's expansion, fetching its children on first use.
    fn debug_toggle_expand(&mut self, reference: i64) {
        if self.debug_panel.expanded.remove(&reference) {
            self.debug_panel.rebuild_rows(self.debug_output.len());
            return;
        }
        self.debug_panel.expanded.insert(reference);
        if !self.debug_panel.variables.contains_key(&reference) {
            self.debug_request(SessionCommand::DebugVariables { reference });
        }
        self.debug_panel.rebuild_rows(self.debug_output.len());
    }

    /// Send an inspection command, remembering its id so the answer is
    /// accepted (anything else is a stale reply from before a resume).
    fn debug_request(&mut self, command: SessionCommand) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        if backend.send(id, command).is_ok() {
            self.debug_panel.pending.insert(id);
        }
    }

    /// Open the evaluate prompt (palette: Debug: Evaluate Expression).
    pub(super) fn debug_evaluate_prompt(&mut self) {
        if self.debug_state == DebugSessionState::Idle {
            self.status = Some("no debug session".to_owned());
            return;
        }
        self.overlay = Some(crate::overlay::Overlay::text(
            "Evaluate in debuggee",
            crate::overlay::TextPurpose::DebugEvaluate,
        ));
    }

    /// Submit an evaluate expression from the prompt.
    pub(super) fn debug_evaluate(&mut self, expression: String) {
        let expression = expression.trim().to_owned();
        if expression.is_empty() {
            return;
        }
        self.debug_panel.repl.push(format!("› {expression}"));
        self.debug_request(SessionCommand::DebugEvaluate {
            expression,
            frame: self.debug_panel.selected_frame,
        });
        self.debug_panel.rebuild_rows(self.debug_output.len());
    }

    /// `Event::DebugStack`: adopt the frames, auto-select the top frame, and
    /// fetch its scopes (VS Code's behavior on stop).
    pub(super) fn on_debug_stack(&mut self, id: Option<RequestId>, frames: Vec<DebugFrame>) {
        if !id.is_some_and(|id| self.debug_panel.pending.remove(&id)) {
            return;
        }
        self.debug_panel.stack = frames;
        if let Some(top) = self.debug_panel.stack.first() {
            let frame = top.id;
            self.debug_panel.selected_frame = Some(frame);
            self.debug_request(SessionCommand::DebugScopes { frame });
        }
        self.debug_panel.rebuild_rows(self.debug_output.len());
    }

    /// `Event::DebugScopes`: adopt, auto-expanding the first cheap scope.
    pub(super) fn on_debug_scopes(
        &mut self,
        id: Option<RequestId>,
        frame: i64,
        scopes: Vec<karet_session::DebugScope>,
    ) {
        if !id.is_some_and(|id| self.debug_panel.pending.remove(&id))
            || self.debug_panel.selected_frame != Some(frame)
        {
            return;
        }
        self.debug_panel.scopes = scopes;
        if let Some(first) = self
            .debug_panel
            .scopes
            .iter()
            .find(|scope| !scope.expensive)
            .map(|scope| scope.reference)
        {
            self.debug_panel.expanded.insert(first);
            self.debug_request(SessionCommand::DebugVariables { reference: first });
        }
        self.debug_panel.rebuild_rows(self.debug_output.len());
    }

    /// `Event::DebugVariables`: adopt one reference's children.
    pub(super) fn on_debug_variables(
        &mut self,
        id: Option<RequestId>,
        reference: i64,
        variables: Vec<karet_session::DebugVariable>,
    ) {
        if !id.is_some_and(|id| self.debug_panel.pending.remove(&id)) {
            return;
        }
        self.debug_panel.variables.insert(reference, variables);
        self.debug_panel.rebuild_rows(self.debug_output.len());
    }

    /// `Event::DebugEvaluated`: append the result to the evaluate log.
    pub(super) fn on_debug_evaluated(&mut self, id: Option<RequestId>, result: String) {
        if !id.is_some_and(|id| self.debug_panel.pending.remove(&id)) {
            return;
        }
        self.debug_panel.repl.push(format!("  = {result}"));
        self.debug_panel.rebuild_rows(self.debug_output.len());
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

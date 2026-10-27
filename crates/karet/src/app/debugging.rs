//! Debugger UI glue: run controls, breakpoint toggling, and the `Debug*`
//! event handling (state segment, stop-location jump, output buffering), with
//! the [`Debugger`] state they act on and the Debug panel it renders.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;

use karet_core::DebugOutputCategory;
use karet_core::DebugScope;
use karet_core::DebugVariable;
use karet_core::FrameId;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Severity;
use karet_core::StackFrame;
use karet_core::VariablesRef;
use karet_session::Command as SessionCommand;
use karet_session::DebugBreakpoint;
use karet_session::DebugCommand;
use karet_session::DebugSessionState;
use karet_session::NotebookCommand;
use karet_session::RequestId;
use karet_widgets::ListSelection;

use crate::app::App;
use crate::app::notifications::Report;
use crate::app::state::DebugChrome;
use crate::app::util::rect_contains;

/// How many console lines the app buffers for the Debug panel.
const DEBUG_OUTPUT_CAP: usize = 500;

/// The debug session as the app sees it: its lifecycle, the console it
/// printed, the Debug panel that shows both, and where the debuggee stopped.
/// Breakpoints are not here: they are armed before any session starts and
/// outlive every one.
#[derive(Default)]
pub(crate) struct Debugger {
    /// The session's lifecycle, mirrored from `Event::DebugState`.
    pub(crate) state: DebugSessionState,
    /// The status-line detail for the current state.
    pub(crate) detail: String,
    /// Buffered console output (ANSI kept; rendered by the Debug panel).
    pub(crate) output: VecDeque<(DebugOutputCategory, String)>,
    /// The Debug sidebar panel (stack, variables tree, evaluate log).
    pub(crate) panel: DebugPanel,
    /// The Debug panel's last-frame hit-test data.
    pub(crate) chrome: DebugChrome,
    /// Where the debuggee is stopped, for the stopped-line tint.
    pub(crate) stopped: Option<(PathBuf, u32)>,
}

impl Debugger {
    /// Rebuild the panel's rows over the current console tail.
    pub(crate) fn rebuild_rows(&mut self) {
        self.panel.rebuild_rows(self.output.len());
    }

    /// Drop every per-stop artifact from the panel (see
    /// [`DebugPanel::clear_inspection`]), keeping the console tail.
    pub(crate) fn clear_inspection(&mut self) {
        self.panel.clear_inspection(self.output.len());
    }
}

impl App {
    /// F5: start a session when idle, continue when stopped.
    pub(super) fn debug_start_or_continue(&mut self) {
        match self.debugger.state {
            DebugSessionState::Stopped => self.debug_send(DebugCommand::Continue),
            DebugSessionState::Idle => {
                self.debug_send(DebugCommand::Start {
                    configuration: None,
                });
            },
            DebugSessionState::Starting | DebugSessionState::Running => {
                self.notify(
                    Report::Refusal,
                    NotificationKind::System,
                    "debug session already running (Shift+F5 stops)",
                );
            },
            // Non-exhaustive: anything newer behaves like running.
            _ => {},
        }
    }

    /// A step control is only meaningful while stopped; nudge otherwise.
    pub(super) fn debug_step(&mut self, command: DebugCommand) {
        if self.debugger.state == DebugSessionState::Stopped {
            self.debug_send(command);
        } else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "the debuggee is not stopped",
            );
        }
    }

    /// Send one debugger command to the backend.
    pub(super) fn debug_send(&mut self, command: impl Into<SessionCommand>) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        let _ = backend.send(id, command.into());
    }

    /// F9: toggle a breakpoint on the caret line of the active code tab.
    pub(super) fn debug_toggle_breakpoint(&mut self) {
        let Some((path, line)) = self.tabs.get(self.active).and_then(|tab| {
            let path = tab.path()?.to_path_buf();
            Some((path, tab.editor.cursor().line))
        }) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "breakpoints need a file tab",
            );
            return;
        };
        self.debug_toggle_breakpoint_at(path, line);
    }

    /// Toggle `line` of `path` and push the file's full set to the backend
    /// (`setBreakpoints` is full-replace per file).
    ///
    /// Only a file a debugger can stop in takes one: source code or a shell
    /// script. Anywhere else (Markdown, plain text, data) it is refused.
    pub(super) fn debug_toggle_breakpoint_at(&mut self, path: PathBuf, line: u32) {
        use karet_filetype::Category;
        if !matches!(
            karet_filetype::category_for_path(&path),
            Category::Code | Category::Shell
        ) {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "breakpoints need a source file",
            );
            return;
        }
        let file = self.breakpoints.entry(path.clone()).or_default();
        if file.remove(&line).is_none() {
            file.insert(line, false);
        }
        let lines: Vec<u32> = file.keys().copied().collect();
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        }
        self.debug_send(DebugCommand::SetBreakpoints { path, lines });
    }

    /// `Event::DebugState`: mirror the lifecycle and surface transitions.
    pub(super) fn on_debug_state(
        &mut self,
        state: DebugSessionState,
        severity: Severity,
        detail: String,
    ) {
        let was = self.debugger.state;
        self.debugger.state = state;
        // Leaving the stopped state invalidates every inspection artifact.
        if state != DebugSessionState::Stopped && was == DebugSessionState::Stopped {
            self.debugger.stopped = None;
            self.debugger.clear_inspection();
        }
        // The backend states how loud each transition is, so a failed start is a
        // failure and every ordinary end is an outcome without anyone reading the
        // detail's wording. Idle with no detail is the resting state, not an
        // event, so it stays silent; a step within a running session reports
        // through `DebugStopped` instead.
        match state {
            DebugSessionState::Idle if !detail.is_empty() => {
                self.notify_tagged(
                    Report::from_severity(severity),
                    NotificationKind::System,
                    format!("debug: {detail}"),
                    Some(Self::DEBUG_SESSION_TAG.to_string()),
                );
            },
            // The adapter can also report `Running` with no detail (it resumed),
            // and `forward_events` is live before `start` returns, so that can
            // land while the session is still starting. Naming the configuration
            // is the point of this card, so an empty detail is not one.
            DebugSessionState::Running
                if was == DebugSessionState::Starting && !detail.is_empty() =>
            {
                self.notify_tagged(
                    Report::from_severity(severity),
                    NotificationKind::System,
                    format!("debugging {detail}"),
                    Some(Self::DEBUG_SESSION_TAG.to_string()),
                );
            },
            _ => {},
        }
        self.debugger.detail = detail;
    }

    /// `Event::DebugStopped`: remember the thread context and jump to the
    /// stop location when the adapter reported one.
    pub(super) fn on_debug_stopped(
        &mut self,
        reason: &str,
        path: Option<PathBuf>,
        line: Option<u32>,
    ) {
        // Tagged: `reason` is "step" for every stepped line, so an untagged card
        // would stack one per keypress while the user walks through a function.
        self.notify_tagged(
            Report::Outcome,
            NotificationKind::System,
            format!("stopped: {reason}"),
            Some(Self::DEBUG_SESSION_TAG.to_string()),
        );
        self.debugger.clear_inspection();
        self.debugger.stopped = path.clone().zip(line);
        if let (Some(path), Some(line)) = (path, line) {
            // Not `jump_to_location`: stepping line by line is not a trail of
            // jumps. Observed navigation still records a stop in another file or
            // a far-away line, so Go Back returns from wherever execution went.
            self.focus_by_file_line(&path, LineCol::new(line, 0), true);
        }
        // Populate the panel for this stop.
        self.debug_request(DebugCommand::StackTrace);
    }

    /// `Event::DebugOutput`: buffer for the Debug panel, capped.
    pub(super) fn on_debug_output(&mut self, category: DebugOutputCategory, text: String) {
        for line in text.lines() {
            self.debugger
                .output
                .push_back((category.clone(), line.to_owned()));
        }
        while self.debugger.output.len() > DEBUG_OUTPUT_CAP {
            self.debugger.output.pop_front();
        }
        self.debugger.rebuild_rows();
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
        self.debugger.panel.selection.move_by(delta);
    }

    /// Route a click inside the Debug panel to its row (select + activate).
    pub(super) fn debug_click(&mut self, col: u16, row_y: u16) {
        if !rect_contains(self.debugger.chrome.results_rect, (col, row_y)) {
            return;
        }
        let index =
            self.debugger.chrome.offset + (row_y - self.debugger.chrome.results_rect.y) as usize;
        if index < self.debugger.panel.rows.len() {
            self.debugger.panel.selection.move_to(index);
            self.debug_activate_row();
        }
    }

    /// Enter/click on a Debug panel row: frames select-and-jump, expandable
    /// nodes toggle (fetching children lazily on first expand).
    pub(super) fn debug_activate_row(&mut self) {
        let Some(&row) = self
            .debugger
            .panel
            .rows
            .get(self.debugger.panel.selection.cursor())
        else {
            return;
        };
        match row {
            DebugRow::Frame(index) => {
                let Some(frame) = self.debugger.panel.stack.get(index) else {
                    return;
                };
                let (id, line, path) = (frame.id, frame.line, frame.source_path.clone());
                self.debugger.panel.selected_frame = Some(id);
                self.debugger.panel.scopes.clear();
                self.debugger.panel.variables.clear();
                self.debugger.panel.expanded.clear();
                if let Some(path) = path {
                    self.jump_to_location(&path, LineCol::new(line, 0));
                }
                self.debug_request(DebugCommand::Scopes { frame: id });
                self.debugger.rebuild_rows();
            },
            DebugRow::Scope(index) => {
                let Some(reference) = self
                    .debugger
                    .panel
                    .scopes
                    .get(index)
                    .map(|scope| scope.variables_reference)
                else {
                    return;
                };
                self.debug_toggle_expand(reference);
            },
            DebugRow::Variable { parent, index, .. } => {
                let reference = self
                    .debugger
                    .panel
                    .variables
                    .get(&parent)
                    .and_then(|children| children.get(index))
                    .map_or(VariablesRef::NONE, |child| child.variables_reference);
                if reference.has_children() {
                    self.debug_toggle_expand(reference);
                }
            },
            _ => {},
        }
    }

    /// Toggle one reference's expansion, fetching its children on first use.
    fn debug_toggle_expand(&mut self, reference: VariablesRef) {
        if self.debugger.panel.expanded.remove(&reference) {
            self.debugger.rebuild_rows();
            return;
        }
        self.debugger.panel.expanded.insert(reference);
        if !self.debugger.panel.variables.contains_key(&reference) {
            self.debug_request(DebugCommand::Variables { reference });
        }
        self.debugger.rebuild_rows();
    }

    /// Send an inspection command, remembering its id so the answer is
    /// accepted (anything else is a stale reply from before a resume).
    fn debug_request(&mut self, command: DebugCommand) {
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        if backend.send(id, command.into()).is_ok() {
            self.debugger.panel.pending.insert(id);
        }
    }

    /// Open the evaluate prompt (palette: Debug: Evaluate Expression).
    pub(super) fn debug_evaluate_prompt(&mut self) {
        if self.debugger.state == DebugSessionState::Idle {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "no debug session",
            );
            return;
        }
        self.open_overlay(crate::overlay::Overlay::text(
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
        self.debugger.panel.repl.push(format!("› {expression}"));
        self.debug_request(DebugCommand::Evaluate {
            expression,
            frame: self.debugger.panel.selected_frame,
        });
        self.debugger.rebuild_rows();
    }

    /// `Event::DebugStack`: adopt the frames, auto-select the top frame, and
    /// fetch its scopes (VS Code's behavior on stop).
    pub(super) fn on_debug_stack(&mut self, id: Option<RequestId>, frames: Vec<StackFrame>) {
        if !id.is_some_and(|id| self.debugger.panel.pending.remove(&id)) {
            return;
        }
        self.debugger.panel.stack = frames;
        if let Some(top) = self.debugger.panel.stack.first() {
            let frame = top.id;
            self.debugger.panel.selected_frame = Some(frame);
            self.debug_request(DebugCommand::Scopes { frame });
        }
        self.debugger.rebuild_rows();
    }

    /// `Event::DebugScopes`: adopt, auto-expanding the first cheap scope.
    pub(super) fn on_debug_scopes(
        &mut self,
        id: Option<RequestId>,
        frame: FrameId,
        scopes: Vec<DebugScope>,
    ) {
        if !id.is_some_and(|id| self.debugger.panel.pending.remove(&id))
            || self.debugger.panel.selected_frame != Some(frame)
        {
            return;
        }
        self.debugger.panel.scopes = scopes;
        if let Some(first) = self
            .debugger
            .panel
            .scopes
            .iter()
            .find(|scope| !scope.expensive)
            .map(|scope| scope.variables_reference)
        {
            self.debugger.panel.expanded.insert(first);
            self.debug_request(DebugCommand::Variables { reference: first });
        }
        self.debugger.rebuild_rows();
    }

    /// `Event::DebugVariables`: adopt one reference's children.
    pub(super) fn on_debug_variables(
        &mut self,
        id: Option<RequestId>,
        reference: VariablesRef,
        variables: Vec<DebugVariable>,
    ) {
        if !id.is_some_and(|id| self.debugger.panel.pending.remove(&id)) {
            return;
        }
        self.debugger.panel.variables.insert(reference, variables);
        self.debugger.rebuild_rows();
    }

    /// `Event::DebugEvaluated`: append the result to the evaluate log.
    pub(super) fn on_debug_evaluated(&mut self, id: Option<RequestId>, result: String) {
        if !id.is_some_and(|id| self.debugger.panel.pending.remove(&id)) {
            return;
        }
        self.debugger.panel.repl.push(format!("  = {result}"));
        self.debugger.rebuild_rows();
    }

    /// Run every code cell of the focused notebook preview (palette:
    /// Notebook: Run All Cells).
    pub(super) fn notebook_run_all(&mut self) {
        let Some(path) = self.tabs.get(self.active).and_then(|tab| {
            tab.path()
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
                })
                .map(std::path::Path::to_path_buf)
        }) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "open a notebook (.ipynb) first",
            );
            return;
        };
        self.debug_send(NotebookCommand::RunAll { path });
    }

    /// The status-bar debug segment, `None` while idle.
    pub(crate) fn debug_status_segment(&self) -> Option<String> {
        match self.debugger.state {
            DebugSessionState::Idle => None,
            DebugSessionState::Starting => Some("⏳ debug".to_owned()),
            DebugSessionState::Running => Some("▶ debug".to_owned()),
            DebugSessionState::Stopped => Some(format!("⏸ {}", self.debugger.detail)),
            _ => None,
        }
    }
}

/// One row of the Debug panel's flattened section list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebugRow {
    /// A section heading (`CALL STACK`, `VARIABLES`, …).
    Section(&'static str),
    /// A stack frame; indexes [`DebugPanel::stack`].
    Frame(usize),
    /// A variables scope; indexes [`DebugPanel::scopes`].
    Scope(usize),
    /// One variable of a fetched reference.
    Variable {
        /// The parent `variablesReference` whose children hold it.
        parent: VariablesRef,
        /// The index within that parent's children.
        index: usize,
        /// The tree depth (scopes' children are depth 1).
        depth: u16,
    },
    /// One evaluate-log line; indexes [`DebugPanel::repl`].
    Repl(usize),
    /// One console-output line; indexes the app's debug output buffer.
    Output(usize),
    /// A muted placeholder note.
    Note(&'static str),
}

/// How many trailing console lines the panel shows.
pub(crate) const DEBUG_CONSOLE_TAIL: usize = 100;
/// The variables tree recursion cap (cyclic references exist in the wild).
const DEBUG_TREE_DEPTH_CAP: u16 = 8;

/// The Debug sidebar panel: the stopped thread's stack, a lazily-fetched
/// variables tree, the evaluate log, and the console tail. All inspection
/// state is per-stop: it clears on resume so nothing stale survives.
#[derive(Default)]
pub(crate) struct DebugPanel {
    /// The stopped thread's frames, top first.
    pub(crate) stack: Vec<StackFrame>,
    /// The frame inspection targets (scopes/evaluate context).
    pub(crate) selected_frame: Option<FrameId>,
    /// The selected frame's scopes.
    pub(crate) scopes: Vec<DebugScope>,
    /// Fetched children per `variablesReference`.
    pub(crate) variables: HashMap<VariablesRef, Vec<DebugVariable>>,
    /// Expanded references (scopes and structured variables).
    pub(crate) expanded: HashSet<VariablesRef>,
    /// In-flight inspection requests; answers not in here are stale and dropped.
    pub(crate) pending: HashSet<RequestId>,
    /// The evaluate log (`expr = result` lines, oldest first).
    pub(crate) repl: Vec<String>,
    /// The rendered rows.
    pub(crate) rows: Vec<DebugRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
}

impl DebugPanel {
    /// Drop every per-stop artifact (stack, scopes, variables, pending).
    /// The evaluate log survives — it is a console, not a snapshot.
    pub(crate) fn clear_inspection(&mut self, output_len: usize) {
        self.stack.clear();
        self.selected_frame = None;
        self.scopes.clear();
        self.variables.clear();
        self.expanded.clear();
        self.pending.clear();
        self.rebuild_rows(output_len);
    }

    /// Rebuild the flattened rows, keeping the cursor clamped.
    pub(crate) fn rebuild_rows(&mut self, output_len: usize) {
        self.rows.clear();
        self.rows.push(DebugRow::Section("CALL STACK"));
        if self.stack.is_empty() {
            self.rows.push(DebugRow::Note("not stopped"));
        }
        self.rows.extend((0..self.stack.len()).map(DebugRow::Frame));
        self.rows.push(DebugRow::Section("VARIABLES"));
        if self.scopes.is_empty() {
            self.rows.push(DebugRow::Note("no frame selected"));
        }
        let scope_refs: Vec<VariablesRef> = self
            .scopes
            .iter()
            .map(|scope| scope.variables_reference)
            .collect();
        for (index, reference) in scope_refs.into_iter().enumerate() {
            self.rows.push(DebugRow::Scope(index));
            if self.expanded.contains(&reference) {
                self.push_children(reference, 1);
            }
        }
        if !self.repl.is_empty() {
            self.rows.push(DebugRow::Section("EVALUATE"));
            self.rows.extend((0..self.repl.len()).map(DebugRow::Repl));
        }
        if output_len > 0 {
            self.rows.push(DebugRow::Section("CONSOLE"));
            let first = output_len.saturating_sub(DEBUG_CONSOLE_TAIL);
            self.rows.extend((first..output_len).map(DebugRow::Output));
        }
        let cursor = self
            .selection
            .cursor()
            .min(self.rows.len().saturating_sub(1));
        self.selection = ListSelection::new(self.rows.len());
        self.selection.move_to(cursor);
    }

    fn push_children(&mut self, parent: VariablesRef, depth: u16) {
        if depth > DEBUG_TREE_DEPTH_CAP {
            return;
        }
        let children = self.variables.get(&parent).cloned().unwrap_or_default();
        for (index, child) in children.iter().enumerate() {
            self.rows.push(DebugRow::Variable {
                parent,
                index,
                depth,
            });
            let reference = child.variables_reference;
            if reference.has_children() && self.expanded.contains(&reference) {
                self.push_children(reference, depth + 1);
            }
        }
    }
}

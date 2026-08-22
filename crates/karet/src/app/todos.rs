//! The Todos panel: run a workspace codetag scan, list what it finds grouped
//! by file or by tag, and jump to any instance. Mirrors the Spelling panel's
//! streaming shape (`ScanWorkspaceTodos` → `TodoScanProgress`* →
//! `TodoScanFinished`).

use super::*;

/// Keep at most this many codetags (see the Spelling panel's identical cap).
pub(crate) const TODO_SCAN_RESULT_CAP: usize = 5_000;

impl App {
    /// Whether the Todos panel is offered at all: it lists what the semantic
    /// codetag pass detects, so with that disabled it could only be empty.
    pub(crate) fn todos_available(&self) -> bool {
        self.settings.editor.semantic_comments.enabled
    }

    /// Drop everything the Todos panel holds once codetags are off.
    pub(super) fn sync_todos_availability(&mut self) {
        if self.todos_available() {
            return;
        }
        if self.sidebar_panel == SidebarPanel::Todos {
            self.sidebar_panel = SidebarPanel::Explorer;
        }
        if let Some(scanning) = self.todos.scanning.take() {
            self.send(SessionCommand::Cancel { request: scanning });
        }
        self.todos.clear();
    }

    /// Show the Todos panel, scanning if it has nothing to show yet.
    pub(super) fn show_todos(&mut self) {
        if !self.todos_available() {
            return;
        }
        self.sidebar_visible = true;
        self.sidebar_panel = SidebarPanel::Todos;
        self.focus = Focus::Sidebar;
        if !self.todos.scanned && self.todos.scanning.is_none() {
            self.scan_workspace_todos();
        }
    }

    /// Start (or restart) the workspace codetag scan.
    pub(super) fn scan_workspace_todos(&mut self) {
        if !self.todos_available() {
            return;
        }
        if let Some(previous) = self.todos.scanning.take() {
            self.send(SessionCommand::Cancel { request: previous });
        }
        self.todos.clear();
        self.todos.scanning = self.send(SessionCommand::ScanWorkspaceTodos {
            limit: TODO_SCAN_RESULT_CAP,
        });
    }

    /// Adopt one streamed batch of scan results.
    pub(super) fn todo_scan_progress(
        &mut self,
        request: Option<RequestId>,
        hits: Vec<karet_session::TodoHit>,
        files_scanned: usize,
    ) {
        if request.is_none() || self.todos.scanning != request {
            return; // a superseded scan's late batch
        }
        self.todos.files_scanned = files_scanned.max(self.todos.files_scanned);
        self.todos.hits.extend(hits);
        self.todos.rebuild_rows();
    }

    /// Adopt a scan's terminal state.
    pub(super) fn todo_scan_finished(
        &mut self,
        request: Option<RequestId>,
        files_scanned: usize,
        truncated: bool,
    ) {
        if request.is_none() || self.todos.scanning != request {
            return;
        }
        self.todos.scanning = None;
        self.todos.files_scanned = files_scanned.max(self.todos.files_scanned);
        self.todos.truncated = truncated;
        self.todos.scanned = true;
    }

    /// Move the Todos selection.
    pub(super) fn todos_select(&mut self, delta: i32) {
        self.todos.selection.move_by(delta);
    }

    /// Switch between by-file and by-tag grouping.
    pub(super) fn todos_toggle_grouping(&mut self) {
        self.todos.by_tag = !self.todos.by_tag;
        self.todos.rebuild_rows();
    }

    /// Open the selected row's file with the caret on the tag's line. A group
    /// heading jumps to its first hit.
    pub(super) fn open_selected_todo(&mut self) {
        let Some(hit) = self
            .todos
            .rows
            .get(self.todos.selection.cursor())
            .and_then(|row| self.todos.order.get(row.hit()))
            .and_then(|&index| self.todos.hits.get(index))
        else {
            return;
        };
        let (path, line) = (hit.path.clone(), hit.line);
        self.focus_by_file_line(&path, karet_core::LineCol::new(line, 0));
    }

    /// Route a click inside the Todos panel: a header action, or a result row.
    pub(super) fn todos_click(&mut self, col: u16, row_y: u16) {
        if let Some(command) = self
            .todos_ui
            .action_hits
            .iter()
            .find(|&&(start, end, row, _)| row == row_y && (start..end).contains(&col))
            .map(|&(_, _, _, command)| command)
        {
            self.dispatch(command);
            return;
        }
        if !rect_contains(self.todos_ui.results_rect, (col, row_y)) {
            return;
        }
        let index = self.todos_ui.offset + (row_y - self.todos_ui.results_rect.y) as usize;
        if index < self.todos.rows.len() {
            self.todos.selection.move_to(index);
            self.open_selected_todo();
        }
    }
}

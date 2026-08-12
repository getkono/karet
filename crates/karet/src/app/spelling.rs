//! The Spelling panel: run a workspace spell scan, list what it finds, and jump
//! to any instance.
//!
//! Results stream in — the backend answers one `ScanWorkspaceSpelling` with a run
//! of `SpellingScanProgress` batches and a single `SpellingScanFinished` — so the
//! list fills as the walk proceeds rather than appearing all at once at the end.

use super::*;

/// Keep at most this many misspellings. A workspace can hold far more than a list
/// is useful for, and every hit costs a `PathBuf` plus its line of context; past
/// this the panel says so rather than growing without bound.
pub(crate) const SPELLING_SCAN_RESULT_CAP: usize = 5_000;

impl App {
    /// Show the Spelling panel, scanning if it has nothing to show yet.
    ///
    /// Re-showing a panel that already has results does *not* re-scan: a scan costs
    /// seconds on a large workspace, and the explicit refresh action is one key away.
    pub(super) fn show_spelling(&mut self) {
        self.sidebar_visible = true;
        self.sidebar_panel = SidebarPanel::Spelling;
        self.focus = Focus::Sidebar;
        if !self.spelling.scanned && self.spelling.scanning.is_none() {
            self.scan_workspace_spelling();
        }
    }

    /// Start a scan the startup path could not, once the backend exists.
    ///
    /// `--command "View: Show Spelling"` (and the startup panel setting) run before
    /// the backend is attached, so their scan submission is dropped and the panel
    /// would sit at "press ⟳ to scan" forever. Called from `attach_backend`
    /// alongside the other deferred startup requests.
    pub(super) fn request_pending_spelling_scan(&mut self) {
        if self.sidebar_panel == SidebarPanel::Spelling
            && !self.spelling.scanned
            && self.spelling.scanning.is_none()
        {
            self.scan_workspace_spelling();
        }
    }

    /// Re-scan because something the results depend on changed (a dictionary
    /// word, a `spellcheck` setting).
    ///
    /// A panel that has never scanned stays idle: opening it is what asks for the
    /// walk, and this must not turn a background config edit into seconds of work
    /// nobody is looking at.
    pub(super) fn invalidate_spelling(&mut self) {
        if !self.spelling.scanned && self.spelling.scanning.is_none() {
            return;
        }
        self.scan_workspace_spelling();
    }

    /// Start (or restart) the workspace spelling scan.
    pub(super) fn scan_workspace_spelling(&mut self) {
        // A scan already running is superseded, not raced: cancel it so its worker
        // stops walking, and drop its results so late batches cannot mix into the
        // new list.
        if let Some(previous) = self.spelling.scanning.take() {
            self.send(SessionCommand::Cancel { request: previous });
        }
        self.spelling.clear();
        self.spelling.scanning = self.send(SessionCommand::ScanWorkspaceSpelling {
            limit: SPELLING_SCAN_RESULT_CAP,
        });
    }

    /// Adopt one streamed batch of scan results.
    pub(super) fn spelling_scan_progress(
        &mut self,
        request: Option<RequestId>,
        hits: Vec<SpellingHit>,
        files_scanned: usize,
    ) {
        if request.is_none() || self.spelling.scanning != request {
            return; // a superseded scan's late batch, or an unsolicited event
        }
        self.spelling.files_scanned = files_scanned.max(self.spelling.files_scanned);
        self.spelling.hits.extend(hits);
        self.spelling.rebuild_rows();
    }

    /// Adopt a scan's terminal state.
    pub(super) fn spelling_scan_finished(
        &mut self,
        request: Option<RequestId>,
        files_scanned: usize,
        truncated: bool,
    ) {
        if request.is_none() || self.spelling.scanning != request {
            return;
        }
        self.spelling.scanning = None;
        self.spelling.files_scanned = files_scanned.max(self.spelling.files_scanned);
        self.spelling.truncated = truncated;
        self.spelling.scanned = true;
    }

    /// Move the Spelling selection.
    pub(super) fn spelling_select(&mut self, delta: i32) {
        self.spelling.selection.move_by(delta);
    }

    /// Open the selected row's file with the caret on the misspelled word. A file
    /// heading jumps to that file's first misspelling.
    pub(super) fn open_selected_spelling(&mut self) {
        let Some(hit) = self
            .spelling
            .rows
            .get(self.spelling.selection.cursor())
            .and_then(|row| self.spelling.hits.get(row.hit()))
        else {
            return;
        };
        let (path, position) = (hit.path.clone(), hit.range.start);
        self.focus_by_file_line(&path, position);
    }

    /// Route a click inside the Spelling panel: a header action, or a result row.
    pub(super) fn spelling_click(&mut self, col: u16, row_y: u16) {
        if let Some(command) = self
            .spelling_ui
            .action_hits
            .iter()
            .find(|&&(start, end, row, _)| row == row_y && (start..end).contains(&col))
            .map(|&(_, _, _, command)| command)
        {
            self.dispatch(command);
            return;
        }
        if !rect_contains(self.spelling_ui.results_rect, (col, row_y)) {
            return;
        }
        let index = self.spelling_ui.offset + (row_y - self.spelling_ui.results_rect.y) as usize;
        if index < self.spelling.rows.len() {
            self.spelling.selection.move_to(index);
            self.open_selected_spelling();
        }
    }
}

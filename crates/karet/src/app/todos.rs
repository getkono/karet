//! The Todos panel: run a workspace codetag scan, list what it finds grouped
//! by file or by tag, and jump to any instance. Mirrors the Spelling panel's
//! streaming shape (`ScanWorkspaceTodos` → `TodoScanProgress`* →
//! `TodoScanFinished`).

use karet_session::Command as SessionCommand;
use karet_session::RequestId;
use karet_session::WorkspaceCommand;
use karet_widgets::ListSelection;

use crate::app::App;
use crate::app::hit::rect_contains;
use crate::keymap::SidebarPanel;

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
        self.keyboard.focus_sidebar();
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
        self.todos.scanning = self.send(WorkspaceCommand::ScanTodos {
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
        self.focus_by_file_line(&path, karet_core::LineCol::new(line, 0), true);
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

/// One row of the Todos panel's list: a grouping header (a file, or a tag when
/// grouped by tag) or one codetag under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TodoRow {
    /// A group heading; `hit` is the group's first hit and `count` its total.
    Group {
        /// Index into [`TodosPanel::order`] of the group's first hit.
        hit: usize,
        /// How many hits the group holds.
        count: usize,
    },
    /// One codetag; `hit` indexes [`TodosPanel::order`].
    Item {
        /// Index into [`TodosPanel::order`].
        hit: usize,
    },
}

impl TodoRow {
    /// The hit this row jumps to when activated.
    pub(crate) fn hit(self) -> usize {
        match self {
            Self::Group { hit, .. } | Self::Item { hit } => hit,
        }
    }
}

/// The workspace codetag (Todos) panel state.
#[derive(Default)]
pub(crate) struct TodosPanel {
    /// Every codetag the current scan has reported, in scan order (grouped by
    /// file, since the walk visits file by file).
    pub(crate) hits: Vec<karet_session::TodoHit>,
    /// Display order: indices into `hits`, regrouped when `by_tag` is set.
    pub(crate) order: Vec<usize>,
    /// The rendered rows derived from `order`.
    pub(crate) rows: Vec<TodoRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
    /// The in-flight scan, if one is running.
    pub(crate) scanning: Option<RequestId>,
    /// How many files the running (or last) scan visited.
    pub(crate) files_scanned: usize,
    /// The last scan stopped at its result limit.
    pub(crate) truncated: bool,
    /// Whether a scan has ever completed.
    pub(crate) scanned: bool,
    /// Group rows by tag (`TODO` / `FIXME` / …) instead of by file.
    pub(crate) by_tag: bool,
}

impl TodosPanel {
    /// Rebuild `order` and [`rows`](Self::rows) from `hits`, clamping the cursor.
    pub(crate) fn rebuild_rows(&mut self) {
        self.order = (0..self.hits.len()).collect();
        if self.by_tag {
            self.order
                .sort_by(|&a, &b| self.hits[a].tag.cmp(&self.hits[b].tag).then(a.cmp(&b)));
        }
        self.rows.clear();
        let same_group = |a: usize, b: usize| {
            if self.by_tag {
                self.hits[a].tag == self.hits[b].tag
            } else {
                self.hits[a].path == self.hits[b].path
            }
        };
        let mut index = 0;
        while index < self.order.len() {
            let count = self.order[index..]
                .iter()
                .take_while(|&&hit| same_group(self.order[index], hit))
                .count();
            self.rows.push(TodoRow::Group { hit: index, count });
            self.rows
                .extend((index..index + count).map(|hit| TodoRow::Item { hit }));
            index += count;
        }
        let cursor = self.selection.cursor();
        self.selection = ListSelection::new(self.rows.len());
        self.selection
            .move_to(cursor.min(self.rows.len().saturating_sub(1)));
    }

    /// Drop every result, readying the panel for a fresh scan.
    pub(crate) fn clear(&mut self) {
        self.hits.clear();
        self.order.clear();
        self.rows.clear();
        self.selection = ListSelection::new(0);
        self.files_scanned = 0;
        self.truncated = false;
        self.scanned = false;
    }
}

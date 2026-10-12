//! Quick-open (go to file, `:` go to line, `@` go to symbol in editor, and `#`
//! go to symbol in workspace) and Open Recent (workspaces).
//!
//! The picker opens at once over the [`FileIndex`](workspace::FileIndex), which
//! walks the workspace off the UI thread and follows filesystem changes; a walk
//! still in flight fills an open picker in when it lands. Recently opened files
//! rank first, and the workspaces karet was opened on are offered by Open Recent,
//! both kept in the user state directory by [`RecentStore`](workspace::RecentStore).
//!
//! The `#` mode asks the language server, so its query is sent only once
//! typing pauses for [`LSP_CHANGE_DEBOUNCE`] (the event loop wakes for it), and
//! only the answer to the latest query is adopted: one to a query typed over
//! since is dropped.

use karet_core::WorkspaceSymbol;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::*;
use crate::overlay::DOCUMENT_SYMBOL_PREFIX;
use crate::overlay::GO_TO_LINE_PREFIX;
use crate::overlay::LineTarget;
use crate::overlay::QuickOpenMode;
use crate::overlay::WORKSPACE_SYMBOL_PREFIX;

/// Go to Symbol in Workspace's conversation with the backend: the query the
/// open picker wants answered, when it goes out, and which answer to adopt.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceSymbolSearch {
    /// The query (the text after `#`) the picker last asked for; `None` while
    /// no workspace-symbol picker is open.
    query: Option<String>,
    /// When that query is due to be sent; `None` once it has been, or when it
    /// is empty (nothing is asked for).
    due: Option<Instant>,
    /// The request whose answer is current. An answer to any other is to a
    /// superseded query, and is dropped.
    sent: Option<RequestId>,
    /// Since when rows for the current query have been awaited, for the
    /// delayed loading note.
    loading: Option<Pending>,
}

impl App {
    /// Open the quick-open (go-to-file) overlay at once, over whatever the file
    /// index holds; a walk still in flight fills it in when it lands.
    pub(super) fn open_quick_open(&mut self) {
        self.file_index.ensure_started();
        self.overlay = Some(self.quick_open_overlay());
    }

    /// The quick-open picker: recently opened files first, then the rest.
    fn quick_open_overlay(&self) -> Overlay {
        self.quick_open_overlay_for("")
    }

    /// The quick-open file picker for `query`, which may end in `:LINE[:COL]`
    /// to open the chosen file there.
    fn quick_open_overlay_for(&self, query: &str) -> Overlay {
        // The store keeps absolute paths; the index lists them under the root as
        // it was given (`.` for a bare `karet`), so rank them in that form.
        let absolute_root = workspace::absolute(&self.root);
        let recent: Vec<PathBuf> = self
            .recent
            .files(&self.root)
            .iter()
            .filter_map(|file| file.strip_prefix(&absolute_root).ok())
            .map(|relative| self.root.join(relative))
            .collect();
        Overlay::quick_open_for(self.file_index.ranked(&recent), query)
    }

    /// What an open quick-open is still waiting on — the first walk of the
    /// workspace, or the active document's symbols — which its delayed loading
    /// note anchors on.
    pub(crate) fn quick_open_loading(&self) -> Option<Pending> {
        match self.overlay.as_ref()?.quick_open_mode()? {
            QuickOpenMode::Files | QuickOpenMode::FileAtLine => self.file_index.loading(),
            QuickOpenMode::DocumentSymbols => self.active_outline_loading(),
            QuickOpenMode::WorkspaceSymbols => self.workspace_symbols.loading,
            // Computed from the open buffer: nothing to wait for.
            QuickOpenMode::Line => None,
        }
    }

    /// Open quick-open already in its `:` mode: Go to Line in the active editor.
    pub(super) fn open_go_to_line(&mut self) {
        self.overlay = Some(Overlay::go_to_line(GO_TO_LINE_PREFIX, self.line_target()));
    }

    /// The active text editor, as Go to Line moves in it; `None` when the
    /// active tab is not a text buffer.
    fn line_target(&self) -> Option<LineTarget<'_>> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { path, buffer, .. } = &tab.kind else {
            return None;
        };
        Some(LineTarget {
            path,
            lines: buffer.line_count(),
            caret: tab.editor.cursor(),
        })
    }

    /// Open quick-open already in its `@` mode: Go to Symbol in Editor over the
    /// active document's symbols, asking for them when none are current.
    pub(super) fn open_document_symbols(&mut self) {
        self.overlay = Some(self.document_symbols_overlay());
        self.request_active_symbols();
    }

    /// Go to Symbol in Editor over the active document's cached symbols (the
    /// last answer, even if the buffer has moved on since; a fresh one replaces
    /// it when it lands).
    fn document_symbols_overlay(&self) -> Overlay {
        let tab = self.tabs.get(self.active);
        let doc = tab.and_then(Self::tab_doc);
        let path = tab.filter(|_| doc.is_some()).and_then(Tab::path);
        let symbols = doc
            .and_then(|doc| self.docs.symbols.get(&doc))
            .map_or(&[][..], Vec::as_slice);
        Overlay::document_symbols(path, symbols)
    }

    /// Apply `edit` to the open overlay's query, then follow it into
    /// quick-open's mode only when the text actually changed: a key that edits
    /// nothing (an arrow, Home, Tab) keeps the computed line lists, and the row
    /// highlighted in them, as they are.
    pub(super) fn edit_overlay_query(&mut self, edit: impl FnOnce(&mut Overlay)) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let before = overlay.query().to_string();
        edit(overlay);
        if overlay.query() != before {
            self.sync_quick_open_mode();
        }
    }

    /// Follow an edit of an open quick-open's query into the mode its sigil now
    /// asks for: typing `@` lists the active document's symbols, `#` searches
    /// the workspace's, `:` goes to a line, a trailing `:LINE` opens the chosen
    /// file there, and deleting them goes back to files. What was typed is kept.
    fn sync_quick_open_mode(&mut self) {
        let Some(overlay) = self.overlay.as_ref() else {
            return;
        };
        let Some(mode) = overlay.quick_open_mode() else {
            return;
        };
        let query = overlay.query().to_string();
        let wanted = QuickOpenMode::of_query(&query);
        // The fuzzy lists filter themselves as the query changes; the two
        // line modes are computed from the whole query, so every edit rebuilds
        // them (and an edit that leaves one rebuilds the list it returns to).
        let computed = |mode| matches!(mode, QuickOpenMode::Line | QuickOpenMode::FileAtLine);
        if wanted == mode && !computed(mode) {
            self.follow_workspace_symbol_query(Instant::now());
            return;
        }
        let next = match wanted {
            QuickOpenMode::Files | QuickOpenMode::FileAtLine => {
                self.file_index.ensure_started();
                self.quick_open_overlay_for(&query)
            },
            QuickOpenMode::Line => Overlay::go_to_line(&query, self.line_target()),
            QuickOpenMode::DocumentSymbols => {
                let mut symbols = self.document_symbols_overlay();
                symbols.push_str(query.strip_prefix(DOCUMENT_SYMBOL_PREFIX).unwrap_or(&query));
                symbols
            },
            QuickOpenMode::WorkspaceSymbols => {
                let mut symbols = Overlay::workspace_symbols(&self.root, &[]);
                symbols.push_str(
                    query
                        .strip_prefix(WORKSPACE_SYMBOL_PREFIX)
                        .unwrap_or(&query),
                );
                symbols
            },
        };
        self.overlay = Some(next);
        if wanted == QuickOpenMode::DocumentSymbols {
            self.request_active_symbols();
        }
        self.follow_workspace_symbol_query(Instant::now());
    }

    /// Open quick-open already in its `#` mode: Go to Symbol in Workspace,
    /// which lists nothing until a query is typed.
    pub(super) fn open_workspace_symbols(&mut self) {
        self.overlay = Some(Overlay::workspace_symbols(&self.root, &[]));
        self.follow_workspace_symbol_query(Instant::now());
    }

    /// Follow the open workspace-symbol picker's query: a changed one is due
    /// to be sent once typing pauses, and the answer to any query before it is
    /// no longer wanted. An empty query asks nothing and lists nothing. With
    /// no such picker open the search is forgotten.
    fn follow_workspace_symbol_query(&mut self, now: Instant) {
        let query = self
            .overlay
            .as_ref()
            .and_then(Overlay::workspace_symbol_query)
            .map(str::to_string);
        let search = &mut self.workspace_symbols;
        let Some(query) = query else {
            *search = WorkspaceSymbolSearch::default();
            return;
        };
        if search.query.as_ref() == Some(&query) {
            return;
        }
        search.sent = None;
        if query.trim().is_empty() {
            search.due = None;
            search.loading = None;
            if let Some(overlay) = self.overlay.as_mut() {
                overlay.set_workspace_symbols(&self.root, &[]);
            }
        } else {
            // The rows already shown stay, filtered by the picker, until the
            // answer to the new query replaces them.
            search.due = Some(now + LSP_CHANGE_DEBOUNCE);
            search.loading.get_or_insert_with(Pending::start);
        }
        self.workspace_symbols.query = Some(query);
    }

    /// Send the workspace-symbol query once it is due. Called after every
    /// frame, as the lightbulb probe is; a picker closed in the meantime
    /// forgets the search instead.
    pub(crate) fn probe_workspace_symbols(&mut self, now: Instant) {
        let open = self
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.workspace_symbol_query().is_some());
        if !open {
            if self.workspace_symbols.query.is_some() {
                self.workspace_symbols = WorkspaceSymbolSearch::default();
            }
            return;
        }
        let search = &mut self.workspace_symbols;
        if search.due.is_none_or(|due| now < due) {
            return;
        }
        search.due = None;
        let query = search.query.clone().unwrap_or_default();
        let sent = self.send(SessionCommand::WorkspaceSymbols { query });
        let search = &mut self.workspace_symbols;
        search.sent = sent;
        if sent.is_none() {
            // No backend to answer: there is nothing to wait for.
            search.loading = None;
        }
    }

    /// How long until the workspace-symbol query is due, so the loop wakes to
    /// send it without another keystroke.
    pub(crate) fn workspace_symbol_next_wake(&self, now: Instant) -> Option<Duration> {
        self.workspace_symbols
            .due
            .map(|due| due.saturating_duration_since(now))
    }

    /// Fill the open Go to Symbol in Workspace with the answer to its current
    /// query. An answer to a query typed over since, or arriving after the
    /// picker closed, is dropped.
    pub(super) fn on_workspace_symbols(
        &mut self,
        id: Option<RequestId>,
        symbols: &[WorkspaceSymbol],
    ) {
        let search = &mut self.workspace_symbols;
        if id.is_none() || id != search.sent {
            return;
        }
        search.sent = None;
        search.loading = None;
        let root = workspace::absolute(&self.root);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_workspace_symbols(&root, symbols);
        }
    }

    /// Fill an open Go to Symbol in Editor with `doc`'s fresh symbols, when
    /// `doc` is the document it lists.
    pub(super) fn refresh_document_symbols(&mut self, doc: DocumentId) {
        let tab = self.tabs.get(self.active);
        if tab.and_then(Self::tab_doc) != Some(doc) {
            return;
        }
        let path = tab.and_then(Tab::path).map(Path::to_path_buf);
        let symbols = self.docs.symbols.get(&doc).map_or(&[][..], Vec::as_slice);
        if let Some(overlay) = self.overlay.as_mut() {
            overlay.set_document_symbols(path.as_deref(), symbols);
        }
    }

    /// Adopt a finished workspace walk, refreshing an open quick-open.
    pub(super) fn on_file_index(&mut self, scan: workspace::Scan) {
        if self.file_index.accept(scan) {
            self.refresh_quick_open();
        }
    }

    /// Follow filesystem changes into the file index.
    pub(super) fn file_index_fs_changed(&mut self, paths: &[PathBuf]) {
        if self.file_index.on_fs_changed(paths) {
            self.refresh_quick_open();
        }
    }

    /// Rebuild an open quick-open over the current list, keeping what the user
    /// typed and, where it is still listed, the row they had highlighted.
    fn refresh_quick_open(&mut self) {
        let Some(open) = self
            .overlay
            .as_ref()
            .filter(|o| o.quick_open_mode().is_some_and(QuickOpenMode::lists_files))
        else {
            return;
        };
        let query = open.query().to_string();
        let selected = open.selected_file().map(Path::to_path_buf);
        let mut overlay = self.quick_open_overlay_for(&query);
        if let Some(selected) = selected {
            overlay.select_file(&selected);
        }
        self.overlay = Some(overlay);
    }

    /// Remember that `path` was opened, for quick-open's recency ranking. Only
    /// files in this workspace are ranked, so only those are remembered.
    pub(super) fn note_recent_file(&mut self, path: &Path) {
        self.recent.touch_file(&self.root, path);
    }

    /// Open the Open Recent picker over the other remembered workspaces that
    /// still exist.
    pub(super) fn open_recent_workspace(&mut self) {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        let this = workspace::absolute(&self.root);
        let items: Vec<(String, PathBuf)> = self
            .recent
            .workspaces()
            .filter(|root| *root != this && root.is_dir())
            .map(|root| (tilde(root, home.as_deref()), root.to_path_buf()))
            .collect();
        if items.is_empty() {
            self.notify(
                Report::Outcome,
                NotificationKind::System,
                "open recent: no other recent workspaces",
            );
            return;
        }
        self.overlay = Some(Overlay::recent_workspaces(items));
    }

    /// Reopen karet on `root`: quit this window (through the usual unsaved
    /// changes prompt), then start again there once the terminal is restored.
    pub(super) fn reopen_on_workspace(&mut self, root: PathBuf) {
        if !root.is_dir() {
            self.notify(
                Report::Failure,
                NotificationKind::System,
                format!("open recent: {} is no longer a directory", root.display()),
            );
            return;
        }
        self.guarded_close(CloseRequest::Quit);
        self.relaunch = Some(workspace::absolute(&root));
    }

    /// The workspace to reopen on after quitting, if Open Recent asked for one.
    pub(super) fn take_relaunch(&mut self) -> Option<PathBuf> {
        self.relaunch.take()
    }

    /// Forget a pending reopen: the quit that would have carried it was cancelled.
    pub(super) fn cancel_relaunch(&mut self) {
        self.relaunch = None;
    }
}

/// `path` for display, with the home directory written `~`.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_shortens_only_paths_under_home() {
        let home = Path::new("/home/me");
        assert_eq!(tilde(Path::new("/home/me/src/k"), Some(home)), "~/src/k");
        assert_eq!(tilde(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(tilde(Path::new("/home/meow"), Some(home)), "/home/meow");
        assert_eq!(tilde(Path::new("/srv/k"), None), "/srv/k");
    }
}

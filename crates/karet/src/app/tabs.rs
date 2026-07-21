use super::*;

impl App {
    /// Open `path`, focusing an existing tab for the same file instead of opening a
    /// duplicate. This is the single entry point for every "open a file" flow
    /// (explorer, quick-open, search result, startup, reopen-closed).
    pub(super) fn open_path(&mut self, path: &Path) {
        let target = canonical(path);
        // Focus an existing editor view for this file, but not a diff tab — a diff
        // is a distinct view of the same path, so opening the file still opens it.
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            self.select_tab(idx);
            return;
        }
        let tab = workspace::open_file(path);
        self.push_tab(tab);
    }

    /// Open `path` into the focused pane's reusable "preview" tab slot (VS
    /// Code-style): used by file-tree navigation (single click / arrow +
    /// activate) and selection-follows-preview. A file already open (preview or
    /// permanent) is just shown. Otherwise the current preview tab, if this pane
    /// has one, is replaced in place; if not, a new preview tab is opened. Every
    /// other caller of `open_path` (LSP jumps, the overlay, reopen-closed,
    /// CLI-provided files) keeps opening permanent tabs — only tree navigation
    /// opens previews.
    ///
    /// `steal_focus` moves keyboard focus to the editor (Enter / click);
    /// selection-follows-preview passes `false` so the sidebar keeps focus and
    /// the user can keep arrowing.
    pub(super) fn open_path_preview(&mut self, path: &Path, steal_focus: bool) {
        let target = canonical(path);
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            self.active = idx;
            self.find_open = false;
            if steal_focus {
                self.focus = Focus::Editor;
            }
            return;
        }
        let mut tab = workspace::open_file(path);
        tab.is_preview = true;
        self.install_preview_tab(tab, steal_focus);
    }

    /// Place `tab` (already flagged [`is_preview`](Tab::is_preview)) into the
    /// focused pane's single preview slot: replace the existing preview tab in
    /// place, or — when this pane has none — open it as a new tab. One slot per
    /// pane regardless of content kind, so a previewed file and a previewed diff
    /// share it. `steal_focus` moves keyboard focus to the editor; otherwise the
    /// current focus is preserved (selection-follows-preview).
    pub(super) fn install_preview_tab(&mut self, mut tab: Tab, steal_focus: bool) {
        tab.view = self.alloc_view();
        match self.tabs.iter().position(|t| t.is_preview) {
            Some(idx) => {
                self.tabs[idx] = tab;
                self.active = idx;
                self.find_open = false;
                if steal_focus {
                    self.focus = Focus::Editor;
                }
                self.register_doc(self.active);
                // The replaced tab's document (if any) is no longer referenced by
                // any tab; this closes it on the session side.
                self.reconcile_open_docs();
            },
            None => {
                if self.tabs.len() == 1 && matches!(self.tabs[0].kind, TabKind::Welcome) {
                    self.tabs[0] = tab;
                    self.active = 0;
                } else {
                    self.tabs.push(tab);
                    self.active = self.tabs.len() - 1;
                }
                self.find_open = false;
                if steal_focus {
                    self.focus = Focus::Editor;
                }
                self.register_doc(self.active);
            },
        }
    }

    /// The "open anyway" override: re-open the active too-large placeholder's file
    /// with the size guard bypassed, replacing the placeholder tab in place (rather
    /// than opening a second tab for the same path). A no-op on any other tab — the
    /// binding is only live over a too-large placeholder.
    pub(super) fn open_active_anyway(&mut self) {
        let path = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Placeholder {
                        kind: FileKind::TooLarge { .. },
                        path,
                        ..
                    },
                ..
            }) => path.clone(),
            _ => return,
        };
        let mut tab = workspace::open_file_ignoring_size(&path);
        tab.view = self.alloc_view();
        self.tabs[self.active] = tab;
        self.focus = Focus::Editor;
        self.register_doc(self.active);
    }

    /// Add a tab, replacing a lone Welcome tab, and focus the editor.
    pub(super) fn push_tab(&mut self, mut tab: Tab) {
        tab.view = self.alloc_view();
        if self.tabs.len() == 1 && matches!(self.tabs[0].kind, TabKind::Welcome) {
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.focus = Focus::Editor;
        // A newly-focused tab never inherits another tab's open find bar.
        self.find_open = false;
        self.register_doc(self.active);
    }

    /// Allocate a fresh [`ViewId`] for a newly-opened view.
    pub(super) fn alloc_view(&mut self) -> ViewId {
        let view = ViewId(self.next_view);
        self.next_view += 1;
        view
    }

    /// The session document backing `tab`, if it is a registered code tab.
    pub(super) fn tab_doc(tab: &Tab) -> Option<DocumentId> {
        match &tab.kind {
            // A preview counts as a view of its document: `reconcile_open_docs` ref-counts
            // through here, so reporting the id keeps the document (and its snapshot
            // stream) alive even after the source tab is closed.
            TabKind::Code { doc, .. } | TabKind::MarkdownPreview { doc, .. } => *doc,
            _ => None,
        }
    }

    /// The currently focused pane.
    pub(crate) fn focus_pane(&self) -> PaneId {
        self.layout.focus()
    }

    /// Stash the focused pane's tabs into storage, so *every* pane's tabs live in
    /// `stored` and the layout can be freely mutated. Pair with [`load_focused`].
    pub(super) fn stash_focused(&mut self) {
        let current = self.layout.focus();
        let tabs = std::mem::take(&mut self.tabs);
        self.stored.insert(
            current,
            StoredPane {
                tabs,
                active: self.active,
            },
        );
        self.active = 0;
    }

    /// Pull the (possibly newly) focused pane's tabs out of storage into the live
    /// `tabs`/`active` fields. A pane with no stored tabs shows a lone welcome tab.
    pub(super) fn load_focused(&mut self) {
        let pane = self.layout.focus();
        if let Some(sp) = self.stored.remove(&pane) {
            self.tabs = sp.tabs;
            self.active = sp.active;
        } else {
            self.tabs = vec![Tab::welcome()];
            self.active = 0;
        }
    }

    /// Make `pane` the focused pane, swapping the current focused tabs into storage
    /// and `pane`'s tabs out. A no-op if `pane` is already focused or unknown.
    pub(super) fn focus_pane_switch(&mut self, pane: PaneId) {
        if pane == self.layout.focus() || !self.layout.contains(pane) {
            return;
        }
        self.stash_focused();
        self.layout.set_focus(pane);
        self.load_focused();
    }

    /// Every tab across every pane (the focused pane plus all stored panes). Used by
    /// backend-event/snapshot handlers that must reach a document wherever it is shown.
    pub(super) fn all_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs
            .iter_mut()
            .chain(self.stored.values_mut().flat_map(|p| p.tabs.iter_mut()))
    }

    /// Every tab across every pane (immutable).
    pub(super) fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs
            .iter()
            .chain(self.stored.values().flat_map(|p| p.tabs.iter()))
    }

    /// Whether any dirty open tab is backed by one of `paths` or a descendant.
    pub(super) fn has_dirty_tabs_under(&self, paths: &[PathBuf]) -> bool {
        self.all_tabs().any(|tab| {
            tab.dirty
                && tab
                    .path()
                    .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
        })
    }

    /// Close every clean tab backed by one of `paths` or a descendant.
    pub(super) fn close_tabs_under(&mut self, paths: &[PathBuf]) {
        self.tabs.retain(|tab| {
            !tab.path()
                .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
        });
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        for pane in self.stored.values_mut() {
            pane.tabs.retain(|tab| {
                !tab.path()
                    .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
            });
            if pane.tabs.is_empty() {
                pane.tabs.push(Tab::welcome());
                pane.active = 0;
            } else if pane.active >= pane.tabs.len() {
                pane.active = pane.tabs.len() - 1;
            }
        }
        self.reconcile_open_docs();
    }

    /// Update open tabs and the session document path map after a filesystem move.
    pub(super) fn retarget_open_paths(&mut self, from: &Path, to: &Path) {
        let mut docs = Vec::new();
        for tab in self.all_tabs_mut() {
            let Some(current) = tab.path().map(Path::to_path_buf) else {
                continue;
            };
            let Some(next) = rebase_path(&current, from, to) else {
                continue;
            };
            let doc = Self::tab_doc(tab);
            retarget_tab_path(tab, &next);
            if let Some(doc) = doc {
                docs.push((doc, next));
            }
        }
        docs.sort_by_key(|(doc, _)| *doc);
        docs.dedup_by_key(|(doc, _)| *doc);
        for (doc, path) in docs {
            self.send_command(SessionCommand::RetargetDocument { doc, path });
        }
    }

    /// Release any session documents no longer shown in a tab (the session
    /// ref-counts opens; the app balances them). Call after closing tabs.
    pub(super) fn reconcile_open_docs(&mut self) {
        let live: HashSet<DocumentId> = self.all_tabs().filter_map(Self::tab_doc).collect();
        let stale: Vec<DocumentId> = self.open_docs.difference(&live).copied().collect();
        for doc in stale {
            self.open_docs.remove(&doc);
            if let Some(backend) = &self.backend {
                let id = backend.next_id();
                let _ = backend.send(id, SessionCommand::CloseDocument { doc });
            }
        }
    }

    /// Switch to the tab at `index`, focusing the editor.
    pub(super) fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus = Focus::Editor;
            // The find bar is keyed to whichever tab it was opened over; switching
            // tabs must not show it over a different file.
            self.find_open = false;
        }
    }

    /// Switch to the next tab (wrapping).
    pub(super) fn next_tab(&mut self) {
        let n = self.tabs.len();
        if n > 1 {
            self.select_tab((self.active + 1) % n);
        }
    }

    /// Switch to the previous tab (wrapping).
    pub(super) fn prev_tab(&mut self) {
        let n = self.tabs.len();
        if n > 1 {
            self.select_tab((self.active + n - 1) % n);
        }
    }

    /// Go to the 1-based tab `n` (9 selects the last tab, VS Code-style).
    pub(super) fn go_to_tab(&mut self, n: u8) {
        let n = n as usize;
        let index = if n >= 9 {
            self.tabs.len().saturating_sub(1)
        } else {
            n.saturating_sub(1)
        };
        self.select_tab(index);
    }

    /// Move the tab at `from` to position `to`, making it active.
    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = to;
    }
    /// Record a closed file tab's path so it can be reopened later.
    pub(super) fn remember_closed(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index)
            && !tab.is_diff()
            && let Some(path) = tab.path()
        {
            let path = path.to_path_buf();
            self.closed.retain(|p| p != &path);
            self.closed.push(path);
        }
    }

    /// Close the focused pane's active tab, routed through the unsaved-changes guard.
    pub(super) fn request_close_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the focused pane's tab at `index`, routed through the unsaved-changes
    /// guard (the tab is captured by its stable view id).
    pub(super) fn request_close_tab_at(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index) {
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the tab at `index`. When it is the pane's final tab, collapse the pane
    /// if another pane remains; the sole pane falls back to a Welcome tab.
    pub(super) fn close_tab_at(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.remember_closed(index);
        if self.tabs.len() == 1 && self.layout.pane_count() > 1 {
            let closing = self.focus_pane();
            self.stash_focused();
            self.stored.remove(&closing);
            if self.layout.close(closing).is_some() {
                self.load_focused();
                self.focus = Focus::Editor;
            }
        } else if self.tabs.len() == 1 {
            self.tabs = vec![Tab::welcome()];
            self.active = 0;
            self.focus = Focus::Sidebar;
        } else {
            self.tabs.remove(index);
            if index < self.active {
                self.active -= 1;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
        }
        // The closed tab's own `find` data goes with it; the flag may now be
        // pointing at a different tab, so drop it too rather than risk showing
        // the bar over whatever tab ends up active.
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Close every tab except the active one.
    pub(super) fn close_other_tabs(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        for i in (0..self.tabs.len()).rev() {
            if i != self.active {
                self.remember_closed(i);
            }
        }
        self.tabs = vec![self.tabs.remove(self.active)];
        self.active = 0;
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Close every tab to the right of the active one.
    pub(super) fn close_tabs_to_right(&mut self) {
        for i in (self.active + 1..self.tabs.len()).rev() {
            self.remember_closed(i);
        }
        self.tabs.truncate(self.active + 1);
        self.reconcile_open_docs();
    }

    /// Close all tabs, leaving a Welcome tab.
    pub(super) fn close_all_tabs(&mut self) {
        for i in (0..self.tabs.len()).rev() {
            self.remember_closed(i);
        }
        self.tabs = vec![Tab::welcome()];
        self.active = 0;
        self.focus = Focus::Sidebar;
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Reopen the most recently closed file tab whose file still exists.
    pub(super) fn reopen_closed_tab(&mut self) {
        while let Some(path) = self.closed.pop() {
            if path.is_file() {
                self.open_path(&path);
                return;
            }
        }
    }
}

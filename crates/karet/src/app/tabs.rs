use super::*;

/// The activation history's hard ceiling. It only bounds memory in a very long
/// session: entries this old are far past any "put me back where I was" reach, and
/// closed views are skipped on read rather than pruned, so nothing else depends on
/// the list being tight.
const MAX_VIEW_HISTORY: usize = 256;

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
            self.set_active(idx);
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

    /// Open (or focus, when it is already open) `path` and put the caret at
    /// `position` — the single "jump to this file:line" primitive behind every
    /// result list that navigates somewhere: workspace search hits, a diff's
    /// underlying file, `--goto` at startup, and the Spelling panel.
    ///
    /// A relative `path` resolves against the workspace root, so a VCS-relative
    /// change path opens and dedups like any explorer open. Focus follows
    /// [`open_path`](Self::open_path) to the editor. `position` is in the editor's
    /// 0-based coordinates and [`goto`](karet_editor::EditorState::goto) clamps it
    /// into the buffer; a non-text tab (image, binary, placeholder) simply has no
    /// caret to place.
    ///
    /// Callers keep their own pre-checks: this deliberately does *not* require the
    /// path to exist, because `--goto` on a missing file is how karet opens a new
    /// one.
    pub(super) fn focus_by_file_line(&mut self, path: &Path, position: LineCol) {
        let target = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        self.open_path(&target);
        // The buffer is cloned out first (an O(1) rope share) so the tab can be
        // borrowed mutably to move its caret.
        let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        };
        // A file whose content has not arrived cannot be positioned in yet:
        // `goto` clamps into the buffer, and clamping into an empty one lands at
        // the top. Remember where the caret was asked to go and apply it when the
        // document's first snapshot lands, so a jump to a line survives however
        // long the content takes to arrive.
        if let Some(view) = self.tabs.get(self.active).map(|tab| tab.view) {
            let ready = buffer
                .as_ref()
                .is_some_and(|buffer| buffer.line_count() > 1);
            if !ready {
                self.pending_goto.insert(view, position);
            }
        }
        if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
            tab.editor.goto(&buffer, position);
        }
    }

    /// Place `tab` (already flagged [`is_preview`](Tab::is_preview)) into the
    /// focused pane's single preview slot: replace the existing preview tab in
    /// place, or — when this pane has none — open it as a new tab. One slot per
    /// pane regardless of content kind, so a previewed file and a previewed diff
    /// share it. `steal_focus` moves keyboard focus to the editor; otherwise the
    /// current focus is preserved (selection-follows-preview).
    pub(super) fn install_preview_tab(&mut self, mut tab: Tab, steal_focus: bool) {
        tab.view = self.alloc_view();
        match self
            .tabs
            .iter()
            .position(|t| t.is_preview && !t.is_github_dashboard())
        {
            Some(idx) => {
                self.tabs[idx] = tab;
                self.set_active(idx);
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
                    self.set_active(0);
                } else {
                    self.tabs.push(tab);
                    self.set_active(self.tabs.len() - 1);
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
            self.set_active(0);
        } else {
            self.tabs.push(tab);
            self.set_active(self.tabs.len() - 1);
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
            TabKind::Code { doc, .. } => *doc,
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
        self.set_active(0);
    }

    /// Pull the (possibly newly) focused pane's tabs out of storage into the live
    /// `tabs`/`active` fields. A pane with no stored tabs shows a lone welcome tab.
    pub(super) fn load_focused(&mut self) {
        let pane = self.layout.focus();
        if let Some(sp) = self.stored.remove(&pane) {
            self.tabs = sp.tabs;
            self.set_active(sp.active);
        } else {
            self.tabs = vec![Tab::welcome()];
            self.set_active(0);
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
    ///
    /// A pane whose active tab survives keeps it in front at its shifted index; one
    /// that loses it falls back to the most recently active tab still open there.
    pub(super) fn close_tabs_under(&mut self, paths: &[PathBuf]) {
        let doomed = |tab: &Tab| {
            tab.path()
                .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
        };
        let active_view = self.tabs.get(self.active).map(|tab| tab.view);
        self.tabs.retain(|tab| !doomed(tab));
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
            self.set_active(0);
        } else {
            let fallback = self.active.min(self.tabs.len() - 1);
            let next =
                Self::refocus_after_removal(&self.view_history, &self.tabs, active_view, fallback);
            self.set_active(next);
        }
        // Field-split the borrow: the stored panes are read against the same history.
        let App {
            stored,
            view_history,
            ..
        } = self;
        for pane in stored.values_mut() {
            let active_view = pane.tabs.get(pane.active).map(|tab| tab.view);
            pane.tabs.retain(|tab| !doomed(tab));
            if pane.tabs.is_empty() {
                pane.tabs.push(Tab::welcome());
                pane.active = 0;
            } else {
                let fallback = pane.active.min(pane.tabs.len() - 1);
                pane.active =
                    Self::refocus_after_removal(view_history, &pane.tabs, active_view, fallback);
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
            self.auto_save_pending.remove(&doc);
            if let Some(backend) = &self.backend {
                let id = backend.next_id();
                let _ = backend.send(id, SessionCommand::CloseDocument { doc });
            }
        }
    }

    /// Make the tab at `index` the focused pane's active one, recording the
    /// activation in [`view_history`](App::view_history).
    ///
    /// **Every** write to [`active`](App::active) goes through here, including the
    /// index shifts that keep the same view in front: that is what keeps the history
    /// behind close-focus from drifting. Recording is idempotent, so a shift that
    /// re-selects the view already in front costs nothing.
    ///
    /// This is the bare mechanism; [`select_tab`](App::select_tab) layers the
    /// editor-focus and find-bar side effects on top. An `index` past the end (or a
    /// momentarily empty tab list) still moves `active` but records nothing.
    pub(super) fn set_active(&mut self, index: usize) {
        self.active = index;
        if let Some(view) = self.tabs.get(index).map(|tab| tab.view) {
            self.note_activation(view);
        }
    }

    /// Record `view` as the most recently activated one, moving it to the front when
    /// it is already known. `ViewId(0)` is ignored: it is the unassigned sentinel
    /// every welcome tab shares, so it identifies no single view.
    fn note_activation(&mut self, view: ViewId) {
        if view == ViewId(0) || self.view_history.last() == Some(&view) {
            return;
        }
        self.view_history.retain(|known| *known != view);
        self.view_history.push(view);
        if self.view_history.len() > MAX_VIEW_HISTORY {
            self.view_history.remove(0);
        }
    }

    /// The focused pane's most recently activated tab that is still open, or
    /// `fallback` when the history knows none of them. Call it *after* removing the
    /// closing tab, so the tab being closed can never be the answer.
    pub(super) fn recent_tab_index(&self, fallback: usize) -> usize {
        Self::recent_index_in(&self.view_history, &self.tabs, fallback)
    }

    /// The index in `tabs` of the most recently activated view still present there,
    /// or `fallback` when `history` knows none of them. An empty `tabs` yields `0`.
    ///
    /// Takes its inputs as slices rather than reading `self` so the same code answers
    /// for the focused pane and for a [`StoredPane`] inside a `stored.values_mut()`
    /// loop, where `self` is already mutably borrowed.
    pub(in crate::app) fn recent_index_in(
        history: &[ViewId],
        tabs: &[Tab],
        fallback: usize,
    ) -> usize {
        if tabs.is_empty() {
            return 0;
        }
        history
            .iter()
            .rev()
            .find_map(|view| tabs.iter().position(|tab| tab.view == *view))
            .unwrap_or(fallback)
    }

    /// Which tab a pane should show after a bulk removal took an arbitrary subset of
    /// its tabs: `active_view` again if it survived (at its shifted index), else the
    /// most recently active tab still open there, else `fallback`.
    ///
    /// Shared by every "tabs vanished underneath the user" path — an external file
    /// delete, the GitHub dashboard being withdrawn — so all of them land the user in
    /// the same place an ordinary close would.
    pub(in crate::app) fn refocus_after_removal(
        history: &[ViewId],
        tabs: &[Tab],
        active_view: Option<ViewId>,
        fallback: usize,
    ) -> usize {
        active_view
            .and_then(|view| tabs.iter().position(|tab| tab.view == view))
            .unwrap_or_else(|| Self::recent_index_in(history, tabs, fallback))
    }

    /// Point pane focus at the pane holding the most recently activated surviving
    /// view, overriding the positional neighbour [`PaneLayout::close`] chose.
    ///
    /// Call it once the collapsed pane is gone from both the layout and `stored`,
    /// while every surviving pane's tabs are still stashed. A no-op when the history
    /// knows none of them, which leaves the positional answer standing.
    fn focus_recent_pane(&mut self) {
        let recent = self.view_history.iter().rev().find_map(|view| {
            self.stored
                .iter()
                .find(|(_, pane)| pane.tabs.iter().any(|tab| tab.view == *view))
                .map(|(id, _)| *id)
        });
        if let Some(pane) = recent
            && self.layout.contains(pane)
        {
            self.layout.set_focus(pane);
        }
    }

    /// Switch to the tab at `index`, focusing the editor.
    pub(super) fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.set_active(index);
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
        if self.tabs[from].is_github_dashboard() || self.tabs[to].is_github_dashboard() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.set_active(to);
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
            if tab.is_github_dashboard() {
                return;
            }
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the focused pane's tab at `index`, routed through the unsaved-changes
    /// guard (the tab is captured by its stable view id).
    pub(super) fn request_close_tab_at(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index) {
            if tab.is_github_dashboard() {
                return;
            }
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the tab at `index`. When it is the pane's final tab, collapse the pane
    /// if another pane remains; the sole pane falls back to a Welcome tab.
    ///
    /// Closing the tab in front hands focus to the tab the user was in most recently
    /// that is still open — not to whichever tab happens to slide into the vacated
    /// slot. Collapsing a pane picks the surviving pane the same way.
    pub(super) fn close_tab_at(&mut self, index: usize) {
        if index >= self.tabs.len() || self.tabs[index].is_github_dashboard() {
            return;
        }
        self.remember_closed(index);
        if self.tabs.len() == 1 && self.layout.pane_count() > 1 {
            let closing = self.focus_pane();
            self.stash_focused();
            self.stored.remove(&closing);
            if self.layout.close(closing).is_some() {
                // Every surviving pane is stashed and the closed one is gone from
                // both the layout and `stored`: the moment `focus_recent_pane` needs.
                self.focus_recent_pane();
                self.load_focused();
                self.focus = Focus::Editor;
            }
        } else if self.tabs.len() == 1 {
            self.tabs = vec![Tab::welcome()];
            self.set_active(0);
            self.focus = Focus::Sidebar;
        } else if index == self.active {
            // The tab in front is going. Fall back to the neighbour that slides into
            // its slot only when the history knows none of the survivors.
            self.tabs.remove(index);
            let fallback = index.min(self.tabs.len() - 1);
            let next = self.recent_tab_index(fallback);
            self.set_active(next);
        } else {
            // A background tab: the same view stays in front, its index just shifts.
            self.tabs.remove(index);
            if index < self.active {
                self.set_active(self.active - 1);
            }
            if self.active >= self.tabs.len() {
                self.set_active(self.tabs.len() - 1);
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
            if i != self.active && !self.tabs[i].is_github_dashboard() {
                self.remember_closed(i);
            }
        }
        let active_view = self.tabs[self.active].view;
        self.tabs
            .retain(|tab| tab.view == active_view || tab.is_github_dashboard());
        let kept = self
            .tabs
            .iter()
            .position(|tab| tab.view == active_view)
            .unwrap_or(0);
        self.set_active(kept);
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
            if !self.tabs[i].is_github_dashboard() {
                self.remember_closed(i);
            }
        }
        self.tabs.retain(Tab::is_github_dashboard);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
        }
        self.set_active(0);
        self.focus = if self.tabs[0].is_github_dashboard() {
            Focus::Editor
        } else {
            Focus::Sidebar
        };
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

    /// Fill the reserved DOCX preview owned by the answering conversion request
    /// with its markdown, or degrade it to a placeholder on failure. A closed
    /// tab drops the answer.
    pub(super) fn apply_document_converted(
        &mut self,
        id: Option<RequestId>,
        path: &Path,
        markdown: Result<String, String>,
    ) {
        let Some(view) = id.and_then(|id| self.pending_conversions.remove(&id)) else {
            // Unsolicited (a notebook kernel re-rendered its preview):
            // refresh matching previews in place, keeping their scroll.
            if let Ok(text) = &markdown {
                for tab in self.all_tabs_mut() {
                    if let TabKind::MarkdownPreview {
                        path: tab_path,
                        buffer,
                        rendered,
                        pending_since: None,
                        ..
                    } = &mut tab.kind
                        && tab_path == path
                    {
                        *buffer = karet_text::TextBuffer::from_text(text);
                        *rendered = None;
                    }
                }
            }
            return;
        };
        let mut failure = None;
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            let pending = matches!(
                tab.kind,
                TabKind::MarkdownPreview {
                    pending_since: Some(_),
                    ..
                }
            );
            if !pending {
                break;
            }
            match &markdown {
                Ok(text) => {
                    tab.kind = Tab::document_preview(path.to_path_buf(), text).kind;
                },
                Err(message) => {
                    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    let kind = if path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
                    {
                        FileKind::Notebook
                    } else {
                        FileKind::Docx
                    };
                    tab.kind = TabKind::Placeholder {
                        path: path.to_path_buf(),
                        kind,
                        dims: None,
                        len,
                    };
                    failure = Some(message.clone());
                },
            }
            break;
        }
        if let Some(message) = failure {
            self.notify(Severity::Error, NotificationKind::Io, message);
        }
    }
}

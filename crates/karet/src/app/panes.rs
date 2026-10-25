use karet_core::CursorState;
use karet_core::Selection;
use karet_session::backup;
use karet_session::backup::LayoutAxis;
use karet_session::backup::LayoutNode;
use karet_session::backup::LayoutPane;
use karet_session::backup::LayoutTab;
use karet_session::backup::WorkspaceLayout;
use karet_widgets::pane::tree::PaneTree;

use super::*;

impl App {
    /// Grow the focused pane toward `dir` by the keyboard resize step.
    ///
    /// A no-op while a pane is maximised: the tiled layout is hidden, so a
    /// resize would change nothing visible yet leave altered split ratios
    /// behind for when the layout tiles again.
    pub(super) fn resize_focused_pane(&mut self, dir: SplitDir) {
        const STEP: u16 = 2;
        if self.maximized_pane().is_some() {
            return;
        }
        self.layout
            .resize_pane(self.focus_pane(), dir, STEP, self.main_rect);
    }

    /// While dragging, move the active tab under column `x` within the focused pane.
    pub(super) fn drag_tab_to(&mut self, x: u16) {
        let focused = self.focus_pane();
        let hit = self
            .pane_frames
            .iter()
            .find(|f| f.pane == focused)
            .and_then(|f| tab_at(&f.tab_hits, x));
        if let Some((target, _)) = hit
            && target != self.active
        {
            self.move_tab(self.active, target);
        }
    }

    /// The pane whose *content* area contains `(x, y)`, and that content rect.
    pub(super) fn pane_at_content(&self, x: u16, y: u16) -> Option<(PaneId, Rect)> {
        self.pane_frames
            .iter()
            .find(|f| rect_contains(f.content_rect, (x, y)))
            .map(|f| (f.pane, f.content_rect))
    }

    /// Update the in-progress tab drag: reorder within the origin pane's strip, or
    /// track a drop target (pane + zone) over another pane's content for preview.
    pub(super) fn drag_tab_update(&mut self, x: u16, y: u16) {
        let Some(drag) = self.tab_drag else {
            return;
        };
        // Over the origin pane's own tab strip: reorder in place, no drop target.
        let on_from_strip = self
            .pane_frames
            .iter()
            .find(|f| f.pane == drag.from_pane)
            .is_some_and(|f| rect_contains(f.tabstrip_rect, (x, y)));
        if on_from_strip {
            self.drag_tab_to(x);
            if let Some(d) = self.tab_drag.as_mut() {
                d.hover = None;
            }
            return;
        }
        let hover = self
            .pane_at_content(x, y)
            .map(|(pane, rect)| (pane, drop_zone(rect, x, y)));
        if let Some(d) = self.tab_drag.as_mut() {
            d.hover = hover;
        }
    }

    /// Finish a tab drag: apply the pending move/split, if any.
    pub(super) fn drag_tab_drop(&mut self) {
        let Some(drag) = self.tab_drag.take() else {
            return;
        };
        if let Some((target, zone)) = drag.hover {
            self.drop_tab_on(target, zone);
        }
    }

    /// Drop the focused pane's active tab onto `target`'s `zone`: an edge splits
    /// `target` and moves the tab into the new pane; the center moves it into
    /// `target`. Collapses the origin pane if it empties.
    pub(super) fn drop_tab_on(&mut self, target: PaneId, zone: DropZone) {
        let from = self.focus_pane();
        if self.tabs.is_empty() || (target == from && zone == DropZone::Center) {
            return;
        }
        let idx = self.active.min(self.tabs.len().saturating_sub(1));
        let tab = self.tabs.remove(idx);
        // The dragged tab leaves this pane, so the origin needs a new tab in front:
        // the one that was active here most recently. A plain write, not
        // `set_active` — focus is moving to `dest`, so the origin's replacement must
        // not top the activation history. `load_focused` records it if the user
        // comes back to this pane.
        let fallback = self.active.min(self.tabs.len().saturating_sub(1));
        self.active = Self::recent_index_in(&self.view_history, &self.tabs, fallback);

        // Move all panes into storage so the layout can be mutated freely.
        self.stash_focused();
        let dest = match zone.split_dir() {
            Some(dir) => {
                let new_pane = self.layout.split(target, dir);
                self.stored.insert(
                    new_pane,
                    StoredPane {
                        tabs: Vec::new(),
                        active: 0,
                    },
                );
                new_pane
            },
            None => target,
        };
        if let Some(sp) = self.stored.get_mut(&dest) {
            sp.tabs.push(tab);
            sp.active = sp.tabs.len().saturating_sub(1);
        }
        // If the origin pane emptied, close it (collapsing the split).
        if from != dest && self.stored.get(&from).is_some_and(|sp| sp.tabs.is_empty()) {
            self.stored.remove(&from);
            self.layout.close(from);
        }
        self.layout.set_focus(dest);
        self.load_focused();
        self.focus = Focus::Editor;
        self.reconcile_open_docs();
    }

    /// Toggle a rendered preview inside the active Markdown editor view.
    ///
    /// The preview is view-local state: it does not create another tab, pane, or
    /// session-document reference. The source editor retains keyboard focus.
    pub(super) fn open_markdown_preview_side(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let is_markdown = match &tab.kind {
            TabKind::Code { path, text, .. } => {
                let head = text.as_bytes();
                let head = head.get(..crate::workspace::HEAD_BYTES).unwrap_or(head);
                karet_filetype::classify_ignoring_size(path, head) == FileKind::Markdown
            },
            _ => false,
        };
        if !is_markdown {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "markdown preview: not a Markdown file",
            );
            return;
        }
        tab.markdown_preview = tab
            .markdown_preview
            .take()
            .is_none()
            .then(MarkdownPreviewState::default);
        self.focus = Focus::Editor;
    }

    /// Split the focused pane in `dir` via the keyboard, opening a second view of the
    /// active document (sharing its session document, with an independent cursor) in
    /// the new pane, which becomes focused.
    pub(super) fn split_focused(&mut self, dir: SplitDir) {
        let from = self.focus_pane();
        let dup = self.duplicate_active_tab();
        self.stash_focused();
        let new_pane = self.layout.split(from, dir);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![dup],
                active: 0,
            },
        );
        self.layout.set_focus(new_pane);
        self.load_focused();
        self.focus = Focus::Editor;
    }

    /// Build a second view of the active tab for a new pane: the same document
    /// (shared edit log) with a fresh [`ViewId`] and independent editor state. A
    /// non-code (or empty) active tab yields a welcome tab.
    pub(super) fn duplicate_active_tab(&mut self) -> Tab {
        let view = self.alloc_view();
        let mut tab = match self.tabs.get(self.active) {
            Some(t) => match &t.kind {
                TabKind::Code {
                    path,
                    language,
                    doc,
                    next_version,
                    buffer,
                    text,
                    highlights,
                    semantic_blocks,
                    folds,
                    folded,
                    decos,
                    search_decos,
                    syntax_errors,
                } => Tab {
                    // The worker's table lines for this document, so the new view
                    // keeps them until the next snapshot rather than losing them.
                    markdown_table_lines: t.markdown_table_lines.clone(),
                    ..Tab::new(
                        t.title.clone(),
                        TabKind::Code {
                            path: path.clone(),
                            language,
                            doc: *doc,
                            next_version: *next_version,
                            // The same buffer, not a copy of it.
                            buffer: std::sync::Arc::clone(buffer),
                            text: text.clone(),
                            highlights: highlights.clone(),
                            semantic_blocks: semantic_blocks.clone(),
                            folds: folds.clone(),
                            folded: folded.clone(),
                            decos: decos.clone(),
                            search_decos: search_decos.clone(),
                            syntax_errors: syntax_errors.clone(),
                        },
                    )
                },
                _ => Tab::welcome(),
            },
            None => Tab::welcome(),
        };
        tab.view = view;
        tab
    }

    /// Cycle window focus to the next (`forward`) or previous pane. A no-op with
    /// fewer than two panes.
    pub(super) fn focus_pane_cycle(&mut self, forward: bool) {
        let panes = self.layout.panes();
        let n = panes.len();
        if n < 2 {
            return;
        }
        let cur = self.layout.focus();
        let i = panes.iter().position(|p| *p == cur).unwrap_or(0);
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.focus_pane_switch(panes[next]);
    }

    /// Move the active tab one slot left (`-1`) or right (`+1`), clamped (no wrap).
    ///
    /// The pinned GitHub dashboard keeps the leftmost slot: it neither moves nor is
    /// displaced by a neighbour, the same refusal [`App::move_tab`] makes for a drag.
    pub(super) fn move_active_tab(&mut self, delta: i32) {
        let n = self.tabs.len() as i64;
        if n < 2 {
            return;
        }
        let target = (self.active as i64 + i64::from(delta)).clamp(0, n - 1) as usize;
        if target == self.active {
            return;
        }
        self.tabs.swap(self.active, target);
        self.set_active(target);
    }
}

// The saved workspace layout (`workbench.restoreLayout`): written on a clean
// exit, reopened by the next start on the same root that has nothing to open.
impl App {
    /// Save this workspace's layout under `dir`, or delete the saved one when
    /// the setting is off or no pane shows a file.
    ///
    /// # Errors
    /// Returns an [`std::io::Error`] if the layout cannot be written.
    pub(super) fn save_workspace_layout(&self, dir: &Path) -> std::io::Result<()> {
        let layout = self
            .settings
            .workbench
            .restore_layout
            .then(|| self.workspace_layout())
            .flatten();
        match layout {
            Some(layout) => backup::write_layout(dir, &self.layout_root(), &layout),
            None => {
                backup::remove_layout(dir, &self.layout_root());
                Ok(())
            },
        }
    }

    /// Reopen the layout saved under `dir` for this workspace, when the
    /// setting is on and startup opened nothing the user asked for.
    pub(super) fn restore_workspace_layout(&mut self, dir: &Path) {
        if !self.settings.workbench.restore_layout || !self.startup_is_pristine() {
            return;
        }
        if let Some(layout) = backup::read_layout(dir, &self.layout_root()) {
            self.apply_workspace_layout(layout);
        }
    }

    /// The root a saved layout is keyed by: the workspace root made absolute.
    /// The root as given is usually the default `.`, which would key every
    /// directory karet is started in to one shared layout.
    fn layout_root(&self) -> PathBuf {
        workspace::absolute(&self.root)
    }

    /// A saved (absolute) tab path in the form this session's own tabs use: a
    /// file under the root is joined onto the root as given, as startup and the
    /// explorer open it, so the same file never shows up under two spellings.
    fn session_path(&self, saved: &Path) -> PathBuf {
        let saved = workspace::absolute(saved);
        match saved.strip_prefix(self.layout_root()) {
            Ok(rel) => self.root.join(rel),
            Err(_) => saved,
        }
    }

    /// Whether startup left only what karet shows unasked: one pane holding a
    /// Welcome tab or the repository README preview. Any file, split, diff, or
    /// `--goto` the command line opened means the user chose this session's
    /// surface, and a saved layout must not replace it.
    fn startup_is_pristine(&self) -> bool {
        let unasked = |tab: &Tab| {
            matches!(tab.kind, TabKind::Welcome)
                || (tab.is_preview && tab.path().is_some_and(|path| self.is_root_readme(path)))
        };
        self.layout.pane_count() == 1
            && self.stored.is_empty()
            && self.pending_startup_diffs.is_empty()
            && self.tabs.iter().all(unasked)
    }

    /// Whether `path` is a README at the workspace root, the file startup
    /// previews when nothing else is opened.
    fn is_root_readme(&self, path: &Path) -> bool {
        path.parent() == Some(self.root.as_path())
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().starts_with("readme"))
    }

    /// The current layout as it would be saved: every pane with at least one
    /// file tab, or `None` when there is none.
    pub(super) fn workspace_layout(&self) -> Option<WorkspaceLayout> {
        let mut kept = Vec::new();
        let root = self.saved_node(&self.layout.tree(), &mut kept)?;
        let focused = self.focus_pane();
        let focus = kept.iter().position(|pane| *pane == focused).unwrap_or(0);
        Some(WorkspaceLayout { root, focus })
    }

    /// The saved form of `tree`, appending each pane it keeps to `kept`.
    fn saved_node(&self, tree: &PaneTree, kept: &mut Vec<PaneId>) -> Option<LayoutNode> {
        match tree {
            PaneTree::Pane(pane) => {
                let (tabs, active) = if *pane == self.focus_pane() {
                    (self.tabs.as_slice(), self.active)
                } else {
                    let stored = self.stored.get(pane)?;
                    (stored.tabs.as_slice(), stored.active)
                };
                let saved = self.saved_pane(tabs, active)?;
                kept.push(*pane);
                Some(LayoutNode::Pane(saved))
            },
            PaneTree::Split {
                axis,
                children,
                weights,
            } => {
                let mut nodes = Vec::new();
                let mut shares = Vec::new();
                for (index, child) in children.iter().enumerate() {
                    if let Some(node) = self.saved_node(child, kept) {
                        nodes.push(node);
                        shares.push(weights.get(index).copied().unwrap_or(1.0));
                    }
                }
                match nodes.len() {
                    0 => None,
                    1 => nodes.pop(),
                    _ => Some(LayoutNode::Split {
                        axis: match axis {
                            SplitAxis::Cols => LayoutAxis::Cols,
                            SplitAxis::Rows => LayoutAxis::Rows,
                        },
                        children: nodes,
                        weights: shares,
                    }),
                }
            },
        }
    }

    /// The saved form of one pane's tabs: each file it shows once, in strip
    /// order, with the front tab (or the nearest file tab before it) active.
    fn saved_pane(&self, tabs: &[Tab], active: usize) -> Option<LayoutPane> {
        let mut saved: Vec<LayoutTab> = Vec::new();
        let mut front = 0;
        for (index, tab) in tabs.iter().enumerate() {
            let Some(entry) = self.saved_tab(tab) else {
                continue;
            };
            let slot = match saved.iter().position(|kept| kept.path == entry.path) {
                Some(slot) => slot,
                None => {
                    saved.push(entry);
                    saved.len() - 1
                },
            };
            if index <= active {
                front = slot;
            }
        }
        (!saved.is_empty()).then_some(LayoutPane {
            tabs: saved,
            active: front,
        })
    }

    /// The saved form of `tab`, when it shows a file (a diff does not: it is a
    /// view of a change, not of the file).
    fn saved_tab(&self, tab: &Tab) -> Option<LayoutTab> {
        if tab.is_diff() {
            return None;
        }
        // A relative tab path is relative to the current directory (the root
        // as given is joined onto it), so it is made absolute the same way.
        let path = workspace::absolute(tab.path()?);
        let (cursors, scroll_line, scroll_col) = match &tab.kind {
            TabKind::Code { .. } => (
                tab.editor.cursors().clone(),
                tab.editor.scroll_line,
                tab.editor.scroll_col,
            ),
            _ => (
                CursorState::single(Selection::caret(LineCol::new(0, 0))),
                0,
                0,
            ),
        };
        Some(LayoutTab {
            path,
            cursors,
            scroll_line,
            scroll_col,
            preview: tab.is_preview,
        })
    }

    /// Replace the startup surface with `layout`. Files gone since it was saved
    /// are skipped and a pane left with none is dropped; with nothing left, the
    /// startup surface stays.
    pub(super) fn apply_workspace_layout(&mut self, layout: WorkspaceLayout) {
        let mut panes = Vec::new();
        let mut focus = None;
        let mut leaf = 0;
        let Some(tree) =
            self.restored_node(layout.root, layout.focus, &mut leaf, &mut panes, &mut focus)
        else {
            return;
        };
        let Some(restored) = PaneLayout::from_tree(tree, focus.unwrap_or(PaneId(0))) else {
            return;
        };
        let focused = restored.focus();
        let mut front = None;
        self.stored.clear();
        for (pane, stored) in panes {
            if pane == focused {
                front = Some(stored);
            } else {
                self.stored.insert(pane, stored);
            }
        }
        let Some(front) = front else {
            return;
        };
        self.layout = restored;
        self.tabs = front.tabs;
        self.set_active(front.active);
        self.find_open = false;
        // Resuming work: the keyboard goes to the restored editor, as it does
        // for any file the user opens, not to the startup sidebar panel.
        self.show_editor();
    }

    /// The live tree for a saved `node`, opening each pane's tabs into `panes`.
    /// `leaf` counts saved panes in tree order, so the one at `saved_focus`
    /// sets `focus`.
    fn restored_node(
        &mut self,
        node: LayoutNode,
        saved_focus: usize,
        leaf: &mut usize,
        panes: &mut Vec<(PaneId, StoredPane)>,
        focus: &mut Option<PaneId>,
    ) -> Option<PaneTree> {
        match node {
            LayoutNode::Pane(saved) => {
                let index = *leaf;
                *leaf += 1;
                let pane = self.restored_pane(saved)?;
                let id = PaneId(u64::try_from(panes.len()).unwrap_or(u64::MAX));
                if index == saved_focus {
                    *focus = Some(id);
                }
                panes.push((id, pane));
                Some(PaneTree::Pane(id))
            },
            LayoutNode::Split {
                axis,
                children,
                weights,
            } => {
                let mut kept = Vec::new();
                let mut shares = Vec::new();
                for (index, child) in children.into_iter().enumerate() {
                    if let Some(tree) = self.restored_node(child, saved_focus, leaf, panes, focus) {
                        kept.push(tree);
                        // A missing weight makes the split unsound, so
                        // `from_tree` sizes its children equally.
                        shares.push(weights.get(index).copied().unwrap_or(f32::NAN));
                    }
                }
                Some(PaneTree::Split {
                    axis: match axis {
                        LayoutAxis::Cols => SplitAxis::Cols,
                        LayoutAxis::Rows => SplitAxis::Rows,
                    },
                    children: kept,
                    weights: shares,
                })
            },
        }
    }

    /// Open a saved pane's tabs that still exist on disk, putting each code
    /// tab's cursors and viewport back. `None` when none of them does.
    fn restored_pane(&mut self, saved: LayoutPane) -> Option<StoredPane> {
        let mut tabs = Vec::new();
        let mut active = 0;
        let mut has_preview = false;
        for (index, entry) in saved.tabs.into_iter().enumerate() {
            let path = self.session_path(&entry.path);
            if !path.is_file() {
                continue;
            }
            let mut tab = workspace::open_file(&path);
            tab.view = self.alloc_view();
            // One preview slot per pane.
            tab.is_preview = entry.preview && !has_preview;
            has_preview |= tab.is_preview;
            if let Tab {
                kind: TabKind::Code { buffer, .. },
                editor,
                ..
            } = &mut tab
            {
                editor.set_cursor_state(buffer, entry.cursors);
                let last_line = u32::try_from(buffer.line_count().saturating_sub(1)).unwrap_or(0);
                editor.scroll_line = entry.scroll_line.min(last_line);
                editor.scroll_col = entry.scroll_col;
            }
            if index <= saved.active {
                active = tabs.len();
            }
            tabs.push(tab);
        }
        (!tabs.is_empty()).then_some(StoredPane { tabs, active })
    }
}

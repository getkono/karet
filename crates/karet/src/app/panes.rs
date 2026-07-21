use super::*;

impl App {
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
        if self.tabs[idx].is_github_dashboard() {
            return;
        }
        let tab = self.tabs.remove(idx);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));

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

    /// Whether `tab` is the markdown preview of the source view `source_view`.
    pub(super) fn previews_view(tab: &Tab, source_view: ViewId) -> bool {
        matches!(&tab.kind, TabKind::MarkdownPreview { source_view: v, .. } if *v == source_view)
    }

    /// The visible (pane-active) tab of some *non-focused* pane matching `pred`.
    pub(super) fn stored_active(&self, pred: impl Fn(&Tab) -> bool) -> Option<&Tab> {
        self.stored
            .values()
            .filter_map(|pane| pane.tabs.get(pane.active))
            .find(|tab| pred(tab))
    }

    /// As [`stored_active`](Self::stored_active), mutably.
    pub(super) fn stored_active_mut(&mut self, pred: impl Fn(&Tab) -> bool) -> Option<&mut Tab> {
        self.stored
            .values_mut()
            .filter_map(|pane| pane.tabs.get_mut(pane.active))
            .find(|tab| pred(tab))
    }

    /// Scroll a markdown preview and its source together.
    ///
    /// The focused pane drives and the other follows. Because the driver's own scroll is
    /// never written back, the pair cannot oscillate even though the projections are lossy
    /// — a whole run of source lines can share one wrapped line.
    ///
    /// The preview only pushes once it has actually been scrolled *away* from where the
    /// source projects it. Without that check, merely moving focus onto the preview would
    /// nudge the source by the rounding error of a `source -> wrapped -> source` round trip.
    ///
    /// Runs once per frame just before drawing, so it reads the `wrapped` model the
    /// previous draw cached; a resize therefore takes one extra frame to settle. A pair
    /// whose halves are not both their pane's visible tab is skipped.
    pub(crate) fn sync_markdown_preview(&mut self) {
        let Some(focused) = self.tabs.get(self.active) else {
            return;
        };
        match &focused.kind {
            TabKind::Code { .. } => {
                let view = focused.view;
                let line = focused.editor.scroll_line as usize;
                let Some(preview) = self.stored_active_mut(|t| Self::previews_view(t, view)) else {
                    return;
                };
                if let TabKind::MarkdownPreview {
                    wrapped, scroll, ..
                } = &mut preview.kind
                {
                    let row = wrapped.wrapped_line_for_source(line);
                    *scroll = u16::try_from(row).unwrap_or(u16::MAX);
                }
            },
            TabKind::MarkdownPreview {
                source_view,
                wrapped,
                scroll,
                ..
            } => {
                let view = *source_view;
                let scroll = *scroll;
                let Some(source) = self.stored_active(|t| t.view == view) else {
                    return;
                };
                let source_line = source.editor.scroll_line as usize;
                if wrapped.wrapped_line_for_source(source_line) == usize::from(scroll) {
                    return; // already consistent: a bare focus change must not move it
                }
                let want = wrapped.source_line_for_wrapped(usize::from(scroll));
                let Some(source) = self.stored_active_mut(|t| t.view == view) else {
                    return;
                };
                if let TabKind::Code { buffer, .. } = &source.kind {
                    let last = buffer.line_count().saturating_sub(1);
                    source.editor.scroll_line = u32::try_from(want.min(last)).unwrap_or(u32::MAX);
                }
            },
            _ => {},
        }
    }

    /// Focus the existing preview of `source_view`, wherever it lives. `false` if there
    /// is none.
    pub(super) fn reveal_markdown_preview(&mut self, source_view: ViewId) -> bool {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| Self::previews_view(t, source_view))
        {
            self.active = index;
            self.focus = Focus::Editor;
            return true;
        }
        let found = self.stored.iter().find_map(|(pane, stored)| {
            stored
                .tabs
                .iter()
                .position(|t| Self::previews_view(t, source_view))
                .map(|index| (*pane, index))
        });
        let Some((pane, index)) = found else {
            return false;
        };
        self.focus_pane_switch(pane);
        self.active = index;
        self.focus = Focus::Editor;
        true
    }

    /// Open a rendered preview of the active Markdown file in a pane to the right.
    ///
    /// Focus deliberately stays in the source editor (unlike [`split_focused`]): the user
    /// invoked this to keep typing and watch the preview follow, and it makes the editor
    /// the scroll master (see [`sync_markdown_preview`](Self::sync_markdown_preview)).
    pub(super) fn open_markdown_preview_side(&mut self) {
        // Take everything the preview needs up front: the rest of this borrows `self`
        // mutably. The rope clone is O(1), so the preview paints from it on its very
        // first frame, before any snapshot has landed.
        let source = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code {
                path,
                doc,
                buffer,
                text,
                ..
            } => {
                let head = text.as_bytes();
                let head = head.get(..crate::workspace::HEAD_BYTES).unwrap_or(head);
                (karet_filetype::classify_ignoring_size(path, head) == FileKind::Markdown)
                    .then(|| (path.clone(), *doc, tab.view, buffer.content_snapshot()))
            },
            _ => None,
        });
        let Some((path, doc, source_view, buffer)) = source else {
            self.status = Some("markdown preview: not a Markdown file".to_string());
            return;
        };
        if self.reveal_markdown_preview(source_view) {
            return;
        }
        let preview = Tab::markdown_preview(path, doc, source_view, buffer);

        let from = self.focus_pane();
        if !self.layout.can_split(from, SplitDir::Right, self.main_rect) {
            // Too narrow to split: a tab in this pane still beats refusing to preview.
            self.push_tab(preview);
            return;
        }
        let mut preview = preview;
        preview.view = self.alloc_view();
        self.stash_focused();
        let new_pane = self.layout.split(from, SplitDir::Right);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![preview],
                active: 0,
            },
        );
        // `split` focuses the pane it created; hand focus back to the source editor.
        self.layout.set_focus(from);
        self.load_focused();
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
                } => Tab::new(
                    t.title.clone(),
                    TabKind::Code {
                        path: path.clone(),
                        language,
                        doc: *doc,
                        next_version: *next_version,
                        buffer: buffer.clone(),
                        text: text.clone(),
                        highlights: highlights.clone(),
                        semantic_blocks: semantic_blocks.clone(),
                        folds: folds.clone(),
                        folded: folded.clone(),
                        decos: decos.clone(),
                        search_decos: search_decos.clone(),
                        syntax_errors: syntax_errors.clone(),
                    },
                ),
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
    pub(super) fn move_active_tab(&mut self, delta: i32) {
        let n = self.tabs.len() as i64;
        if n < 2 {
            return;
        }
        let target = (self.active as i64 + i64::from(delta)).clamp(0, n - 1) as usize;
        if target != self.active {
            self.tabs.swap(self.active, target);
            self.active = target;
        }
    }
}

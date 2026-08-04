use super::*;

impl App {
    /// Apply a document snapshot to the matching code tab(s): the snapshot is the
    /// render source of truth (buffer, highlights, the search text, and the
    /// unsaved-changes flag).
    pub(super) fn on_snapshot(&mut self, doc: DocumentId, snap: &DocSnapshot) {
        for tab in self.all_tabs_mut() {
            let matches = matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
            if !matches {
                continue;
            }
            if let TabKind::Code {
                buffer,
                highlights,
                semantic_blocks,
                folds,
                folded,
                text,
                next_version,
                syntax_errors,
                ..
            } = &mut tab.kind
            {
                // A slow-arriving snapshot must not regress a tab that has since
                // advanced further via `submit_edit`'s local speculative apply —
                // only the buffer/text catch up when the snapshot is at least as
                // new as what's already applied locally.
                if snap.version >= buffer.version() {
                    *buffer = snap.buffer.clone();
                    *text = snap.buffer.text();
                }
                *highlights = (*snap.highlights).clone();
                *semantic_blocks = (*snap.semantic_blocks).clone();
                *folds = (*snap.folds).clone();
                *syntax_errors = snap.syntax_error_lines.as_ref().clone();
                *next_version = (*next_version).max(snap.version);
                // Drop collapsed markers whose fold no longer starts where it did (an
                // edit shifted or removed it), so stale hidden lines can't linger.
                let starts: HashSet<u32> = folds.regions().iter().map(|r| r.start).collect();
                folded.retain(|line| starts.contains(line));
            }
            // The clean→dirty transition permanently promotes a preview tab (VS
            // Code behavior): once edited, it survives being navigated away from
            // instead of getting silently replaced by the next preview-opened file.
            if snap.dirty && !tab.dirty {
                tab.is_preview = false;
            }
            tab.dirty = snap.dirty;
            // Undo/redo snapshots carry the caret to jump to; ordinary edits carry
            // `None` so the optimistic placement from `submit_edit` is preserved.
            if let Some(cursor) = &snap.cursor {
                let heads: Vec<LineCol> = cursor.selections.iter().map(|s| s.head).collect();
                if !heads.is_empty() {
                    tab.editor.set_carets(&heads);
                    tab.editor.scroll_to(cursor.primary().head);
                }
            }
        }
        if snap.dirty {
            self.schedule_auto_save(doc, snap.version, Instant::now());
        } else if self
            .auto_save_pending
            .get(&doc)
            .is_some_and(|pending| pending.version <= snap.version)
        {
            self.auto_save_pending.remove(&doc);
        }
        self.request_active_outline();
        // If the find bar is open, an edit (e.g. a replace) just changed the buffer,
        // so recompute the match highlights against the fresh text.
        if self.find_open {
            self.run_find();
        }
        // Likewise for global search matches: a newly-opened or just-edited tab
        // should show its highlights immediately, not only after the next
        // explicit search re-run.
        if !self.search.query.is_empty() {
            self.refresh_search_decorations();
        }
        // An undo/redo snapshot may have moved the caret away from the popup's
        // anchor; re-validate it.
        self.reconcile_completion();
        self.request_live_blame();
    }
}

use super::*;

impl App {
    /// Apply a document snapshot to the matching code tab(s): the snapshot is the
    /// render source of truth (buffer, highlights, the search text, and the
    /// unsaved-changes flag).
    pub(super) fn on_snapshot(&mut self, doc: DocumentId, snap: &DocSnapshot) {
        // Most snapshots do not move the text -- a highlight pass republishes
        // the same version -- and those must not re-ask for hints. One that
        // does holds the next request until the edits pause.
        let mut advanced = false;
        // The edit that carries the held hints to this version, found against
        // the one tab still showing the version they are positioned for. The
        // snapshot does not say what changed -- an undo, a formatter, a
        // reload -- so it is recovered from the two texts.
        let held = self.inlay_hint_version(doc);
        // Navigation entries ride the same recovered edit, so it is also worth
        // recovering when only they are positioned against the old text.
        let wanted_versions: Vec<u64> = self
            .all_tabs()
            .filter_map(|tab| match &tab.kind {
                TabKind::Code {
                    doc: Some(d),
                    buffer,
                    ..
                } if *d == doc => Some(buffer.version()),
                _ => None,
            })
            .filter(|&version| held == Some(version) || self.navigation_holds(doc, version))
            .collect();
        let mut carried: Option<(u64, Option<karet_core::TextEdit>)> = None;
        // One lazy snapshot of the new text, shared by every split on this
        // document: taking it copies nothing, and whichever reader first needs
        // the contiguous text (find, a Markdown scan, the edit recovery above)
        // builds it once for all of them.
        let snap_text = snap.buffer.text_snapshot();
        // Likewise one buffer for all of them: each view that catches up holds
        // a clone of this `Arc`, not a copy of the buffer.
        let snap_buffer = std::sync::Arc::new(snap.buffer.clone());
        // An undo/redo's selection set belongs to the view that asked for it;
        // failing a record of one, to the focused view when it shows this
        // document. Every other view keeps its own carets.
        let shows =
            |tab: &Tab| matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
        let restore_in = snap.cursor.as_ref().and_then(|_| {
            self.docs
                .history_view
                .get(&doc)
                .copied()
                .filter(|view| self.all_tabs().any(|tab| tab.view == *view && shows(tab)))
                .or_else(|| {
                    self.tabs
                        .get(self.active)
                        .filter(|tab| shows(tab))
                        .map(|tab| tab.view)
                })
        });
        for tab in self.all_tabs_mut() {
            if !shows(tab) {
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
                    let old_version = buffer.version();
                    advanced |= snap.version > old_version;
                    // A reserved tab's first fill is the file loading, not an
                    // edit: its caret goes where the open asked for, rather
                    // than riding the insertion of the whole text to its end.
                    let fill = tab.fill_caret.take();
                    let fill_view = tab.fill_view.take();
                    if fill.is_some() {
                        tab.fill_since = None;
                        tab.fill_error = None;
                        // The empty placeholder and the loaded text can share a
                        // version (a file with no restored history loads at the
                        // placeholder's), so what was drawn from the placeholder
                        // under that version is dropped rather than kept as fresh.
                        tab.conflict_decorations = None;
                        if let Some(preview) = tab.markdown_preview.as_mut() {
                            preview.rendered = None;
                        }
                    }
                    // What turned this view's text into the snapshot's: the
                    // change it did not make itself (another view's edit, a
                    // formatter, a reload, an undo). Recovered only when
                    // something reads it: the view an undo restores does not
                    // map its carets.
                    let restore = snap
                        .cursor
                        .as_ref()
                        .filter(|_| restore_in == Some(tab.view));
                    let want_carried = carried.is_none() && wanted_versions.contains(&old_version);
                    let change = (snap.version > old_version
                        && ((restore.is_none() && fill.is_none()) || want_carried))
                        .then(|| super::inlay::diff_edit(text, &snap_text))
                        .flatten();
                    if want_carried && snap.version > old_version {
                        carried = Some((old_version, change.clone()));
                    }
                    *buffer = std::sync::Arc::clone(&snap_buffer);
                    *text = snap_text.clone();
                    match (fill, restore) {
                        (Some(position), _) => {
                            crate::tab::place_fill(&mut tab.editor, buffer, position, fill_view);
                        },
                        (None, Some(cursor)) => tab.editor.set_cursor_state(buffer, cursor.clone()),
                        // Every caret follows the text it sat in, and none is
                        // left past the end of a line or of the buffer. A
                        // republish of the version on screen changed nothing.
                        (None, None) if snap.version > old_version => {
                            tab.editor.map_cursors(buffer, |pos| match &change {
                                Some(edit) => map_caret(pos, edit),
                                None => pos,
                            });
                        },
                        (None, None) => {},
                    }
                }
                *highlights = (*snap.highlights).clone();
                *semantic_blocks = (*snap.semantic_blocks).clone();
                *folds = (*snap.folds).clone();
                *syntax_errors = snap.syntax_error_lines.as_ref().clone();
                tab.markdown_table_lines = std::sync::Arc::clone(&snap.table_lines);
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
        }
        if let Some((from, edit)) = carried {
            self.shift_inlay_hints(doc, from, snap.version, edit.as_slice());
            self.shift_code_lenses(doc, from, snap.version, edit.as_slice());
            self.shift_links_and_colors(doc, from, snap.version, edit.as_slice());
            self.shift_navigation(doc, from, snap.version, edit.as_slice());
        }
        if advanced {
            self.note_inlay_edit(doc, Instant::now());
            self.note_code_lens_edit(doc, Instant::now());
            self.note_link_color_edit(doc, Instant::now());
        }
        // The first snapshot of a document asks for its gutter markers; later
        // ones only when the text moved (a highlight pass republishes it as is).
        let unasked = !self.docs.line_changes.contains_key(&doc)
            && !self.docs.line_changes_pending.contains_key(&doc);
        if advanced || unasked {
            self.request_line_changes(doc);
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
        // so recompute the match highlights against the fresh text. Only the
        // highlights: a snapshot is not a motion, so the caret stays where it is.
        if self.find_open {
            self.refresh_find();
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
        // A first snapshot is what fills a reserved tab, so queued `--command`
        // work waiting on its text becomes servable here.
        self.run_startup_commands_when_ready();
    }
}

/// Where a caret at `pos` in the text before `edit` sits in the text after it.
///
/// Before the edit it stays, and after it, it moves with the text. Inside the
/// replaced span it keeps its place, but not past the replacement's end: the
/// span is the smallest single edit that explains two texts, so for a
/// formatter or several separate changes it can cover far more than what
/// changed, and keeping the line is truer than collapsing onto its start.
fn map_caret(pos: LineCol, edit: &karet_core::TextEdit) -> LineCol {
    let karet_core::Range { start, end } = edit.range;
    if pos > start && pos < end {
        return pos.min(karet_editor::editing::caret_after_insert(
            start,
            &edit.new_text,
        ));
    }
    super::inlay::map_position(pos, std::slice::from_ref(edit))
}

use super::*;

impl App {
    /// Scroll the active tab by `delta` lines/rows (clamped to its content).
    pub(super) fn scroll_lines(&mut self, delta: i32) {
        // The browser has no free scroll: a wheel notch moves the commit selection.
        if matches!(
            self.tabs.get(self.active).map(|t| &t.kind),
            Some(TabKind::CommitGraph { .. })
        ) {
            self.graph_select(delta.signum());
            return;
        }
        let word_wrap = self.tabs.get(self.active).is_some_and(|tab| {
            effective_word_wrap(
                tab,
                self.settings
                    .editor
                    .for_language(tab_language(tab))
                    .word_wrap(),
            )
        });
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code {
                buffer,
                folds,
                folded,
                ..
            } => {
                let fold_lines = resolve_folds(folds, folded);
                tab.editor
                    .scroll_rows(buffer, &fold_lines, word_wrap, delta);
            },
            // The wrapped length is known, so clamp to it rather than to `u16::MAX` —
            // otherwise scrolling past the end would silently bank offset that the
            // synchronized source pane would then read back as a jump.
            TabKind::MarkdownPreview {
                wrapped, scroll, ..
            } => {
                let max = wrapped.lines.len().saturating_sub(1) as i64;
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, max);
                *scroll = next as u16;
            },
            TabKind::Diff { scroll, .. }
            | TabKind::StashPreview { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
            | TabKind::CommitLoading { scroll, .. } => {
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                *scroll = next as u16;
            },
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                let next =
                    (i64::from(view.scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                view.scroll = next as u16;
            },
            TabKind::Hex { bytes, scroll, .. } => {
                let max = bytes.len().div_ceil(16).saturating_sub(1) as i64;
                let next = (*scroll as i64 + i64::from(delta)).clamp(0, max);
                *scroll = next as usize;
            },
            TabKind::Github(crate::app::github::GithubViewState::Issue { scroll, .. })
            | TabKind::Github(crate::app::github::GithubViewState::WorkflowRun {
                scroll, ..
            }) => {
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                *scroll = next as u16;
            },
            TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) => {
                let next =
                    (i64::from(view.scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                view.scroll = next as u16;
            },
            // Scrolling a document turns pages (one page per scroll gesture).
            #[cfg(feature = "pdf")]
            TabKind::Document {
                page, page_count, ..
            } => {
                let max = (*page_count).saturating_sub(1) as i64;
                let step = i64::from(delta.signum());
                *page = (*page as i64 + step).clamp(0, max) as usize;
            },
            _ => {},
        }
    }

    /// Scroll the active code tab's in-editor Markdown preview and align the
    /// source editor to the preview's nearest source anchor.
    pub(super) fn scroll_markdown_preview(&mut self, delta: i32) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return;
        };
        let Some(preview) = tab.markdown_preview.as_mut() else {
            return;
        };
        let max = preview.wrapped.lines.len().saturating_sub(1) as i64;
        let next = (i64::from(preview.scroll) + i64::from(delta)).clamp(0, max);
        preview.scroll = next as u16;
        let source = preview
            .wrapped
            .source_line_for_wrapped(usize::from(preview.scroll));
        let last = buffer.line_count().saturating_sub(1);
        tab.editor.scroll_line = u32::try_from(source.min(last)).unwrap_or(u32::MAX);
    }

    /// Scroll the active overflow-mode code tab horizontally by `delta` columns.
    pub(super) fn scroll_columns(&mut self, delta: i32) {
        let word_wrap = self.tabs.get(self.active).is_some_and(|tab| {
            effective_word_wrap(
                tab,
                self.settings
                    .editor
                    .for_language(tab_language(tab))
                    .word_wrap(),
            )
        });
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if word_wrap {
            return;
        }
        if let TabKind::Code { buffer, .. } = &tab.kind {
            tab.editor.scroll_columns(buffer, delta);
        }
    }

    /// Jump to the top or bottom of the active tab.
    pub(super) fn scroll_edge(&mut self, top: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } => {
                tab.editor.scroll_line = if top {
                    0
                } else {
                    buffer.line_count().saturating_sub(1) as u32
                };
            },
            TabKind::MarkdownPreview {
                wrapped, scroll, ..
            } => {
                let last = u16::try_from(wrapped.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                *scroll = if top { 0 } else { last };
            },
            TabKind::Diff { scroll, .. }
            | TabKind::StashPreview { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
            | TabKind::CommitLoading { scroll, .. } => {
                *scroll = if top { 0 } else { u16::MAX };
            },
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                view.scroll = if top { 0 } else { u16::MAX };
            },
            TabKind::Hex { bytes, scroll, .. } => {
                *scroll = if top {
                    0
                } else {
                    bytes.len().div_ceil(16).saturating_sub(1)
                };
            },
            TabKind::Github(crate::app::github::GithubViewState::Issue { scroll, .. })
            | TabKind::Github(crate::app::github::GithubViewState::WorkflowRun {
                scroll, ..
            }) => *scroll = if top { 0 } else { u16::MAX },
            TabKind::Github(crate::app::github::GithubViewState::PullRequest(view)) => {
                view.scroll = if top { 0 } else { u16::MAX };
            },
            #[cfg(feature = "pdf")]
            TabKind::Document {
                page, page_count, ..
            } => {
                *page = if top {
                    0
                } else {
                    (*page_count).saturating_sub(1)
                };
            },
            _ => {},
        }
    }

    /// Toggle the active diff tab between unified and side-by-side.
    pub(super) fn toggle_diff_layout(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let TabKind::Diff { view, scroll, .. } = &mut tab.kind
        {
            *view = match *view {
                ViewMode::Unified => ViewMode::SideBySide,
                ViewMode::SideBySide => ViewMode::Unified,
            };
            *scroll = 0;
            // Remember the choice so subsequently-opened diffs adopt it.
            self.diff_layout = *view;
        }
    }

    /// Fold or unfold the code region at the cursor: prefer a fold headered on the
    /// cursor line, else the innermost fold containing it. Collapsing a region the
    /// cursor sits inside relocates the caret to the (visible) header line.
    pub(super) fn toggle_fold(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let line = tab.editor.cursor().line;
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &mut tab.kind
        else {
            return;
        };
        let target = folds
            .regions()
            .iter()
            .find(|r| r.start == line)
            .or_else(|| {
                folds
                    .regions()
                    .iter()
                    .filter(|r| r.start <= line && line <= r.end)
                    .min_by_key(|r| r.end - r.start)
            })
            .copied();
        let Some(region) = target else {
            return;
        };
        // `remove` returns whether it was collapsed: toggle by remove-or-insert.
        if !folded.remove(&region.start) {
            folded.insert(region.start);
            if line > region.start {
                let pos = LineCol::new(region.start, tab.editor.cursor().col);
                tab.editor.set_caret(buffer, pos);
            }
        }
    }

    /// Replace the active diff tab with the next/previous changed file.
    pub(super) fn step_changed_file(&mut self, delta: i32) {
        if let Some(TabKind::Commit { files, view, .. } | TabKind::Compare { files, view, .. }) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            if files.is_empty() || view.file_anchors.is_empty() {
                return;
            }
            let current = view
                .file_anchors
                .iter()
                .rposition(|anchor| *anchor <= view.scroll);
            let next = current.map_or(0, |file| {
                (file as i64 + i64::from(delta))
                    .clamp(0, view.file_anchors.len().saturating_sub(1) as i64)
                    as usize
            });
            view.scroll = view.file_anchors[next];
            return;
        }
        if !self.active_is_diff() {
            return;
        }
        let len = self.scm.changes.len();
        if len == 0 {
            return;
        }
        let next = (self.scm.selection.cursor() as i64 + i64::from(delta)).clamp(0, len as i64 - 1)
            as usize;
        self.scm.selection.move_to(next);
        let view = match &self.tabs[self.active].kind {
            TabKind::Diff { view, .. } => *view,
            _ => ViewMode::Unified,
        };
        let change = self.scm.changes[next].clone();
        let section = self.scm.section(next);
        let title = change
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diff")
            .to_string();
        let file = FileView::new(change, section, self.syntax);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.title = title;
            tab.kind = TabKind::Diff {
                file: Box::new(file),
                view,
                scroll: 0,
            };
        }
    }

    /// Open the active diff's underlying file in a normal editor tab — the Enter
    /// action on a focused diff ("editor mode") — placing the caret at the diff's
    /// first changed line. Routes through [`open_path`](Self::open_path), so an
    /// already-open tab for the file is focused rather than duplicated. Degrades
    /// gracefully when the file is gone from the working tree (a deleted change):
    /// a status message, never a dead tab.
    pub(super) fn open_diff_file(&mut self) {
        let Some(TabKind::Diff { file, .. }) = self.tabs.get(self.active).map(|t| &t.kind) else {
            return;
        };
        let line = file.first_changed_line().unwrap_or(1);
        let path = file.change.path.clone();
        // Change paths come from the VCS repo-relative; resolve against the
        // workspace root so the file opens (and dedups) like any explorer open.
        let abs = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        if !abs.is_file() {
            let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            self.status = Some(format!("open file: {name} is not in the working tree"));
            return;
        }
        self.open_path(&abs);
        // Land the caret on the first changed line (`goto` clamps into the buffer;
        // a non-text tab — image, binary — simply has no caret to place).
        let pos = LineCol::new(line.saturating_sub(1), 0);
        let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        };
        if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
            tab.editor.goto(&buffer, pos);
        }
    }
    /// Apply a caret `motion` to the active code tab, extending the selection when
    /// `extend` is set and clearing it otherwise.
    pub(super) fn caret_motion(
        &mut self,
        extend: bool,
        motion: impl Fn(&mut EditorState, &TextBuffer),
    ) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            // The motion moves every caret's head; a non-extending motion then
            // collapses each selection onto its new head, while an extending one keeps
            // the anchors so the selection grows.
            motion(editor, buffer);
            if !extend {
                editor.clear_selection();
            }
        }
    }

    /// Select the whole buffer in the active editor tab (Ctrl+A).
    pub(super) fn editor_select_all(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.select_all(buffer);
        }
    }

    /// Add a caret one line above or below the primary caret (Ctrl+Alt+Up/Down).
    pub(super) fn add_cursor_vertical(&mut self, above: bool) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            if above {
                editor.add_caret_above(buffer);
            } else {
                editor.add_caret_below(buffer);
            }
        }
    }

    /// Select the word under the caret, then add a caret at the next occurrence
    /// (Ctrl+D).
    pub(super) fn add_cursor_next_occurrence(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.add_next_occurrence(buffer);
        }
    }

    /// Esc in the editor: collapse multiple carets to the primary; with a single
    /// caret it is a no-op, so repeated Esc never leaves the editor view.
    pub(super) fn collapse_carets_or_unfocus(&mut self) {
        let multi = matches!(
            self.tabs.get(self.active),
            Some(Tab {
                kind: TabKind::Code { .. },
                editor,
                ..
            }) if editor.has_multiple_cursors()
        );
        if multi && let Some(Tab { editor, .. }) = self.tabs.get_mut(self.active) {
            editor.collapse_to_primary();
        }
    }

    /// Update and return the multi-click streak for a click at `(col, row)`.
    pub(super) fn click_streak(&mut self, col: u16, row: u16) -> u8 {
        let now = Instant::now();
        let streak = match self.last_click {
            Some((t, c, r))
                if c == col && r == row && now.duration_since(t) < Duration::from_millis(400) =>
            {
                self.click_streak % 3 + 1
            },
            _ => 1,
        };
        self.last_click = Some((now, col, row));
        self.click_streak = streak;
        streak
    }

    /// Handle a left click in the editor: focus it and place the caret (single
    /// click), extend the selection to the click (Shift+click), or select the word
    /// (double) / line (triple).
    pub(super) fn handle_editor_click(&mut self, mouse: MouseEvent) {
        let point = (mouse.column, mouse.row);
        // Route the click to the pane whose content it landed in, focusing it.
        let Some((pane, area, file_hit)) = self
            .pane_frames
            .iter()
            .find(|f| rect_contains(f.content_rect, point))
            .map(|f| {
                (
                    f.pane,
                    f.content_rect,
                    f.commit_file_hits
                        .iter()
                        .find(|hit| rect_contains(hit.rect, point))
                        .copied(),
                )
            })
        else {
            return;
        };
        self.focus_pane_switch(pane);
        self.focus = Focus::Editor;
        if let Some(hit) = file_hit
            && let Some(TabKind::Commit { view, .. } | TabKind::Compare { view, .. }) =
                self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            view.scroll = hit.scroll;
            self.editor_selecting = false;
            return;
        }
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        let alt = mouse.modifiers.contains(KeyModifiers::ALT);
        let streak = self.click_streak(mouse.column, mouse.row);
        // Double-clicking the commit view's signature badge reveals, for a few seconds,
        // what its "Verified" / "Signed" state means.
        if streak == 2
            && self
                .commit_badge_rect
                .is_some_and(|r| rect_contains(r, point))
            && let Some(Tab {
                kind: TabKind::Commit { explain_since, .. },
                ..
            }) = self.tabs.get_mut(self.active)
        {
            *explain_since = Some(Instant::now());
            self.editor_selecting = false;
            return;
        }
        if let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    folds,
                    folded,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let fold_lines = resolve_folds(folds, folded);
            let pos = editor.pos_at(area, buffer, &fold_lines, mouse.column, mouse.row);
            match streak {
                2 => {
                    let (anchor, head) = word_at(buffer, pos);
                    editor.set_selection(buffer, anchor, head);
                },
                3 => {
                    let (anchor, head) = line_span(buffer, pos.line);
                    editor.set_selection(buffer, anchor, head);
                },
                // Alt+click adds (or toggles off) a caret at the click, building a
                // multi-cursor set.
                _ if alt => editor.add_caret(buffer, pos),
                // Shift+click extends the selection from the current caret to the click
                // point (VS Code style); a plain click places the caret, discarding any
                // secondary carets.
                _ if shift => {
                    editor.collapse_to_primary();
                    editor.extend_to(buffer, pos);
                },
                _ => editor.set_caret(buffer, pos),
            }
        }
        // A single click (plain or shift) starts a drag-select so the pointer can
        // keep extending; word/line clicks are atomic.
        self.editor_selecting = streak == 1;
    }

    /// Extend the editor selection to the cell under `(col, row)` while dragging.
    pub(super) fn drag_select_to(&mut self, col: u16, row: u16) {
        let area = self.editor_rect;
        if let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    folds,
                    folded,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let fold_lines = resolve_folds(folds, folded);
            let pos = editor.pos_at(area, buffer, &fold_lines, col, row);
            editor.extend_to(buffer, pos);
        }
    }
    // Editing routes through the headless session backend and reflects its
    // snapshots back into the active code tab.
    /// Register every already-open code tab with the session (called once the
    /// backend is attached at startup).
    pub(super) fn register_open_tabs(&mut self) {
        for idx in 0..self.tabs.len() {
            self.register_doc(idx);
        }
    }

    /// Register the code tab at `idx` with the session so it can be edited, if it is
    /// an as-yet-unregistered code tab and a backend is attached.
    pub(super) fn register_doc(&mut self, idx: usize) {
        let (path, view) = match self.tabs.get(idx) {
            Some(Tab {
                kind: TabKind::Code {
                    path, doc: None, ..
                },
                view,
                ..
            }) => (path.clone(), *view),
            _ => return,
        };
        let Some(backend) = &self.backend else {
            return;
        };
        let id = backend.next_id();
        let _ = backend.send(
            id,
            SessionCommand::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        self.pending_open.insert(id, PendingOpen { path, view });
    }

    /// Build an edit from the active code tab's caret/selection via `build` and
    /// submit it through the session, moving the caret optimistically.
    pub(super) fn submit_edit<F>(&mut self, build: F)
    where
        F: Fn(LineCol, Option<Range>, &TextBuffer, u64) -> Option<editing::Edit>,
    {
        self.submit_edit_with_cause(EditCause::Replace, build);
    }

    pub(super) fn submit_edit_with_cause<F>(&mut self, cause: EditCause, build: F)
    where
        F: Fn(LineCol, Option<Range>, &TextBuffer, u64) -> Option<editing::Edit>,
    {
        if self.backend.is_none() {
            return;
        }
        let idx = self.active;
        // Build one edit per selection against the same base version, then flatten to a
        // single non-overlapping batch (the buffer applies it bottom-up). Each caret is
        // repositioned by the edits that fall strictly before its selection. With a
        // single cursor this collapses to exactly the former single-edit behavior.
        let (doc, base, edits, carets) = match self.tabs.get(idx) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        doc: Some(doc),
                        buffer,
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => {
                let base = *next_version;
                let mut per: Vec<(LineCol, Vec<TextEdit>, LineCol)> = Vec::new();
                for sel in &editor.cursors().selections {
                    let range = sel.range();
                    let selection = (!range.is_empty()).then_some(range);
                    if let Some(e) = build(sel.head, selection, buffer, base) {
                        per.push((range.start, e.change.edits, e.caret));
                    }
                }
                if per.is_empty() {
                    return;
                }
                per.sort_by_key(|(start, ..)| *start);
                // Track which per-entry (cursor) each flattened edit belongs to, so
                // "earlier" below can mean "from a cursor before this one" rather
                // than a byte-position comparison — a backward-deleting edit (e.g.
                // backspace) starts *before* its own original caret, so comparing
                // positions would wrongly count an edit as "earlier than itself"
                // and double-shift that same cursor's landing caret by one extra
                // position on every backspace.
                let mut flat: Vec<TextEdit> = Vec::new();
                let mut owner: Vec<usize> = Vec::new();
                for (i, (_, es, _)) in per.iter().enumerate() {
                    for e in es {
                        flat.push(e.clone());
                        owner.push(i);
                    }
                }
                let carets: Vec<LineCol> = per
                    .iter()
                    .enumerate()
                    .map(|(i, (_, _, local))| {
                        let earlier: Vec<TextEdit> = flat
                            .iter()
                            .zip(&owner)
                            .filter(|&(_, &o)| o < i)
                            .map(|(e, _)| e.clone())
                            .collect();
                        editing::reflow_caret(*local, &earlier)
                    })
                    .collect();
                (*doc, base, flat, carets)
            },
            _ => return,
        };
        let change = Change::new(base, edits);
        if let Some(backend) = &self.backend {
            let id = backend.next_id();
            let _ = backend.send(
                id,
                SessionCommand::ApplyChange {
                    doc,
                    change: change.clone(),
                    cause,
                },
            );
        }
        let mut auto_save_version = None;
        if let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    text,
                    next_version,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(idx)
        {
            // Apply the same change locally so the displayed text advances in
            // lockstep with the caret instead of lagging behind the async
            // snapshot echo (the prior cause of "backspace skips characters"
            // under fast/held input). `base` was just read from this same
            // buffer above, so this should never fail; if it somehow does,
            // leave `buffer`/`text` alone and let the next snapshot resync.
            if let Ok(applied) = buffer.apply(
                &change,
                karet_text::EditContext {
                    cause,
                    ..Default::default()
                },
            ) {
                *next_version = applied.version;
                *text = buffer.text();
                auto_save_version = Some(applied.version);
            }
            editor.set_carets(&carets);
            let head = editor.cursor();
            editor.scroll_to(head);
        }
        if let Some(version) = auto_save_version {
            self.schedule_auto_save(doc, version, Instant::now());
        }
    }
}

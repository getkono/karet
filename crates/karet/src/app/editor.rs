pub(super) mod column;
mod folding;
mod gutter;
mod render;

use karet_editor::Motion;
use karet_session::FormatRequest;
use karet_session::FormatScope;

use super::*;
use crate::app::scroll::adjust;

/// What a left click landed on, for multi-click counting: a press on another
/// surface than the last one starts a new streak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClickSurface {
    /// The text of the editor in this pane. A press within one cell of the last
    /// one continues the streak, so a hand that drifts a cell between presses
    /// still double-clicks.
    Editor(PaneId),
    /// Any other surface: tabs, sidebar rows, panels. Their rows are separate
    /// targets, so only a press on the very same cell continues the streak.
    Other,
}

impl App {
    /// The display width of hard tabs in `tab`, after per-document EditorConfig and
    /// language settings have been resolved.
    pub(crate) fn tab_width_for(&self, tab: &Tab) -> u16 {
        let fallback = u16::from(
            self.settings
                .editor
                .for_language(tab_language(tab))
                .tab_size(),
        );
        App::tab_doc(tab)
            .and_then(|doc| self.docs.settings.get(&doc))
            .map_or(fallback, |settings| settings.tab_width)
            .max(1)
    }

    /// The concrete indentation string for the active document. EditorConfig may
    /// combine hard tabs and spaces when an indent is not divisible by tab width.
    pub(super) fn active_indentation(&self) -> String {
        let tab = self.tabs.get(self.active);
        let resolved = tab.map(|tab| self.settings.editor.for_language(tab_language(tab)));
        let fallback_size = resolved.map_or(4, |settings| u16::from(settings.tab_size()));
        let fallback_spaces = resolved.is_none_or(|settings| settings.insert_spaces());
        let document = tab
            .and_then(App::tab_doc)
            .and_then(|doc| self.docs.settings.get(&doc).copied());
        let size = usize::from(document.map_or(fallback_size, |settings| settings.indent_size));
        let insert_spaces = document.map_or(fallback_spaces, |settings| settings.insert_spaces);
        if insert_spaces {
            return " ".repeat(size);
        }
        let tab_width = tab.map_or(1, |tab| self.tab_width_for(tab)) as usize;
        format!(
            "{}{}",
            "\t".repeat(size / tab_width),
            " ".repeat(size % tab_width)
        )
    }

    /// Scroll the active tab by `delta` lines/rows (clamped to its content).
    pub(super) fn scroll_lines(&mut self, delta: i32) {
        // The graph view pans freely — the wheel moves the viewport, not the selection,
        // so a wide history can be read without dragging the cursor across it.
        if let Some(TabKind::CommitGraph { list_offset, .. }) =
            self.tabs.get(self.active).map(|t| &t.kind)
        {
            let target = i64::from(*list_offset) + i64::from(delta);
            self.graph_scroll_to(target.max(0) as usize);
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
        // Borrowed before the tab, from a different field, so the manager arm can
        // read the inventory it filters while holding its view mutably.
        let language_servers = &self.lsp_runtime.servers;
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
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::ServerLog { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => adjust(&mut pager.scroll, delta),
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                adjust(&mut view.scroll, delta);
            },
            TabKind::Hex { bytes, scroll, .. } => {
                let max = bytes.len().div_ceil(16).saturating_sub(1) as i64;
                let next = (*scroll as i64 + i64::from(delta)).clamp(0, max);
                *scroll = next as usize;
            },
            // One server per notch, not three: the render pins this view's offset to
            // its selection, so a notch has to move the *selection* to move the
            // window — the same bargain the Seam spine and the outline already make.
            // A notch therefore also retargets the action strip, which is the point:
            // the card under the cursor is the one "Restart" restarts.
            TabKind::LanguageServers(view) => {
                view.select_relative(view.visible_indices(language_servers).len(), delta.signum());
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
        let current = self
            .tabs
            .get(self.active)
            .and_then(|tab| tab.markdown_preview.as_ref())
            .map_or(0, |preview| usize::from(preview.scroll));
        let next = (current as i64 + i64::from(delta)).max(0) as usize;
        self.set_markdown_preview_scroll(next);
    }

    /// Put the in-editor Markdown preview's first visible wrapped line at `position`.
    ///
    /// The source pane follows, exactly as it does for the wheel: the two are kept in
    /// sync through the preview's source anchors, so a dragged preview bar must not
    /// leave the code beside it behind.
    pub(super) fn set_markdown_preview_scroll(&mut self, position: usize) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return;
        };
        let Some(preview) = tab.markdown_preview.as_mut() else {
            return;
        };
        let max = preview.wrapped.lines.len().saturating_sub(1);
        preview.scroll = u16::try_from(position.min(max)).unwrap_or(u16::MAX);
        let source = preview
            .wrapped
            .source_line_for_wrapped(usize::from(preview.scroll));
        let last = buffer.line_count().saturating_sub(1);
        tab.editor
            .scroll_to_line(u32::try_from(source.min(last)).unwrap_or(u32::MAX));
    }

    /// Jump to the top or bottom of the active tab.
    pub(super) fn scroll_edge(&mut self, top: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } => {
                tab.editor.scroll_to_line(if top {
                    0
                } else {
                    buffer.line_count().saturating_sub(1) as u32
                });
            },
            TabKind::MarkdownPreview {
                wrapped, scroll, ..
            } => {
                let last = u16::try_from(wrapped.lines.len().saturating_sub(1)).unwrap_or(u16::MAX);
                *scroll = if top { 0 } else { last };
            },
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::ServerLog { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => {
                pager.scroll = if top { 0 } else { u16::MAX };
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

    /// Open the active diff's underlying file in a normal editor tab — the Enter
    /// action on a focused diff ("editor mode") — placing the caret at the diff's
    /// first changed line. Routes through [`open_path`](Self::open_path), so an
    /// already-open tab for the file is focused rather than duplicated. Degrades
    /// gracefully when the file is gone from the working tree (a deleted change):
    /// a refusal, never a dead tab.
    pub(super) fn open_diff_file(&mut self) {
        let Some(TabKind::Diff { path, file, .. }) = self.tabs.get(self.active).map(|t| &t.kind)
        else {
            return;
        };
        let line = file
            .as_ref()
            .and_then(|file| file.first_changed_line())
            .unwrap_or(1);
        let path = path.clone();
        // Change paths come from the VCS repo-relative; `focus_by_file_line` resolves
        // them against the workspace root, but the working-tree check has to happen
        // here — a deleted change must produce a refusal, not a new empty file.
        let abs = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        if !abs.is_file() {
            let name = abs.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                format!("open file: {name} is not in the working tree"),
            );
            return;
        }
        // Land the caret on the diff's first changed line (1-based from the VCS).
        self.focus_by_file_line(&abs, LineCol::new(line.saturating_sub(1), 0), true);
    }
    /// Fold or unfold the code region at the cursor: prefer a fold headered on the
    /// cursor line, else the innermost fold containing it. Collapsing a region the
    /// cursor sits inside relocates the caret to the (visible) header line.
    pub(super) fn toggle_fold(&mut self) {
        if let Some(region) = self.region_at_cursor() {
            self.toggle_fold_at(region.start);
        }
    }

    /// Format Document (Shift+Alt+F) and Format Selection (Ctrl+K Ctrl+F): ask
    /// the session to format the active code tab, whole or at every selection
    /// (a bare caret formats its line), with the formatter its language names.
    ///
    /// The edits come back as an ordinary snapshot, so they undo as one step
    /// and every caret follows the text it sat in. A refusal -- no formatter,
    /// or a server without the method -- is the session's to report.
    pub(super) fn format_active(&mut self, selection: bool) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code { doc: Some(doc), .. } = &tab.kind else {
            return;
        };
        let doc = *doc;
        let scope = if selection {
            let selections = &tab.editor.cursors().selections;
            FormatScope::Ranges(selections.iter().map(|s| s.range()).collect())
        } else {
            FormatScope::Document
        };
        let _ = self.send(LspCommand::Format(FormatRequest { doc, scope }));
    }

    /// Expand every collapsed fold that hides a caret, in every code tab of
    /// every pane, so no caret is ever hidden.
    ///
    /// Caret motion never enters a fold and collapsing one lifts the carets
    /// out of it, so what this catches is a jump made without regard to
    /// folds — find, go to definition, an outline or problems entry, an undo
    /// — which opens the fold it lands in, as VS Code does. Run before each
    /// frame is drawn, so it covers every such path at once.
    pub(crate) fn unfold_around_carets(&mut self) {
        for tab in self.all_tabs_mut() {
            let TabKind::Code { folds, folded, .. } = &mut tab.kind else {
                continue;
            };
            if folded.is_empty() {
                continue;
            }
            for selection in &tab.editor.cursors().selections {
                let line = selection.head.line;
                folded.retain(|start| {
                    !folds
                        .regions()
                        .iter()
                        .any(|r| r.start == *start && r.start < line && line <= r.end)
                });
            }
        }
    }

    /// Apply a caret `motion` to the active code tab, extending the selection when
    /// `extend` is set and collapsing it first otherwise.
    pub(super) fn caret_motion(&mut self, extend: bool, motion: Motion) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.move_carets(buffer, motion, extend);
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

    /// Run the multi-caret operation `op` on the active code tab: Ctrl+D, Select
    /// All Occurrences, Skip Occurrence, Add Cursors to Line Ends, and Undo Last
    /// Cursor Operation.
    pub(super) fn caret_op(&mut self, op: impl FnOnce(&mut EditorState, &TextBuffer)) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            op(editor, buffer);
        }
    }

    /// Esc in the editor: with several carets keep only the first-placed one, with
    /// its selection; with one caret clear its selection. Focus never leaves the
    /// editor, so repeated Esc is harmless.
    pub(super) fn collapse_carets_or_unfocus(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            if editor.has_multiple_cursors() {
                editor.collapse_to_original(buffer);
            } else {
                editor.clear_selection();
            }
        }
    }

    /// Update and return the multi-click streak for an unmodified click at
    /// `(col, row)` on a surface other than the editor's text.
    pub(super) fn click_streak(&mut self, col: u16, row: u16) -> u8 {
        self.click_streak_on(col, row, ClickSurface::Other, KeyModifiers::NONE)
    }

    /// Update and return the multi-click streak for a click at `(col, row)` on
    /// `surface` with `modifiers` held.
    ///
    /// The click continues the streak when it comes within the multi-click
    /// interval, on the same surface, with the same modifiers, and near enough
    /// the last one (see [`ClickSurface`]); otherwise it starts a new one. In
    /// the editor's text the streak stops counting at three, so a fourth quick
    /// click keeps line granularity rather than wrapping back to a single click.
    pub(super) fn click_streak_on(
        &mut self,
        col: u16,
        row: u16,
        surface: ClickSurface,
        modifiers: KeyModifiers,
    ) -> u8 {
        let now = Instant::now();
        let editor = matches!(surface, ClickSurface::Editor(_));
        let slack = u16::from(editor);
        let continues = self.click_origin == (surface, modifiers)
            && self.last_click.is_some_and(|(t, c, r)| {
                now.duration_since(t) < MULTI_CLICK_INTERVAL
                    && c.abs_diff(col) <= slack
                    && r.abs_diff(row) <= slack
            });
        let streak = if !continues {
            1
        } else if editor {
            // In text the count stops at three: a fourth quick click keeps the
            // line rather than collapsing it back to a caret.
            self.click_streak.saturating_add(1).min(3)
        } else {
            // Elsewhere only "a second click or later" is read, and the count
            // keeps cycling 1, 2, 3, 1, so four quick clicks on a list row act
            // as they always have.
            self.click_streak % 3 + 1
        };
        self.last_click = Some((now, col, row));
        self.click_origin = (surface, modifiers);
        self.click_streak = streak;
        streak
    }

    /// Handle a left click in the editor: focus it and place the caret (single
    /// click), extend the selection to the click (Shift+click), or select the word
    /// (double) / line (triple).
    pub(super) fn handle_editor_click(&mut self, mouse: MouseEvent) {
        let point = (mouse.column, mouse.row);
        // Route the click to the pane whose content it landed in, focusing it.
        let Some((pane, area, file_hit, dir_hit, collapse_hit)) = self
            .pane_frames
            .iter()
            .find(|f| rect_contains(f.content_rect, point))
            .map(|f| {
                (
                    f.pane,
                    f.editor_rect,
                    f.commit_file_hits
                        .iter()
                        .find(|hit| rect_contains(hit.rect, point))
                        .copied(),
                    f.commit_dir_hits
                        .iter()
                        .find(|hit| rect_contains(hit.rect, point))
                        .cloned(),
                    f.commit_collapse_hits
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
        if self.handle_language_server_click(mouse.column, mouse.row) {
            self.editor_drag = None;
            return;
        }
        if collapse_hit.is_some_and(|hit| self.toggle_commit_file(hit.file)) {
            self.editor_drag = None;
            return;
        }
        // Ahead of the file rows: a directory row folds rather than jumping the diff.
        if dir_hit.is_some_and(|hit| self.toggle_commit_dir(&hit.path)) {
            self.editor_drag = None;
            return;
        }
        if let Some(hit) = file_hit
            && let Some(TabKind::Commit { view, .. } | TabKind::Compare { view, .. }) =
                self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            view.scroll = hit.scroll;
            self.editor_drag = None;
            return;
        }
        // A click on a graph row selects that commit and opens it as its own tab.
        if self.graph_click(point) {
            self.editor_drag = None;
            return;
        }
        if !rect_contains(area, point) {
            self.editor_drag = None;
            return;
        }
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        let alt = mouse.modifiers.contains(KeyModifiers::ALT);
        let streak = self.click_streak_on(
            mouse.column,
            mouse.row,
            ClickSurface::Editor(pane),
            mouse.modifiers,
        );
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
            self.editor_drag = None;
            return;
        }
        // The gutter's lanes, and a collapsed fold's `⋯`, are controls rather
        // than text: each does its own thing (`gutter.rs`).
        if self.gutter_click(mouse, area, pane) {
            return;
        }
        let code_pos = self.tabs.get(self.active).and_then(|tab| {
            let TabKind::Code {
                buffer,
                folds,
                folded,
                ..
            } = &tab.kind
            else {
                return None;
            };
            let fold_lines = resolve_folds(folds, folded);
            // The character under the pointer, not where a click would put the
            // caret: `pos_at` rounds the right half of a wide character or a
            // tab's trailing cells to the next column, which would make the
            // word gestures below pick the neighbouring word.
            Some(
                tab.editor
                    .char_pos_at(area, buffer, &fold_lines, mouse.column, mouse.row),
            )
        });
        // Ctrl/Cmd+click jumps to the definition of the identifier under the
        // pointer. Anywhere else (whitespace, punctuation, past the line end,
        // the gutter) there is no symbol to resolve, and the click does nothing
        // rather than ask for a definition that cannot exist. Alt and Shift keep
        // their multi-caret and extend-selection meanings.
        if mouse
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
            && !alt
            && !shift
        {
            if let Some((pos, _)) = self.identifier_under(area, point) {
                self.go_to_definition_at(pos);
            }
            // A Ctrl+drag must not turn into a text selection.
            self.editor_drag = None;
            return;
        }
        // Shift+Alt+click box-selects from the box in progress (or the primary
        // selection's anchor) to the cell under the pointer, and arms a drag
        // that keeps growing the box.
        if alt && shift {
            self.column_select_pointer(area, mouse.column, mouse.row);
            let view = self.tabs.get(self.active).map(|tab| tab.view);
            self.editor_drag = code_pos.zip(view).map(|(pos, view)| EditorDrag {
                anchor: (pos, pos),
                granularity: DragGranularity::Column,
                pointer: (mouse.column, mouse.row),
                area,
                pane,
                view,
                last_step: None,
            });
            return;
        }
        // A modifier outranks the click count: a quick second Alt+click toggles
        // the caret it just added, and a quick second Shift+click extends again,
        // neither selecting a word.
        let modified = alt || shift;
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
            let under = code_pos.unwrap_or(pos);
            match streak {
                // Alt+click adds (or toggles off) a caret at the click, building a
                // multi-cursor set.
                _ if alt => editor.add_caret(buffer, pos),
                // Shift+click extends the selection from the current caret to the click
                // point (VS Code style).
                _ if shift => {
                    editor.collapse_to_primary();
                    editor.extend_to(buffer, pos);
                },
                // A plain click places the caret, discarding any secondary carets.
                1 => editor.set_caret(buffer, pos),
                // A double click selects the word, punctuation run or whitespace
                // run under the pointer.
                2 => {
                    let (anchor, head) = class_run_at(buffer, under);
                    editor.set_selection(buffer, anchor, head);
                },
                // A triple click, and every quick click after it, selects the
                // whole line with its line break.
                _ => {
                    let (anchor, head) = line_span(buffer, under.line);
                    editor.set_selection(buffer, anchor, head);
                },
            }
        }
        // Every click arms a drag; the streak decides the unit it extends by, so
        // a double-click drag grows word by word and a triple-click line by line.
        let granularity = match streak {
            _ if modified => DragGranularity::Character,
            1 => DragGranularity::Character,
            2 => DragGranularity::Word,
            _ => DragGranularity::Line,
        };
        self.editor_drag = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code { buffer, .. } => {
                let pos = code_pos?;
                Some(EditorDrag {
                    anchor: App::drag_span(buffer, pos, granularity),
                    granularity,
                    pointer: (mouse.column, mouse.row),
                    area,
                    pane,
                    view: tab.view,
                    last_step: None,
                })
            },
            _ => None,
        });
    }

    /// Paste the primary selection at a middle click in a code tab's text, as
    /// X11 and Wayland applications do (`editor.middleClickPaste`): focus the
    /// pane, put a single caret at the click, and insert there. With the
    /// setting off, or outside editable text, it does nothing.
    pub(super) fn middle_click_paste(&mut self, mouse: MouseEvent) {
        if !self.settings.editor.middle_click_paste() {
            return;
        }
        let point = (mouse.column, mouse.row);
        let Some((pane, area)) = self
            .pane_frames
            .iter()
            .find(|frame| rect_contains(frame.editor_rect, point))
            .map(|frame| (frame.pane, frame.editor_rect))
        else {
            return;
        };
        self.focus_pane_switch(pane);
        if !matches!(
            self.tabs.get(self.active).map(|tab| &tab.kind),
            Some(TabKind::Code { doc: Some(_), .. })
        ) {
            return;
        }
        self.focus = Focus::Editor;
        let text = match self.clipboard.get_primary() {
            Ok(text) => text.replace("\r\n", "\n").replace('\r', "\n"),
            Err(_) => {
                self.notify(
                    Report::Failure,
                    NotificationKind::System,
                    "paste: no primary selection available",
                );
                return;
            },
        };
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
            editor.set_caret(buffer, pos);
        }
        if !text.is_empty() {
            self.submit_edit_with_cause(EditCause::Paste, move |caret, sel, _, base| {
                Some(editing::insert(caret, sel, base, &text))
            });
        }
    }

    // Editing routes through the headless session backend and reflects its
    // snapshots back into the active code tab.
    /// Register every already-open code tab with the session (called once the
    /// backend is attached at startup), in every pane: a `--split` or a
    /// restored layout leaves tabs in panes that are not focused.
    pub(super) fn register_open_tabs(&mut self) {
        for idx in 0..self.tabs.len() {
            self.register_doc(idx);
        }
        let panes: Vec<PaneId> = self.stored.keys().copied().collect();
        for pane in panes {
            let Some(mut stored) = self.stored.remove(&pane) else {
                continue;
            };
            // `register_doc` reads the focused pane's tabs; lend it this
            // pane's for the duration, without moving focus or recording an
            // activation.
            std::mem::swap(&mut self.tabs, &mut stored.tabs);
            for idx in 0..self.tabs.len() {
                self.register_doc(idx);
            }
            std::mem::swap(&mut self.tabs, &mut stored.tabs);
            self.stored.insert(pane, stored);
        }
    }

    /// Register the code tab at `idx` with the session so it can be edited, if it is
    /// an as-yet-unregistered code tab and a backend is attached. A reserved DOCX
    /// preview instead gets its backend conversion request.
    pub(super) fn register_doc(&mut self, idx: usize) {
        let (path, view) = match self.tabs.get(idx) {
            Some(Tab {
                kind: TabKind::Code {
                    path, doc: None, ..
                },
                view,
                ..
            }) => (path.clone(), *view),
            Some(Tab {
                kind:
                    TabKind::MarkdownPreview {
                        path,
                        pending_since: Some(_),
                        ..
                    },
                view,
                ..
            }) => {
                let (path, view) = (path.clone(), *view);
                if self
                    .pending_conversions
                    .values()
                    .any(|owner| *owner == view)
                {
                    return; // already requested (attach re-walks every tab)
                }
                if let Some(request) = self.send(DocumentCommand::Convert { path }) {
                    self.pending_conversions.insert(request, view);
                }
                return;
            },
            _ => return,
        };
        let Some(backend) = &self.backend else {
            return;
        };
        let id = backend.next_id();
        let _ = backend.send(
            id,
            DocumentCommand::Open {
                path: path.clone(),
                language: None,
            }
            .into(),
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
        if let Some(edit) = self.compose_edit(build) {
            self.submit_multi_edit(cause, edit);
        }
    }

    /// The batch [`Self::submit_edit_with_cause`] would submit for `build`,
    /// for a caller that adds to it before submitting it itself. `None` with
    /// no session, no code tab, or no selection building a text change.
    pub(super) fn compose_edit<F>(&self, build: F) -> Option<editing::MultiEdit>
    where
        F: Fn(LineCol, Option<Range>, &TextBuffer, u64) -> Option<editing::Edit>,
    {
        self.backend.as_ref()?;
        // Build one edit per selection against the same base version, then compose
        // them into one non-overlapping batch: overlapping edits merge, and a
        // selection whose edit is a no-op stays put. With a single cursor this is
        // exactly the single edit.
        match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        doc: Some(_),
                        buffer,
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => {
                let base = *next_version;
                let cursors = editor.cursors();
                let items: Vec<_> = cursors
                    .selections
                    .iter()
                    .map(|sel| {
                        let range = sel.range();
                        let selection = (!range.is_empty()).then_some(range);
                        (*sel, build(sel.head, selection, buffer, base))
                    })
                    .collect();
                editing::compose(base, cursors.primary, &items)
            },
            _ => None,
        }
    }

    /// Submit a [`editing::MultiEdit`] built over the active code tab's whole cursor
    /// set (e.g. a comment toggle), adopting the cursor set it leaves behind.
    pub(super) fn submit_multi_edit(&mut self, cause: EditCause, edit: editing::MultiEdit) {
        let doc = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { doc: Some(doc), .. },
                ..
            }) if self.backend.is_some() => *doc,
            _ => return,
        };
        let cursors = edit.cursors;
        self.apply_doc_change(
            self.active,
            doc,
            edit.change,
            cause,
            move |editor, buffer| {
                editor.set_cursor_state(buffer, cursors);
            },
        );
    }

    /// Apply `change` locally to tab `idx` and, once the buffer accepts it, send it
    /// to the session and let `place` position the cursors against the edited
    /// buffer before revealing the caret. A change the buffer rejects moves
    /// nothing and is never sent.
    fn apply_doc_change<F>(
        &mut self,
        idx: usize,
        doc: DocumentId,
        change: Change,
        cause: EditCause,
        place: F,
    ) where
        F: FnOnce(&mut EditorState, &TextBuffer),
    {
        let base = change.base_version;
        let mut landed = None;
        if let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    text,
                    next_version,
                    ..
                },
            editor,
            view,
            // A tab still waiting for its text holds an empty placeholder, not
            // the document: a change built against it never lands.
            fill_caret: None,
            ..
        }) = self.tabs.get_mut(idx)
        {
            let before = editor.cursors().clone();
            // Apply the same change locally so the displayed text advances in
            // lockstep with the caret instead of lagging behind the async
            // snapshot echo (the prior cause of "backspace skips characters"
            // under fast/held input). `base` was read from this same buffer, so
            // the session would reject whatever this rejects: on failure leave
            // the text, the carets and the session alone. A buffer another
            // split view still shares is copied first (the rope structurally),
            // so this view runs ahead alone until the snapshot reunites them.
            let Ok(applied) = std::sync::Arc::make_mut(buffer).apply(
                &change,
                karet_text::EditContext {
                    cause,
                    ..Default::default()
                },
            ) else {
                return;
            };
            *next_version = applied.version;
            *text = buffer.text_snapshot();
            place(editor, buffer);
            landed = Some((applied.version, *view, before, editor.cursors().clone()));
            let head = editor.cursor();
            // `reveal`, not `scroll_to`: the merge-conflict panes copy this
            // editor's `scroll_col` before it next renders.
            editor.reveal(buffer, head);
        }
        if let Some((version, view, before, after)) = landed {
            self.carry_auto_closers(view, (base, version), (&before, &after), &change.edits);
            self.carry_snippet(view, (base, version), &change.edits);
            if let Some(backend) = &self.backend {
                // The selection set on either side of the edit, each reported
                // at the version it describes: the one before is what undoing
                // the edit restores, the one after what redoing it restores.
                let report = |cursors| DocumentCommand::SetCursor { doc, view, cursors };
                let apply = DocumentCommand::ApplyChange {
                    doc,
                    change: change.clone(),
                    cause,
                };
                for command in [report(before), apply, report(after)] {
                    let _ = backend.send(backend.next_id(), command.into());
                }
            }
            // The hints on screen move with the text until the server's
            // replacement for this version arrives.
            self.shift_inlay_hints(doc, base, version, &change.edits);
            self.shift_code_lenses(doc, base, version, &change.edits);
            self.shift_links_and_colors(doc, base, version, &change.edits);
            self.shift_navigation(doc, base, version, &change.edits);
            self.note_inlay_edit(doc, Instant::now());
            self.note_code_lens_edit(doc, Instant::now());
            self.note_link_color_edit(doc, Instant::now());
            self.schedule_auto_save(doc, version, Instant::now());
        }
    }
}

use super::*;

impl App {
    /// Open the find-in-file bar (only over a text/code tab). Restores this tab's
    /// last query/toggles if it has one (from a previous open-then-Esc on the same
    /// tab) instead of always starting blank.
    pub(super) fn open_find(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { .. },
            find,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let find = find.get_or_insert_with(FindState::default);
            find.focused = true;
            // A bar that was closed by any path — Esc, a tab switch, a tab
            // close, a preview install — reopens without the Search in
            // Selection scope it had: the selection it was captured from is
            // gone. Re-focusing a bar that is still open keeps it.
            if !self.find_open {
                find.scope = None;
            }
            self.find_open = true;
            self.focus = Focus::Editor;
            // Rebuild decorations against the current buffer — cheap no-op for a
            // blank query, necessary to refresh a restored non-empty one. Opening
            // (or re-focusing) the bar is not a motion: the caret stays put, and
            // the next Enter moves from it.
            self.refresh_find();
        } else {
            self.notify(
                Report::Refusal,
                NotificationKind::Search,
                "find: open a text file first",
            );
        }
    }

    /// Close the find bar (but keep this tab's query/toggles for next time) and
    /// clear the active tab's match highlights (cheap to rebuild on reopen).
    pub(super) fn close_find(&mut self) {
        self.find_open = false;
        // The selection a scope was captured from is gone by the time the bar
        // reopens, so Search in Selection does not outlive the bar.
        if let Some(find) = self.active_find_mut() {
            find.scope = None;
        }
        if let Some(Tab {
            kind: TabKind::Code { decos, .. },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            decos.clear();
        }
    }

    /// Edit the active find field with the same GUI-style cursor and selection
    /// behavior as the Search panel. Command keys (Esc / Enter / Ctrl+G) resolve
    /// via the keymap's `Find` layer instead.
    pub(super) fn find_input(&mut self, key: KeyEvent) {
        let Some(find) = self.active_find_mut() else {
            return;
        };
        let editing_query = find.field == SearchField::Find;
        let (target, edit) = if editing_query {
            (&mut find.query, &mut find.query_edit)
        } else {
            (&mut find.replace, &mut find.replace_edit)
        };
        // Compared rather than inferred from the key: replacing a selection with
        // text of the same length still changes what matches.
        let before = target.clone();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let command = key.modifiers.contains(KeyModifiers::SUPER);
        match key.code {
            KeyCode::Backspace => edit.backspace(target, alt || ctrl),
            KeyCode::Delete => edit.delete(target, alt || ctrl),
            KeyCode::Left if command => edit.move_start(target, false, shift),
            KeyCode::Right if command => edit.move_end(target, false, shift),
            KeyCode::Left if alt || ctrl => edit.move_word_left(target, shift),
            KeyCode::Right if alt || ctrl => edit.move_word_right(target, shift),
            KeyCode::Left => edit.move_left(target, shift),
            KeyCode::Right => edit.move_right(target, shift),
            KeyCode::Home => edit.move_start(target, ctrl, shift),
            KeyCode::End => edit.move_end(target, ctrl, shift),
            KeyCode::Char('a' | 'A') if ctrl || command => edit.select_all(target),
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                edit.insert(target, &c.to_string());
            },
            _ => return,
        }
        // Motions leave the text alone; only an edit changes what matches.
        if *target == before {
            return;
        }
        // Only re-run the search when the query changed (the replacement doesn't
        // affect what matches).
        if editing_query {
            self.run_find();
        }
    }

    /// Re-run the in-file search after the query or its options changed, and move
    /// to the first match at or after the caret (wrapping to the first): the
    /// user is searching, so the caret follows what they typed.
    pub(super) fn run_find(&mut self) {
        self.seek_find(true);
    }

    /// Rebuild the match highlights against the current text without moving the
    /// caret; the counter points at the match at or after the caret. What a
    /// snapshot (an auto-save, a reload, an edit) and opening the bar do: neither
    /// is a motion, so neither may move the caret, collapse a multi-caret, or
    /// drop a selection.
    pub(super) fn refresh_find(&mut self) {
        self.seek_find(false);
    }

    /// Rebuild the active tab's match decorations and point the bar's counter at
    /// the match at or after the caret. With `jump`, also move the caret there.
    fn seek_find(&mut self, jump: bool) {
        let Some((q, scope)) = self.active_find().map(|f| (f.query_spec(), f.scope)) else {
            return;
        };
        let mut count = 0;
        let mut current = 0;
        if let Some(Tab {
            kind: TabKind::Code { buffer, decos, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            if q.pattern.is_empty() {
                decos.clear();
            } else {
                // Over the rope's chunks, so a find bar left open while typing
                // never copies the whole document per keystroke (#420).
                let matches = search_chunks(buffer.chunks(), &q).unwrap_or_default();
                let end = buffer.byte_to_line_col(BytePos(buffer.len_bytes()));
                *decos = matches
                    .iter()
                    .map(|m| Decoration {
                        range: Range {
                            start: buffer.byte_to_line_col(BytePos(m.start)),
                            end: buffer.byte_to_line_col(BytePos(m.end)),
                        },
                        kind: DecorationKind::TextBackground,
                        role: Some(ThemeRole::SearchMatch),
                    })
                    .filter(|deco| in_scope(scope, deco.range, end))
                    .collect();
                count = decos.len();
                // Resolved from the caret in both cases, never carried over by
                // index: an edit that adds or removes a match earlier in the
                // file shifts every index after it, while the caret moves with
                // the text it sits in. The counter, the next Replace and the
                // caret therefore always agree on which match is current.
                let caret = editor.cursors().primary().range().start;
                current = match_at_or_after(decos, caret);
                mark_current(decos, current);
                if jump && let Some(deco) = decos.get(current) {
                    let pos = deco.range.start;
                    editor.goto(buffer, pos);
                }
            }
        }
        if let Some(find) = self.active_find_mut() {
            find.count = count;
            find.current = current;
        }
    }

    /// Move to the next (`delta > 0`) or previous match from the caret, wrapping,
    /// and scroll it into view. Measured from the caret rather than from the
    /// counter, so stepping continues from wherever the user — or a Replace —
    /// left it.
    pub(super) fn find_step(&mut self, delta: i32) {
        if self.active_find().is_none_or(|find| find.count == 0) {
            return;
        }
        let Some(Tab {
            kind: TabKind::Code { buffer, decos, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        let caret = editor.cursor();
        let next = if delta >= 0 {
            decos
                .iter()
                .position(|deco| deco.range.start > caret)
                .unwrap_or(0)
        } else {
            decos
                .iter()
                .rposition(|deco| deco.range.start < caret)
                .unwrap_or(decos.len().saturating_sub(1))
        };
        let Some(pos) = decos.get(next).map(|deco| deco.range.start) else {
            return;
        };
        mark_current(decos, next);
        editor.goto(buffer, pos);
        if let Some(find) = self.active_find_mut() {
            find.current = next;
        }
    }

    /// Enter in the find bar: advance to the next match, or (in the replace field)
    /// replace the current match.
    pub(super) fn find_submit(&mut self) {
        if self.active_find().map(|f| f.field) == Some(SearchField::Replace) {
            self.find_replace_current();
        } else {
            self.find_step(1);
        }
    }

    /// Replace the current in-file match with the replacement text, then move to
    /// the next match (VS Code behaviour). The edit is applied through the document
    /// (undoable) and locally at once, so the matches are rebuilt against the new
    /// text here rather than waiting for the snapshot.
    pub(super) fn find_replace_current(&mut self) {
        // Measured against the text as it is now, and resolved from the caret:
        // an edit since the last search would otherwise leave the counter on a
        // stale range, or on an index that now names a different match.
        self.refresh_find();
        let Some(find) = self.active_find() else {
            return;
        };
        if find.count == 0 {
            return;
        }
        let current = find.current;
        let query = find.query_spec();
        let (replace, preserve) = (find.replace.clone(), find.preserve_case);
        let planned = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        decos,
                        text,
                        buffer,
                        ..
                    },
                ..
            }) => decos.get(current).map(|d| {
                // The same plan Replace All builds, so `$1` expands and the
                // case is preserved exactly as it would be there.
                let text = planned_replacements(text, buffer, &query, &replace, preserve)
                    .into_iter()
                    .find(|edit| edit.range == d.range)
                    .map_or_else(|| replace.clone(), |edit| edit.new_text);
                (d.range, text)
            }),
            _ => None,
        };
        let Some((range, replacement)) = planned else {
            return;
        };
        let edit = TextEdit {
            range,
            new_text: replacement.clone(),
        };
        if let Some(find) = self.active_find_mut() {
            find.scope = map_scope(find.scope, &[edit]);
        }
        // One caret, at the match: the edit is built once per caret, and every
        // other caret would replace the same range again.
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.goto(buffer, range.start);
        }
        self.submit_edit(move |caret, _sel, _buf, base| {
            Some(editing::insert(caret, Some(range), base, &replacement))
        });
        // The caret sits after the replacement, so this lands on the match after
        // it — never on replacement text that happens to match the query too.
        self.run_find();
    }

    /// Replace every in-file match at once, as one undoable batch with an edit per
    /// match. Every caret and selection stays on the text it was on, moved through
    /// the replacements.
    pub(super) fn find_replace_all(&mut self) {
        let Some(find) = self.active_find() else {
            return;
        };
        let query = find.query_spec();
        let replacement = find.replace.clone();
        let (preserve, scope) = (find.preserve_case, find.scope);
        if query.pattern.is_empty() {
            return;
        }
        let edit = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Code {
                        text,
                        buffer,
                        next_version,
                        ..
                    },
                editor,
                ..
            }) => {
                let end = buffer.byte_to_line_col(BytePos(buffer.len_bytes()));
                let edits: Vec<TextEdit> =
                    planned_replacements(text, buffer, &query, &replacement, preserve)
                        .into_iter()
                        .filter(|edit| in_scope(scope, edit.range, end))
                        .collect();
                if edits.is_empty() {
                    return;
                }
                let mut cursors = editor.cursors().clone();
                for selection in &mut cursors.selections {
                    *selection = editing::map_selection(*selection, &edits);
                }
                let scope = map_scope(scope, &edits);
                let edit = editing::MultiEdit {
                    change: Change::new(*next_version, edits),
                    cursors,
                };
                (edit, scope)
            },
            _ => return,
        };
        let (edit, scope) = edit;
        self.submit_multi_edit(EditCause::Replace, edit);
        if let Some(find) = self.active_find_mut() {
            find.scope = scope;
        }
    }

    /// Alt+Enter in the find bar: in the query field, turn every match into a
    /// selection (one caret per match, the current match primary) and hand the
    /// keyboard to the text, so typing edits them all (VS Code's *Select All
    /// Matches*); in the replace field, replace every match.
    pub(super) fn find_select_all_matches(&mut self) {
        if self.active_find().map(|f| f.field) == Some(SearchField::Replace) {
            self.find_replace_all();
            return;
        }
        self.refresh_find();
        let Some(current) = self.active_find().map(|f| f.current) else {
            return;
        };
        let Some(Tab {
            kind: TabKind::Code { buffer, decos, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        if decos.is_empty() {
            return;
        }
        let selections = decos
            .iter()
            .map(|deco| karet_core::Selection {
                anchor: deco.range.start,
                head: deco.range.end,
            })
            .collect();
        editor.set_cursor_state(
            buffer,
            karet_core::CursorState {
                selections,
                primary: current,
            },
        );
        if let Some(find) = self.active_find_mut() {
            find.focused = false;
        }
        self.focus = Focus::Editor;
    }

    /// Show or hide the find bar's replace field (collapsing returns to the query).
    pub(super) fn find_toggle_replace(&mut self) {
        if let Some(find) = self.active_find_mut() {
            find.replace_visible = !find.replace_visible;
            if !find.replace_visible {
                find.field = SearchField::Find;
            }
        }
    }

    /// Switch the edited find-bar field between find and replace.
    pub(super) fn find_toggle_field(&mut self) {
        if let Some(find) = self.active_find_mut() {
            find.field = match find.field {
                SearchField::Find => {
                    find.replace_visible = true;
                    SearchField::Replace
                },
                SearchField::Replace => SearchField::Find,
            };
        }
    }

    /// Toggle a find-bar option and refresh matches. Preserve Case changes no
    /// match, so it leaves the caret alone; the others re-run the search.
    pub(super) fn find_toggle_option(&mut self, option: SearchOption) {
        let scope = (option == SearchOption::InSelection)
            .then(|| self.selection_scope())
            .flatten();
        if let Some(find) = self.active_find_mut() {
            match option {
                SearchOption::Regex => find.regex = !find.regex,
                SearchOption::Case => find.case_sensitive = !find.case_sensitive,
                SearchOption::Word => find.whole_word = !find.whole_word,
                SearchOption::PreserveCase => {
                    find.preserve_case = !find.preserve_case;
                    return;
                },
                SearchOption::InSelection => {
                    find.scope = if find.scope.is_some() { None } else { scope };
                },
            }
        }
        self.run_find();
    }

    /// The range Search in Selection scopes to: the primary selection, or the
    /// caret's whole line (its line break included) when nothing is selected.
    fn selection_scope(&self) -> Option<Range> {
        let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get(self.active)
        else {
            return None;
        };
        let range = editor.cursors().primary().range();
        if !range.is_empty() {
            return Some(range);
        }
        let line = range.start.line;
        let end = if (line as usize) + 1 < buffer.line_count() {
            LineCol::new(line + 1, 0)
        } else {
            buffer.byte_to_line_col(BytePos(buffer.len_bytes()))
        };
        Some(Range {
            start: LineCol::new(line, 0),
            end,
        })
    }

    /// Focus the Search panel and (re)start the query input.
    pub(super) fn start_global_search(&mut self) {
        self.sidebar_panel = SidebarPanel::Search;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.search.input = true;
        self.search
            .query_edit
            .set_cursor(&self.search.query, self.search.query.len(), false);
    }

    /// Edit the active Search field with GUI-style cursor and selection behavior.
    pub(super) fn search_edit(&mut self, key: KeyEvent) {
        let (target, edit) = self.search.active_field();
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let command = key.modifiers.contains(KeyModifiers::SUPER);
        match key.code {
            KeyCode::Backspace => edit.backspace(target, alt || ctrl),
            KeyCode::Delete => edit.delete(target, alt || ctrl),
            KeyCode::Left if command => edit.move_start(target, false, shift),
            KeyCode::Right if command => edit.move_end(target, false, shift),
            KeyCode::Up if command => edit.move_start(target, true, shift),
            KeyCode::Down if command => edit.move_end(target, true, shift),
            KeyCode::Left if alt || ctrl => edit.move_word_left(target, shift),
            KeyCode::Right if alt || ctrl => edit.move_word_right(target, shift),
            KeyCode::Left => edit.move_left(target, shift),
            KeyCode::Right => edit.move_right(target, shift),
            KeyCode::Home => edit.move_start(target, ctrl, shift),
            KeyCode::End => edit.move_end(target, ctrl, shift),
            KeyCode::Char('a' | 'A') if ctrl || command => edit.select_all(target),
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                edit.insert(target, &c.to_string());
            },
            _ => {},
        }
    }

    /// Run the Search query and return to the results list.
    pub(super) fn run_search_query(&mut self) {
        // Enter runs the find search; while editing the replace field it applies the
        // replacement across the current matches instead.
        if self.search.field == SearchPanelField::Replace {
            self.search_replace_all();
        } else {
            self.run_global_search();
            self.search.input = false;
        }
    }

    /// Build a [`SearchQuery`] from the panel's query text and option toggles.
    pub(super) fn build_search_query(&self) -> SearchQuery {
        SearchQuery {
            pattern: self.search.query.clone(),
            regex: self.search.regex,
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
            includes: SearchPanel::globs(&self.search.includes),
            excludes: SearchPanel::globs(&self.search.excludes),
        }
    }

    /// Toggle the visibility of the replace field (collapsing it returns focus to the
    /// find field).
    pub(super) fn search_toggle_replace(&mut self) {
        self.search.replace_visible = !self.search.replace_visible;
        if !self.search.replace_visible {
            self.search.field = SearchPanelField::Find;
        }
    }

    /// Cycle the edited field, revealing whichever section the next field lives in
    /// and keeping the panel in input mode.
    ///
    /// Hidden sections are skipped rather than silently focused, so Tab never
    /// parks the cursor on a field the user cannot see.
    pub(super) fn search_toggle_field(&mut self) {
        let next = match self.search.field {
            SearchPanelField::Find => {
                self.search.replace_visible = true;
                SearchPanelField::Replace
            },
            SearchPanelField::Replace if self.search.filters_visible => SearchPanelField::Includes,
            SearchPanelField::Includes => SearchPanelField::Excludes,
            SearchPanelField::Replace | SearchPanelField::Excludes => SearchPanelField::Find,
        };
        self.search_focus_field(next);
    }

    /// Put the panel's focus in `field`, with the caret at the end of its text and
    /// nothing selected — where typing continues rather than replaces.
    pub(super) fn search_focus_field(&mut self, field: SearchPanelField) {
        self.search.input = true;
        self.search.field = field;
        let (text, edit) = self.search.active_field();
        let len = text.len();
        let owned = text.clone();
        edit.set_cursor(&owned, len, false);
    }

    /// Show or hide the include/exclude glob fields (collapsing them returns focus
    /// to the query).
    pub(super) fn search_toggle_filters(&mut self) {
        self.search.filters_visible = !self.search.filters_visible;
        if !self.search.filters_visible {
            if matches!(
                self.search.field,
                SearchPanelField::Includes | SearchPanelField::Excludes
            ) {
                self.search.field = SearchPanelField::Find;
            }
            // Hiding the fields must also stop them filtering, or a search would
            // stay narrowed by globs no longer on screen.
            let had_globs = !self.search.includes.is_empty() || !self.search.excludes.is_empty();
            self.search.includes.clear();
            self.search.excludes.clear();
            if had_globs {
                self.rerun_search();
            }
        } else {
            self.search.field = SearchPanelField::Includes;
            self.search.input = true;
        }
    }

    /// Ask the backend to apply the replacement across every workspace match;
    /// [`SessionEvent::SearchReplaced`] answers with the summary and triggers the
    /// refresh. Open buffers pick up the change through the file watcher.
    pub(super) fn search_replace_all(&mut self) {
        if self.search.query.is_empty() {
            return;
        }
        let query = self.build_search_query();
        let replacement = self.search.replace.clone();
        self.send_command(SessionCommand::SearchReplaceAll { query, replacement });
        self.search.input = false;
    }

    /// Re-run the workspace search if there is a non-empty query (after an option
    /// toggle changes what matches).
    pub(super) fn rerun_search(&mut self) {
        if !self.search.query.is_empty() {
            self.run_global_search();
        }
    }

    /// Toggle the regex option and refresh results.
    pub(super) fn search_toggle_regex(&mut self) {
        self.search.regex = !self.search.regex;
        self.rerun_search();
    }

    /// Toggle case-sensitivity and refresh results.
    pub(super) fn search_toggle_case(&mut self) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.rerun_search();
    }

    /// Toggle whole-word matching and refresh results.
    pub(super) fn search_toggle_word(&mut self) {
        self.search.whole_word = !self.search.whole_word;
        self.rerun_search();
    }

    /// Ask the backend for the workspace search results (the walk runs on the
    /// backend's search worker, never this thread); [`SessionEvent::SearchProgress`]
    /// batches stream in and one [`SessionEvent::SearchFinished`] closes the run.
    pub(super) fn run_global_search(&mut self) {
        // Cancel the previous run before starting another: without this a long
        // walk over a huge tree keeps burning a worker thread for results that
        // are already superseded.
        if let Some(previous) = self.search.searching.take() {
            self.send_command(SessionCommand::Cancel { request: previous });
        }
        // A re-run must not cost the reader their place: any file save re-runs the
        // live search through the watcher, and `clear` would send the cursor back
        // to the top and unfold everything. Carried across and re-clamped by
        // `rebuild_rows`; a genuinely new query's folds are recomputed when it
        // finishes anyway.
        let cursor = self.search.selection.cursor();
        let collapsed = std::mem::take(&mut self.search.collapsed);
        let folds_touched = self.search.folds_touched;
        self.search.clear();
        self.search.collapsed = collapsed;
        self.search.folds_touched = folds_touched;
        self.search.pending_cursor = Some(cursor);
        if self.search.query.is_empty() {
            self.refresh_search_decorations();
            return;
        }
        let query = self.build_search_query();
        self.search.started = Some(Pending::start());
        self.search.searching = self.send(SessionCommand::Search {
            query,
            file_limit: SEARCH_RESULT_CAP,
            match_limit: SEARCH_MATCH_CAP,
        });
        self.refresh_search_decorations();
    }

    /// Adopt one streamed batch of workspace search results.
    ///
    /// Results for anything but the in-flight request are dropped: cancelling
    /// cannot recall a batch already in the event channel, so a stale answer
    /// would otherwise overwrite a newer query's results.
    pub(super) fn search_progress(
        &mut self,
        request: Option<RequestId>,
        hits: Vec<SearchHit>,
        files_scanned: usize,
        matches_found: usize,
    ) {
        if request.is_none() || self.search.searching != request {
            return;
        }
        self.search.hits.extend(hits);
        self.search.files_scanned = files_scanned;
        self.search.matches_found = matches_found;
        self.search.rebuild_rows();
        self.refresh_search_decorations();
    }

    /// Adopt a search's terminal state.
    pub(super) fn search_finished(
        &mut self,
        request: Option<RequestId>,
        files_scanned: usize,
        matches_found: usize,
        truncated: bool,
        error: Option<String>,
    ) {
        if request.is_none() || self.search.searching != request {
            return;
        }
        self.search.searching = None;
        self.search.started = None;
        self.search.files_scanned = files_scanned;
        self.search.matches_found = matches_found;
        self.search.truncated = truncated;
        self.search.error = error;
        self.search.searched = true;
        // Adaptive expansion, applied only now that the size is known — collapsing
        // mid-stream would snap groups shut under the cursor while the user reads.
        // Skipped once the user has folded something themselves: an automatic
        // default may pick the starting state, but it must not undo a decision.
        if !self.search.folds_touched {
            self.search
                .set_all_collapsed(matches_found > SEARCH_AUTO_EXPAND);
        }
        // A settled search with nothing in it leaves the results holding a focus
        // with no row under it — and no arrow key crosses back out of the list —
        // so hand the focus to the query, which is the thing you go on to edit.
        // Only here, never on the re-run that empties the list: any file save
        // re-runs a live search through the watcher, and pulling focus into a text
        // field mid-stream would turn a reader's next arrow press into typing.
        if !self.search.input && self.search.rows.is_empty() {
            self.search_focus_field(SearchPanelField::Find);
        }
        self.refresh_search_decorations();
    }

    /// Recompute global-search match decorations for every open tab across every
    /// pane, from the current Search panel query and result set — this is what
    /// makes matches highlight inline in any already-open pane, not just the
    /// flat results list. Matches are recomputed against each tab's own **live**
    /// buffer (not the on-disk `FileHit` byte offsets), so a dirty/unsaved tab's
    /// highlights stay correct even though its content differs from disk.
    pub(super) fn refresh_search_decorations(&mut self) {
        let query = self.build_search_query();
        // Owned, not borrowed: `all_tabs_mut()` below needs `&mut self`, which a
        // set of `&Path` borrowed from `self.search.hits` would conflict with.
        let hit_paths: HashSet<PathBuf> = self.search.hits.iter().map(|h| h.path.clone()).collect();
        for tab in self.all_tabs_mut() {
            if let TabKind::Code {
                path,
                buffer,
                search_decos,
                ..
            } = &mut tab.kind
            {
                *search_decos = if !query.pattern.is_empty() && hit_paths.contains(path.as_path()) {
                    search_chunks(buffer.chunks(), &query)
                        .unwrap_or_default()
                        .iter()
                        .map(|m| Decoration {
                            range: Range {
                                start: buffer.byte_to_line_col(BytePos(m.start)),
                                end: buffer.byte_to_line_col(BytePos(m.end)),
                            },
                            kind: DecorationKind::TextBackground,
                            role: Some(ThemeRole::SearchMatch),
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            }
        }
    }

    /// Move the selection within the search results.
    pub(super) fn search_select(&mut self, delta: i32) {
        self.search.selection.move_by(delta);
    }

    /// Open the selected row: a match row jumps to that exact match, a file
    /// heading to the file's first one.
    ///
    /// The panel keeps the keyboard. Opening a hit is a step *through* a result
    /// list, not out of it — the next arrow should reach the next hit, so the file
    /// appears in the editor while the focus stays where the reader put it. `Esc`
    /// is what hands the keyboard to the editor.
    pub(super) fn open_selected_result(&mut self) {
        let Some(row) = self
            .search
            .rows
            .get(self.search.selection.cursor())
            .copied()
        else {
            return;
        };
        let Some(hit) = self.search.hits.get(row.hit()) else {
            return;
        };
        let index = match row {
            SearchRow::Match { index, .. } => index,
            SearchRow::File { .. } => 0,
        };
        let path = hit.path.clone();
        // The backend already converted the engine's *byte* column to a character
        // column, so this lands on the match rather than the start of the line.
        let position = hit
            .matches
            .get(index)
            .map_or(LineCol::new(0, 0), |m| m.range.start);
        self.focus_by_file_line(&path, position, false);
    }

    /// Expand or collapse the selected file group.
    pub(super) fn search_toggle_row(&mut self) {
        let Some(row) = self
            .search
            .rows
            .get(self.search.selection.cursor())
            .copied()
        else {
            return;
        };
        let Some(path) = self.search.hits.get(row.hit()).map(|hit| hit.path.clone()) else {
            return;
        };
        self.search.folds_touched = true;
        self.search.toggle_file(&path);
    }

    /// Expand the selected group, or step into it when it is already open.
    ///
    /// A match row is a leaf, so it absorbs the key rather than stepping on: a
    /// `Right` that quietly acts as a `Down` reads as the list losing the press.
    pub(super) fn search_expand(&mut self) {
        let Some(row) = self
            .search
            .rows
            .get(self.search.selection.cursor())
            .copied()
        else {
            return;
        };
        match row {
            SearchRow::File {
                expanded: false, ..
            } => self.search_toggle_row(),
            SearchRow::File { .. } => self.search.selection.move_by(1),
            SearchRow::Match { .. } => {},
        }
    }

    /// Collapse the selected group, or walk up out of it: from a match row to its
    /// heading, and from an already-collapsed heading to the previous file's.
    pub(super) fn search_collapse(&mut self) {
        let cursor = self.search.selection.cursor();
        let Some(row) = self.search.rows.get(cursor).copied() else {
            return;
        };
        // From a match, collapsing walks up to the file it belongs to, which is
        // where a second press then collapses; from a heading that is already
        // shut, a third walks back to the file above it, so repeated presses step
        // through the result set a file at a time.
        let heading_of = |hit: Option<usize>| {
            self.search.rows[..cursor]
                .iter()
                .rposition(|row| match (row, hit) {
                    (SearchRow::File { hit: h, .. }, Some(hit)) => *h == hit,
                    (SearchRow::File { .. }, None) => true,
                    _ => false,
                })
        };
        let target = match row {
            SearchRow::File { expanded: true, .. } => {
                self.search_toggle_row();
                return;
            },
            SearchRow::File { .. } => heading_of(None),
            SearchRow::Match { hit, .. } => heading_of(Some(hit)),
        };
        if let Some(heading) = target {
            self.search.selection.move_to(heading);
        }
    }

    /// Collapse every file group, or expand them all when none are collapsed.
    pub(super) fn search_toggle_all(&mut self) {
        let any_open = self
            .search
            .hits
            .iter()
            .any(|hit| !self.search.collapsed.contains(&hit.path));
        self.search.folds_touched = true;
        self.search.set_all_collapsed(any_open);
    }
}

/// The match the caret is in or the first one after it, wrapping to the first
/// when every match lies behind it.
fn match_at_or_after(decos: &[Decoration], caret: LineCol) -> usize {
    decos
        .iter()
        .position(|deco| deco.range.start >= caret || deco.range.end > caret)
        .unwrap_or(0)
}

/// Paint the match at `current` with the current-match role and every other
/// one with the plain match role, so exactly one match reads as current.
fn mark_current(decos: &mut [Decoration], current: usize) {
    for (i, deco) in decos.iter_mut().enumerate() {
        deco.role = Some(if i == current {
            ThemeRole::SearchMatchCurrent
        } else {
            ThemeRole::SearchMatch
        });
    }
}

/// Whether a match over `range` counts under the Search in Selection `scope`:
/// every match without one, otherwise only a match wholly inside it. An
/// empty-width match (a regex `^` or `$`) at `scope.end` sits on the boundary
/// with the text after the scope, so it counts only for an empty scope or
/// where `scope.end` is `buffer_end`, with no text after it to belong to.
fn in_scope(scope: Option<Range>, range: Range, buffer_end: LineCol) -> bool {
    scope.is_none_or(|scope| {
        scope.start <= range.start
            && range.end <= scope.end
            && (range.start < scope.end || scope.start == scope.end || scope.end == buffer_end)
    })
}

/// `scope` carried through `edits` (sorted, non-overlapping), so a replacement
/// inside the scope grows or shrinks it rather than leaving it on stale text.
/// Text inserted exactly at `scope.start` lands inside the scope: the start
/// stays before it rather than being pushed past it.
fn map_scope(scope: Option<Range>, edits: &[TextEdit]) -> Option<Range> {
    scope.map(|scope| {
        let moves_start: Vec<TextEdit> = edits
            .iter()
            .filter(|edit| !(edit.range.is_empty() && edit.range.start == scope.start))
            .cloned()
            .collect();
        Range {
            start: editing::map_position(scope.start, &moves_start),
            end: editing::map_position(scope.end, edits),
        }
    })
}

/// One edit per match of `query` in `text`, replacing it with `replacement`:
/// regex captures (`$1`, `${name}`) expanded for a regex query, then, with
/// `preserve`, the case of the matched text carried onto the result.
fn planned_replacements(
    text: &str,
    buffer: &karet_text::TextBuffer,
    query: &SearchQuery,
    replacement: &str,
    preserve: bool,
) -> Vec<TextEdit> {
    karet_search::plan_replacements(text, query, replacement)
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let new_text = if preserve {
                karet_search::preserve_case(text.get(r.start..r.end).unwrap_or_default(), &r.text)
            } else {
                r.text
            };
            TextEdit {
                range: Range {
                    start: buffer.byte_to_line_col(BytePos(r.start)),
                    end: buffer.byte_to_line_col(BytePos(r.end)),
                },
                new_text,
            }
        })
        .collect()
}

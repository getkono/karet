use super::*;

impl App {
    /// Open the quick-open (go-to-file) overlay.
    pub(super) fn open_quick_open(&mut self) {
        let files = workspace::list_files(&self.root, 2000);
        self.overlay = Some(Overlay::quick_open(files));
    }

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
            find.get_or_insert_with(FindState::default);
            self.find_open = true;
            self.focus = Focus::Editor;
            // Rebuild decorations against the current buffer — cheap no-op for a
            // blank query, necessary to refresh a restored non-empty one.
            self.run_find();
        } else {
            self.status = Some("find: open a text file first".to_string());
        }
    }

    /// Close the find bar (but keep this tab's query/toggles for next time) and
    /// clear the active tab's match highlights (cheap to rebuild on reopen).
    pub(super) fn close_find(&mut self) {
        self.find_open = false;
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

    /// Re-run the in-file search and rebuild the active tab's match decorations.
    pub(super) fn run_find(&mut self) {
        let q = match self.active_find() {
            Some(find) => find.query_spec(),
            None => return,
        };
        let mut count = 0;
        if let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    text,
                    decos,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            if q.pattern.is_empty() {
                decos.clear();
            } else {
                let matches = search_in_file(text, &q).unwrap_or_default();
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
                    .collect();
                count = decos.len();
                if let Some(first) = decos.first() {
                    let pos = first.range.start;
                    editor.goto(buffer, pos);
                }
            }
        }
        if let Some(find) = self.active_find_mut() {
            find.count = count;
            find.current = 0;
        }
    }

    /// Move to the next/previous match (wrapping) and scroll it into view.
    pub(super) fn find_step(&mut self, delta: i32) {
        let (count, current) = match self.active_find() {
            Some(find) => (find.count, find.current),
            None => return,
        };
        if count == 0 {
            return;
        }
        let next = (current as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
        if let Some(find) = self.active_find_mut() {
            find.current = next;
        }
        if let Some(Tab {
            kind: TabKind::Code { buffer, decos, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
            && let Some(deco) = decos.get(next)
        {
            let pos = deco.range.start;
            editor.goto(buffer, pos);
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

    /// Replace the current in-file match with the replacement text. The edit is
    /// applied through the document (undoable); find re-runs when the snapshot lands.
    pub(super) fn find_replace_current(&mut self) {
        let Some(find) = self.active_find() else {
            return;
        };
        if find.count == 0 {
            return;
        }
        let current = find.current;
        let replacement = find.replace.clone();
        let range = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { decos, .. },
                ..
            }) => decos.get(current).map(|d| d.range),
            _ => None,
        };
        let Some(range) = range else {
            return;
        };
        self.submit_edit(move |caret, _sel, _buf, base| {
            Some(editing::insert(caret, Some(range), base, &replacement))
        });
    }

    /// Replace every in-file match at once by rewriting the whole buffer through a
    /// single undoable edit (offset-safe via `karet_search::apply_replacements`).
    pub(super) fn find_replace_all(&mut self) {
        let Some(find) = self.active_find() else {
            return;
        };
        let query = find.query_spec();
        let replacement = find.replace.clone();
        if query.pattern.is_empty() {
            return;
        }
        let (text, whole) = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { text, buffer, .. },
                ..
            }) => (
                text.clone(),
                Range {
                    start: LineCol::new(0, 0),
                    end: buffer.byte_to_line_col(BytePos(text.len())),
                },
            ),
            _ => return,
        };
        let plan = karet_search::plan_replacements(&text, &query, &replacement).unwrap_or_default();
        if plan.is_empty() {
            return;
        }
        let updated = karet_search::apply_replacements(&text, &plan);
        self.submit_edit(move |caret, _sel, _buf, base| {
            Some(editing::insert(caret, Some(whole), base, &updated))
        });
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

    /// Toggle a find-bar match option (regex / case / whole-word) and refresh matches.
    pub(super) fn find_toggle_option(&mut self, option: SearchOption) {
        if let Some(find) = self.active_find_mut() {
            match option {
                SearchOption::Regex => find.regex = !find.regex,
                SearchOption::Case => find.case_sensitive = !find.case_sensitive,
                SearchOption::Word => find.whole_word = !find.whole_word,
            }
        }
        self.run_find();
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

    /// Walk the panel's one vertical focus ring: the visible fields top to bottom,
    /// then the result rows.
    ///
    /// The ring stops at both ends rather than wrapping — a `Down` that jumped
    /// from the last result back to the query box would turn a held key into a
    /// text field the user is not looking at. It also never *reveals* a hidden
    /// section the way `Tab` does: `Down` navigates what is painted.
    ///
    /// Only the *sign* of `delta` is read: this is a one-step walk over a ring
    /// whose two halves count in different units, so a magnitude would mean rows
    /// below the seam and fields above it. Entering the list therefore lands on
    /// its first row, not wherever the cursor was left — `Esc` is the way back to
    /// a place you were holding.
    pub(super) fn search_focus_step(&mut self, delta: i32) {
        let step = delta.signum();
        if step == 0 {
            return;
        }
        if !self.search.input {
            // Leaving the list only happens off its first row, which covers the
            // empty-result case (the cursor is 0) — otherwise there would be no
            // way back to the query from a search that found nothing.
            if step < 0 && self.search.selection.cursor() == 0 {
                // `visible_fields` always yields at least the query, so the list
                // always has a field to step back into.
                if let Some(last) = self.search.visible_fields().next_back() {
                    self.search_focus_field(last);
                }
            } else {
                self.search.selection.move_by(step);
            }
            return;
        }
        let fields: Vec<SearchPanelField> = self.search.visible_fields().collect();
        let at = fields
            .iter()
            .position(|&field| field == self.search.field)
            .unwrap_or(0);
        let next = at as i64 + i64::from(step);
        if let Ok(index) = usize::try_from(next)
            && let Some(&field) = fields.get(index)
        {
            self.search_focus_field(field);
        } else if step > 0 && !self.search.rows.is_empty() {
            // Past the last field is the list; with no rows there is nowhere to
            // go, so the key is absorbed rather than moving focus off screen.
            self.search.input = false;
            self.search.selection.move_to(0);
        }
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
        // with no row under it, so hand the focus back to the query — that is the
        // thing you go on to edit. Only here, never on the re-run that empties the
        // list: any file save re-runs a live search through the watcher, and
        // pulling focus into a text field mid-stream would turn a reader's next
        // arrow press into typing.
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
                text,
                search_decos,
                ..
            } = &mut tab.kind
            {
                *search_decos = if !query.pattern.is_empty() && hit_paths.contains(path.as_path()) {
                    search_in_file(text, &query)
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
        self.focus_by_file_line(&path, position);
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

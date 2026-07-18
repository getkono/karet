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

    /// Edit the find query with an unbound key (backspace / printable), re-running
    /// the search. Command keys (Esc / Enter / Ctrl+G / arrows) resolve via the
    /// keymap's `Find` layer instead.
    pub(super) fn find_input(&mut self, key: KeyEvent) {
        let Some(find) = self.active_find_mut() else {
            return;
        };
        let editing_query = find.field == SearchField::Find;
        let target = if editing_query {
            &mut find.query
        } else {
            &mut find.replace
        };
        match key.code {
            KeyCode::Backspace => {
                target.pop();
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                target.push(c);
            },
            _ => return,
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
    }

    /// Edit the Search query with an unbound key (backspace / printable) while the
    /// `SearchInput` modal is active. Navigation and mode keys resolve via the
    /// keymap's `SearchInput` / `SearchList` layers instead.
    pub(super) fn search_edit(&mut self, key: KeyEvent) {
        let target = match self.search.field {
            SearchField::Find => &mut self.search.query,
            SearchField::Replace => &mut self.search.replace,
        };
        match key.code {
            KeyCode::Backspace => {
                target.pop();
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                target.push(c);
            },
            _ => {},
        }
    }

    /// Run the Search query and return to the results list.
    pub(super) fn run_search_query(&mut self) {
        // Enter runs the find search; while editing the replace field it applies the
        // replacement across the current matches instead.
        if self.search.field == SearchField::Replace {
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
            ..Default::default()
        }
    }

    /// Toggle the visibility of the replace field (collapsing it returns focus to the
    /// find field).
    pub(super) fn search_toggle_replace(&mut self) {
        self.search.replace_visible = !self.search.replace_visible;
        if !self.search.replace_visible {
            self.search.field = SearchField::Find;
        }
    }

    /// Switch the edited field between find and replace (revealing the replace field
    /// when moving to it), keeping the panel in input mode.
    pub(super) fn search_toggle_field(&mut self) {
        self.search.input = true;
        self.search.field = match self.search.field {
            SearchField::Find => {
                self.search.replace_visible = true;
                SearchField::Replace
            },
            SearchField::Replace => SearchField::Find,
        };
    }

    /// Apply the replacement across every match in the workspace, then refresh the
    /// results. Open buffers pick up the change through the file watcher.
    pub(super) fn search_replace_all(&mut self) {
        if self.search.query.is_empty() {
            return;
        }
        let query = self.build_search_query();
        let replacement = self.search.replace.clone();
        let summary = WorkspaceSearch::new()
            .replace(&self.root, &query, &replacement)
            .unwrap_or_default();
        self.notify(
            Severity::Information,
            NotificationKind::System,
            format!(
                "replaced {} occurrence(s) in {} file(s)",
                summary.replacements, summary.files_changed
            ),
        );
        // Re-run the search so the (now empty, unless the replacement re-matches)
        // results reflect the edited files.
        self.run_global_search();
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

    /// Run the workspace search for the current query, collecting up to the cap.
    pub(super) fn run_global_search(&mut self) {
        self.search.results.clear();
        self.search.selected = 0;
        if self.search.query.is_empty() {
            self.refresh_search_decorations();
            return;
        }
        let query = self.build_search_query();
        let mut results = Vec::new();
        let _ = WorkspaceSearch::new().run(&self.root, &query, |hit| {
            if results.len() < SEARCH_RESULT_CAP {
                results.push(hit);
            }
        });
        self.search.results = results;
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
        // set of `&Path` borrowed from `self.search.results` would conflict with.
        let hit_paths: HashSet<PathBuf> =
            self.search.results.iter().map(|h| h.path.clone()).collect();
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
        let len = self.search.results.len();
        if len > 0 {
            let next = (self.search.selected as i64 + i64::from(delta)).clamp(0, len as i64 - 1);
            self.search.selected = next as usize;
        }
    }

    /// Open the selected result, scrolling to its first match.
    pub(super) fn open_selected_result(&mut self) {
        let Some(hit) = self.search.results.get(self.search.selected) else {
            return;
        };
        let path = hit.path.clone();
        let line = hit.matches.first().map(|m| m.line);
        self.open_path(&path);
        if let (
            Some(line),
            Some(Tab {
                kind: TabKind::Code { buffer, .. },
                editor,
                ..
            }),
        ) = (line, self.tabs.get_mut(self.active))
        {
            editor.goto(buffer, LineCol::new(line, 0));
        }
    }
}

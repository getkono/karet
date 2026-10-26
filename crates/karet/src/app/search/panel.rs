//! The workspace-search panel's state: the query and glob fields being edited,
//! the streamed results grouped into file and match rows, and the folding and
//! focus rules the Search panel's commands in [`super`] drive.

use super::*;

/// The workspace-search panel state.
pub(crate) struct SearchPanel {
    /// The query being typed/run.
    pub(crate) query: String,
    /// Cursor and selection state for the query field.
    pub(crate) query_edit: TextFieldState,
    /// The replacement text.
    pub(crate) replace: String,
    /// Cursor and selection state for the replacement field.
    pub(crate) replace_edit: TextFieldState,
    /// The streamed results (one entry per matching file).
    pub(crate) hits: Vec<SearchHit>,
    /// The rendered rows derived from [`hits`](Self::hits).
    pub(crate) rows: Vec<SearchRow>,
    /// Files whose match rows are hidden. Stores the *collapsed* set rather than
    /// the expanded one so a file arriving in a later streaming batch shows its
    /// matches by default instead of appearing empty.
    pub(crate) collapsed: HashSet<PathBuf>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
    /// The in-flight search, if one is running. Answers for any other request are
    /// stale — a newer query supersedes its predecessor.
    pub(crate) searching: Option<RequestId>,
    /// When the running search started, for the delayed loading reveal.
    pub(crate) started: Option<Pending>,
    /// How many files the running (or last) search visited.
    pub(crate) files_scanned: usize,
    /// How many matches the running (or last) search found.
    pub(crate) matches_found: usize,
    /// The last search stopped at a file or match cap.
    pub(crate) truncated: bool,
    /// Why the last search could not run — an invalid regex or glob.
    pub(crate) error: Option<String>,
    /// Whether a search has ever completed, so an empty list can distinguish
    /// "no matches" from "nothing asked for yet".
    pub(crate) searched: bool,
    /// Whether the user has folded or unfolded anything during this search, which
    /// suppresses the adaptive expansion applied when the search finishes.
    pub(crate) folds_touched: bool,
    /// A cursor to restore once rows exist again. A re-run empties the list, so
    /// the position cannot be re-applied until the first batch lands.
    pub(crate) pending_cursor: Option<usize>,
    /// Whether a field is being edited (vs. browsing results).
    pub(crate) input: bool,
    /// Which field the input edits.
    pub(crate) field: SearchPanelField,
    /// Glob patterns limiting the search to matching paths (ripgrep `-g`).
    pub(crate) includes: String,
    /// Cursor and selection state for the include field.
    pub(crate) includes_edit: TextFieldState,
    /// Glob patterns excluding matching paths.
    pub(crate) excludes: String,
    /// Cursor and selection state for the exclude field.
    pub(crate) excludes_edit: TextFieldState,
    /// Whether the include/exclude fields are shown (collapsible; hidden by default).
    pub(crate) filters_visible: bool,
    /// Whether the replace field is shown (collapsible; shown by default).
    pub(crate) replace_visible: bool,
    /// Interpret the query as a regular expression.
    pub(crate) regex: bool,
    /// Match case-sensitively.
    pub(crate) case_sensitive: bool,
    /// Match whole words only.
    pub(crate) whole_word: bool,
}

/// Which Search-panel field the input edits.
///
/// Deliberately not [`SearchField`](crate::tab::SearchField), which the in-file
/// find bar shares: that bar has no glob fields, and widening its enum would give
/// it unreachable states to handle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SearchPanelField {
    /// The query.
    #[default]
    Find,
    /// The replacement.
    Replace,
    /// Glob patterns limiting the search to matching paths.
    Includes,
    /// Glob patterns excluding matching paths.
    Excludes,
}

/// One row of the Search panel's list: a file heading, or one match under it.
///
/// Both are selectable — a heading jumps to the file's first match, a match row
/// to that exact line and column — and grouping is what keeps a result set of a
/// few thousand matches readable in a narrow sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchRow {
    /// A file heading; `hit` indexes [`SearchPanel::hits`].
    File {
        /// Index into [`SearchPanel::hits`].
        hit: usize,
        /// How many matches this file has.
        count: usize,
        /// Whether this file's match rows are shown.
        expanded: bool,
    },
    /// One match; `hit` indexes [`SearchPanel::hits`] and `index` that hit's matches.
    Match {
        /// Index into [`SearchPanel::hits`].
        hit: usize,
        /// Index into that hit's `matches`.
        index: usize,
    },
}

impl SearchRow {
    /// The hit this row belongs to.
    pub(crate) fn hit(self) -> usize {
        match self {
            Self::File { hit, .. } | Self::Match { hit, .. } => hit,
        }
    }
}

impl SearchPanel {
    /// The text and cursor state of the field the input is editing, read-only.
    pub(crate) fn active_field_ref(&self) -> (&str, &TextFieldState) {
        match self.field {
            SearchPanelField::Find => (&self.query, &self.query_edit),
            SearchPanelField::Replace => (&self.replace, &self.replace_edit),
            SearchPanelField::Includes => (&self.includes, &self.includes_edit),
            SearchPanelField::Excludes => (&self.excludes, &self.excludes_edit),
        }
    }

    /// The text and cursor state of the field the input is editing.
    pub(crate) fn active_field(&mut self) -> (&mut String, &mut TextFieldState) {
        match self.field {
            SearchPanelField::Find => (&mut self.query, &mut self.query_edit),
            SearchPanelField::Replace => (&mut self.replace, &mut self.replace_edit),
            SearchPanelField::Includes => (&mut self.includes, &mut self.includes_edit),
            SearchPanelField::Excludes => (&mut self.excludes, &mut self.excludes_edit),
        }
    }

    /// The panel's fields in the order they are painted, with the fields of a
    /// hidden section left out.
    ///
    /// This is the top half of the panel's vertical focus ring — `Up`/`Down` walk
    /// it and then step into the result rows — so it must list exactly what is on
    /// screen: focus parked on an unpainted field is a cursor the user cannot see.
    // `use<>`: the iterator owns its array and borrows nothing, so a caller can
    // hold it across a `&mut self` call.
    pub(crate) fn visible_fields(
        &self,
    ) -> impl DoubleEndedIterator<Item = SearchPanelField> + use<> {
        [
            (SearchPanelField::Find, true),
            (SearchPanelField::Replace, self.replace_visible),
            (SearchPanelField::Includes, self.filters_visible),
            (SearchPanelField::Excludes, self.filters_visible),
        ]
        .into_iter()
        .filter_map(|(field, shown)| shown.then_some(field))
    }

    /// Split each glob field into the patterns a [`SearchQuery`] takes.
    ///
    /// Comma or whitespace separated, so `*.rs, src/**` reads the way a user
    /// expects to type it.
    pub(crate) fn globs(text: &str) -> Vec<String> {
        text.split([',', ' ', '\t'])
            .map(str::trim)
            .filter(|glob| !glob.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Rebuild [`rows`](Self::rows) from [`hits`](Self::hits), keeping the cursor
    /// in range. Called after every streamed batch and every expand/collapse.
    pub(crate) fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (hit, file) in self.hits.iter().enumerate() {
            let expanded = !self.collapsed.contains(&file.path);
            self.rows.push(SearchRow::File {
                hit,
                count: file.matches.len(),
                expanded,
            });
            if expanded {
                self.rows
                    .extend((0..file.matches.len()).map(|index| SearchRow::Match { hit, index }));
            }
        }
        let cursor = self
            .pending_cursor
            .take()
            .unwrap_or_else(|| self.selection.cursor());
        self.selection = ListSelection::new(self.rows.len());
        self.selection
            .move_to(cursor.min(self.rows.len().saturating_sub(1)));
        self.clamp_focus();
    }

    /// Keep the panel's focus on something the panel actually paints.
    ///
    /// Every path that hides a section already bounces the field, so this is an
    /// invariant rather than a fix — it means no future one can leave the caret
    /// on a field that is no longer on screen.
    pub(crate) fn clamp_focus(&mut self) {
        if self.input && !self.visible_fields().any(|field| field == self.field) {
            self.field = SearchPanelField::Find;
        }
    }

    /// Show or hide one file's match rows.
    pub(crate) fn toggle_file(&mut self, path: &Path) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_path_buf());
        }
        self.rebuild_rows();
    }

    /// Collapse or expand every file at once.
    pub(crate) fn set_all_collapsed(&mut self, collapsed: bool) {
        self.collapsed = if collapsed {
            self.hits.iter().map(|hit| hit.path.clone()).collect()
        } else {
            HashSet::new()
        };
        self.rebuild_rows();
    }

    /// Drop every result, readying the panel for a fresh search.
    pub(crate) fn clear(&mut self) {
        self.hits.clear();
        self.rows.clear();
        self.collapsed.clear();
        self.selection = ListSelection::new(0);
        self.files_scanned = 0;
        self.matches_found = 0;
        self.truncated = false;
        self.error = None;
        self.searched = false;
        self.folds_touched = false;
        self.pending_cursor = None;
    }
}

impl Default for SearchPanel {
    fn default() -> Self {
        Self {
            query: String::new(),
            query_edit: TextFieldState::default(),
            replace: String::new(),
            replace_edit: TextFieldState::default(),
            hits: Vec::new(),
            rows: Vec::new(),
            collapsed: HashSet::new(),
            selection: ListSelection::new(0),
            searching: None,
            started: None,
            files_scanned: 0,
            matches_found: 0,
            truncated: false,
            error: None,
            searched: false,
            folds_touched: false,
            pending_cursor: None,
            input: false,
            field: SearchPanelField::Find,
            includes: String::new(),
            includes_edit: TextFieldState::default(),
            excludes: String::new(),
            excludes_edit: TextFieldState::default(),
            // Hidden by default: an empty pair of globs is the common case, and
            // the sidebar is narrow enough that two idle rows cost real results.
            filters_visible: false,
            // The replace field is shown by default (collapsible via keybinding).
            replace_visible: true,
            regex: false,
            case_sensitive: false,
            whole_word: false,
        }
    }
}

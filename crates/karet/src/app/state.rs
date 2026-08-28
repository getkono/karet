//! Grouped [`App`](super::App) sub-states: terminal capabilities, per-panel
//! chrome recorded during the last render (mouse hit-testing), and per-document
//! caches. Pure data — behavior stays on `App`.

use super::*;

/// What the terminal was confirmed to support at startup. Capabilities are
/// only ever *confirmed* (via handshakes), never assumed.
pub(crate) struct TerminalCaps {
    /// The detected terminal graphics protocol.
    pub(crate) graphics: GraphicsProtocol,
    /// Whether Kitty graphics support was detected or confirmed at startup.
    pub(crate) kitty_graphics: bool,
    /// Whether crossterm confirmed Kitty keyboard protocol support at startup.
    pub(crate) kitty_keyboard: bool,
    /// Whether the terminal was confirmed (via a startup handshake) to support
    /// OSC 22 mouse-pointer-shape hints. `false` means every pointer-shape
    /// hint is a no-op.
    pub(crate) pointer_shapes: bool,
    /// The last OSC 22 pointer shape sent (so hover doesn't re-send every
    /// mouse event), or `None` for the terminal's default shape.
    pub(crate) pointer_shape: Option<&'static str>,
}

impl TerminalCaps {
    /// Probe the environment for the graphics protocol; everything else starts
    /// unconfirmed until the startup handshakes answer.
    pub(crate) fn detect() -> Self {
        let graphics = image::detect_protocol();
        Self {
            graphics,
            kitty_graphics: graphics == GraphicsProtocol::Kitty,
            kitty_keyboard: false,
            pointer_shapes: false,
            pointer_shape: None,
        }
    }
}

/// The right-side outline panel: visibility, selection, and last-frame
/// geometry.
pub(crate) struct OutlinePanel {
    /// Whether the panel is shown.
    pub(crate) visible: bool,
    /// Whether the outline currently overlays (rather than reserves) editor
    /// space.
    pub(crate) overlay: bool,
    /// The row selection, driving keyboard navigation.
    pub(crate) sel: ListSelection,
    /// The panel rect from the last frame (mouse hit-testing).
    pub(crate) rect: Rect,
    /// The content area (below the header) from the last frame.
    pub(crate) content_rect: Rect,
    /// The panel width in columns.
    pub(crate) width: u16,
    /// The list's scroll offset (first visible row) from the last frame, so a
    /// click maps to the correct entry even when the list is scrolled.
    pub(crate) scroll: usize,
}

impl Default for OutlinePanel {
    fn default() -> Self {
        Self {
            visible: false,
            overlay: false,
            sel: ListSelection::new(0),
            rect: Rect::default(),
            content_rect: Rect::default(),
            width: OUTLINE_WIDTH,
            scroll: 0,
        }
    }
}

/// The Source-Control panel's render chrome from the last frame: scroll
/// offsets, hit-test geometry, and the draggable changes/commits divider.
pub(crate) struct ScmChrome {
    /// The changes-region scroll offset (top region; wheel + selection-follow).
    pub(crate) offset: usize,
    /// The changes-region viewport rect from the last frame (hit/hover).
    pub(crate) changes_rect: Rect,
    /// The editable inner rect of the permanent commit-message field.
    pub(crate) commit_rect: Rect,
    /// The total number of changes display rows from the last frame.
    pub(crate) total_rows: usize,
    /// The commit-log region scroll offset (bottom pinned region).
    pub(crate) commits_offset: usize,
    /// The commit-log region viewport rect from the last frame.
    pub(crate) commits_rect: Rect,
    /// The pinned, clickable `COMMITS` title's rect from the last frame.
    pub(crate) commits_title_rect: Rect,
    /// The total number of commit-log display rows from the last frame.
    pub(crate) commits_total: usize,
    /// The display row *within the commit-log region* of the "load more"
    /// affordance.
    pub(crate) more_row: Option<usize>,
    /// User-controlled height (rows) of the pinned commit-log region.
    pub(crate) commits_h: u16,
    /// The y of the changes/commits drag divider from the last frame
    /// (0 = not shown).
    pub(crate) divider_y: u16,
    /// Whether a commits-divider resize drag is in progress.
    pub(crate) resizing: bool,
    /// Changes display-row → change-index map from the last frame.
    pub(crate) row_map: Vec<Option<usize>>,
    /// Header controls `(start, end, row, command)` from the last frame.
    pub(crate) header_hits: Vec<(u16, u16, u16, Command)>,
}

impl Default for ScmChrome {
    fn default() -> Self {
        Self {
            offset: 0,
            changes_rect: Rect::default(),
            commit_rect: Rect::default(),
            total_rows: 0,
            commits_offset: 0,
            commits_rect: Rect::default(),
            commits_title_rect: Rect::default(),
            commits_total: 0,
            more_row: None,
            commits_h: DEFAULT_SCM_COMMITS_H,
            divider_y: 0,
            resizing: false,
            row_map: Vec::new(),
            header_hits: Vec::new(),
        }
    }
}

/// The Search panel's render chrome from the last frame.
#[derive(Default)]
pub(crate) struct SearchChrome {
    /// The results area.
    pub(crate) results_rect: Rect,
    /// The results-list scroll offset.
    pub(crate) offset: usize,
    /// The editable query rect.
    pub(crate) query_rect: Rect,
    /// The editable replacement rect, if shown.
    pub(crate) replace_rect: Option<Rect>,
    /// The editable include-glob rect, if shown.
    pub(crate) includes_rect: Option<Rect>,
    /// The editable exclude-glob rect, if shown.
    pub(crate) excludes_rect: Option<Rect>,
    /// Clickable header buttons `(start, end, row, command)` (option toggles
    /// and replace-all).
    pub(crate) action_hits: Vec<(u16, u16, u16, Command)>,
}

/// The Spelling panel's render chrome from the last frame.
#[derive(Default)]
pub(crate) struct SpellingChrome {
    /// The results area.
    pub(crate) results_rect: Rect,
    /// The results-list scroll offset.
    pub(crate) offset: usize,
    /// Clickable header buttons `(start, end, row, command)` (the re-scan action).
    pub(crate) action_hits: Vec<(u16, u16, u16, Command)>,
}

/// One row of the Spelling panel's list: a file header, or one misspelled word
/// under it. Both are selectable — the issue asks for every *instance* to be
/// clickable, and grouping keeps a narrow sidebar readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpellingRow {
    /// A file heading; `hit` is the file's first hit and `count` its total.
    File {
        /// Index into [`SpellingPanel::hits`] of this file's first misspelling.
        hit: usize,
        /// How many misspellings this file has.
        count: usize,
    },
    /// One misspelled word; `hit` indexes [`SpellingPanel::hits`].
    Word {
        /// Index into [`SpellingPanel::hits`].
        hit: usize,
    },
}

impl SpellingRow {
    /// The hit this row jumps to when activated.
    pub(crate) fn hit(self) -> usize {
        match self {
            Self::File { hit, .. } | Self::Word { hit } => hit,
        }
    }
}

/// The workspace-spelling panel state.
#[derive(Default)]
pub(crate) struct SpellingPanel {
    /// Every misspelling the current scan has reported, grouped by file.
    pub(crate) hits: Vec<SpellingHit>,
    /// The rendered rows derived from [`hits`](Self::hits).
    pub(crate) rows: Vec<SpellingRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
    /// The in-flight scan, if one is running. Progress for any other request is
    /// stale — a re-scan supersedes its predecessor.
    pub(crate) scanning: Option<RequestId>,
    /// How many files the running (or last) scan visited.
    pub(crate) files_scanned: usize,
    /// The last scan stopped at its result limit.
    pub(crate) truncated: bool,
    /// Whether a scan has ever completed, so an empty list can distinguish
    /// "nothing found" from "nothing asked for yet".
    pub(crate) scanned: bool,
}

impl SpellingPanel {
    /// Rebuild [`rows`](Self::rows) from `hits`, clamping the cursor.
    ///
    /// `hits` arrive grouped by file already (the scan walks a directory tree, and
    /// the session emits each open document's hits together), so a run-length pass
    /// over consecutive equal paths is enough to insert the headings.
    pub(crate) fn rebuild_rows(&mut self) {
        self.rows.clear();
        let mut index = 0;
        while index < self.hits.len() {
            let path = &self.hits[index].path;
            let count = self.hits[index..]
                .iter()
                .take_while(|hit| hit.path == *path)
                .count();
            self.rows.push(SpellingRow::File { hit: index, count });
            self.rows
                .extend((index..index + count).map(|hit| SpellingRow::Word { hit }));
            index += count;
        }
        let cursor = self.selection.cursor();
        self.selection = ListSelection::new(self.rows.len());
        self.selection
            .move_to(cursor.min(self.rows.len().saturating_sub(1)));
    }

    /// Drop every result, readying the panel for a fresh scan.
    pub(crate) fn clear(&mut self) {
        self.hits.clear();
        self.rows.clear();
        self.selection = ListSelection::new(0);
        self.files_scanned = 0;
        self.truncated = false;
        self.scanned = false;
    }
}

/// One row of the Todos panel's list: a grouping header (a file, or a tag when
/// grouped by tag) or one codetag under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TodoRow {
    /// A group heading; `hit` is the group's first hit and `count` its total.
    Group {
        /// Index into [`TodosPanel::order`] of the group's first hit.
        hit: usize,
        /// How many hits the group holds.
        count: usize,
    },
    /// One codetag; `hit` indexes [`TodosPanel::order`].
    Item {
        /// Index into [`TodosPanel::order`].
        hit: usize,
    },
}

impl TodoRow {
    /// The hit this row jumps to when activated.
    pub(crate) fn hit(self) -> usize {
        match self {
            Self::Group { hit, .. } | Self::Item { hit } => hit,
        }
    }
}

/// The workspace codetag (Todos) panel state.
#[derive(Default)]
pub(crate) struct TodosPanel {
    /// Every codetag the current scan has reported, in scan order (grouped by
    /// file, since the walk visits file by file).
    pub(crate) hits: Vec<karet_session::TodoHit>,
    /// Display order: indices into `hits`, regrouped when `by_tag` is set.
    pub(crate) order: Vec<usize>,
    /// The rendered rows derived from `order`.
    pub(crate) rows: Vec<TodoRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
    /// The in-flight scan, if one is running.
    pub(crate) scanning: Option<RequestId>,
    /// How many files the running (or last) scan visited.
    pub(crate) files_scanned: usize,
    /// The last scan stopped at its result limit.
    pub(crate) truncated: bool,
    /// Whether a scan has ever completed.
    pub(crate) scanned: bool,
    /// Group rows by tag (`TODO` / `FIXME` / …) instead of by file.
    pub(crate) by_tag: bool,
}

impl TodosPanel {
    /// Rebuild `order` and [`rows`](Self::rows) from `hits`, clamping the cursor.
    pub(crate) fn rebuild_rows(&mut self) {
        self.order = (0..self.hits.len()).collect();
        if self.by_tag {
            self.order
                .sort_by(|&a, &b| self.hits[a].tag.cmp(&self.hits[b].tag).then(a.cmp(&b)));
        }
        self.rows.clear();
        let same_group = |a: usize, b: usize| {
            if self.by_tag {
                self.hits[a].tag == self.hits[b].tag
            } else {
                self.hits[a].path == self.hits[b].path
            }
        };
        let mut index = 0;
        while index < self.order.len() {
            let count = self.order[index..]
                .iter()
                .take_while(|&&hit| same_group(self.order[index], hit))
                .count();
            self.rows.push(TodoRow::Group { hit: index, count });
            self.rows
                .extend((index..index + count).map(|hit| TodoRow::Item { hit }));
            index += count;
        }
        let cursor = self.selection.cursor();
        self.selection = ListSelection::new(self.rows.len());
        self.selection
            .move_to(cursor.min(self.rows.len().saturating_sub(1)));
    }

    /// Drop every result, readying the panel for a fresh scan.
    pub(crate) fn clear(&mut self) {
        self.hits.clear();
        self.order.clear();
        self.rows.clear();
        self.selection = ListSelection::new(0);
        self.files_scanned = 0;
        self.truncated = false;
        self.scanned = false;
    }
}

/// The Todos panel's per-frame chrome — same shape as the Spelling panel's.
pub(crate) type TodosChrome = SpellingChrome;

/// Per-document caches fed by backend events, keyed by session document.
/// (Manifest hints live beside diagnostics: both are per-document layers.)
#[derive(Default)]
pub(crate) struct DocState {
    /// Editing/save behavior resolved per open session document.
    pub(crate) settings: HashMap<DocumentId, DocumentSettings>,
    /// Latest complete diagnostic set per editable backend document.
    pub(crate) diagnostics: HashMap<DocumentId, Vec<Diagnostic>>,
    /// Latest language-server symbol tree for each open document.
    pub(crate) symbols: HashMap<DocumentId, Vec<Symbol>>,
    /// Buffer version represented by each cached symbol tree.
    pub(crate) outline_versions: HashMap<DocumentId, u64>,
    /// In-flight symbol request version and start time per document.
    pub(crate) outline_loading: HashMap<DocumentId, (u64, Pending)>,
    /// Dependency-freshness hints per open manifest, with the checked version.
    pub(crate) manifest_hints: HashMap<DocumentId, (u64, Vec<karet_session::ManifestHint>)>,
}

/// The Source-Control panel state: the changed files (staged first) and selection.
pub(crate) struct Scm {
    /// Changed files: the staged group first, then the working group. Identity
    /// and line counts only — the backend prepares a displayable diff on demand.
    pub(crate) changes: Vec<ChangeSummary>,
    /// The number of staged files at the front of `changes`.
    pub(crate) staged_count: usize,
    /// The cursor and multi-file selection over `changes`.
    pub(crate) selection: ListSelection,
    /// The loaded commit-log page(s), newest first (lazily fetched).
    pub(crate) log: Vec<Commit>,
    /// Every ref per commit hash, refreshed with each log page.
    pub(crate) ref_labels: HashMap<String, Vec<karet_vcs::RefLabel>>,
    /// Whether more commits exist beyond the loaded ones.
    pub(crate) log_has_more: bool,
    /// Whether a log page request is currently in flight.
    pub(crate) log_loading: bool,
    /// When the current log-page request began, if one is in flight.
    pub(crate) log_loading_since: Option<Pending>,
    /// Latest branch, remote, recovery, and stash snapshot.
    pub(crate) repository: Option<RepositorySnapshot>,
    /// Whether a repository snapshot is being loaded.
    pub(crate) repository_loading_since: Option<Pending>,
    /// Request currently loading the repository snapshot.
    pub(crate) repository_request: Option<RequestId>,
    /// The repository action currently running, if any.
    pub(crate) operation: Option<VcsAction>,
}

/// Live current-buffer blame that still matches the active document and cursor.
#[derive(Clone)]
pub(crate) struct LiveBlame {
    pub(crate) doc: DocumentId,
    pub(crate) version: u64,
    pub(crate) line: u32,
    pub(crate) attribution: Option<BlameAttribution>,
}

impl LiveBlame {
    /// Compact attribution text shown after the active line.
    pub(crate) fn text(&self) -> Option<String> {
        match self.attribution.as_ref()? {
            BlameAttribution::Commit(commit) => Some(format!(
                "  {} {}",
                commit.author,
                crate::ui::relative_time(commit.author_time)
            )),
            BlameAttribution::Uncommitted => Some("  Uncommitted changes".to_string()),
            _ => None,
        }
    }

    /// Commit opened by the inline attribution's detail action.
    pub(crate) fn commit_hash(&self) -> Option<&str> {
        match self.attribution.as_ref()? {
            BlameAttribution::Commit(commit) => Some(&commit.hash),
            _ => None,
        }
    }

    /// Compact current-line attribution rendered as editor virtual text.
    pub(crate) fn decoration(&self) -> Option<Decoration> {
        let text = self.text()?;
        Some(Decoration {
            range: Range {
                start: LineCol::new(self.line, 0),
                end: LineCol::new(self.line, 1),
            },
            kind: DecorationKind::InlineText {
                text,
                before: false,
            },
            role: Some(ThemeRole::Muted),
        })
    }
}

impl Scm {
    /// The Source-Control [`Section`] for the entry at `index`.
    pub(crate) fn section(&self, index: usize) -> Section {
        if index < self.staged_count {
            Section::Staged
        } else {
            Section::Working
        }
    }

    /// The repository-relative paths of the selected file(s).
    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .selected_indices()
            .into_iter()
            .filter_map(|i| self.changes.get(i))
            .map(|c| c.path.clone())
            .collect()
    }
}

/// The tab list of a pane that does not currently hold focus. The focused pane's
/// tabs live directly on [`App`] (`tabs`/`active`); switching focus swaps a pane's
/// tabs in and out of here, so the vast majority of the shell operates on "the
/// current pane" without knowing about the split layout.
pub(crate) struct StoredPane {
    /// The pane's open tabs.
    pub(crate) tabs: Vec<Tab>,
    /// The pane's active tab index.
    pub(crate) active: usize,
}

/// A toggleable match option shared by the Search panel and the in-file find bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchOption {
    /// Interpret the query as a regular expression.
    Regex,
    /// Match case-sensitively.
    Case,
    /// Match whole words only.
    Word,
}

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

/// Persistent multiline commit-message editor shown in the Source Control panel.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitInput {
    /// Draft message, retained while the field is blurred and while a commit runs.
    pub(crate) text: String,
    /// Cursor, selection, and viewport state within the draft.
    pub(crate) edit: TextAreaState,
    /// Whether keyboard input is currently routed into the field.
    pub(crate) focused: bool,
    /// Commit request in flight; prevents accidental duplicate submissions.
    pub(crate) pending: Option<RequestId>,
}

/// A quit request waiting for a repository mutation that must not be interrupted.
pub(crate) struct OperationBlocker {
    /// Human-readable operation label.
    pub(crate) label: String,
    /// Point after which shutdown stops waiting.
    pub(crate) deadline: Instant,
}

/// A document open owned by one concrete editor view.
pub(crate) struct PendingOpen {
    pub(crate) path: PathBuf,
    pub(crate) view: ViewId,
}

/// Which filesystem operation the explorer's internal file clipboard will perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExplorerFileOp {
    /// Duplicate the selected files/directories on paste.
    Copy,
    /// Move the selected files/directories on paste.
    Cut,
}

/// The settings layer that should receive an accepted spelling word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DictionaryTarget {
    /// The current repository's `.karet/setting.jsonc`.
    Project,
    /// The platform user configuration.
    User,
}

/// The explorer's internal file clipboard. This is intentionally separate from the
/// system text clipboard: terminal clipboards do not carry portable file lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExplorerFileClipboard {
    pub(crate) op: ExplorerFileOp,
    pub(crate) paths: Vec<PathBuf>,
}

/// The action dispatched by a positioned context-menu row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContextMenuAction {
    /// Dispatch an ordinary named application command.
    Command(Command),
    /// Replace one misspelled range with a dictionary suggestion.
    ReplaceSpelling {
        /// The document containing the warning.
        doc: DocumentId,
        /// The exact warning range to replace.
        range: Range,
        /// The suggested replacement.
        replacement: String,
    },
    /// Add a word to one spell-check dictionary layer.
    AddSpellingToDictionary {
        /// The word accepted by the user.
        word: String,
        /// The configuration layer to update.
        target: DictionaryTarget,
    },
}

/// One row of the app's context menu (the shared widget over
/// [`ContextMenuAction`]).
pub(crate) type ContextMenuEntry = karet_widgets::menu::ContextMenuEntry<ContextMenuAction>;
/// The app's positioned context menu (opened from the explorer or over a pane).
pub(crate) type ContextMenu = karet_widgets::menu::ContextMenu<ContextMenuAction>;

impl From<Command> for ContextMenuAction {
    fn from(command: Command) -> Self {
        Self::Command(command)
    }
}

/// App-side accessors over the shared menu entry.
pub(crate) trait ContextMenuEntryExt {
    /// The named command behind this row, when it is a regular command action.
    fn command(&self) -> Option<Command>;
}

impl ContextMenuEntryExt for ContextMenuEntry {
    fn command(&self) -> Option<Command> {
        match &self.action {
            ContextMenuAction::Command(command) => Some(*command),
            ContextMenuAction::ReplaceSpelling { .. }
            | ContextMenuAction::AddSpellingToDictionary { .. } => None,
        }
    }
}

/// A remote action parked until the backend answers the facts request that
/// enables it (see [`App::copy_remote_link`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingRemoteAction {
    /// Copy a web link once the facts for `path` arrive.
    CopyLink {
        /// Which link flavor to build.
        kind: remote::LinkKind,
        /// The file the link points at.
        path: PathBuf,
        /// The 1-based line anchoring a permalink, when any.
        line: Option<u32>,
    },
}

/// The repository/remote facts behind the pane menu's link actions, resolved on
/// the backend's VCS worker and cached per path (see [`App::cached_remote_facts`]).
pub(crate) struct RemoteFacts {
    /// The parsed origin remote.
    pub(crate) remote: remote::Remote,
    /// The full `HEAD` commit hash, or `None` on an unborn branch.
    pub(crate) head: Option<String>,
    /// The current branch's short name, or `None` when `HEAD` is detached.
    pub(crate) branch: Option<String>,
    /// The file's path relative to the repository worktree root.
    pub(crate) rel_path: PathBuf,
    /// Whether the file exists in the `HEAD` commit's tree.
    pub(crate) tracked: bool,
}

impl RemoteFacts {
    /// Borrow these facts as a [`remote::LinkTarget`] for link building.
    pub(crate) fn link_target(&self) -> remote::LinkTarget<'_> {
        remote::LinkTarget {
            remote: &self.remote,
            head: self.head.as_deref(),
            branch: self.branch.as_deref(),
            rel_path: &self.rel_path,
            tracked: self.tracked,
        }
    }
}

/// An irreversible close routed through the unified unsaved-changes guard. Every
/// entry point that can drop a tab (or the whole app) names its intent here so the
/// guard can decide, uniformly, whether it must first confirm the loss of unsaved
/// changes (see [`App::guarded_close`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseRequest {
    /// Quit the application.
    Quit,
    /// Close a single tab in the focused pane, identified by its stable view id so
    /// the request survives index shifts while a save-then-close is in flight.
    Tab {
        /// The view id of the tab to close.
        view: ViewId,
    },
    /// Close every tab in the focused pane except the active one.
    OtherTabs,
    /// Close every tab to the right of the active one in the focused pane.
    TabsToRight,
    /// Close every tab in the focused pane (leaving a Welcome tab).
    AllTabs,
}

/// An edit waiting for the configured automatic-save trigger.
#[derive(Clone, Copy)]
pub(crate) struct PendingAutoSave {
    /// Newest document version covered by this trigger.
    pub(crate) version: u64,
    /// Debounce deadline, or `None` when waiting for an editor-focus change.
    pub(crate) deadline: Option<Instant>,
}

/// One save request in flight.
#[derive(Clone, Copy)]
pub(crate) struct PendingSave {
    pub(crate) doc: DocumentId,
}

/// The Debug panel's chrome (same shape as the other list panels).
pub(crate) type DebugChrome = SpellingChrome;

/// One row of the Debug panel's flattened section list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebugRow {
    /// A section heading (`CALL STACK`, `VARIABLES`, …).
    Section(&'static str),
    /// A stack frame; indexes [`DebugPanel::stack`].
    Frame(usize),
    /// A variables scope; indexes [`DebugPanel::scopes`].
    Scope(usize),
    /// One variable of a fetched reference.
    Variable {
        /// The parent `variablesReference` whose children hold it.
        parent: i64,
        /// The index within that parent's children.
        index: usize,
        /// The tree depth (scopes' children are depth 1).
        depth: u16,
    },
    /// One evaluate-log line; indexes [`DebugPanel::repl`].
    Repl(usize),
    /// One console-output line; indexes the app's debug output buffer.
    Output(usize),
    /// A muted placeholder note.
    Note(&'static str),
}

/// How many trailing console lines the panel shows.
pub(crate) const DEBUG_CONSOLE_TAIL: usize = 100;
/// The variables tree recursion cap (cyclic references exist in the wild).
const DEBUG_TREE_DEPTH_CAP: u16 = 8;

/// The Debug sidebar panel: the stopped thread's stack, a lazily-fetched
/// variables tree, the evaluate log, and the console tail. All inspection
/// state is per-stop: it clears on resume so nothing stale survives.
#[derive(Default)]
pub(crate) struct DebugPanel {
    /// The stopped thread's frames, top first.
    pub(crate) stack: Vec<karet_session::DebugFrame>,
    /// The frame inspection targets (scopes/evaluate context).
    pub(crate) selected_frame: Option<i64>,
    /// The selected frame's scopes.
    pub(crate) scopes: Vec<karet_session::DebugScope>,
    /// Fetched children per `variablesReference`.
    pub(crate) variables: HashMap<i64, Vec<karet_session::DebugVariable>>,
    /// Expanded references (scopes and structured variables).
    pub(crate) expanded: HashSet<i64>,
    /// In-flight inspection requests; answers not in here are stale and dropped.
    pub(crate) pending: HashSet<RequestId>,
    /// The evaluate log (`expr = result` lines, oldest first).
    pub(crate) repl: Vec<String>,
    /// The rendered rows.
    pub(crate) rows: Vec<DebugRow>,
    /// The cursor over `rows`.
    pub(crate) selection: ListSelection,
}

impl DebugPanel {
    /// Drop every per-stop artifact (stack, scopes, variables, pending).
    /// The evaluate log survives — it is a console, not a snapshot.
    pub(crate) fn clear_inspection(&mut self, output_len: usize) {
        self.stack.clear();
        self.selected_frame = None;
        self.scopes.clear();
        self.variables.clear();
        self.expanded.clear();
        self.pending.clear();
        self.rebuild_rows(output_len);
    }

    /// Rebuild the flattened rows, keeping the cursor clamped.
    pub(crate) fn rebuild_rows(&mut self, output_len: usize) {
        self.rows.clear();
        self.rows.push(DebugRow::Section("CALL STACK"));
        if self.stack.is_empty() {
            self.rows.push(DebugRow::Note("not stopped"));
        }
        self.rows.extend((0..self.stack.len()).map(DebugRow::Frame));
        self.rows.push(DebugRow::Section("VARIABLES"));
        if self.scopes.is_empty() {
            self.rows.push(DebugRow::Note("no frame selected"));
        }
        let scope_refs: Vec<i64> = self.scopes.iter().map(|scope| scope.reference).collect();
        for (index, reference) in scope_refs.into_iter().enumerate() {
            self.rows.push(DebugRow::Scope(index));
            if self.expanded.contains(&reference) {
                self.push_children(reference, 1);
            }
        }
        if !self.repl.is_empty() {
            self.rows.push(DebugRow::Section("EVALUATE"));
            self.rows.extend((0..self.repl.len()).map(DebugRow::Repl));
        }
        if output_len > 0 {
            self.rows.push(DebugRow::Section("CONSOLE"));
            let first = output_len.saturating_sub(DEBUG_CONSOLE_TAIL);
            self.rows.extend((first..output_len).map(DebugRow::Output));
        }
        let cursor = self
            .selection
            .cursor()
            .min(self.rows.len().saturating_sub(1));
        self.selection = ListSelection::new(self.rows.len());
        self.selection.move_to(cursor);
    }

    fn push_children(&mut self, parent: i64, depth: u16) {
        if depth > DEBUG_TREE_DEPTH_CAP {
            return;
        }
        let children = self.variables.get(&parent).cloned().unwrap_or_default();
        for (index, child) in children.iter().enumerate() {
            self.rows.push(DebugRow::Variable {
                parent,
                index,
                depth,
            });
            if child.reference > 0 && self.expanded.contains(&child.reference) {
                self.push_children(child.reference, depth + 1);
            }
        }
    }
}

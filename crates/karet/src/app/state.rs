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
    /// Clickable header buttons `(start, end, row, command)` (option toggles
    /// and replace-all).
    pub(crate) action_hits: Vec<(u16, u16, u16, Command)>,
}

/// Per-document caches fed by backend events, keyed by session document.
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
    pub(crate) results: Vec<FileHit>,
    /// The selected result.
    pub(crate) selected: usize,
    /// Whether a field is being edited (vs. browsing results).
    pub(crate) input: bool,
    /// Which field the input edits (find / replace).
    pub(crate) field: SearchField,
    /// Whether the replace field is shown (collapsible; shown by default).
    pub(crate) replace_visible: bool,
    /// Interpret the query as a regular expression.
    pub(crate) regex: bool,
    /// Match case-sensitively.
    pub(crate) case_sensitive: bool,
    /// Match whole words only.
    pub(crate) whole_word: bool,
}

impl Default for SearchPanel {
    fn default() -> Self {
        Self {
            query: String::new(),
            query_edit: TextFieldState::default(),
            replace: String::new(),
            replace_edit: TextFieldState::default(),
            results: Vec::new(),
            selected: 0,
            input: false,
            field: SearchField::Find,
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
    /// Cursor and selection state within the draft.
    pub(crate) edit: TextFieldState,
    /// First wrapped display row visible inside the field.
    pub(crate) scroll: u16,
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

/// Where a resolved commit detail should be shown.
#[derive(Clone)]
pub(crate) enum CommitDest {
    /// Fill the already-open standalone commit tab with this view id.
    Tab { view: ViewId },
    /// Fill the graph browser's detail pane if it still selects this hash.
    Browser { view: ViewId, hash: String },
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

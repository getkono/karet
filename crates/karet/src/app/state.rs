//! Grouped [`App`](super::App) sub-states: terminal capabilities, per-panel
//! chrome recorded during the last render (mouse hit-testing), per-document
//! caches, and the small records the shell's flows pass around. Data with at
//! most derived accessors: a panel whose state carries behaviour (rebuilding
//! its rows, folding, field focus) lives beside the module that drives it —
//! `debugging`, `search`, `spelling`, `todos` — and the terminal probe itself
//! is [`crate::term_caps`].

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use karet_core::BlameAttribution;
use karet_core::Decoration;
use karet_core::Diagnostic;
use karet_core::InlayHint;
use karet_core::Range;
use karet_core::Symbol;
use karet_core::ThemeRole;
use karet_core::VirtualText;
use karet_fileview::image::GraphicsProtocol;
use karet_session::ChangeSummary;
use karet_session::DocumentId;
use karet_session::DocumentSettings;
use karet_session::RepositorySnapshot;
use karet_session::RequestId;
use karet_session::VcsAction;
use karet_session::ViewId;
use karet_vcs::Commit;
use karet_widgets::ListSelection;
use karet_widgets::PaneId;
use karet_widgets::textarea::TextAreaState;
use ratatui::layout::Rect;

use super::inlay::HintRange;
use super::inlay::PendingInlay;
use crate::app::DEFAULT_SCM_COMMITS_H;
use crate::app::OUTLINE_WIDTH;
use crate::app::pending::Pending;
use crate::command::Command;
use crate::remote;
use crate::render::Section;
use crate::tab::Tab;

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
    /// Whether the Kitty graphics terminal also draws unicode placeholders, which
    /// the markdown preview paints its images with.
    pub(crate) placeholders: bool,
    /// The pixel size of one terminal cell, as the terminal last reported it.
    pub(crate) cell_px: (u32, u32),
    /// Whether the environment names a terminal that draws a curly underline,
    /// which diagnostics then use instead of a straight one.
    pub(crate) undercurl: bool,
}

impl TerminalCaps {
    /// What the environment alone answers ([`crate::term_caps::probe_env`]);
    /// everything a handshake confirms starts unconfirmed until the live app
    /// probes the terminal.
    pub(crate) fn detect() -> Self {
        Self::from_capabilities(&crate::term_caps::probe_env())
    }

    /// The app's view of one probe: the same [`TerminalCapabilities`]
    /// `--doctor` reports, so the two cannot disagree.
    ///
    /// [`TerminalCapabilities`]: crate::term_caps::TerminalCapabilities
    pub(crate) fn from_capabilities(caps: &crate::term_caps::TerminalCapabilities) -> Self {
        Self {
            graphics: caps.effective_graphics(),
            kitty_graphics: caps.kitty_graphics_supported(),
            kitty_keyboard: caps.keyboard_enhancement,
            pointer_shapes: caps.pointer_shapes_supported(),
            pointer_shape: None,
            placeholders: caps.kitty_placeholders(),
            cell_px: karet_markdown::DEFAULT_CELL_PIXELS,
            undercurl: caps.undercurl,
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
    /// The active Markdown view's outline rows, reused across frames until its
    /// text changes or another view becomes active.
    pub(crate) markdown: crate::outline::MarkdownOutlineCache,
}

/// The screen-reclaiming modes for small terminals: Zen Mode and a maximised
/// pane. Both are toggles that hide chrome without forgetting it.
#[derive(Default)]
pub(crate) struct ScreenModes {
    /// While Zen Mode is on, the panels it hid, so ending it restores them.
    /// `None` when Zen Mode is off.
    pub(crate) zen: Option<ZenRestore>,
    /// The pane drawn alone across the whole content area, or `None` when every
    /// pane is tiled. Cleared when focus moves to another pane.
    pub(crate) maximized: Option<PaneId>,
}

/// What Zen Mode hid when it began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ZenRestore {
    /// Whether the sidebar was shown.
    pub(crate) sidebar: bool,
    /// Whether the outline was shown.
    pub(crate) outline: bool,
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
            markdown: crate::outline::MarkdownOutlineCache::default(),
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
    /// The commit box's AI chip from the last frame, for hit-testing. Empty when
    /// the chip was not painted, so a click cannot reach an invisible target.
    pub(crate) ai_chip_rect: Rect,
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
            ai_chip_rect: Rect::default(),
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
    /// Latest inlay-hint set per document, positioned in buffer columns.
    pub(crate) inlay_hints: HashMap<DocumentId, Vec<InlayHint>>,
    /// The buffer version each held hint set is positioned against: the
    /// version it was answered for, then each version an edit carried it to.
    pub(crate) inlay_version: HashMap<DocumentId, u64>,
    /// What each cached hint set covers, so an unchanged viewport asks again
    /// for nothing.
    pub(crate) inlay_covered: HashMap<DocumentId, HintRange>,
    /// The outstanding hint request per document, for matching its answer.
    pub(crate) inlay_pending: HashMap<DocumentId, PendingInlay>,
    /// Bumped whenever hint coverage is invalidated, so a request issued
    /// before it stops counting as covering anything.
    pub(crate) inlay_epoch: u64,
    /// When each recently edited document will have been quiet long enough to
    /// ask about; its hint requests wait until then.
    pub(crate) inlay_quiet_until: HashMap<DocumentId, Instant>,
    /// How many hint requests in a row each document's server has left
    /// unanswered, which sets how long the next re-ask waits.
    pub(crate) inlay_failures: HashMap<DocumentId, u32>,
    /// Latest language-server symbol tree for each open document.
    pub(crate) symbols: HashMap<DocumentId, Vec<Symbol>>,
    /// Buffer version represented by each cached symbol tree.
    pub(crate) outline_versions: HashMap<DocumentId, u64>,
    /// In-flight symbol request version and start time per document.
    pub(crate) outline_loading: HashMap<DocumentId, (u64, Pending)>,
    /// Dependency-freshness hints per open manifest, with the checked version.
    pub(crate) manifest_hints: HashMap<DocumentId, (u64, Vec<karet_session::ManifestHint>)>,
    /// Latest uncommitted-line gutter markers per document. Held across edits
    /// until the next answer replaces them, so typing does not flicker them off.
    pub(crate) line_changes: HashMap<DocumentId, Vec<Decoration>>,
    /// The one in-flight line-change request per document.
    pub(crate) line_changes_pending: HashMap<DocumentId, RequestId>,
    /// Documents that changed while their request was in flight, to re-ask for
    /// as soon as it answers.
    pub(crate) line_changes_rerun: HashSet<DocumentId>,
    /// The view that last sent an undo or redo for each document. The
    /// snapshot answering it restores its selection set in that view only;
    /// every other view of the document maps its own carets through the change.
    pub(crate) history_view: HashMap<DocumentId, ViewId>,
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
                crate::app::relative_time(commit.author_time)
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

    /// Compact current-line attribution in the line's end-of-line slot, at
    /// [`BLAME_EOL_PRIORITY`](Self::BLAME_EOL_PRIORITY).
    pub(crate) fn virtual_text(&self) -> Option<VirtualText> {
        Some(VirtualText::end_of_line(
            self.line,
            self.text()?,
            ThemeRole::Muted,
            Self::BLAME_EOL_PRIORITY,
        ))
    }

    /// Inline blame is cut first when a line's end-of-line slot is short: it
    /// is context, where a dependency hint on the same line is a call to act.
    pub(crate) const BLAME_EOL_PRIORITY: u8 = 0;
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
    /// Preserve the replaced text's case in the replacement (find bar only).
    PreserveCase,
    /// Search only inside the selection (find bar only).
    InSelection,
}

/// Persistent multiline commit-message editor shown in the Source Control panel.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitInput {
    /// Draft message, retained while the field is blurred and while a commit runs.
    pub(crate) text: String,
    /// Cursor, selection, and viewport state within the draft.
    pub(crate) edit: TextAreaState,
    /// Commit request in flight; prevents accidental duplicate submissions.
    pub(crate) pending: Option<RequestId>,
    /// Whether the viewport was deliberately scrolled off the caret. The box
    /// follows the caret by default; a wheel notch or a scrollbar drag suspends
    /// that until the caret next moves, so scrolling back through a long draft is
    /// not undone by the next frame.
    pub(crate) scrolled_away: bool,
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
    /// Run one code action from the quick-fix menu.
    CodeAction {
        /// The document the actions were listed for.
        doc: DocumentId,
        /// The document version they were listed at.
        version: u64,
        /// The action, as the language server listed it.
        action: Box<karet_core::CodeAction>,
    },
    /// A group heading: shown, never selectable, does nothing.
    Heading,
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
            | ContextMenuAction::AddSpellingToDictionary { .. }
            | ContextMenuAction::CodeAction { .. }
            | ContextMenuAction::Heading => None,
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
    pub(crate) remote: karet_vcs::RemoteUrl,
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

//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Command`]s resolved from key events.

mod backend_events;
mod commands;
mod completion;
mod editor;
mod explorer;
mod history;
mod input;
mod lifecycle;
mod mouse;
mod panes;
mod remote_actions;
mod runtime;
mod scm;
mod search;
mod sidebar;
mod startup;
mod tabs;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use color_eyre::eyre::eyre;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableMouseCapture;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::EnableMouseCapture;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::KeyboardEnhancementFlags;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use crossterm::event::PopKeyboardEnhancementFlags;
use crossterm::event::PushKeyboardEnhancementFlags;
use crossterm::event::{self};
use crossterm::terminal::SetTitle;
use karet_core::BlameAttribution;
use karet_core::BytePos;
use karet_core::Change;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::LineCol;
use karet_core::Notification;
use karet_core::NotificationId;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Severity;
use karet_core::TextEdit;
use karet_core::ThemeRole;
use karet_editor::EditorState;
use karet_editor::Fold;
use karet_filetype::FileKind;
use karet_filetype::IconStyle;
use karet_filetype::WrapMode;
use karet_filetype::file_type_for_path;
use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::{self};
use karet_search::FileHit;
use karet_search::SearchQuery;
use karet_search::WorkspaceSearch;
use karet_search::search_in_file;
use karet_session::Backend;
use karet_session::BackendError;
use karet_session::Command as SessionCommand;
use karet_session::ConfigDiagnostic;
use karet_session::DocSnapshot;
use karet_session::DocumentId;
use karet_session::Event as SessionEvent;
use karet_session::EventRx;
use karet_session::GithubVerification;
use karet_session::LoadedConfig;
use karet_session::PullRequestSummary;
use karet_session::RangeSpec;
use karet_session::RepositorySnapshot;
use karet_session::RequestId;
use karet_session::Session;
use karet_session::SessionConfig;
use karet_session::Settings;
use karet_session::SnapshotRx;
use karet_session::SwapInfo;
use karet_session::VcsAction;
use karet_session::VcsOutcome;
use karet_session::ViewId;
use karet_session::local;
use karet_syntax::FoldRegions;
use karet_text::EditCause;
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::FileChange;
use karet_vcs::StatusKind;
use karet_widgets::DropZone;
use karet_widgets::FileTreeState;
use karet_widgets::ListSelection;
use karet_widgets::PaneId;
use karet_widgets::PaneLayout;
use karet_widgets::PendingEdit;
use karet_widgets::SplitDir;
use karet_widgets::drop_zone;
use ratatui::layout::Rect;
pub(crate) use runtime::run;
use tokio::sync::mpsc;

use crate::clipboard::Clipboard;
use crate::command::Command;
use crate::compat;
use crate::compat::GraphicsCaret;
use crate::editing;
use crate::keymap::Context;
use crate::keymap::EditorTab;
use crate::keymap::Focus;
use crate::keymap::FocusTarget;
use crate::keymap::KeyChord;
use crate::keymap::Modal;
use crate::keymap::Resolved;
use crate::keymap::SidebarPanel;
use crate::keymap::{self};
use crate::notify::NotificationCenter;
use crate::outline::OutlineRow;
use crate::outline::OutlineTarget;
use crate::overlay::DiffTarget;
use crate::overlay::Overlay;
use crate::overlay::OverlayEvent;
use crate::overlay::StashAction;
use crate::overlay::TextPurpose;
use crate::remote;
use crate::render::FileView;
use crate::render::Section;
use crate::tab::CommitViewState;
use crate::tab::FindState;
use crate::tab::MarkdownPreviewState;
use crate::tab::SearchField;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;
use crate::tab::commit_title;
use crate::ui;
use crate::workspace;

/// The Source-Control panel state: the changed files (staged first) and selection.
pub(crate) struct Scm {
    /// Changed files: the staged group first, then the working group.
    pub(crate) changes: Vec<FileChange>,
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
    pub(crate) log_loading_since: Option<Instant>,
    /// Latest branch, remote, recovery, and stash snapshot.
    pub(crate) repository: Option<RepositorySnapshot>,
    /// Whether a repository snapshot is being loaded.
    pub(crate) repository_loading_since: Option<Instant>,
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
    fn section(&self, index: usize) -> Section {
        if index < self.staged_count {
            Section::Staged
        } else {
            Section::Working
        }
    }

    /// The repository-relative paths of the selected file(s).
    fn selected_paths(&self) -> Vec<PathBuf> {
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
    /// The replacement text.
    pub(crate) replace: String,
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
            replace: String::new(),
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

/// The maximum number of matching files the workspace search panel collects.
const SEARCH_RESULT_CAP: usize = 500;

/// How many commits the source-control log fetches per lazily-loaded page.
const SCM_LOG_PAGE: usize = 25;

/// The default height (rows) of the pinned Source-Control commit-log region.
const DEFAULT_SCM_COMMITS_H: u16 = 8;

/// The minimum height (rows) each Source-Control region keeps when both the changes
/// and the pinned commit-log region are shown.
pub(crate) const MIN_SCM_REGION: u16 = 3;

/// The default sidebar width in columns (before the user drags the divider).
pub(crate) const DEFAULT_SIDEBAR_WIDTH: u16 = 30;

/// The minimum sidebar width in columns; dragging the divider narrower than this
/// collapses the sidebar entirely.
pub(crate) const SIDEBAR_MIN_WIDTH: u16 = 16;

/// The width of the right-side outline panel in columns.
pub(crate) const OUTLINE_WIDTH: u16 = 30;

/// Load the next commit page once the Source-Control viewport comes within this many
/// rows of the end of the loaded log.
const COMMIT_AUTOLOAD_THRESHOLD: usize = 3;

/// How long the commit view's signature-badge explanation stays revealed after a
/// double-click before it auto-hides.
pub(crate) const COMMIT_REVEAL: Duration = Duration::from_secs(5);

/// Delay before rendering non-blocking loading text. Fast operations can complete
/// without visual churn; slower ones get an explicit, stable placeholder.
pub(crate) const LOADING_REVEAL_DELAY: Duration = Duration::from_millis(200);
/// Maximum graceful wait for a repository mutation during application shutdown.
const OPERATION_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(60);

/// Half-period for the app-drawn graphical editor caret.
const GRAPHICS_CARET_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// A clickable tab region in the tab strip, recorded during the last render.
#[derive(Clone, Copy)]
pub(crate) struct TabHit {
    /// First column of the tab (inclusive).
    pub(crate) start: u16,
    /// One past the last column of the tab (exclusive).
    pub(crate) end: u16,
    /// Column of the close (×) glyph.
    pub(crate) close: u16,
}

/// A clickable breadcrumb segment recorded during the last frame: its column span
/// on the breadcrumb row and the path prefix it resolves to (always within the
/// workspace root — segments above the root are never recorded).
#[derive(Clone)]
pub(crate) struct BreadcrumbHit {
    /// First column of the segment (inclusive).
    pub(crate) start: u16,
    /// One past the last column of the segment (exclusive).
    pub(crate) end: u16,
    /// The absolute path up to (and including) this segment's component.
    pub(crate) path: PathBuf,
}

/// A clickable changed-file row from a commit or compare view's last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommitFileHit {
    /// The rendered row in screen coordinates.
    pub(crate) rect: Rect,
    /// The changed file's index in the tab.
    pub(crate) file: usize,
    /// The layout-specific scroll offset that puts its card header at the top.
    pub(crate) scroll: u16,
}

/// A visible link run in the focused Markdown preview's last rendered frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkdownLinkHit {
    /// The rendered cells occupied by this run.
    pub(crate) rect: Rect,
    /// The renderer-neutral target from the Markdown source.
    pub(crate) target: String,
}

/// Persistent multiline commit-message editor shown in the Source Control panel.
#[derive(Clone, Debug, Default)]
pub(crate) struct CommitInput {
    /// Draft message, retained while the field is blurred and while a commit runs.
    pub(crate) text: String,
    /// Byte offset of the insertion caret (always a UTF-8 boundary).
    pub(crate) cursor: usize,
    /// First wrapped display row visible inside the field.
    pub(crate) scroll: u16,
    /// Whether keyboard input is currently routed into the field.
    pub(crate) focused: bool,
    /// Commit request in flight; prevents accidental duplicate submissions.
    pub(crate) pending: Option<RequestId>,
}

/// A rendered pane's clickable regions, recorded during the last frame for mouse
/// hit-testing (which pane a click lands in, and its tab strip / content).
#[derive(Clone)]
pub(crate) struct PaneFrame {
    /// The pane this frame belongs to.
    pub(crate) pane: PaneId,
    /// The pane's tab strip row.
    pub(crate) tabstrip_rect: Rect,
    /// Per-tab clickable regions within the strip.
    pub(crate) tab_hits: Vec<TabHit>,
    /// The pane's breadcrumb row (zero-sized when the active tab has no path).
    pub(crate) breadcrumb_rect: Rect,
    /// Per-segment clickable regions within the breadcrumb row.
    pub(crate) breadcrumb_hits: Vec<BreadcrumbHit>,
    /// The pane's content (editor) area.
    pub(crate) content_rect: Rect,
    /// Changed-file rows clickable within the pane's commit-like view.
    pub(crate) commit_file_hits: Vec<CommitFileHit>,
}

/// An in-progress tab drag: the pane it started from and the current drop target
/// (a pane plus which zone of it), used to preview and apply a move/split on release.
#[derive(Clone, Copy)]
pub(crate) struct TabDrag {
    /// The pane the dragged tab started in (and is still in until dropped).
    pub(crate) from_pane: PaneId,
    /// The current drop target: a pane and the zone the cursor is over, if any.
    pub(crate) hover: Option<(PaneId, DropZone)>,
}

/// A clickable toast card, recorded during the last render for click hit-testing.
#[derive(Clone, Copy)]
pub(crate) struct ToastHit {
    /// The card rectangle (a click anywhere on it dismisses the notification).
    pub(crate) rect: Rect,
    /// The notification the card shows.
    pub(crate) id: NotificationId,
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
enum CommitDest {
    /// Fill the already-open standalone commit tab with this view id.
    Tab { view: ViewId },
    /// Fill the graph browser's detail pane if it still selects this hash.
    Browser { view: ViewId, hash: String },
}

/// Which filesystem operation the explorer's internal file clipboard will perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplorerFileOp {
    /// Duplicate the selected files/directories on paste.
    Copy,
    /// Move the selected files/directories on paste.
    Cut,
}

/// The explorer's internal file clipboard. This is intentionally separate from the
/// system text clipboard: terminal clipboards do not carry portable file lists.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExplorerFileClipboard {
    op: ExplorerFileOp,
    paths: Vec<PathBuf>,
}

/// One row of a positioned context menu: the command it dispatches, whether it can
/// run right now, and an optional note explaining why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextMenuEntry {
    /// The command this row dispatches when accepted.
    pub(crate) command: Command,
    /// Whether the row can be activated. A disabled row renders dimmed, is skipped
    /// by keyboard navigation, and refuses Accept.
    pub(crate) enabled: bool,
    /// Why the row is disabled, surfaced as a status message when the user tries to
    /// activate it anyway (e.g. by clicking it).
    pub(crate) note: Option<String>,
}

impl ContextMenuEntry {
    /// An enabled entry dispatching `command`.
    fn enabled(command: Command) -> Self {
        Self {
            command,
            enabled: true,
            note: None,
        }
    }

    /// A disabled entry for `command`, greyed out with an explanatory `note`.
    fn disabled(command: Command, note: impl Into<String>) -> Self {
        Self {
            command,
            enabled: false,
            note: Some(note.into()),
        }
    }
}

/// A positioned context menu (opened from the explorer or over a pane).
pub(crate) struct ContextMenu {
    /// The column where the menu should be anchored.
    pub(crate) x: u16,
    /// The row where the menu should be anchored.
    pub(crate) y: u16,
    /// The rows shown in the menu, in display order.
    pub(crate) entries: Vec<ContextMenuEntry>,
    /// The selected row index.
    pub(crate) selected: usize,
    /// The menu rect from the last render.
    pub(crate) rect: Rect,
}

impl ContextMenu {
    fn new(x: u16, y: u16, entries: Vec<ContextMenuEntry>) -> Self {
        // Land the initial selection on the first activatable row.
        let selected = entries.iter().position(|e| e.enabled).unwrap_or(0);
        Self {
            x,
            y,
            entries,
            selected,
            rect: Rect::default(),
        }
    }

    /// Move the selection by `delta` rows, skipping disabled entries. When fewer
    /// enabled rows exist in that direction, the selection lands on the last one
    /// found (or stays put).
    fn select_by(&mut self, delta: i32) {
        if self.entries.is_empty() || delta == 0 {
            return;
        }
        let step: i64 = if delta > 0 { 1 } else { -1 };
        let mut remaining = i64::from(delta).abs();
        let mut idx = self.selected as i64;
        let mut landed = self.selected as i64;
        loop {
            idx += step;
            if idx < 0 || idx >= self.entries.len() as i64 {
                break;
            }
            if self.entries[idx as usize].enabled {
                landed = idx;
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
        }
        self.selected = landed as usize;
    }

    fn selected_entry(&self) -> Option<&ContextMenuEntry> {
        self.entries.get(self.selected)
    }
}

/// The repository/remote facts behind the pane menu's link actions, gathered
/// synchronously from a short-lived repository handle (see [`App::remote_facts`]).
struct RemoteFacts {
    /// The parsed origin remote.
    remote: remote::Remote,
    /// The full `HEAD` commit hash, or `None` on an unborn branch.
    head: Option<String>,
    /// The current branch's short name, or `None` when `HEAD` is detached.
    branch: Option<String>,
    /// The file's path relative to the repository worktree root.
    rel_path: PathBuf,
    /// Whether the file exists in the `HEAD` commit's tree.
    tracked: bool,
}

impl RemoteFacts {
    /// Borrow these facts as a [`remote::LinkTarget`] for link building.
    fn link_target(&self) -> remote::LinkTarget<'_> {
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

/// The IDE shell state.
pub struct App {
    /// The workspace root.
    pub(crate) root: PathBuf,
    /// The loaded, verified configuration (see `karet_session::config`). Applied to
    /// the UI at startup and handed to the session backend.
    pub(crate) settings: Settings,
    /// The loaded configuration plus provenance for the settings inspector.
    pub(crate) loaded_config: LoadedConfig,
    /// Config-load diagnostics awaiting display as startup notifications.
    pub(crate) config_diagnostics: Vec<ConfigDiagnostic>,
    /// The active color theme.
    pub(crate) theme: Theme,
    /// Whether syntax highlighting is enabled.
    pub(crate) syntax: bool,
    /// The icon style for the explorer and activity bar.
    pub(crate) icon_style: IconStyle,
    /// Command-line icon selection, which remains authoritative across config reloads.
    icon_override: Option<IconStyle>,
    /// The detected terminal graphics protocol.
    pub(crate) graphics: GraphicsProtocol,
    /// Whether Kitty graphics support was detected or confirmed at startup.
    kitty_graphics_supported: bool,
    /// Whether crossterm confirmed Kitty keyboard protocol support at startup.
    kitty_keyboard_supported: bool,
    /// Whether the terminal was confirmed (via a startup handshake) to support
    /// OSC 22 mouse-pointer-shape hints. `false` means every pointer-shape hint
    /// is a no-op — never assumed, only confirmed, mirroring `graphics`.
    pub(crate) pointer_shapes_supported: bool,
    /// The last OSC 22 pointer shape sent (so hover doesn't re-send every mouse
    /// event), or `None` for the terminal's default shape.
    pub(crate) pointer_shape: Option<&'static str>,
    /// Which area has keyboard focus.
    pub(crate) focus: Focus,
    /// The active sidebar panel.
    pub(crate) sidebar_panel: SidebarPanel,
    /// Whether the sidebar is shown.
    pub(crate) sidebar_visible: bool,
    /// The file-explorer tree state.
    pub(crate) explorer: FileTreeState,
    /// Files/directories selected for an explorer copy or cut operation.
    explorer_clipboard: Option<ExplorerFileClipboard>,
    /// The active context menu (explorer or pane), if any.
    pub(crate) context_menu: Option<ContextMenu>,
    /// The Source-Control panel state.
    pub(crate) scm: Scm,
    /// Most recent stale-checked live blame result.
    pub(crate) live_blame: Option<LiveBlame>,
    /// Request currently computing live blame.
    pub(crate) pending_blame: Option<(RequestId, DocumentId, u64, u32)>,
    /// Failed blame anchor, suppressed until its inputs change.
    pub(crate) failed_blame: Option<(DocumentId, u64, u32)>,
    /// Open-pull-request query currently filling the picker.
    pub(crate) pending_pull_requests: Option<RequestId>,
    /// Pull-request pages accumulated until GitHub has no next page.
    pub(crate) pull_request_items: Vec<PullRequestSummary>,
    /// Remote associated with the accumulating pull-request query.
    pub(crate) pull_request_remote: Option<String>,
    /// Repository action parked until all dirty editors save successfully.
    pub(crate) vcs_after_save: Option<VcsAction>,
    /// The focused pane's open tabs.
    pub(crate) tabs: Vec<Tab>,
    /// The focused pane's active tab index.
    pub(crate) active: usize,
    /// The window split layout; its focused pane's tabs are `tabs`/`active` above.
    pub(crate) layout: PaneLayout,
    /// The tabs of every pane that does not currently hold focus, keyed by pane id.
    pub(crate) stored: HashMap<PaneId, StoredPane>,
    /// Paths of recently-closed file tabs, for "reopen closed editor" (newest last).
    pub(crate) closed: Vec<PathBuf>,
    /// The open modal overlay (quick-open / command palette), if any.
    pub(crate) overlay: Option<Overlay>,
    /// Whether the find-in-file bar is currently shown. The query/toggle data
    /// itself lives on the active tab (`Tab::find`), so this only tracks
    /// visibility — closing the bar (Esc) clears this without discarding that
    /// data, and it is reset whenever the active tab changes.
    pub(crate) find_open: bool,
    /// The permanent multiline Source-Control commit-message editor.
    pub(crate) commit_input: CommitInput,
    /// The in-progress revision text while the go-to-commit input is open.
    pub(crate) rev_input: Option<String>,
    /// Paths awaiting a discard confirmation (set after pressing discard; cleared
    /// when the user confirms or cancels).
    pub(crate) pending_discard: Option<Vec<PathBuf>>,
    /// Paths awaiting explorer-delete confirmation.
    pub(crate) pending_explorer_delete: Option<Vec<PathBuf>>,
    /// The irreversible close awaiting the unsaved-changes confirmation prompt, if
    /// one is armed (unified across quit and tab/pane closes).
    pub(crate) pending_close: Option<CloseRequest>,
    /// Destructive backend work currently delaying a requested quit.
    pub(crate) operation_blocker: Option<OperationBlocker>,
    /// The close parked mid-save after choosing "save & close": run it once the
    /// issued saves drain (see [`App::on_backend_event`]).
    pub(crate) saving_close: Option<CloseRequest>,
    /// Crash-recovery swaps offered by the backend at startup, awaiting the user's
    /// recover/discard decision.
    pub(crate) pending_swaps: Option<Vec<SwapInfo>>,
    /// Chords typed so far toward a multi-key binding (empty when not mid-sequence).
    pub(crate) pending: Vec<KeyChord>,
    /// The workspace-search panel state.
    pub(crate) search: SearchPanel,
    /// A transient status message.
    pub(crate) status: Option<String>,
    /// The centralized notification stack (errors, out-of-band conditions).
    pub(crate) notifications: NotificationCenter,
    /// Clickable toast cards from the last frame (mouse hit-testing).
    pub(crate) toast_hits: Vec<ToastHit>,
    /// The sidebar rect from the last frame (mouse hit-testing).
    pub(crate) sidebar_rect: Rect,
    /// The main content rect from the last frame.
    pub(crate) main_rect: Rect,
    /// The user-controlled sidebar width in columns (draggable; clamped responsively
    /// to the terminal width each frame).
    pub(crate) sidebar_width: u16,
    /// The x column of the sidebar's drag divider from the last frame (hit-testing).
    pub(crate) sidebar_divider_x: u16,
    /// Whether a sidebar-resize drag is currently in progress.
    pub(crate) sidebar_resizing: bool,
    /// The last-used diff layout; newly-opened diffs adopt it so the choice sticks.
    pub(crate) diff_layout: ViewMode,
    /// Per-pane clickable regions from the last frame (mouse hit-testing).
    pub(crate) pane_frames: Vec<PaneFrame>,
    /// The in-progress tab drag, if the pointer is dragging a tab.
    pub(crate) tab_drag: Option<TabDrag>,
    /// The sidebar's content area (below the header) from the last frame.
    pub(crate) sidebar_content_rect: Rect,
    /// The current mouse position while hovering the sidebar content, for a
    /// secondary-accent row highlight (explorer / source-control lists).
    pub(crate) hover: Option<(u16, u16)>,
    /// The current mouse position while hovering the sidebar header controls.
    pub(crate) sidebar_header_hover: Option<(u16, u16)>,
    /// The header panel-switcher cells (`1 2 3`) from the last frame.
    pub(crate) panel_hits: Vec<(u16, u16, SidebarPanel)>,
    /// Whether the right-side outline panel is shown.
    pub(crate) outline_visible: bool,
    /// The outline panel's row selection, driving keyboard navigation.
    pub(crate) outline_sel: ListSelection,
    /// The outline panel rect from the last frame (mouse hit-testing).
    pub(crate) outline_rect: Rect,
    /// The outline panel's content area (below its header) from the last frame.
    pub(crate) outline_content_rect: Rect,
    /// The outline panel width in columns.
    pub(crate) outline_width: u16,
    /// The outline list's scroll offset (first visible row) from the last frame, so a
    /// click maps to the correct entry even when the list is scrolled.
    pub(crate) outline_scroll: usize,
    /// The explorer header toolbar-button cells `(start, end, command)` from the last
    /// frame (new file / new folder / refresh / collapse all).
    pub(crate) header_action_hits: Vec<(u16, u16, Command)>,
    /// Source-Control *changes* display-row → change-index map from the last frame.
    pub(crate) scm_row_map: Vec<Option<usize>>,
    /// Source-Control header controls `(start, end, row, command)`.
    pub(crate) scm_header_hits: Vec<(u16, u16, u16, Command)>,
    /// The changes-region scroll offset (top region; wheel + selection-follow).
    pub(crate) scm_offset: usize,
    /// The changes-region viewport rect from the last frame (hit-testing/hover).
    pub(crate) scm_changes_rect: Rect,
    /// The editable inner rect of the permanent Source-Control commit field.
    pub(crate) scm_commit_rect: Rect,
    /// The total number of changes display rows from the last frame.
    pub(crate) scm_total_rows: usize,
    /// The commit-log region scroll offset (bottom pinned region; wheel + autoload).
    pub(crate) scm_commits_offset: usize,
    /// The commit-log region viewport rect from the last frame (hit-testing).
    pub(crate) scm_commits_rect: Rect,
    /// The total number of commit-log display rows from the last frame.
    pub(crate) scm_commits_total: usize,
    /// The display row *within the commit-log region* of the "load more" affordance.
    pub(crate) scm_more_row: Option<usize>,
    /// User-controlled height (rows) of the pinned commit-log region (draggable).
    pub(crate) scm_commits_h: u16,
    /// The y of the changes/commits drag divider from the last frame (0 = not shown).
    pub(crate) scm_divider_y: u16,
    /// Whether a commits-divider resize drag is in progress.
    pub(crate) scm_resizing: bool,
    /// The search-results area from the last frame.
    pub(crate) search_results_rect: Rect,
    /// The search-results list scroll offset from the last frame.
    pub(crate) search_offset: usize,
    /// The Search panel's find-field row y from the last frame (click to edit).
    pub(crate) search_query_row: u16,
    /// The Search panel's replace-field row y from the last frame, if shown.
    pub(crate) search_replace_row: Option<u16>,
    /// The Search panel's clickable header buttons `(start, end, row, command)` from
    /// the last frame (option toggles and replace-all).
    pub(crate) search_action_hits: Vec<(u16, u16, u16, Command)>,
    /// The status bar rect from the last frame (mouse hit-testing).
    pub(crate) status_rect: Rect,
    /// Clickable status-bar segments `(start, end, command)` from the last frame.
    pub(crate) status_hits: Vec<(u16, u16, Command)>,
    /// The active code tab's editor content area from the last frame.
    pub(crate) editor_rect: Rect,
    /// The active code tab's in-editor Markdown preview area from the last frame.
    pub(crate) markdown_preview_rect: Rect,
    /// Visible committed-attribution text from the last frame, for click routing.
    pub(crate) blame_rect: Option<Rect>,
    /// Visible Markdown link runs from the focused preview's last frame.
    pub(crate) markdown_link_hits: Vec<MarkdownLinkHit>,
    /// Current mouse position when it rests over a visible Markdown link.
    pub(crate) markdown_link_hover: Option<(u16, u16)>,
    /// The focused commit view's signature-badge rect (screen coords) from the last
    /// frame, for double-click hit-testing. `None` when no badge is on screen.
    pub(crate) commit_badge_rect: Option<Rect>,
    /// Whether a mouse text-selection drag is in progress in the editor.
    pub(crate) editor_selecting: bool,
    /// The last left-click `(time, column, row)`, for multi-click detection.
    last_click: Option<(Instant, u16, u16)>,
    /// The current multi-click streak (1 = single, 2 = double, 3 = triple).
    click_streak: u8,
    /// The system clipboard (OSC 52).
    clipboard: Clipboard,
    /// The active Kitty image placement rect (set by the renderer), if any.
    pub(crate) image_area: Option<Rect>,
    /// The tab index whose image is currently transmitted to the terminal.
    shown_image: Option<ViewId>,
    /// The document page currently transmitted, so paging a PDF re-transmits even
    /// though the view (and thus [`shown_image`](Self::shown_image)) is unchanged.
    shown_page: usize,
    /// The graphical caret placement currently transmitted to the terminal.
    shown_graphics_caret: Option<GraphicsCaret>,
    /// Start of the current graphical-caret blink phase.
    graphics_caret_blink_epoch: Instant,
    /// Whether the app should quit.
    should_quit: bool,
    /// The headless editor backend; edits route through it. `None` in unit tests,
    /// where editing commands are inert.
    backend: Option<Arc<dyn Backend>>,
    /// Open requests awaiting their `Opened` event, mapping request id → file path.
    pending_open: HashMap<RequestId, PathBuf>,
    /// In-flight save requests, mapping request id → document, so the tab's saving
    /// spinner clears when the answering event (saved or error) arrives.
    pending_saves: HashMap<RequestId, DocumentId>,
    /// The in-flight completion request, if any (see [`crate::completion`]).
    pub(crate) pending_completion: Option<crate::completion::PendingCompletion>,
    /// The open completion popup, if any.
    pub(crate) completion: Option<crate::completion::CompletionUi>,
    /// The reusable fuzzy matcher backing the completion popup's filtering.
    pub(crate) completion_matcher: karet_fuzzy::Matcher,
    /// In-flight commit-detail requests, mapping request id → where its result goes
    /// (a new standalone commit tab, or the graph browser's detail pane).
    pending_commit_detail: HashMap<RequestId, CommitDest>,
    /// The graph browser's in-flight history-page request, so its answering
    /// [`SessionEvent::VcsLog`] fills the browser rather than the sidebar log.
    graph_log_req: Option<(RequestId, ViewId)>,
    /// Requests cancelled because their owning view closed. Late queued events
    /// bearing these ids are ignored and cannot resurrect UI.
    cancelled_requests: HashSet<RequestId>,
    /// Session documents the app has opened, so closing the last tab for a document
    /// can release it (the session ref-counts; the app must balance opens/closes).
    open_docs: HashSet<DocumentId>,
    /// Allocator for per-tab [`ViewId`]s. A view is a window onto a document; this
    /// is the seam future tiled/split panes build on — multiple views can share one
    /// document, whose edit log already lives once in the session.
    next_view: u64,
}

pub(crate) fn resolve_folds(folds: &FoldRegions, folded: &BTreeSet<u32>) -> Vec<Fold> {
    folds
        .regions()
        .iter()
        .map(|r| Fold {
            start: r.start,
            end: r.end,
            collapsed: folded.contains(&r.start),
        })
        .collect()
}

/// Whether the screen point `(x, y)` lies inside `r`.
fn rect_contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// The unsaved-changes confirmation prompt for `request`, naming the scope and its
/// `count` at-risk files. The default (any other key) is always to abort.
fn close_prompt_message(request: CloseRequest, count: usize) -> String {
    let files = if count == 1 { "file" } else { "files" };
    if matches!(request, CloseRequest::Quit) {
        format!(
            "{count} unsaved {files} — press s to save all & quit, d to discard & quit, \
             any other key to cancel"
        )
    } else {
        format!(
            "{count} unsaved {files} — press s to save & close, d to discard & close, \
             any other key to cancel"
        )
    }
}

/// Whether screen row `y` lies within `r`'s vertical span (column ignored).
fn row_in_rect(r: Rect, y: u16) -> bool {
    r.height > 0 && y >= r.y && y < r.bottom()
}

/// The tab at column `x` among `hits`, and whether `x` is on its close glyph.
fn tab_at(hits: &[TabHit], x: u16) -> Option<(usize, bool)> {
    hits.iter()
        .enumerate()
        .find_map(|(i, h)| (x >= h.start && x < h.end).then_some((i, x == h.close)))
}

/// A non-empty language selector for resolving editor configuration.
pub(crate) fn tab_language(tab: &Tab) -> Option<&str> {
    let language = tab.language();
    (!language.is_empty()).then_some(language)
}

/// Resolve a code tab's long-line behavior from its configured override or file type.
pub(crate) fn effective_word_wrap(tab: &Tab, override_: Option<bool>) -> bool {
    override_.unwrap_or_else(|| {
        matches!(
            &tab.kind,
            TabKind::Code { path, .. }
                if file_type_for_path(path).wrap_mode() == WrapMode::Wrap
        )
    })
}

fn loading_delay_remaining(since: Instant, now: Instant) -> Option<Duration> {
    LOADING_REVEAL_DELAY.checked_sub(now.saturating_duration_since(since))
}

/// Recursively copy a file or directory tree.
fn copy_path_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Move a file or directory, falling back to copy-then-delete for cross-device moves.
fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            copy_path_recursive(from, to)?;
            let remove = if from.is_dir() {
                std::fs::remove_dir_all(from)
            } else {
                std::fs::remove_file(from)
            };
            remove.map_err(|_| rename_err)
        },
    }
}

/// Whether two paths resolve to the same filesystem location.
fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

fn path_under(root: &Path, path: &Path) -> bool {
    canonical(path).starts_with(canonical(root))
}

fn rebase_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    if !path_under(from, path) {
        return None;
    }
    let suffix = path.strip_prefix(from).ok()?;
    Some(to.join(suffix))
}

fn retarget_tab_path(tab: &mut Tab, path: &Path) {
    let target = match &mut tab.kind {
        TabKind::Code { path: p, .. }
        | TabKind::Hex { path: p, .. }
        | TabKind::Placeholder { path: p, .. } => Some(p),
        #[cfg(feature = "images")]
        TabKind::Image { path: p, .. } => Some(p),
        #[cfg(feature = "pdf")]
        TabKind::Document { path: p, .. } => Some(p),
        _ => None,
    };
    if let Some(p) = target {
        *p = path.to_path_buf();
        tab.is_symlink =
            std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink());
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            tab.title = name.to_string();
        }
    }
}

/// Whether `child` resolves to `parent` or a path below it.
fn path_contains_or_equals(parent: &Path, child: &Path) -> bool {
    canonical(child).starts_with(canonical(parent))
}

/// A destination path under `dir`, suffixing when the source name already exists.
fn unique_child_path(dir: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "item".to_string());
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }

    for n in 1usize.. {
        let candidate = dir.join(copy_name(source, &name, n));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("unbounded suffix search should always return");
}

/// Build `name copy.ext`, `name copy 2.ext`, or `dir copy` style conflict names.
fn copy_name(source: &Path, fallback: &str, n: usize) -> String {
    let suffix = if n == 1 {
        " copy".to_string()
    } else {
        format!(" copy {n}")
    };
    if source.is_dir() {
        return format!("{fallback}{suffix}");
    }
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    match source.extension().map(|ext| ext.to_string_lossy()) {
        Some(ext) if !ext.is_empty() => format!("{stem}{suffix}.{ext}"),
        _ => format!("{stem}{suffix}"),
    }
}

/// The canonical form of `path` for tab de-duplication. For a missing leaf, resolve
/// its nearest existing ancestor and append the unresolved suffix; this preserves
/// macOS `/var` → `/private/var` normalization before a new file is created.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    for ancestor in path.ancestors().skip(1) {
        let Ok(resolved) = std::fs::canonicalize(ancestor) else {
            continue;
        };
        let Ok(suffix) = path.strip_prefix(ancestor) else {
            continue;
        };
        return resolved.join(suffix);
    }
    path.to_path_buf()
}

/// The (anchor, head) span of the word under `pos`, or the single character there
/// when the cursor is not on a word character. Delegates to the widget's
/// [`karet_editor::word_bounds`] so double-click and word motions agree.
fn word_at(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    karet_editor::word_bounds(buffer, pos)
}

/// Parse a revision-range spec typed into the go-to-commit input into
/// `(base, head, merge_base)`, or `None` when it is a single revision.
///
/// A three-dot `a...b` selects the merge-base range; a two-dot `a..b` the raw tips. An
/// omitted side defaults to `HEAD` (matching git: `..b`, `a..`). Whitespace is trimmed.
fn parse_rev_range(input: &str) -> Option<(String, String, bool)> {
    // Three-dot first: "..." also contains "..".
    let (sep, merge_base) = if input.contains("...") {
        ("...", true)
    } else if input.contains("..") {
        ("..", false)
    } else {
        return None;
    };
    let (base, head) = input.split_once(sep)?;
    let side = |s: &str| {
        let s = s.trim();
        if s.is_empty() { "HEAD" } else { s }.to_string()
    };
    Some((side(base), side(head), merge_base))
}

/// The text within `range`, sliced from the tab's `source` using byte offsets
/// derived from `buffer`. Returns `None` if the range cannot be resolved.
fn selection_text(buffer: &TextBuffer, source: &str, range: Range) -> Option<String> {
    let start = buffer.line_col_to_byte(range.start).ok()?.0;
    let end = buffer.line_col_to_byte(range.end).ok()?.0;
    source.get(start..end).map(str::to_string)
}

/// The (anchor, head) span covering all of `line`.
fn line_span(buffer: &TextBuffer, line: u32) -> (LineCol, LineCol) {
    let len = buffer
        .line(line as usize)
        .map_or(0, |s| s.chars().count() as u32);
    (LineCol::new(line, 0), LineCol::new(line, len))
}

/// Pops the kitty keyboard-enhancement flags on drop, so they are cleared even if
/// the event loop panics (ratatui's panic hook restores the rest of the terminal).
struct KeyboardEnhancementGuard;

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Run the IDE shell: require the kitty keyboard protocol, set up the terminal,
/// loop until quit, then restore it.
///
/// karet targets modern terminals, so a terminal without kitty keyboard support is
/// a hard error rather than a degraded fallback.
/// Resolve a `workbench.colorTheme` setting to a [`Theme`]: the built-in `"dark"`
/// (also the empty string), or a path to a `.tmTheme` or VS Code `.json` theme file.
/// Returns a human-readable message on a read/parse failure so the caller can warn
/// and fall back to the default.
fn load_theme(name: &str) -> Result<Theme, String> {
    if name.is_empty() || name == "dark" {
        return Ok(Theme::dark());
    }
    let path = Path::new(name);
    let bytes = std::fs::read(path).map_err(|e| format!("theme `{name}`: {e}"))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "json" {
        let text = String::from_utf8(bytes).map_err(|e| format!("theme `{name}`: {e}"))?;
        Theme::load_vscode(&text).map_err(|e| format!("theme `{name}`: {e}"))
    } else {
        Theme::load_tmtheme(&bytes).map_err(|e| format!("theme `{name}`: {e}"))
    }
}

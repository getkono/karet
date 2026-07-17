//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Command`]s resolved from key events.

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
use karet_session::RangeSpec;
use karet_session::RequestId;
use karet_session::Session;
use karet_session::SessionConfig;
use karet_session::Settings;
use karet_session::SnapshotRx;
use karet_session::SwapInfo;
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
use crate::remote;
use crate::render::FileView;
use crate::render::Section;
use crate::tab::FindState;
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

/// Where a resolved commit detail should be shown.
#[derive(Clone)]
enum CommitDest {
    /// Fill the already-open standalone commit tab with this view id.
    Tab { view: ViewId },
    /// Fill the graph browser's detail pane if it still selects this hash.
    Browser { hash: String },
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
    /// The in-progress commit message while the Source-Control commit input is open.
    pub(crate) commit_input: Option<String>,
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
    /// The changes-region scroll offset (top region; wheel + selection-follow).
    pub(crate) scm_offset: usize,
    /// The changes-region viewport rect from the last frame (hit-testing/hover).
    pub(crate) scm_changes_rect: Rect,
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
    graph_log_req: Option<RequestId>,
    /// Session documents the app has opened, so closing the last tab for a document
    /// can release it (the session ref-counts; the app must balance opens/closes).
    open_docs: HashSet<DocumentId>,
    /// Allocator for per-tab [`ViewId`]s. A view is a window onto a document; this
    /// is the seam future tiled/split panes build on — multiple views can share one
    /// document, whose edit log already lives once in the session.
    next_view: u64,
}

impl App {
    /// Build the shell rooted at `root`, with the staged/working change groups for
    /// the Source-Control panel.
    #[must_use]
    pub fn new(
        root: PathBuf,
        staged: Vec<FileChange>,
        working: Vec<FileChange>,
        syntax: bool,
    ) -> Self {
        let staged_count = staged.len();
        let mut changes = staged;
        changes.extend(working);
        let graphics = image::detect_protocol();
        Self {
            root,
            settings: Settings::default(),
            loaded_config: LoadedConfig::default(),
            config_diagnostics: Vec::new(),
            theme: Theme::dark(),
            syntax,
            icon_style: IconStyle::default(),
            icon_override: None,
            graphics,
            kitty_graphics_supported: graphics == GraphicsProtocol::Kitty,
            kitty_keyboard_supported: false,
            pointer_shapes_supported: false,
            pointer_shape: None,
            focus: Focus::Sidebar,
            sidebar_panel: SidebarPanel::Explorer,
            sidebar_visible: true,
            explorer: FileTreeState::new(),
            explorer_clipboard: None,
            context_menu: None,
            scm: Scm {
                selection: ListSelection::new(changes.len()),
                changes,
                staged_count,
                log: Vec::new(),
                log_has_more: false,
                log_loading: false,
                log_loading_since: None,
            },
            tabs: vec![Tab::welcome()],
            active: 0,
            layout: PaneLayout::new(),
            stored: HashMap::new(),
            closed: Vec::new(),
            overlay: None,
            find_open: false,
            commit_input: None,
            rev_input: None,
            pending_discard: None,
            pending_explorer_delete: None,
            pending_close: None,
            saving_close: None,
            pending_swaps: None,
            pending: Vec::new(),
            search: SearchPanel::default(),
            status: None,
            notifications: NotificationCenter::default(),
            toast_hits: Vec::new(),
            sidebar_rect: Rect::default(),
            main_rect: Rect::default(),
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            sidebar_divider_x: 0,
            sidebar_resizing: false,
            diff_layout: ViewMode::Unified,
            pane_frames: Vec::new(),
            tab_drag: None,
            sidebar_content_rect: Rect::default(),
            hover: None,
            sidebar_header_hover: None,
            panel_hits: Vec::new(),
            outline_visible: false,
            outline_sel: ListSelection::new(0),
            outline_rect: Rect::default(),
            outline_content_rect: Rect::default(),
            outline_width: OUTLINE_WIDTH,
            outline_scroll: 0,
            header_action_hits: Vec::new(),
            scm_row_map: Vec::new(),
            scm_offset: 0,
            scm_changes_rect: Rect::default(),
            scm_total_rows: 0,
            scm_commits_offset: 0,
            scm_commits_rect: Rect::default(),
            scm_commits_total: 0,
            scm_more_row: None,
            scm_commits_h: DEFAULT_SCM_COMMITS_H,
            scm_divider_y: 0,
            scm_resizing: false,
            search_results_rect: Rect::default(),
            search_offset: 0,
            search_query_row: 0,
            search_replace_row: None,
            search_action_hits: Vec::new(),
            status_rect: Rect::default(),
            status_hits: Vec::new(),
            editor_rect: Rect::default(),
            commit_badge_rect: None,
            editor_selecting: false,
            last_click: None,
            click_streak: 0,
            clipboard: Clipboard::new(),
            image_area: None,
            shown_image: None,
            shown_page: 0,
            shown_graphics_caret: None,
            graphics_caret_blink_epoch: Instant::now(),
            should_quit: false,
            backend: None,
            pending_open: HashMap::new(),
            pending_saves: HashMap::new(),
            pending_completion: None,
            completion: None,
            completion_matcher: karet_fuzzy::Matcher::new(),
            pending_commit_detail: HashMap::new(),
            graph_log_req: None,
            open_docs: HashSet::new(),
            next_view: 1,
        }
    }

    /// Set the icon style (builder-style; chains off [`App::new`]).
    #[must_use]
    pub fn with_icons(mut self, style: IconStyle) -> Self {
        self.icon_style = style;
        self.icon_override = Some(style);
        self
    }

    /// Apply the loaded configuration to the UI shell (builder-style). Stores the
    /// settings (later handed to the session backend) and any load diagnostics (shown
    /// as startup notifications), and applies the `workbench.*` slice: colour theme,
    /// icon style, and the startup sidebar panel.
    #[must_use]
    pub fn with_settings(mut self, settings: Settings, diagnostics: Vec<ConfigDiagnostic>) -> Self {
        let mut loaded = LoadedConfig::from_settings(settings);
        loaded.diagnostics = diagnostics;
        self = self.with_loaded_config(loaded);
        self
    }

    /// Apply a loaded configuration report to the UI shell (builder-style).
    #[must_use]
    pub fn with_loaded_config(mut self, loaded: LoadedConfig) -> Self {
        self.apply_loaded_config(loaded, true);
        self
    }

    /// Apply a configuration snapshot. Live reload deliberately leaves the startup
    /// panel alone; it is a startup action rather than persistent UI state.
    fn apply_loaded_config(&mut self, loaded: LoadedConfig, apply_startup_panel: bool) {
        use karet_session::config::schema::IconStyleSetting;
        use karet_session::config::schema::StartupPanel;

        let settings = loaded.settings.clone();
        self.config_diagnostics = loaded.diagnostics.clone();
        self.loaded_config = loaded;

        // Theme: the built-in "dark", or a path to a .tmTheme / VS Code .json theme.
        match load_theme(&settings.workbench.color_theme) {
            Ok(theme) => self.theme = theme,
            Err(message) => self.config_diagnostics.push(ConfigDiagnostic {
                path: PathBuf::from(&settings.workbench.color_theme),
                message,
                severity: Severity::Warning,
            }),
        }

        if self.icon_override.is_none() {
            self.icon_style = match settings.workbench.icon_style {
                IconStyleSetting::NerdFont => IconStyle::NerdFont,
                IconStyleSetting::Unicode => IconStyle::Unicode,
                IconStyleSetting::Ascii => IconStyle::Ascii,
            };
        }

        if apply_startup_panel {
            match settings.workbench.startup_panel {
                StartupPanel::Explorer => {
                    self.sidebar_panel = SidebarPanel::Explorer;
                    self.sidebar_visible = true;
                },
                StartupPanel::Search => {
                    self.sidebar_panel = SidebarPanel::Search;
                    self.sidebar_visible = true;
                },
                StartupPanel::SourceControl => {
                    self.sidebar_panel = SidebarPanel::SourceControl;
                    self.sidebar_visible = true;
                },
                StartupPanel::None => self.sidebar_visible = false,
            }
        }

        self.settings = settings;
    }

    /// Open `path` as the initial tab at startup (used when `karet <file>` is run).
    pub fn open_initial(&mut self, path: &Path) {
        self.open_path(path);
    }

    /// Open `path` as a startup preview without stealing focus from the configured
    /// startup panel.
    pub fn open_initial_preview(&mut self, path: &Path) {
        self.open_path_preview(path, false);
    }

    /// Open `path` at startup and place the caret at 1-based `line`/`col` (from the
    /// `--goto` flag), then focus the editor. The file opens as a permanent tab (or
    /// re-focuses an already-open one); the target is converted to the editor's
    /// 0-based coordinates and clamped into the buffer. A non-text target (image,
    /// binary, …) simply opens with no caret to place.
    pub fn open_startup_goto(&mut self, path: &Path, line: u32, col: u32) {
        self.open_path(path);
        // `line`/`col` are 1-based with a minimum of 1; `saturating_sub` maps them to
        // the editor's 0-based coordinates, and `goto` clamps into the buffer.
        let pos = LineCol::new(line.saturating_sub(1), col.saturating_sub(1));
        let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        };
        if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
            tab.editor.goto(&buffer, pos);
        }
        self.focus = Focus::Editor;
    }

    /// Open `path` in a new right split at startup (from the `--split` flag): the
    /// file becomes the sole tab of a fresh pane to the right of the focused pane,
    /// which then takes focus, so repeated calls chain panes left-to-right. When the
    /// layout has no room for another pane (per `can_split` against
    /// [`Self::main_rect`], which the caller seeds with the terminal size before the
    /// first draw), the file opens as a tab in the current pane instead and a
    /// warning notification says so — automation gets its file either way.
    pub fn open_startup_split(&mut self, path: &Path) {
        let from = self.focus_pane();
        if !self.layout.can_split(from, SplitDir::Right, self.main_rect) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            self.notify(
                Severity::Warning,
                NotificationKind::System,
                format!("--split: no room for another pane; opened {name} in the current pane"),
            );
            self.open_path(path);
            return;
        }
        let mut tab = workspace::open_file(path);
        tab.view = self.alloc_view();
        self.stash_focused();
        let new_pane = self.layout.split(from, SplitDir::Right);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![tab],
                active: 0,
            },
        );
        // `split` already focuses the pane it created; make that explicit, then pull
        // the new pane's tabs live and register its document with the backend.
        self.layout.set_focus(new_pane);
        self.load_focused();
        self.focus = Focus::Editor;
        self.register_doc(self.active);
    }

    /// Open a diff of two arbitrary files as a startup tab (from the `--diff`
    /// flag): `old` renders as the "before" side and `new` as the "after",
    /// syntax-aware like any Source-Control diff. `old_text`/`new_text` carry each
    /// file's content, already read by the caller (which fails fast on an unreadable
    /// file), with `None` marking non-UTF-8 bytes — either side non-text flags the
    /// change binary, rendering the standard binary-change placeholder (matching the
    /// [`FileChange::is_binary`] contract that both texts are then empty).
    pub fn open_startup_diff(
        &mut self,
        old: &Path,
        new: &Path,
        old_text: Option<String>,
        new_text: Option<String>,
    ) {
        let is_binary = old_text.is_none() || new_text.is_none();
        let change = FileChange {
            path: new.to_path_buf(),
            // The "renamed from" marker only applies when the two sides differ.
            old_path: (old != new).then(|| old.to_path_buf()),
            status: StatusKind::Modified,
            is_binary,
            old: if is_binary {
                String::new()
            } else {
                old_text.unwrap_or_default()
            },
            new: if is_binary {
                String::new()
            } else {
                new_text.unwrap_or_default()
            },
        };
        let name = |p: &Path| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("diff")
                .to_string()
        };
        let (old_name, new_name) = (name(old), name(new));
        let title = if old_name == new_name {
            new_name
        } else {
            format!("{old_name} ↔ {new_name}")
        };
        let file = FileView::new(change, Section::Working, self.syntax);
        self.push_tab(Tab::new(
            title,
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        ));
    }

    /// Dispatch a palette command at startup (from the `--command` flag), after
    /// every other startup flag is applied. Runs through the same
    /// [`dispatch`](Self::dispatch) path a key binding or the palette uses, so CLI
    /// automation and interactive use cannot drift.
    pub fn apply_startup_command(&mut self, command: Command) {
        self.dispatch(command);
    }

    /// Apply the CLI's startup focus override after startup tabs are opened.
    pub fn apply_startup_focus(&mut self, focus: crate::cli::FocusChoice) {
        self.focus = match focus {
            crate::cli::FocusChoice::Sidebar if self.sidebar_visible => Focus::Sidebar,
            crate::cli::FocusChoice::Sidebar | crate::cli::FocusChoice::Editor => Focus::Editor,
        };
    }

    /// Whether the active tab is a diff (enables diff-specific keys).
    fn active_is_diff(&self) -> bool {
        self.tabs.get(self.active).is_some_and(Tab::is_diff)
    }

    /// The content kind of the active editor tab, mapping the shell's tab model
    /// down to the coarse [`EditorTab`] the keymap layers on. Read-only scrollable
    /// views ([`EditorTab::Pager`]) scroll on the arrows; a too-large placeholder
    /// gets its own "open anyway" layer; a diff its layout/next-change keys; every
    /// other tab is [`EditorTab::Plain`].
    fn active_editor_tab(&self) -> EditorTab {
        match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Diff { .. }) => EditorTab::Diff,
            Some(
                TabKind::CommitLoading { .. }
                | TabKind::Commit { .. }
                | TabKind::Compare { .. }
                | TabKind::Blame { .. }
                | TabKind::Graph { .. }
                | TabKind::LoadedConfig { .. }
                | TabKind::MarkdownPreview { .. }
                | TabKind::Hex { .. },
            ) => EditorTab::Pager,
            Some(TabKind::CommitGraph { .. }) => EditorTab::CommitGraph,
            Some(TabKind::Placeholder {
                kind: FileKind::TooLarge { .. },
                ..
            }) => EditorTab::Oversize,
            _ => EditorTab::Plain,
        }
    }

    /// The pane that currently holds keyboard focus — the single value that
    /// determines which keybinding layer is live.
    pub(crate) fn focus_target(&self) -> FocusTarget {
        FocusTarget::from(self.focus, self.sidebar_panel, self.active_editor_tab())
    }

    /// Whether the active frame should suppress the editor's cell caret because the
    /// app will draw the Kitty graphics caret after ratatui flushes the frame.
    pub(crate) fn graphical_cursor_enabled(&self) -> bool {
        let configured =
            self.tabs
                .get(self.active)
                .map_or(self.settings.editor.graphical_cursor, |tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .graphical_cursor()
                });
        match configured {
            Some(false) => false,
            Some(true) => self.graphical_cursor_compatible(),
            None => self.graphical_cursor_compatible(),
        }
    }

    fn graphical_cursor_compatible(&self) -> bool {
        self.kitty_keyboard_supported
            && self.kitty_graphics_supported
            && self.graphics == GraphicsProtocol::Kitty
    }

    /// Handle a key press: resolve it against the layered keymap for the current
    /// [input context](Self::input_context) and dispatch, or fall through to the
    /// active modal's text input when nothing is bound.
    fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        // Esc dismisses a showing notification first (VS Code-style), but only when no
        // modal already owns Esc — so overlay/find/commit cancels are untouched, and
        // base Esc behaves normally whenever no toast is visible.
        if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && !self.notifications.is_empty()
            && self.input_context().modal.is_none()
        {
            self.notifications.dismiss_latest();
            return;
        }
        let ctx = self.input_context();
        match ctx.modal {
            Some(modal) => match keymap::resolve(ctx, &[KeyChord::from_event(key)]) {
                Resolved::Command(command) => self.dispatch(command),
                Resolved::Pending | Resolved::None => self.modal_text(modal, key),
            },
            None => {
                // The completion popup is a light key layer over the editor:
                // it consumes only its navigation/accept/dismiss keys and lets
                // everything else (typing, movement) fall through.
                if self.completion_key(key) {
                    return;
                }
                self.resolve_key(key);
            },
        }
        // Any key may have moved the caret or switched tabs; a popup or pending
        // request whose anchor no longer holds is dismissed.
        self.reconcile_completion();
    }

    /// The current input context: the active modal (if any) over the focused pane.
    /// The precedence mirrors how the shell stacks these overlays. Also drives the
    /// context-aware status hints bar ([`crate::ui`]).
    pub(crate) fn input_context(&self) -> Context {
        let modal = if self.pending_swaps.is_some() {
            // A startup recovery decision blocks everything else until made.
            Some(Modal::SwapRecover)
        } else if self.pending_close.is_some() {
            Some(Modal::CloseConfirm)
        } else if self.overlay.is_some() {
            Some(Modal::Overlay)
        } else if self.commit_input.is_some() {
            Some(Modal::CommitInput)
        } else if self.rev_input.is_some() {
            Some(Modal::RevInput)
        } else if self.pending_discard.is_some() {
            Some(Modal::DiscardConfirm)
        } else if self.pending_explorer_delete.is_some() {
            Some(Modal::ExplorerDeleteConfirm)
        } else if self.context_menu.is_some() {
            Some(Modal::ContextMenu)
        } else if self.find_open {
            Some(Modal::Find)
        } else if self.explorer.is_editing() {
            Some(Modal::ExplorerEdit)
        } else if self.focus == Focus::Sidebar && self.sidebar_panel == SidebarPanel::Search {
            Some(if self.search.input {
                Modal::SearchInput
            } else {
                Modal::SearchList
            })
        } else {
            None
        };
        Context {
            modal,
            target: self.focus_target(),
        }
    }

    /// Resolve a focus-context key against the layered keymap, accumulating
    /// multi-key chord sequences. An unbound printable in the editor becomes text
    /// input; a broken sequence is dropped.
    fn resolve_key(&mut self, key: KeyEvent) {
        self.pending.push(KeyChord::from_event(key));
        let ctx = Context::focus(self.focus_target());
        match keymap::resolve(ctx, &self.pending) {
            Resolved::Command(command) => {
                self.pending.clear();
                self.dispatch(command);
            },
            Resolved::Pending => {
                // A prefix of a longer binding: keep waiting. The status bar reads
                // `self.pending` directly to surface the typed chord and its
                // available completions (see `crate::ui::draw_status`).
            },
            Resolved::None => {
                let mid_sequence = self.pending.len() > 1;
                self.pending.clear();
                if !mid_sequence
                    && self.focus == Focus::Editor
                    && self.active_code_doc().is_some()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && let KeyCode::Char(c) = key.code
                {
                    self.dispatch(Command::InsertChar(c));
                }
            },
        }
    }

    /// Feed a key with no modal binding to the active modal's text input — the
    /// documented fall-through. The results list captures no text (unbound keys do
    /// nothing); an unbound key at the discard prompt cancels it.
    fn modal_text(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Overlay => self.overlay_input(key),
            Modal::Find => self.find_input(key),
            Modal::CommitInput => self.commit_edit(key),
            Modal::RevInput => self.rev_edit(key),
            Modal::ExplorerEdit => self.explorer_edit(key),
            Modal::SearchInput => self.search_edit(key),
            Modal::SearchList => {},
            Modal::DiscardConfirm => self.resolve_discard(false),
            Modal::ExplorerDeleteConfirm => self.resolve_explorer_delete(false),
            Modal::ContextMenu => self.close_context_menu(),
            // An unbound key cancels the close prompt (stay in the editor); the
            // default for every irreversible close is to abort.
            Modal::CloseConfirm => self.cancel_close(),
            // …and dismisses the recovery prompt, keeping the swaps for a later launch.
            Modal::SwapRecover => {
                self.pending_swaps = None;
                self.status = Some("recovery dismissed (backups kept)".to_string());
            },
        }
    }

    /// Feed pasted text to the active modal's text field, mirroring `modal_text`
    /// for keys. Without this, paste always landed in the main editor buffer
    /// regardless of which text field was actually focused — corrupting the
    /// editor's selection with clipboard text meant for Find/Search/Commit/the
    /// explorer rename box/the quick-open query. A no-op for non-text modals.
    fn modal_paste(&mut self, modal: Modal, text: &str) {
        match modal {
            Modal::Overlay => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.push_str(text);
                }
            },
            Modal::Find => {
                let Some(find) = self.active_find_mut() else {
                    return;
                };
                let editing_query = find.field == SearchField::Find;
                let target = if editing_query {
                    &mut find.query
                } else {
                    &mut find.replace
                };
                target.push_str(text);
                if editing_query {
                    self.run_find();
                }
            },
            Modal::CommitInput => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.push_str(text);
                }
            },
            Modal::RevInput => {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.push_str(text);
                }
            },
            Modal::ExplorerEdit => self.explorer.edit_paste(text),
            Modal::SearchInput => {
                let target = match self.search.field {
                    SearchField::Find => &mut self.search.query,
                    SearchField::Replace => &mut self.search.replace,
                };
                target.push_str(text);
            },
            Modal::SearchList
            | Modal::DiscardConfirm
            | Modal::ExplorerDeleteConfirm
            | Modal::ContextMenu
            | Modal::CloseConfirm
            | Modal::SwapRecover => {},
        }
    }

    /// Feed a key to the explorer inline name editor: printable characters extend the
    /// name, Backspace trims it (Enter/Esc are handled as bound commands).
    fn explorer_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => self.explorer.edit_backspace(),
            KeyCode::Delete => self.explorer.edit_delete(),
            KeyCode::Left => self.explorer.edit_left(),
            KeyCode::Right => self.explorer.edit_right(),
            KeyCode::Home => self.explorer.edit_home(),
            KeyCode::End => self.explorer.edit_end(),
            KeyCode::Char('a') | KeyCode::Char('A')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.explorer.edit_select_all();
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.explorer.edit_push(c);
            },
            _ => {},
        }
    }

    /// Accept the highlighted overlay row (open a file / run a command), then close.
    fn overlay_accept(&mut self) {
        let event = match self.overlay.as_ref() {
            Some(overlay) => overlay.accept(),
            None => return,
        };
        self.overlay = None;
        match event {
            OverlayEvent::Close => {},
            OverlayEvent::AcceptFile(path) => self.open_path(&path),
            OverlayEvent::AcceptCommand(cmd) => self.dispatch(cmd),
            OverlayEvent::AcceptDiffTarget { rev, label } => {
                self.open_changes_with(&rev, &label);
            },
        }
    }

    /// Edit the overlay query with an unbound key (backspace / printable).
    fn overlay_input(&mut self, key: KeyEvent) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Backspace => overlay.pop_char(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                overlay.push_char(c);
            },
            _ => {},
        }
    }

    /// Open the quick-open (go-to-file) overlay.
    fn open_quick_open(&mut self) {
        let files = workspace::list_files(&self.root, 2000);
        self.overlay = Some(Overlay::quick_open(files));
    }

    /// Open the find-in-file bar (only over a text/code tab). Restores this tab's
    /// last query/toggles if it has one (from a previous open-then-Esc on the same
    /// tab) instead of always starting blank.
    fn open_find(&mut self) {
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
    fn close_find(&mut self) {
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
    fn find_input(&mut self, key: KeyEvent) {
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
    fn run_find(&mut self) {
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
    fn find_step(&mut self, delta: i32) {
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
    fn find_submit(&mut self) {
        if self.active_find().map(|f| f.field) == Some(SearchField::Replace) {
            self.find_replace_current();
        } else {
            self.find_step(1);
        }
    }

    /// Replace the current in-file match with the replacement text. The edit is
    /// applied through the document (undoable); find re-runs when the snapshot lands.
    fn find_replace_current(&mut self) {
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
    fn find_replace_all(&mut self) {
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
    fn find_toggle_replace(&mut self) {
        if let Some(find) = self.active_find_mut() {
            find.replace_visible = !find.replace_visible;
            if !find.replace_visible {
                find.field = SearchField::Find;
            }
        }
    }

    /// Switch the edited find-bar field between find and replace.
    fn find_toggle_field(&mut self) {
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
    fn find_toggle_option(&mut self, option: SearchOption) {
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
    fn start_global_search(&mut self) {
        self.sidebar_panel = SidebarPanel::Search;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.search.input = true;
    }

    /// Edit the Search query with an unbound key (backspace / printable) while the
    /// `SearchInput` modal is active. Navigation and mode keys resolve via the
    /// keymap's `SearchInput` / `SearchList` layers instead.
    fn search_edit(&mut self, key: KeyEvent) {
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
    fn run_search_query(&mut self) {
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
    fn build_search_query(&self) -> SearchQuery {
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
    fn search_toggle_replace(&mut self) {
        self.search.replace_visible = !self.search.replace_visible;
        if !self.search.replace_visible {
            self.search.field = SearchField::Find;
        }
    }

    /// Switch the edited field between find and replace (revealing the replace field
    /// when moving to it), keeping the panel in input mode.
    fn search_toggle_field(&mut self) {
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
    fn search_replace_all(&mut self) {
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
    fn rerun_search(&mut self) {
        if !self.search.query.is_empty() {
            self.run_global_search();
        }
    }

    /// Toggle the regex option and refresh results.
    fn search_toggle_regex(&mut self) {
        self.search.regex = !self.search.regex;
        self.rerun_search();
    }

    /// Toggle case-sensitivity and refresh results.
    fn search_toggle_case(&mut self) {
        self.search.case_sensitive = !self.search.case_sensitive;
        self.rerun_search();
    }

    /// Toggle whole-word matching and refresh results.
    fn search_toggle_word(&mut self) {
        self.search.whole_word = !self.search.whole_word;
        self.rerun_search();
    }

    /// Run the workspace search for the current query, collecting up to the cap.
    fn run_global_search(&mut self) {
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
    fn refresh_search_decorations(&mut self) {
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
    fn search_select(&mut self, delta: i32) {
        let len = self.search.results.len();
        if len > 0 {
            let next = (self.search.selected as i64 + i64::from(delta)).clamp(0, len as i64 - 1);
            self.search.selected = next as usize;
        }
    }

    /// Open the selected result, scrolling to its first match.
    fn open_selected_result(&mut self) {
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

    /// Apply a resolved [`Command`].
    fn dispatch(&mut self, command: Command) {
        match command {
            Command::Quit => self.request_quit(),
            Command::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Command::ToggleFocus => self.toggle_focus(),
            Command::SelectPanel(panel) => {
                self.sidebar_panel = panel;
                self.sidebar_visible = true;
                self.focus = Focus::Sidebar;
                // Lazily fetch the first commit-log page when Source Control opens.
                if panel == SidebarPanel::SourceControl && self.scm.log.is_empty() {
                    self.request_scm_log(0);
                }
            },
            Command::OpenQuickOpen => self.open_quick_open(),
            Command::OpenCommandPalette => self.overlay = Some(Overlay::command_palette()),
            Command::OpenFind => self.open_find(),
            Command::OpenGlobalSearch => self.start_global_search(),
            Command::CloseTab => self.request_close_active_tab(),
            Command::NextTab => self.next_tab(),
            Command::PrevTab => self.prev_tab(),
            Command::MoveTabLeft => self.move_active_tab(-1),
            Command::MoveTabRight => self.move_active_tab(1),
            Command::GoToTab(n) => self.go_to_tab(n),
            Command::CloseOtherTabs => self.guarded_close(CloseRequest::OtherTabs),
            Command::CloseTabsToRight => self.guarded_close(CloseRequest::TabsToRight),
            Command::CloseAllTabs => self.guarded_close(CloseRequest::AllTabs),
            Command::ReopenClosedTab => self.reopen_closed_tab(),
            Command::OpenAnyway => self.open_active_anyway(),
            Command::DismissNotification => self.notifications.dismiss_latest(),
            Command::DismissAllNotifications => self.notifications.dismiss_all(),
            Command::MarkdownPreviewSide => self.open_markdown_preview_side(),
            Command::SplitRight => self.split_focused(SplitDir::Right),
            Command::SplitDown => self.split_focused(SplitDir::Down),
            Command::FocusNextPane => self.focus_pane_cycle(true),
            Command::FocusPrevPane => self.focus_pane_cycle(false),
            Command::Copy => self.copy_selection(),
            Command::CopyPath => self.copy_path(false),
            Command::CopyRelativePath => self.copy_path(true),
            Command::RevealActiveInExplorer => self.reveal_active_in_explorer(),
            Command::CopyRemoteFileUrl => self.copy_remote_link(remote::LinkKind::RemoteFile),
            Command::CopyGithubPermalink => {
                self.copy_remote_link(remote::LinkKind::GithubPermalink);
            },
            Command::CopyGithubHeadLink => {
                self.copy_remote_link(remote::LinkKind::GithubHeadLink);
            },
            Command::OpenChangesWithPrevious => self.open_changes_with("HEAD", "HEAD"),
            Command::OpenChangesWithRevision => self.open_changes_pick_revision(),
            Command::OpenChangesWithBranch => self.open_changes_pick_branch(),
            Command::SidebarUp => self.sidebar_step(-1),
            Command::SidebarDown => self.sidebar_step(1),
            Command::SidebarActivate => self.sidebar_activate(),
            Command::SidebarCollapse => self.sidebar_collapse(),
            Command::SidebarToggleExpand => self.sidebar_toggle_expand(),
            Command::ToggleOutline => self.toggle_outline(),
            Command::OutlineUp => self.outline_step(-1),
            Command::OutlineDown => self.outline_step(1),
            Command::OutlineActivate => self.outline_activate(),
            Command::OutlineCollapse => self.outline_collapse(),
            Command::CaretUp => self.caret_motion(false, EditorState::move_up),
            Command::CaretDown => self.caret_motion(false, EditorState::move_down),
            Command::CaretLeft => self.caret_motion(false, EditorState::move_left),
            Command::CaretRight => self.caret_motion(false, EditorState::move_right),
            Command::SelectUp => self.caret_motion(true, EditorState::move_up),
            Command::SelectDown => self.caret_motion(true, EditorState::move_down),
            Command::SelectLeft => self.caret_motion(true, EditorState::move_left),
            Command::SelectRight => self.caret_motion(true, EditorState::move_right),
            Command::CaretWordLeft => self.caret_motion(false, EditorState::move_word_left),
            Command::CaretWordRight => self.caret_motion(false, EditorState::move_word_right),
            Command::CaretLineStart => self.caret_motion(false, EditorState::move_line_start),
            Command::CaretLineEnd => self.caret_motion(false, EditorState::move_line_end),
            Command::CaretDocStart => self.caret_motion(false, EditorState::move_doc_start),
            Command::CaretDocEnd => self.caret_motion(false, EditorState::move_doc_end),
            Command::SelectWordLeft => self.caret_motion(true, EditorState::move_word_left),
            Command::SelectWordRight => self.caret_motion(true, EditorState::move_word_right),
            Command::SelectLineStart => self.caret_motion(true, EditorState::move_line_start),
            Command::SelectLineEnd => self.caret_motion(true, EditorState::move_line_end),
            Command::SelectDocStart => self.caret_motion(true, EditorState::move_doc_start),
            Command::SelectDocEnd => self.caret_motion(true, EditorState::move_doc_end),
            Command::SelectPageUp => self.caret_motion(true, EditorState::page_up),
            Command::SelectPageDown => self.caret_motion(true, EditorState::page_down),
            Command::EditorSelectAll => self.editor_select_all(),
            Command::AddCursorAbove => self.add_cursor_vertical(true),
            Command::AddCursorBelow => self.add_cursor_vertical(false),
            Command::AddCursorNextOccurrence => self.add_cursor_next_occurrence(),
            Command::CollapseCarets => self.collapse_carets_or_unfocus(),
            Command::ScrollUp => self.scroll_lines(-1),
            Command::ScrollDown => self.scroll_lines(1),
            Command::PageUp => self.scroll_lines(-i32::from(self.main_rect.height.max(1))),
            Command::PageDown => self.scroll_lines(i32::from(self.main_rect.height.max(1))),
            Command::Top => self.scroll_edge(true),
            Command::Bottom => self.scroll_edge(false),
            Command::ToggleDiffLayout => self.toggle_diff_layout(),
            Command::ToggleFold => self.toggle_fold(),
            Command::NextChangedFile => self.step_changed_file(1),
            Command::PrevChangedFile => self.step_changed_file(-1),
            Command::OpenDiffFile => self.open_diff_file(),
            Command::TriggerCompletion => self.trigger_completion(true),
            Command::InsertChar(c) => {
                let s = c.to_string();
                self.submit_edit_with_cause(EditCause::Type, move |caret, sel, _b, base| {
                    Some(editing::insert(caret, sel, base, &s))
                });
                self.maybe_auto_complete(c);
            },
            Command::InsertNewline => {
                self.submit_edit_with_cause(EditCause::Newline, |caret, sel, buf, base| {
                    Some(editing::newline(caret, sel, buf, base))
                });
            },
            Command::DeleteBackward => {
                self.submit_edit_with_cause(EditCause::Delete, editing::backspace)
            },
            Command::DeleteForward => {
                self.submit_edit_with_cause(EditCause::Delete, editing::delete_forward);
            },
            Command::Indent => {
                self.submit_edit(|caret, sel, _b, base| Some(editing::indent(caret, sel, base)));
            },
            Command::Dedent => {
                self.submit_edit(|caret, _sel, buf, base| editing::dedent(caret, buf, base));
            },
            Command::Undo => self.send_doc_command(|doc| SessionCommand::Undo { doc }),
            Command::Redo => self.send_doc_command(|doc| SessionCommand::Redo { doc }),
            Command::Save => self.save_active(),
            Command::Cut => self.cut(),
            Command::Paste => self.paste_from_clipboard(),
            Command::SelectExtendUp => self.sidebar_select_extend(-1),
            Command::SelectExtendDown => self.sidebar_select_extend(1),
            Command::SelectToggle => self.sidebar_select_toggle(),
            Command::SelectAll => self.sidebar_select_all(),
            Command::ScmStage => self.scm_send_paths(|paths| SessionCommand::Stage { paths }),
            Command::ScmUnstage => self.scm_send_paths(|paths| SessionCommand::Unstage { paths }),
            Command::ScmToggleStage => self.scm_toggle_stage(),
            Command::ScmStageAll => self.send_vcs(SessionCommand::StageAll),
            Command::ScmUnstageAll => self.send_vcs(SessionCommand::UnstageAll),
            Command::ScmDiscard => self.scm_arm_discard(),
            Command::ScmCommit => self.scm_open_commit_input(),
            Command::ScmRefresh => self.send_vcs(SessionCommand::RefreshVcs),
            Command::ShowBlame => self.open_blame(false),
            Command::BlameFunction => self.open_blame(true),
            Command::ShowLoadedConfig => {
                if self.backend.is_some() {
                    self.send_command(SessionCommand::LoadedConfig);
                } else {
                    self.open_loaded_config(self.loaded_config.clone());
                }
            },
            Command::ExplorerNewFile => self.explorer_begin_new(false),
            Command::ExplorerNewFolder => self.explorer_begin_new(true),
            Command::ExplorerRename => self.explorer_begin_rename(),
            Command::ExplorerRefresh => self.explorer_refresh(),
            Command::ExplorerCollapseAll => self.explorer.collapse_all(),
            Command::ExplorerCopy => self.explorer_copy_files(),
            Command::ExplorerCut => self.explorer_cut_files(),
            Command::ExplorerPaste => self.explorer_paste_files(),
            Command::ExplorerDuplicate => self.explorer_duplicate_files(),
            Command::ExplorerDelete => self.explorer_arm_delete(),
            Command::ExplorerCopyPath => self.explorer_copy_path(false),
            Command::ExplorerCopyRelativePath => self.explorer_copy_path(true),
            Command::ExplorerOpenContextMenu => self.open_context_menu_for_selection(),

            // Modal-scoped commands (resolved only while a modal context is active).
            Command::OverlayUp => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.select_up();
                }
            },
            Command::OverlayDown => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.select_down();
                }
            },
            Command::OverlayAccept => self.overlay_accept(),
            Command::OverlayCancel => self.overlay = None,
            Command::FindNext => self.find_step(1),
            Command::FindPrev => self.find_step(-1),
            Command::FindCancel => self.close_find(),
            Command::FindSubmit => self.find_submit(),
            Command::FindReplaceAll => self.find_replace_all(),
            Command::FindToggleReplace => self.find_toggle_replace(),
            Command::FindToggleField => self.find_toggle_field(),
            Command::FindToggleRegex => self.find_toggle_option(SearchOption::Regex),
            Command::FindToggleCase => self.find_toggle_option(SearchOption::Case),
            Command::FindToggleWord => self.find_toggle_option(SearchOption::Word),
            Command::CommitSubmit => self.commit_submit(),
            Command::CommitCancel => self.commit_cancel(),
            Command::CommitGenerate => self.commit_generate(),
            Command::ExplorerEditSubmit => self.explorer_commit_edit(),
            Command::ExplorerEditCancel => self.explorer.cancel_edit(),
            Command::ConfirmDiscard => self.resolve_discard(true),
            Command::ConfirmExplorerDelete => self.resolve_explorer_delete(true),
            Command::ContextMenuUp => self.context_menu_step(-1),
            Command::ContextMenuDown => self.context_menu_step(1),
            Command::ContextMenuAccept => self.accept_context_menu(),
            Command::ContextMenuCancel => self.close_context_menu(),
            Command::CloseConfirmSave => self.close_save(),
            Command::CloseConfirmDiscard => self.close_discard(),
            Command::RecoverSwaps => {
                // Open a tab for each backed-up file first (so the recovered content
                // has somewhere to land), then ask the backend to restore the buffers.
                if let Some(swaps) = self.pending_swaps.take() {
                    for info in &swaps {
                        self.open_path(&info.original);
                    }
                    self.status = Some(format!("recovering {} file(s)…", swaps.len()));
                }
                self.send_command(SessionCommand::RecoverSwaps);
            },
            Command::DiscardSwaps => {
                self.pending_swaps = None;
                self.send_command(SessionCommand::DiscardSwaps);
            },
            Command::ShowDependencyGraph => {
                self.status = Some("building dependency graph…".to_string());
                self.send_command(SessionCommand::DependencyGraph);
            },
            Command::ShowCommitGraph => self.open_commit_graph(),
            Command::CommitGraphNext => self.graph_select(1),
            Command::CommitGraphPrev => self.graph_select(-1),
            Command::CommitGraphOpen => self.graph_open_selected(),
            Command::OpenCommitByHash => self.open_rev_input(),
            Command::RevInputSubmit => self.rev_submit(),
            Command::RevInputCancel => self.rev_cancel(),
            Command::ShowFileHistory => self.open_file_history(),
            Command::DiffUnpushed => self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::Unpushed,
            }),
            Command::DiffSinceBase => self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::SinceBase { base: None },
            }),
            Command::CommitGraphMarkBase => self.graph_mark_base(),
            Command::CommitGraphCompare => self.graph_compare(),
            Command::SearchSelectUp => self.search_select(-1),
            Command::SearchSelectDown => self.search_select(1),
            Command::SearchOpen => self.open_selected_result(),
            Command::SearchBeginInput => self.search.input = true,
            Command::SearchQuit => self.should_quit = true,
            Command::SearchRun => self.run_search_query(),
            Command::SearchEndInput => self.search.input = false,
            Command::SearchToggleReplace => self.search_toggle_replace(),
            Command::SearchToggleField => self.search_toggle_field(),
            Command::SearchReplaceAll => self.search_replace_all(),
            Command::SearchToggleRegex => self.search_toggle_regex(),
            Command::SearchToggleCase => self.search_toggle_case(),
            Command::SearchToggleWord => self.search_toggle_word(),
        }
    }

    /// Move focus between the sidebar and the editor.
    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Editor,
            Focus::Editor => Focus::Sidebar,
            // Toggling out of the outline returns to the editor it annotates.
            Focus::Outline => Focus::Editor,
        };
    }

    /// The flattened outline rows for the active tab, or empty when it has none.
    pub(crate) fn active_outline_rows(&self) -> Vec<OutlineRow> {
        match self.tabs.get(self.active).map(|t| &t.kind) {
            #[cfg(feature = "pdf")]
            Some(TabKind::Document { outline, .. }) => {
                crate::outline::flatten(&crate::outline::from_pdf(outline))
            },
            _ => Vec::new(),
        }
    }

    /// Keep the outline row selection's length in step with the active tab's outline.
    fn sync_outline_selection(&mut self) {
        let n = self.active_outline_rows().len();
        self.outline_sel.set_len(n);
    }

    /// Show or hide the right-side outline panel. Showing it focuses the panel (so it
    /// is navigable at once); hiding it returns focus to the editor.
    fn toggle_outline(&mut self) {
        self.outline_visible = !self.outline_visible;
        if self.outline_visible {
            self.sync_outline_selection();
            // Focus the panel for immediate navigation, but only when it has content —
            // an empty "No outline" panel should not steal focus from the editor.
            if !self.active_outline_rows().is_empty() {
                self.focus = Focus::Outline;
            }
        } else if self.focus == Focus::Outline {
            self.focus = Focus::Editor;
        }
    }

    /// Move the outline selection by `delta` rows.
    fn outline_step(&mut self, delta: i32) {
        self.sync_outline_selection();
        self.outline_sel.move_by(delta);
    }

    /// Leave the outline panel, returning focus to the editor (the panel stays open).
    fn outline_collapse(&mut self) {
        self.focus = Focus::Editor;
    }

    /// Navigate to the selected outline entry: jump a document to its page, or move
    /// the editor caret to its position.
    fn outline_activate(&mut self) {
        let rows = self.active_outline_rows();
        let Some(target) = rows.get(self.outline_sel.cursor()).and_then(|r| r.target) else {
            return;
        };
        match target {
            OutlineTarget::Page(page) => self.set_document_page(page),
            OutlineTarget::Text(pos) => {
                // Clone the buffer (O(1) rope share) so the editor borrow is free of the
                // tab-kind borrow.
                let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
                    Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
                    _ => None,
                };
                if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
                    tab.editor.goto(&buffer, pos);
                }
            },
        }
    }

    /// Route a left-click in the outline panel: focus it, select the clicked row, and
    /// navigate to it. `outline_scroll` (recorded during draw) maps the screen row to
    /// the right entry even when the list is scrolled.
    fn handle_outline_click(&mut self, row_y: u16) {
        self.focus = Focus::Outline;
        let top = self.outline_content_rect.y;
        if row_y < top {
            return; // a click on the header just focuses the panel
        }
        let rows = self.active_outline_rows();
        self.outline_sel.set_len(rows.len());
        let idx = self.outline_scroll + usize::from(row_y - top);
        if idx >= rows.len() {
            return;
        }
        self.outline_sel.move_to(idx);
        self.outline_activate();
    }

    /// Set the active document tab's current page (clamped to the page range). A
    /// no-op without the `pdf` feature, where no document tab exists.
    fn set_document_page(&mut self, page: usize) {
        #[cfg(feature = "pdf")]
        if let Some(Tab {
            kind:
                TabKind::Document {
                    page: current,
                    page_count,
                    ..
                },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            *current = page.min(page_count.saturating_sub(1));
        }
        #[cfg(not(feature = "pdf"))]
        let _ = page;
    }

    /// Route a mouse-wheel notch over the sidebar: the Source-Control panel scrolls
    /// its list (so the commit log is reachable), while the explorer and search move
    /// their selection one step per notch.
    fn sidebar_wheel(&mut self, delta: i32, at_row: u16) {
        match self.sidebar_panel {
            // Route to whichever Source-Control region the pointer is over: the pinned
            // commit-log at the bottom, or the changes list above it.
            SidebarPanel::SourceControl => {
                if row_in_rect(self.scm_commits_rect, at_row) {
                    self.scm_scroll_commits(delta);
                } else {
                    self.scm_scroll_changes(delta);
                }
            },
            // The wheel only moves the selection — it must not open previews, or
            // scrolling past files would thrash the preview slot open with every
            // notch. Deliberate navigation (the arrow keys) goes through
            // [`sidebar_step`](Self::sidebar_step), which does preview.
            _ => self.sidebar_move(delta.signum()),
        }
    }

    /// Move the sidebar selection within the active panel, without opening
    /// anything (the wheel path; [`sidebar_step`](Self::sidebar_step) layers
    /// selection-follows-preview on top for keyboard navigation).
    fn sidebar_move(&mut self, delta: i32) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                if delta > 0 {
                    self.explorer.select_next();
                } else {
                    self.explorer.select_prev();
                }
            },
            // A plain move collapses any range or multi-selection; the viewport then
            // follows the change cursor so it stays visible.
            SidebarPanel::SourceControl => {
                self.scm.selection.move_by(delta);
                self.scm_follow_cursor();
            },
            SidebarPanel::Search => self.search_select(delta),
        }
    }

    /// Move the sidebar selection one step by keyboard (arrows / `j`/`k`), then
    /// follow it with a preview: the landed-on file or change opens in the pane's
    /// preview slot *without* stealing focus, so navigation keeps flowing and the
    /// panel's own keys (e.g. staging) stay live.
    fn sidebar_step(&mut self, delta: i32) {
        self.sidebar_move(delta);
        match self.sidebar_panel {
            // A directory row leaves the editor area untouched.
            SidebarPanel::Explorer => self.preview_selected_explorer_row(),
            SidebarPanel::SourceControl => self.preview_selected_diff(),
            SidebarPanel::Search => {},
        }
    }

    /// The display row of the Source-Control change cursor. Both section headers are
    /// always drawn, and an empty section reserves one placeholder line, so the
    /// staged block is `1` header + `max(staged, 1)` rows regardless of contents.
    fn scm_cursor_display_row(&self) -> usize {
        let i = self.scm.selection.cursor();
        let staged = self.scm.staged_count;
        if i < staged {
            // In the staged section: the "STAGED CHANGES" header sits above it.
            1 + i
        } else {
            // In the working section: the full staged block plus the "CHANGES" header.
            let staged_block = 1 + staged.max(1);
            staged_block + 1 + (i - staged)
        }
    }

    /// Scroll the changes region so the change cursor stays visible.
    fn scm_follow_cursor(&mut self) {
        let h = self.scm_changes_rect.height as usize;
        if h == 0 {
            return;
        }
        let row = self.scm_cursor_display_row();
        if row < self.scm_offset {
            self.scm_offset = row;
        } else if row >= self.scm_offset + h {
            self.scm_offset = row + 1 - h;
        }
    }

    /// Scroll the changes region by `delta` rows, clamped to its content.
    fn scm_scroll_changes(&mut self, delta: i32) {
        let max = self
            .scm_total_rows
            .saturating_sub(self.scm_changes_rect.height as usize);
        let next = (self.scm_offset as i64 + i64::from(delta)).clamp(0, max as i64);
        self.scm_offset = next as usize;
    }

    /// Scroll the pinned commit-log region by `delta` rows, clamped to its content,
    /// and lazily load more commits near the bottom.
    fn scm_scroll_commits(&mut self, delta: i32) {
        let max = self
            .scm_commits_total
            .saturating_sub(self.scm_commits_rect.height as usize);
        let next = (self.scm_commits_offset as i64 + i64::from(delta)).clamp(0, max as i64);
        self.scm_commits_offset = next as usize;
        self.maybe_autoload_commits();
    }

    /// Request the next commit page once the commit-log region nears the end of what
    /// is loaded.
    fn maybe_autoload_commits(&mut self) {
        if !self.scm.log_has_more || self.scm.log_loading {
            return;
        }
        let bottom = self.scm_commits_offset + self.scm_commits_rect.height as usize;
        if bottom + COMMIT_AUTOLOAD_THRESHOLD >= self.scm_commits_total {
            self.load_more_scm_log();
        }
    }

    /// Activate the selected sidebar row — the explicit Enter "commit into the
    /// view" action: expand a dir, or open the file/diff *materialized* (never a
    /// preview) with keyboard focus moving into it. An already-open view — even
    /// the preview slot — is re-focused and made permanent instead of duplicated.
    /// Browsing (arrow moves) previews without stealing focus; a single click
    /// previews with focus (see [`handle_sidebar_click`](Self::handle_sidebar_click)).
    fn sidebar_activate(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => self.sidebar_promote_or_open_permanent(),
            SidebarPanel::SourceControl => self.open_selected_diff(),
            SidebarPanel::Search => {},
        }
    }

    /// Open the explorer's selected file in the pane's preview slot with keyboard
    /// focus moving to the editor — the single-click action (VS Code parity: a
    /// click previews and focuses; Enter / double-click materializes). A directory
    /// row toggles its expansion.
    fn explorer_preview_with_focus(&mut self) {
        self.explorer.ensure_built(&self.root);
        if let Some(row) = self.explorer.selected() {
            let path = row.path.clone();
            if row.is_dir {
                self.explorer.toggle(&path);
            } else {
                self.open_path_preview(&path, true);
            }
        }
    }

    /// Open the explorer's selected row in the pane's preview slot without
    /// stealing keyboard focus (selection-follows-preview). A directory row (or an
    /// empty selection) changes nothing; a file already open is just shown. The
    /// sidebar keeps focus so the user can keep arrowing through the tree.
    fn preview_selected_explorer_row(&mut self) {
        let Some(row) = self.explorer.selected() else {
            return;
        };
        if row.is_dir {
            return;
        }
        let path = row.path.clone();
        self.open_path_preview(&path, false);
    }

    /// Enter or double-click on a file in the tree: promote it to a permanent tab
    /// instead of the single-click preview behavior. If it's already open (as the
    /// preview tab or otherwise), just clears its preview flag in place; if not
    /// yet open, opens it as a new permanent tab via [`open_path`](Self::open_path).
    fn sidebar_promote_or_open_permanent(&mut self) {
        if self.sidebar_panel != SidebarPanel::Explorer {
            return;
        }
        self.explorer.ensure_built(&self.root);
        let Some(row) = self.explorer.selected() else {
            return;
        };
        if row.is_dir {
            self.explorer.toggle(&row.path.clone());
            return;
        }
        let path = row.path.clone();
        let target = canonical(&path);
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.is_preview = false;
            }
            self.select_tab(idx);
        } else {
            self.open_path(&path);
        }
    }

    /// Collapse the selected directory (explorer only).
    fn sidebar_collapse(&mut self) {
        if self.sidebar_panel == SidebarPanel::Explorer
            && let Some(row) = self.explorer.selected()
            && row.is_dir
        {
            let path = row.path.clone();
            self.explorer.collapse(&path);
        }
    }

    /// Toggle expansion of the selected directory (explorer only).
    fn sidebar_toggle_expand(&mut self) {
        if self.sidebar_panel == SidebarPanel::Explorer {
            self.explorer.toggle_selected();
        }
    }

    /// Reveal `path` in the Explorer sidebar (VS Code's "Reveal in Explorer"): show
    /// the Explorer panel, expand every ancestor directory within the workspace root
    /// (and the target itself when it is a directory), select the target's row,
    /// scroll it into view (the tree clamps its offset to the cursor on the next
    /// render), and move keyboard focus to the sidebar.
    ///
    /// A no-op — save a short status note — when `path` lies outside the workspace
    /// root or no longer maps to a row in the tree.
    pub(crate) fn reveal_in_explorer(&mut self, path: &Path) {
        if !path_under(&self.root, path) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("path");
            self.status = Some(format!("reveal: {name} is outside the workspace"));
            return;
        }
        // The workspace root itself has no row (the tree lists its children): just
        // show and focus the Explorer without disturbing the selection.
        if same_path(path, &self.root) {
            let root = self.root.clone();
            self.explorer.ensure_built(&root);
            self.sidebar_panel = SidebarPanel::Explorer;
            self.sidebar_visible = true;
            self.focus = Focus::Sidebar;
            return;
        }
        // Expand every ancestor directory from the root down to the target, plus the
        // target when it is a directory. Inserting every ancestor covers directory
        // chain compaction: the chain's tip is always among them, so a single rebuild
        // unfolds the whole path.
        let root = self.root.clone();
        for anc in path.ancestors() {
            if anc == path {
                continue;
            }
            if !path_under(&root, anc) {
                break;
            }
            self.explorer.expand(anc);
        }
        if path.is_dir() {
            self.explorer.expand(path);
        }
        self.explorer.ensure_built(&root);
        let Some(idx) = self.explorer_row_index(path) else {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("path");
            self.status = Some(format!("reveal: {name} is not in the explorer"));
            return;
        };
        self.explorer.select_index(idx);
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
    }

    /// The explorer row index for `path`: an exact row match (files, plain
    /// directories, and directory-chain tips), else the shallowest compacted chain
    /// row whose tip lies within `path` (a directory folded into an `a/b` row).
    fn explorer_row_index(&self, path: &Path) -> Option<usize> {
        let rows = self.explorer.rows();
        if let Some(idx) = rows.iter().position(|row| row.path == path) {
            return Some(idx);
        }
        rows.iter()
            .enumerate()
            .filter(|(_, row)| row.is_dir && row.path.starts_with(path))
            .min_by_key(|(_, row)| row.path.components().count())
            .map(|(idx, _)| idx)
    }

    /// Begin creating a new file (or folder) in the explorer, ensuring the panel is
    /// visible and focused so its inline name editor is shown.
    fn explorer_begin_new(&mut self, folder: bool) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.explorer.ensure_built(&self.root);
        self.explorer.begin_new(folder);
    }

    /// Begin renaming the selected explorer entry (no-op unless the Explorer panel is
    /// the active sidebar panel).
    fn explorer_begin_rename(&mut self) {
        if self.sidebar_panel != SidebarPanel::Explorer {
            return;
        }
        self.explorer.ensure_built(&self.root);
        self.explorer.begin_rename();
    }

    /// Hard-reload the explorer tree and re-request VCS status — a bullet-proof
    /// refresh that drops every cached row and re-reads the filesystem.
    fn explorer_refresh(&mut self) {
        self.explorer.rebuild(&self.root);
        self.send_vcs(SessionCommand::RefreshVcs);
    }

    /// Apply the explorer inline edit: create the file/folder or rename on disk, then
    /// reload the tree (and open a newly-created file).
    fn explorer_commit_edit(&mut self) {
        let Some(pending) = self.explorer.take_edit() else {
            return;
        };
        match &pending {
            PendingEdit::Create { path, folder } => {
                let result = if *folder {
                    std::fs::create_dir_all(path)
                } else {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::File::create(path).map(|_| ())
                };
                match result {
                    Ok(()) => {
                        self.explorer.rebuild(&self.root);
                        self.send_vcs(SessionCommand::RefreshVcs);
                        if !*folder {
                            self.open_path(path);
                        }
                    },
                    Err(e) => {
                        self.explorer.restore_edit(&pending);
                        self.notify(
                            Severity::Error,
                            NotificationKind::Io,
                            format!("create failed: {e}"),
                        );
                    },
                }
            },
            PendingEdit::Rename { from, to } => match std::fs::rename(from, to) {
                Ok(()) => {
                    self.retarget_open_paths(from, to);
                    self.explorer.rebuild(&self.root);
                    self.send_vcs(SessionCommand::RefreshVcs);
                },
                Err(e) => {
                    self.explorer.restore_edit(&pending);
                    self.notify(
                        Severity::Error,
                        NotificationKind::Io,
                        format!("rename failed: {e}"),
                    );
                },
            },
        }
    }

    /// Copy the explorer's selected files/directories into the internal file
    /// clipboard.
    fn explorer_copy_files(&mut self) {
        self.explorer_store_files(ExplorerFileOp::Copy);
    }

    /// Cut the explorer's selected files/directories into the internal file
    /// clipboard.
    fn explorer_cut_files(&mut self) {
        self.explorer_store_files(ExplorerFileOp::Cut);
    }

    /// Store the current explorer selection as the source for a future paste.
    fn explorer_store_files(&mut self, op: ExplorerFileOp) {
        self.explorer.ensure_built(&self.root);
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.status = Some("explorer: select a file first".to_string());
            return;
        }
        let count = paths.len();
        self.explorer_clipboard = Some(ExplorerFileClipboard { op, paths });
        let verb = match op {
            ExplorerFileOp::Copy => "copied",
            ExplorerFileOp::Cut => "cut",
        };
        self.status = Some(format!("{verb} {count} explorer item(s)"));
    }

    /// Paste the internal explorer file clipboard into the selected destination.
    fn explorer_paste_files(&mut self) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            self.status = Some("paste: no explorer files".to_string());
            return;
        };
        let dest_dir = self.explorer_paste_destination();
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            self.notify(
                Severity::Error,
                NotificationKind::Io,
                format!("paste failed: {e}"),
            );
            return;
        }

        let mut pasted = 0usize;
        let mut skipped = 0usize;
        let mut failed = 0usize;
        let mut first_error: Option<String> = None;
        let mut moves = Vec::new();

        for source in &clipboard.paths {
            if !source.exists() {
                failed += 1;
                first_error.get_or_insert_with(|| {
                    format!("paste failed: {} no longer exists", source.display())
                });
                continue;
            }
            if clipboard.op == ExplorerFileOp::Cut
                && source
                    .parent()
                    .is_some_and(|parent| same_path(parent, &dest_dir))
            {
                skipped += 1;
                continue;
            }
            if source.is_dir() && path_contains_or_equals(source, &dest_dir) {
                failed += 1;
                first_error.get_or_insert_with(|| {
                    format!(
                        "paste failed: cannot paste {} into itself",
                        source.display()
                    )
                });
                continue;
            }

            let target = unique_child_path(&dest_dir, source);
            let result = match clipboard.op {
                ExplorerFileOp::Copy => copy_path_recursive(source, &target),
                ExplorerFileOp::Cut => move_path(source, &target),
            };
            match result {
                Ok(()) => {
                    pasted += 1;
                    if clipboard.op == ExplorerFileOp::Cut {
                        moves.push((source.clone(), target));
                    }
                },
                Err(e) => {
                    failed += 1;
                    first_error.get_or_insert_with(|| format!("paste failed: {e}"));
                },
            }
        }

        if pasted > 0 {
            for (from, to) in &moves {
                self.retarget_open_paths(from, to);
            }
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            if clipboard.op == ExplorerFileOp::Cut {
                self.explorer_clipboard = None;
            }
        }

        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
        }

        self.status = if pasted > 0 && failed > 0 {
            Some(format!("pasted {pasted} item(s), {failed} failed"))
        } else if pasted > 0 {
            Some(format!("pasted {pasted} item(s)"))
        } else if skipped > 0 && failed == 0 {
            Some("paste: already in target folder".to_string())
        } else {
            Some("paste failed".to_string())
        };
    }

    /// The explorer paste target: selected directory, selected file's parent, or root.
    fn explorer_paste_destination(&mut self) -> PathBuf {
        self.explorer.ensure_built(&self.root);
        match self.explorer.selected() {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// The explorer's selected paths after ensuring its row cache is current.
    fn explorer_selected_paths(&mut self) -> Vec<PathBuf> {
        self.explorer.ensure_built(&self.root);
        self.explorer.selected_paths()
    }

    /// The paths currently dimmed as cut in the explorer.
    pub(crate) fn explorer_cut_paths(&self) -> &[PathBuf] {
        self.explorer_clipboard
            .as_ref()
            .filter(|clipboard| clipboard.op == ExplorerFileOp::Cut)
            .map_or(&[], |clipboard| clipboard.paths.as_slice())
    }

    /// Duplicate the selected explorer item(s) beside themselves.
    fn explorer_duplicate_files(&mut self) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.status = Some("duplicate: select a file first".to_string());
            return;
        }
        let mut copied = 0usize;
        let mut first_error = None;
        for source in paths {
            let Some(parent) = source.parent() else {
                continue;
            };
            let target = unique_child_path(parent, &source);
            match copy_path_recursive(&source, &target) {
                Ok(()) => copied += 1,
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("duplicate failed: {e}"));
                },
            }
        }
        if copied > 0 {
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            self.status = Some(format!("duplicated {copied} item(s)"));
        }
        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
        }
    }

    /// Copy selected explorer paths to the system clipboard.
    fn explorer_copy_path(&mut self, relative: bool) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.status = Some("copy path: select a file first".to_string());
            return;
        }
        let text = paths
            .iter()
            .map(|path| {
                let display = if relative {
                    path.strip_prefix(&self.root).unwrap_or(path)
                } else {
                    path.as_path()
                };
                display.to_string_lossy().into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.copy_to_clipboard(text, "path");
    }

    /// Arm deletion of the selected explorer item(s).
    fn explorer_arm_delete(&mut self) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.status = Some("delete: select a file first".to_string());
            return;
        }
        if self.has_dirty_tabs_under(&paths) {
            self.notify(
                Severity::Warning,
                NotificationKind::Io,
                "delete blocked: save or close dirty files first",
            );
            return;
        }
        self.context_menu_clear();
        self.status = Some(format!(
            "delete {} item(s)? press y to confirm, any other key to cancel",
            paths.len()
        ));
        self.pending_explorer_delete = Some(paths);
    }

    /// Resolve a pending explorer delete confirmation.
    fn resolve_explorer_delete(&mut self, confirmed: bool) {
        let Some(paths) = self.pending_explorer_delete.take() else {
            return;
        };
        if !confirmed {
            self.status = Some("delete cancelled".to_string());
            return;
        }
        self.close_tabs_under(&paths);
        let mut deleted = 0usize;
        let mut first_error = None;
        for path in &paths {
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(()) => deleted += 1,
                Err(e) if !path.exists() => {
                    deleted += 1;
                    first_error.get_or_insert_with(|| format!("delete warning: {e}"));
                },
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("delete failed: {e}"));
                },
            }
        }
        if deleted > 0 {
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            self.status = Some(format!("deleted {deleted} item(s)"));
        }
        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
        }
    }

    fn row_context_items(&self) -> Vec<ContextMenuEntry> {
        [
            Command::SidebarActivate,
            Command::ExplorerRename,
            Command::ExplorerNewFile,
            Command::ExplorerNewFolder,
            Command::ExplorerCopy,
            Command::ExplorerCut,
            Command::ExplorerPaste,
            Command::ExplorerDuplicate,
            Command::ExplorerDelete,
            Command::ExplorerCopyPath,
            Command::ExplorerCopyRelativePath,
            Command::ExplorerRefresh,
        ]
        .into_iter()
        .map(ContextMenuEntry::enabled)
        .collect()
    }

    fn blank_context_items(&self) -> Vec<ContextMenuEntry> {
        [
            Command::ExplorerNewFile,
            Command::ExplorerNewFolder,
            Command::ExplorerPaste,
            Command::ExplorerRefresh,
            Command::ExplorerCollapseAll,
        ]
        .into_iter()
        .map(ContextMenuEntry::enabled)
        .collect()
    }

    fn context_menu_clear(&mut self) {
        self.context_menu = None;
    }

    fn open_context_menu(&mut self, x: u16, y: u16, row: Option<usize>) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.explorer.ensure_built(&self.root);
        let items = if let Some(row) = row {
            if !self.explorer.is_selected(row) {
                self.explorer.select_index(row);
            }
            self.row_context_items()
        } else {
            self.blank_context_items()
        };
        self.context_menu = Some(ContextMenu::new(x, y, items));
    }

    fn open_context_menu_for_selection(&mut self) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.explorer.ensure_built(&self.root);
        let cursor = self.explorer.cursor();
        let y = self.sidebar_content_rect.y.saturating_add(
            cursor
                .saturating_sub(self.explorer.offset())
                .try_into()
                .unwrap_or(0),
        );
        let x = self.sidebar_content_rect.x.saturating_add(2);
        let row = (!self.explorer.rows().is_empty()).then_some(cursor);
        self.open_context_menu(x, y, row);
    }

    /// Open the pane context menu at `(x, y)` for the focused pane's active tab.
    /// Only file-backed tabs get one; a pathless tab (Welcome, commit graph, …)
    /// opens nothing.
    fn open_pane_context_menu(&mut self, x: u16, y: u16) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            return;
        };
        let entries = self.pane_context_entries(&path);
        self.context_menu = Some(ContextMenu::new(x, y, entries));
    }

    /// The pane context menu's rows for the active file at `path`. The path items
    /// always work; the link items are enabled exactly when [`remote::link`] can
    /// build them (the same call their dispatch runs), with its refusal reason as
    /// the disabled note.
    fn pane_context_entries(&self, path: &Path) -> Vec<ContextMenuEntry> {
        let mut entries = vec![
            ContextMenuEntry::enabled(Command::CopyPath),
            ContextMenuEntry::enabled(Command::CopyRelativePath),
            ContextMenuEntry::enabled(Command::RevealActiveInExplorer),
        ];
        let facts = self.remote_facts(path);
        let link_entry = |command, kind| match &facts {
            Ok(facts) => match remote::link(&facts.link_target(), kind, None) {
                Ok(_) => ContextMenuEntry::enabled(command),
                Err(note) => ContextMenuEntry::disabled(command, note),
            },
            Err(note) => ContextMenuEntry::disabled(command, note.clone()),
        };
        entries.push(link_entry(
            Command::CopyRemoteFileUrl,
            remote::LinkKind::RemoteFile,
        ));
        // The Open Changes actions need a repository and a tracked file, but no
        // remote — their enablement is checked separately from the link rows.
        let changes_note = self.open_changes_note(path);
        for command in [
            Command::OpenChangesWithPrevious,
            Command::OpenChangesWithRevision,
            Command::OpenChangesWithBranch,
        ] {
            entries.push(match &changes_note {
                None => ContextMenuEntry::enabled(command),
                Some(note) => ContextMenuEntry::disabled(command, note.clone()),
            });
        }
        entries.push(link_entry(
            Command::CopyGithubPermalink,
            remote::LinkKind::GithubPermalink,
        ));
        entries.push(link_entry(
            Command::CopyGithubHeadLink,
            remote::LinkKind::GithubHeadLink,
        ));
        entries
    }

    fn context_menu_step(&mut self, delta: i32) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.select_by(delta);
        }
    }

    fn accept_context_menu(&mut self) {
        let Some(entry) = self
            .context_menu
            .as_ref()
            .and_then(ContextMenu::selected_entry)
        else {
            self.context_menu = None;
            return;
        };
        if !entry.enabled {
            // Refuse a disabled row: surface its explanatory note (when it has one)
            // and keep the menu open so another row can be chosen.
            if let Some(note) = entry.note.clone() {
                self.status = Some(note);
            }
            return;
        }
        let command = entry.command;
        self.context_menu = None;
        self.dispatch(command);
    }

    fn close_context_menu(&mut self) {
        self.context_menu_clear();
    }

    /// Open the Source-Control cursor's change as a materialized (permanent) diff
    /// view and move keyboard focus into it — the explicit Enter / double-click
    /// "take me into the view" action. Browsing (arrow moves, single click) goes
    /// through [`preview_selected_diff`](Self::preview_selected_diff) instead,
    /// which keeps focus on the panel so the staging keys stay live.
    fn open_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        // Never duplicate: an existing diff tab for the same change — the preview
        // slot or a permanent one — is materialized and focused instead.
        if let Some(idx) = self.find_diff_tab(&change.path, section) {
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.is_preview = false;
            }
            self.select_tab(idx);
            return;
        }
        let tab = self.build_diff_tab(change, section);
        self.push_tab(tab);
    }

    /// Show the Source-Control cursor's change in the pane's shared preview slot
    /// *without* stealing keyboard focus (selection-follows-preview): browsing the
    /// change list with the arrows (or a single click) shows each diff while the
    /// panel keeps focus, so stage/unstage/discard/commit and the selection keys
    /// keep working. An existing diff tab for the same change is just shown;
    /// otherwise the preview slot is replaced in place — never one new tab per
    /// visited change.
    fn preview_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        if let Some(idx) = self.find_diff_tab(&change.path, section) {
            self.active = idx;
            self.find_open = false;
            return;
        }
        let mut tab = self.build_diff_tab(change, section);
        tab.is_preview = true;
        self.install_preview_tab(tab, false);
    }

    /// The index of this pane's existing diff tab for `path` in `section`, if any
    /// (preview or permanent) — the dedup lookup for the Source-Control open paths.
    fn find_diff_tab(&self, path: &Path, section: Section) -> Option<usize> {
        self.tabs.iter().position(|t| {
            matches!(&t.kind, TabKind::Diff { file, .. }
                if file.change.path == *path && file.section == section)
        })
    }

    /// Diff and highlight `change` into a fresh [`TabKind::Diff`] tab using the
    /// remembered layout. The caller decides how the tab enters the pane (preview
    /// slot vs permanent) and where focus lands.
    fn build_diff_tab(&self, change: FileChange, section: Section) -> Tab {
        let title = change
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diff")
            .to_string();
        let file = FileView::new(change, section, self.syntax);
        Tab::new(
            title,
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        )
    }

    // --- source control ---------------------------------------------------

    /// Request one page of the commit log starting at `skip`, unless one is already
    /// in flight. The result arrives as [`SessionEvent::VcsLog`].
    fn request_scm_log(&mut self, skip: usize) {
        if self.scm.log_loading {
            return;
        }
        self.scm.log_loading = true;
        self.scm.log_loading_since = Some(Instant::now());
        self.send_vcs(SessionCommand::VcsLog {
            skip,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Fetch the next page of the commit log (from the end of what is loaded).
    fn load_more_scm_log(&mut self) {
        if self.scm.log_has_more {
            let skip = self.scm.log.len();
            self.request_scm_log(skip);
        }
    }

    /// Open the commit view for `rev` (a hash or ref) immediately, fill metadata when
    /// [`SessionEvent::CommitDetailReady`] arrives, then fill changed files when
    /// [`SessionEvent::CommitReady`] arrives.
    fn open_commit(&mut self, rev: String) {
        self.push_tab(Tab::commit_loading(rev.clone()));
        let view = self.tabs[self.active].view;
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev }) {
            self.pending_commit_detail
                .insert(id, CommitDest::Tab { view });
        }
    }

    /// Open the full-screen commit graph browser and request its first history page.
    fn open_commit_graph(&mut self) {
        self.push_tab(Tab::commit_graph(None, "Commits"));
        self.graph_log_req = self.send_command_id(SessionCommand::VcsLog {
            skip: 0,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Open the graph browser scoped to the active file's history (`git log -- file`).
    fn open_file_history(&mut self) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("file history: open a file first".to_string());
            return;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("history")
            .to_string();
        self.push_tab(Tab::commit_graph(Some(path.clone()), format!("⌥ {name}")));
        self.graph_log_req = self.send_command_id(SessionCommand::FileHistory {
            path,
            skip: 0,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Open the go-to-commit input; the typed revision resolves via [`open_commit`].
    fn open_rev_input(&mut self) {
        self.rev_input = Some(String::new());
    }

    /// Cancel the go-to-commit input.
    fn rev_cancel(&mut self) {
        self.rev_input = None;
        self.status = Some("go to commit cancelled".to_string());
    }

    /// Submit the typed revision: open a range when it contains `..`/`...`, otherwise the
    /// single commit; re-prompt when empty.
    fn rev_submit(&mut self) {
        let rev = self.rev_input.take().unwrap_or_default().trim().to_string();
        if rev.is_empty() {
            self.rev_input = Some(String::new());
            self.status =
                Some("go to commit: enter a hash, ref, or range (a..b, a...b)".to_string());
        } else if let Some((base, head, merge_base)) = parse_rev_range(&rev) {
            self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::Between {
                    base,
                    head,
                    merge_base,
                },
            });
        } else {
            self.open_commit(rev);
        }
    }

    /// Edit the go-to-commit revision with an unbound key (backspace / printable).
    fn rev_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.pop();
                }
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.push(c);
                }
            },
            _ => {},
        }
    }

    /// The active tab's commit graph browser, if it is one.
    fn active_commit_graph(&mut self) -> Option<&mut TabKind> {
        let tab = self.tabs.get_mut(self.active)?;
        matches!(tab.kind, TabKind::CommitGraph { .. }).then_some(&mut tab.kind)
    }

    /// Move the browser's selection by `delta` (clamped), and request the newly
    /// selected commit's detail if it isn't already shown.
    fn graph_select(&mut self, delta: i32) {
        let Some(TabKind::CommitGraph {
            history_path,
            commits,
            selected,
            has_more,
            loading,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        if commits.is_empty() {
            return;
        }
        let last = commits.len() - 1;
        let next = (*selected as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
        *selected = next;
        // Page in more history when nearing the end, from the same source (whole-repo
        // log or a single file's history).
        let near_end = next + COMMIT_AUTOLOAD_THRESHOLD >= commits.len();
        let want_more = *has_more && !*loading && near_end;
        let loaded = commits.len();
        let path = history_path.clone();
        let hash = commits[next].hash.clone();
        self.graph_request_detail(hash);
        if want_more {
            if let Some(TabKind::CommitGraph {
                loading,
                loading_since,
                ..
            }) = self.active_commit_graph()
            {
                *loading = true;
                *loading_since = Some(Instant::now());
            }
            let command = match path {
                Some(path) => SessionCommand::FileHistory {
                    path,
                    skip: loaded,
                    limit: SCM_LOG_PAGE,
                },
                None => SessionCommand::VcsLog {
                    skip: loaded,
                    limit: SCM_LOG_PAGE,
                },
            };
            self.graph_log_req = self.send_command_id(command);
        }
    }

    /// Request `hash`'s detail for the browser pane, unless it is already the shown
    /// detail (avoids re-fetching when re-selecting the same commit).
    fn graph_request_detail(&mut self, hash: String) {
        if let Some(TabKind::CommitGraph { detail, .. }) = self.active_commit_graph()
            && detail.as_ref().is_some_and(|d| d.hash == hash)
        {
            return;
        }
        if let Some(TabKind::CommitGraph {
            detail,
            files,
            files_loading_since,
            files_error,
            verification,
            detail_loading_since,
            ..
        }) = self.active_commit_graph()
        {
            *detail = None;
            files.clear();
            *files_loading_since = None;
            *files_error = None;
            *verification = None;
            *detail_loading_since = Some(Instant::now());
        }
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev: hash.clone() }) {
            self.pending_commit_detail
                .insert(id, CommitDest::Browser { hash });
        }
    }

    /// Open the browser's selected commit as a standalone commit tab.
    fn graph_open_selected(&mut self) {
        if let Some(TabKind::CommitGraph {
            commits, selected, ..
        }) = self.active_commit_graph()
            && let Some(commit) = commits.get(*selected)
        {
            let hash = commit.hash.clone();
            self.open_commit(hash);
        }
    }

    /// Request a range diff; the answering [`SessionEvent::RangeReady`] opens the compare
    /// tab, and an unresolvable range answers with a VCS notification instead.
    fn open_range(&mut self, command: SessionCommand) {
        self.status = Some("computing diff…".to_string());
        self.send_vcs(command);
    }

    /// Mark the browser's selected commit as the base for a two-commit comparison.
    fn graph_mark_base(&mut self) {
        if let Some(TabKind::CommitGraph {
            commits,
            selected,
            compare_base,
            ..
        }) = self.active_commit_graph()
            && let Some(commit) = commits.get(*selected)
        {
            let short = commit.short_hash.clone();
            *compare_base = Some(commit.hash.clone());
            self.status = Some(format!(
                "compare base marked: {short} (select another, then compare)"
            ));
        }
    }

    /// Compare the browser's marked base commit against the current selection (a two-dot
    /// `base..selected` diff). Reports a status when no base has been marked yet.
    fn graph_compare(&mut self) {
        let Some(TabKind::CommitGraph {
            commits,
            selected,
            compare_base,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        let Some(base) = compare_base.clone() else {
            self.status =
                Some("mark a compare base first (Commit Graph: Mark Compare Base)".to_string());
            return;
        };
        let Some(head) = commits.get(*selected).map(|c| c.hash.clone()) else {
            return;
        };
        self.open_range(SessionCommand::RangeChanges {
            spec: RangeSpec::Between {
                base,
                head,
                merge_base: false,
            },
        });
    }

    /// Fill the graph browser's metadata pane from a resolved commit, and fire the lazy
    /// GitHub verification fetch. A no-op if no browser is open.
    fn fill_graph_metadata(&mut self, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::CommitGraph {
                commits,
                selected,
                detail: slot,
                files,
                files_loading_since,
                files_error,
                verification,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                *slot = Some(detail.clone());
                files.clear();
                *files_loading_since = Some(Instant::now());
                *files_error = None;
                *verification = None;
                *detail_loading_since = None;
                filled = true;
            }
        }
        if filled {
            self.send_vcs(SessionCommand::FetchCommitVerification { hash });
        }
    }

    /// Fill the graph browser's detail pane from a resolved commit, and fire the lazy
    /// GitHub verification fetch. A no-op if no browser is open.
    fn fill_graph_detail(&mut self, detail: Box<CommitDetail>, changes: Vec<FileChange>) {
        let syntax = self.syntax;
        let hash = detail.hash.clone();
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::CommitGraph {
                commits,
                selected,
                detail: slot,
                files,
                files_loading_since,
                files_error,
                verification,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                let keep_verification = slot.as_ref().is_some_and(|d| d.hash == hash)
                    && verification.as_ref().is_some();
                *files = changes
                    .iter()
                    .cloned()
                    .map(|c| FileView::new(c, Section::Staged, syntax))
                    .collect();
                *slot = Some(detail.clone());
                *files_loading_since = None;
                *files_error = None;
                if !keep_verification {
                    *verification = None;
                }
                *detail_loading_since = None;
                filled = true;
            }
        }
        if filled {
            self.send_vcs(SessionCommand::FetchCommitVerification { hash });
        }
    }

    /// Apply a fetched history page to the graph browser: replace on the first page,
    /// append otherwise. On the first page, select the top commit and load its detail.
    fn apply_graph_log(&mut self, skip: usize, commits: Vec<Commit>, has_more: bool) {
        let mut first_hash = None;
        for tab in self.all_tabs_mut() {
            if let TabKind::CommitGraph {
                commits: loaded,
                has_more: more,
                loading,
                loading_since,
                selected,
                ..
            } = &mut tab.kind
            {
                *loading = false;
                *loading_since = None;
                *more = has_more;
                if skip == 0 {
                    *loaded = commits.clone();
                    *selected = 0;
                    first_hash = loaded.first().map(|c| c.hash.clone());
                } else if skip == loaded.len() {
                    loaded.extend(commits.clone());
                }
            }
        }
        if let Some(hash) = first_hash {
            self.graph_request_detail(hash);
        }
    }

    /// Build and open a commit tab from a resolved [`CommitDetail`] and its changes,
    /// then fire the lazy GitHub verification fetch to upgrade the signature badge.
    fn open_commit_tab(&mut self, detail: Box<CommitDetail>, changes: Vec<FileChange>) {
        let files = changes
            .into_iter()
            .map(|c| FileView::new(c, Section::Staged, self.syntax))
            .collect();
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, files));
        self.send_vcs(SessionCommand::FetchCommitVerification { hash });
    }

    /// Open a standalone commit tab with metadata visible while changed files are still
    /// loading. Used for unsolicited commit-detail events.
    fn open_commit_metadata_tab(&mut self, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, Vec::new()));
        if let TabKind::Commit {
            files_loading_since,
            ..
        } = &mut self.tabs[self.active].kind
        {
            *files_loading_since = Some(Instant::now());
        }
        self.send_vcs(SessionCommand::FetchCommitVerification { hash });
    }

    /// Fill an already-open pending commit tab with metadata, leaving its changed-file
    /// block in a progressive loading state.
    fn fill_commit_metadata(&mut self, view: ViewId, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        let title = commit_title(&detail.short_hash);
        let mut detail = Some(detail);
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            tab.title = title;
            if let Some(detail) = detail.take() {
                let scroll = match &tab.kind {
                    TabKind::CommitLoading { scroll, .. } | TabKind::Commit { scroll, .. } => {
                        *scroll
                    },
                    _ => 0,
                };
                tab.kind = TabKind::Commit {
                    detail,
                    files: Vec::new(),
                    files_loading_since: Some(Instant::now()),
                    files_error: None,
                    verification: None,
                    explain_since: None,
                    scroll,
                };
                filled = true;
            }
            break;
        }
        if filled {
            self.send_vcs(SessionCommand::FetchCommitVerification { hash });
        }
    }

    /// Fill an already-open pending commit tab. If the tab was closed before the
    /// request answered, the detail is discarded instead of surprising the user with
    /// a late tab.
    fn fill_commit_tab(
        &mut self,
        view: ViewId,
        detail: Box<CommitDetail>,
        changes: Vec<FileChange>,
    ) {
        let mut files = Some(
            changes
                .into_iter()
                .map(|c| FileView::new(c, Section::Staged, self.syntax))
                .collect::<Vec<_>>(),
        );
        let hash = detail.hash.clone();
        let title = commit_title(&detail.short_hash);
        let mut detail = Some(detail);
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view == view {
                tab.title = title;
                if let (Some(detail), Some(files)) = (detail.take(), files.take()) {
                    match &mut tab.kind {
                        TabKind::Commit {
                            detail: slot,
                            files: current_files,
                            files_loading_since,
                            files_error,
                            ..
                        } if slot.hash == hash => {
                            *slot = detail;
                            *current_files = files;
                            *files_loading_since = None;
                            *files_error = None;
                        },
                        _ => {
                            tab.kind = TabKind::Commit {
                                detail,
                                files,
                                files_loading_since: None,
                                files_error: None,
                                verification: None,
                                explain_since: None,
                                scroll: 0,
                            };
                        },
                    }
                    filled = true;
                }
                break;
            }
        }
        if filled {
            self.send_vcs(SessionCommand::FetchCommitVerification { hash });
        }
    }

    /// Mark a pending commit-detail request as failed and clear any visible loading
    /// placeholder tied to that request.
    fn fail_pending_commit_detail(&mut self, request: RequestId, message: &str) {
        let Some(dest) = self.pending_commit_detail.remove(&request) else {
            return;
        };
        match dest {
            CommitDest::Tab { view } => {
                for tab in self.all_tabs_mut() {
                    if tab.view != view {
                        continue;
                    }
                    match &mut tab.kind {
                        TabKind::CommitLoading { error, .. } => {
                            *error = Some(message.to_string());
                        },
                        TabKind::Commit {
                            files_loading_since,
                            files_error,
                            ..
                        } => {
                            *files_loading_since = None;
                            *files_error = Some(message.to_string());
                        },
                        _ => {},
                    }
                    break;
                }
            },
            CommitDest::Browser { hash } => {
                for tab in self.all_tabs_mut() {
                    if let TabKind::CommitGraph {
                        commits,
                        selected,
                        detail,
                        detail_loading_since,
                        files_loading_since,
                        files_error,
                        ..
                    } = &mut tab.kind
                    {
                        let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                        if selected_hash != Some(hash.as_str()) {
                            continue;
                        }
                        if detail.as_ref().is_some_and(|d| d.hash == hash) {
                            *files_loading_since = None;
                            *files_error = Some(message.to_string());
                        } else {
                            *detail_loading_since = None;
                        }
                    }
                }
            },
        }
    }

    /// Build and open a compare tab from a resolved range and its changes. An empty
    /// range (identical endpoints) opens with a "no changes" state rather than nothing.
    fn open_compare_tab(
        &mut self,
        base_label: String,
        head_label: String,
        merge_base: bool,
        changes: Vec<FileChange>,
    ) {
        if changes.is_empty() {
            self.status = Some(format!("no changes between {base_label} and {head_label}"));
        }
        let files = changes
            .into_iter()
            .map(|c| FileView::new(c, Section::Staged, self.syntax))
            .collect();
        self.push_tab(Tab::compare(base_label, head_label, merge_base, files));
    }

    /// Apply the forge's verification verdict to every open commit view for `hash` —
    /// both standalone commit tabs and the graph browser's shown detail.
    fn apply_commit_verification(&mut self, hash: &str, status: GithubVerification) {
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Commit {
                    detail,
                    verification,
                    ..
                } if detail.hash == hash => *verification = Some(status.clone()),
                TabKind::CommitGraph {
                    detail: Some(detail),
                    verification,
                    ..
                } if detail.hash == hash => *verification = Some(status.clone()),
                _ => {},
            }
        }
    }

    /// Send a fire-and-forget command to the backend (no document context).
    fn send_vcs(&mut self, command: SessionCommand) {
        self.send_command(command);
    }

    /// Submit a fire-and-forget backend command (the answering event, if any, is
    /// handled generically), surfacing a dropped-backend error as a notification.
    fn send_command(&mut self, command: SessionCommand) {
        let result = self.backend.as_ref().map(|backend| {
            let id = backend.next_id();
            backend.send(id, command)
        });
        if let Some(Err(e)) = result {
            self.notify_backend_error(e);
        }
    }

    /// Submit a backend command and return its [`RequestId`], so the answering event
    /// can be correlated (e.g. to route a commit detail to the right destination).
    /// Returns `None` when there is no backend or the submission failed.
    fn send_command_id(&mut self, command: SessionCommand) -> Option<RequestId> {
        let (id, result) = {
            let backend = self.backend.as_ref()?;
            let id = backend.next_id();
            (id, backend.send(id, command))
        };
        match result {
            Ok(()) => Some(id),
            Err(e) => {
                self.notify_backend_error(e);
                None
            },
        }
    }

    /// Send a path-scoped Source-Control command for the current selection.
    fn scm_send_paths(&mut self, make: impl FnOnce(Vec<PathBuf>) -> SessionCommand) {
        let paths = self.scm.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.send_vcs(make(paths));
    }

    /// Toggle staging for the selection. A multi-file selection may span both
    /// groups, so partition it by section: staged rows are unstaged, working rows
    /// are staged.
    fn scm_toggle_stage(&mut self) {
        let mut to_stage = Vec::new();
        let mut to_unstage = Vec::new();
        for i in self.scm.selection.selected_indices() {
            let Some(change) = self.scm.changes.get(i) else {
                continue;
            };
            match self.scm.section(i) {
                Section::Staged => to_unstage.push(change.path.clone()),
                Section::Working => to_stage.push(change.path.clone()),
            }
        }
        if !to_unstage.is_empty() {
            self.send_vcs(SessionCommand::Unstage { paths: to_unstage });
        }
        if !to_stage.is_empty() {
            self.send_vcs(SessionCommand::Stage { paths: to_stage });
        }
    }

    /// Extend the focused list panel's range selection by `delta` rows.
    fn sidebar_select_extend(&mut self, delta: i32) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.select_extend(delta);
            },
            SidebarPanel::SourceControl => self.scm.selection.extend_by(delta),
            SidebarPanel::Search => {},
        }
    }

    /// Toggle the cursor row in the focused list panel's selection.
    fn sidebar_select_toggle(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.mark_toggle();
            },
            SidebarPanel::SourceControl => self.scm.selection.toggle_cursor(),
            SidebarPanel::Search => {},
        }
    }

    /// Select every row in the focused list panel.
    fn sidebar_select_all(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.select_all();
            },
            SidebarPanel::SourceControl => self.scm.selection.select_all(),
            SidebarPanel::Search => {},
        }
    }

    /// Open the commit-message input, if there is something staged to commit.
    fn scm_open_commit_input(&mut self) {
        if self.scm.staged_count == 0 {
            self.status = Some("commit: stage changes first".to_string());
            return;
        }
        self.commit_input = Some(String::new());
    }

    /// Arm a discard confirmation for the current selection.
    fn scm_arm_discard(&mut self) {
        let paths = self.scm.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.status = Some(format!(
            "discard {} file(s)? press y to confirm, any other key to cancel",
            paths.len()
        ));
        self.pending_discard = Some(paths);
    }

    /// Cancel the commit input.
    fn commit_cancel(&mut self) {
        self.commit_input = None;
        self.status = Some("commit cancelled".to_string());
    }

    /// Submit the commit message (or report that one is required).
    fn commit_submit(&mut self) {
        let message = self.commit_input.take().unwrap_or_default();
        let message = message.trim().to_string();
        if message.is_empty() {
            self.commit_input = Some(String::new());
            self.status = Some("commit: message required".to_string());
        } else {
            self.send_vcs(SessionCommand::Commit { message });
        }
    }

    /// Ask the backend to draft a commit message from the staged diff. The result
    /// arrives asynchronously as [`SessionEvent::CommitMessageGenerated`] and replaces
    /// the input; problems (nothing staged, disabled, generator error) come back as a
    /// notification.
    fn commit_generate(&mut self) {
        self.status = Some("generating commit message…".to_string());
        self.send_vcs(SessionCommand::GenerateCommitMessage);
    }

    /// Edit the commit message with an unbound key (backspace / printable).
    fn commit_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.pop();
                }
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(message) = self.commit_input.as_mut() {
                    message.push(c);
                }
            },
            _ => {},
        }
    }

    /// Resolve a pending discard: `confirmed` discards the armed paths, otherwise
    /// the prompt is cancelled. Any key without a `DiscardConfirm` binding cancels.
    fn resolve_discard(&mut self, confirmed: bool) {
        let paths = self.pending_discard.take();
        if confirmed {
            if let Some(paths) = paths {
                self.send_vcs(SessionCommand::Discard { paths });
                self.notify(
                    Severity::Information,
                    NotificationKind::Vcs,
                    "discarded changes",
                );
            }
        } else {
            self.status = Some("discard cancelled".to_string());
        }
    }

    /// Replace the Source-Control panel state from a fresh backend status,
    /// reconciling the existing selection against the new row count.
    fn apply_vcs_status(&mut self, staged: Vec<FileChange>, working: Vec<FileChange>) {
        let staged_count = staged.len();
        let mut changes = staged;
        changes.extend(working);
        self.scm.changes = changes;
        self.scm.staged_count = staged_count;
        self.scm.selection.set_len(self.scm.changes.len());
    }

    /// Apply a fetched commit-log page: the first page (`skip == 0`) replaces the
    /// log; a later page appends. Guards against duplicate appends if a page is
    /// re-delivered.
    fn apply_vcs_log(&mut self, skip: usize, commits: Vec<Commit>, has_more: bool) {
        self.scm.log_loading = false;
        self.scm.log_loading_since = None;
        self.scm.log_has_more = has_more;
        if skip == 0 {
            // A fresh first page (initial load or a reconciliation reset) replaces the
            // log; scroll back to the top so the newest commits are in view.
            self.scm.log = commits;
            self.scm_commits_offset = 0;
        } else if skip == self.scm.log.len() {
            self.scm.log.extend(commits);
        }
    }

    /// Prepend newly-observed commits reported by the backend (an external commit
    /// picked up via file-watching). Duplicates are dropped, and the viewport is
    /// nudged so the user's position in the older history is preserved.
    fn apply_vcs_commits_prepended(&mut self, mut commits: Vec<Commit>) {
        let known: HashSet<&str> = self.scm.log.iter().map(|c| c.hash.as_str()).collect();
        commits.retain(|c| !known.contains(c.hash.as_str()));
        let inserted = commits.len();
        if inserted == 0 {
            return;
        }
        commits.append(&mut self.scm.log);
        self.scm.log = commits;
        // If the user had scrolled into the log, shift down so the same commits stay
        // put; at the top (offset 0) keep them at the newest.
        if self.scm_commits_offset > 0 {
            self.scm_commits_offset += inserted;
        }
    }

    /// Open `path`, focusing an existing tab for the same file instead of opening a
    /// duplicate. This is the single entry point for every "open a file" flow
    /// (explorer, quick-open, search result, startup, reopen-closed).
    fn open_path(&mut self, path: &Path) {
        let target = canonical(path);
        // Focus an existing editor view for this file, but not a diff tab — a diff
        // is a distinct view of the same path, so opening the file still opens it.
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            self.select_tab(idx);
            return;
        }
        let tab = workspace::open_file(path);
        self.push_tab(tab);
    }

    /// Open `path` into the focused pane's reusable "preview" tab slot (VS
    /// Code-style): used by file-tree navigation (single click / arrow +
    /// activate) and selection-follows-preview. A file already open (preview or
    /// permanent) is just shown. Otherwise the current preview tab, if this pane
    /// has one, is replaced in place; if not, a new preview tab is opened. Every
    /// other caller of `open_path` (LSP jumps, the overlay, reopen-closed,
    /// CLI-provided files) keeps opening permanent tabs — only tree navigation
    /// opens previews.
    ///
    /// `steal_focus` moves keyboard focus to the editor (Enter / click);
    /// selection-follows-preview passes `false` so the sidebar keeps focus and
    /// the user can keep arrowing.
    fn open_path_preview(&mut self, path: &Path, steal_focus: bool) {
        let target = canonical(path);
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            self.active = idx;
            self.find_open = false;
            if steal_focus {
                self.focus = Focus::Editor;
            }
            return;
        }
        let mut tab = workspace::open_file(path);
        tab.is_preview = true;
        self.install_preview_tab(tab, steal_focus);
    }

    /// Place `tab` (already flagged [`is_preview`](Tab::is_preview)) into the
    /// focused pane's single preview slot: replace the existing preview tab in
    /// place, or — when this pane has none — open it as a new tab. One slot per
    /// pane regardless of content kind, so a previewed file and a previewed diff
    /// share it. `steal_focus` moves keyboard focus to the editor; otherwise the
    /// current focus is preserved (selection-follows-preview).
    fn install_preview_tab(&mut self, mut tab: Tab, steal_focus: bool) {
        tab.view = self.alloc_view();
        match self.tabs.iter().position(|t| t.is_preview) {
            Some(idx) => {
                self.tabs[idx] = tab;
                self.active = idx;
                self.find_open = false;
                if steal_focus {
                    self.focus = Focus::Editor;
                }
                self.register_doc(self.active);
                // The replaced tab's document (if any) is no longer referenced by
                // any tab; this closes it on the session side.
                self.reconcile_open_docs();
            },
            None => {
                if self.tabs.len() == 1 && matches!(self.tabs[0].kind, TabKind::Welcome) {
                    self.tabs[0] = tab;
                    self.active = 0;
                } else {
                    self.tabs.push(tab);
                    self.active = self.tabs.len() - 1;
                }
                self.find_open = false;
                if steal_focus {
                    self.focus = Focus::Editor;
                }
                self.register_doc(self.active);
            },
        }
    }

    /// The "open anyway" override: re-open the active too-large placeholder's file
    /// with the size guard bypassed, replacing the placeholder tab in place (rather
    /// than opening a second tab for the same path). A no-op on any other tab — the
    /// binding is only live over a too-large placeholder.
    fn open_active_anyway(&mut self) {
        let path = match self.tabs.get(self.active) {
            Some(Tab {
                kind:
                    TabKind::Placeholder {
                        kind: FileKind::TooLarge { .. },
                        path,
                        ..
                    },
                ..
            }) => path.clone(),
            _ => return,
        };
        let mut tab = workspace::open_file_ignoring_size(&path);
        tab.view = self.alloc_view();
        self.tabs[self.active] = tab;
        self.focus = Focus::Editor;
        self.register_doc(self.active);
    }

    /// Add a tab, replacing a lone Welcome tab, and focus the editor.
    fn push_tab(&mut self, mut tab: Tab) {
        tab.view = self.alloc_view();
        if self.tabs.len() == 1 && matches!(self.tabs[0].kind, TabKind::Welcome) {
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.focus = Focus::Editor;
        // A newly-focused tab never inherits another tab's open find bar.
        self.find_open = false;
        self.register_doc(self.active);
    }

    /// Allocate a fresh [`ViewId`] for a newly-opened view.
    fn alloc_view(&mut self) -> ViewId {
        let view = ViewId(self.next_view);
        self.next_view += 1;
        view
    }

    /// The session document backing `tab`, if it is a registered code tab.
    fn tab_doc(tab: &Tab) -> Option<DocumentId> {
        match &tab.kind {
            // A preview counts as a view of its document: `reconcile_open_docs` ref-counts
            // through here, so reporting the id keeps the document (and its snapshot
            // stream) alive even after the source tab is closed.
            TabKind::Code { doc, .. } | TabKind::MarkdownPreview { doc, .. } => *doc,
            _ => None,
        }
    }

    /// The currently focused pane.
    pub(crate) fn focus_pane(&self) -> PaneId {
        self.layout.focus()
    }

    /// Stash the focused pane's tabs into storage, so *every* pane's tabs live in
    /// `stored` and the layout can be freely mutated. Pair with [`load_focused`].
    fn stash_focused(&mut self) {
        let current = self.layout.focus();
        let tabs = std::mem::take(&mut self.tabs);
        self.stored.insert(
            current,
            StoredPane {
                tabs,
                active: self.active,
            },
        );
        self.active = 0;
    }

    /// Pull the (possibly newly) focused pane's tabs out of storage into the live
    /// `tabs`/`active` fields. A pane with no stored tabs shows a lone welcome tab.
    fn load_focused(&mut self) {
        let pane = self.layout.focus();
        if let Some(sp) = self.stored.remove(&pane) {
            self.tabs = sp.tabs;
            self.active = sp.active;
        } else {
            self.tabs = vec![Tab::welcome()];
            self.active = 0;
        }
    }

    /// Make `pane` the focused pane, swapping the current focused tabs into storage
    /// and `pane`'s tabs out. A no-op if `pane` is already focused or unknown.
    fn focus_pane_switch(&mut self, pane: PaneId) {
        if pane == self.layout.focus() || !self.layout.contains(pane) {
            return;
        }
        self.stash_focused();
        self.layout.set_focus(pane);
        self.load_focused();
    }

    /// Every tab across every pane (the focused pane plus all stored panes). Used by
    /// backend-event/snapshot handlers that must reach a document wherever it is shown.
    fn all_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs
            .iter_mut()
            .chain(self.stored.values_mut().flat_map(|p| p.tabs.iter_mut()))
    }

    /// Every tab across every pane (immutable).
    fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs
            .iter()
            .chain(self.stored.values().flat_map(|p| p.tabs.iter()))
    }

    /// Whether any dirty open tab is backed by one of `paths` or a descendant.
    fn has_dirty_tabs_under(&self, paths: &[PathBuf]) -> bool {
        self.all_tabs().any(|tab| {
            tab.dirty
                && tab
                    .path()
                    .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
        })
    }

    /// Close every clean tab backed by one of `paths` or a descendant.
    fn close_tabs_under(&mut self, paths: &[PathBuf]) {
        self.tabs.retain(|tab| {
            !tab.path()
                .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
        });
        if self.tabs.is_empty() {
            self.tabs.push(Tab::welcome());
            self.active = 0;
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        for pane in self.stored.values_mut() {
            pane.tabs.retain(|tab| {
                !tab.path()
                    .is_some_and(|path| paths.iter().any(|root| path_under(root, path)))
            });
            if pane.tabs.is_empty() {
                pane.tabs.push(Tab::welcome());
                pane.active = 0;
            } else if pane.active >= pane.tabs.len() {
                pane.active = pane.tabs.len() - 1;
            }
        }
        self.reconcile_open_docs();
    }

    /// Update open tabs and the session document path map after a filesystem move.
    fn retarget_open_paths(&mut self, from: &Path, to: &Path) {
        let mut docs = Vec::new();
        for tab in self.all_tabs_mut() {
            let Some(current) = tab.path().map(Path::to_path_buf) else {
                continue;
            };
            let Some(next) = rebase_path(&current, from, to) else {
                continue;
            };
            let doc = Self::tab_doc(tab);
            retarget_tab_path(tab, &next);
            if let Some(doc) = doc {
                docs.push((doc, next));
            }
        }
        docs.sort_by_key(|(doc, _)| *doc);
        docs.dedup_by_key(|(doc, _)| *doc);
        for (doc, path) in docs {
            self.send_command(SessionCommand::RetargetDocument { doc, path });
        }
    }

    /// Release any session documents no longer shown in a tab (the session
    /// ref-counts opens; the app balances them). Call after closing tabs.
    fn reconcile_open_docs(&mut self) {
        let live: HashSet<DocumentId> = self.all_tabs().filter_map(Self::tab_doc).collect();
        let stale: Vec<DocumentId> = self.open_docs.difference(&live).copied().collect();
        for doc in stale {
            self.open_docs.remove(&doc);
            if let Some(backend) = &self.backend {
                let id = backend.next_id();
                let _ = backend.send(id, SessionCommand::CloseDocument { doc });
            }
        }
    }

    /// Open a semantic-blame view (`blameline`) for the active code tab.
    ///
    /// With `function_scope`, blame is narrowed to the function enclosing the caret;
    /// otherwise the whole file is blamed. Computed synchronously on demand (like
    /// find), so the original file tab stays open alongside the new Blame tab.
    fn open_blame(&mut self, function_scope: bool) {
        // Snapshot the inputs and release the borrow before mutating `self`.
        let input = self.tabs.get(self.active).and_then(|t| match &t.kind {
            TabKind::Code { path, text, .. } => {
                Some((path.clone(), text.clone(), t.editor.cursor().line))
            },
            _ => None,
        });
        let Some((path, text, line)) = input else {
            self.status = Some("blame: open a text file first".to_string());
            return;
        };

        // Absolutize first so blameline resolves the path against the worktree root
        // rather than doubling a relative path onto its own parent directory.
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let repo_root = abs.parent().unwrap_or(abs.as_path());
        let result = if function_scope {
            blameline::blame_function(repo_root, &abs, &text, line)
        } else {
            blameline::blame_file(repo_root, &abs)
        };
        let groups = match result {
            Ok(groups) if !groups.is_empty() => groups,
            Ok(_) => {
                self.status = Some("blame: no commits touch this file".to_string());
                return;
            },
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("blame: {e}"),
                );
                return;
            },
        };

        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let title = if function_scope {
            format!("Blame ❯ {name}")
        } else {
            format!("Blame: {name}")
        };
        let tab = Tab::new(
            title,
            TabKind::Blame {
                path,
                groups,
                scroll: 0,
            },
        );
        self.push_tab(tab);
    }

    /// Switch to the tab at `index`, focusing the editor.
    fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus = Focus::Editor;
            // The find bar is keyed to whichever tab it was opened over; switching
            // tabs must not show it over a different file.
            self.find_open = false;
        }
    }

    /// Switch to the next tab (wrapping).
    fn next_tab(&mut self) {
        let n = self.tabs.len();
        if n > 1 {
            self.select_tab((self.active + 1) % n);
        }
    }

    /// Switch to the previous tab (wrapping).
    fn prev_tab(&mut self) {
        let n = self.tabs.len();
        if n > 1 {
            self.select_tab((self.active + n - 1) % n);
        }
    }

    /// Go to the 1-based tab `n` (9 selects the last tab, VS Code-style).
    fn go_to_tab(&mut self, n: u8) {
        let n = n as usize;
        let index = if n >= 9 {
            self.tabs.len().saturating_sub(1)
        } else {
            n.saturating_sub(1)
        };
        self.select_tab(index);
    }

    /// Move the tab at `from` to position `to`, making it active.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = to;
    }

    /// While dragging, move the active tab under column `x` within the focused pane.
    fn drag_tab_to(&mut self, x: u16) {
        let focused = self.focus_pane();
        let hit = self
            .pane_frames
            .iter()
            .find(|f| f.pane == focused)
            .and_then(|f| tab_at(&f.tab_hits, x));
        if let Some((target, _)) = hit
            && target != self.active
        {
            self.move_tab(self.active, target);
        }
    }

    /// The pane whose *content* area contains `(x, y)`, and that content rect.
    fn pane_at_content(&self, x: u16, y: u16) -> Option<(PaneId, Rect)> {
        self.pane_frames
            .iter()
            .find(|f| rect_contains(f.content_rect, (x, y)))
            .map(|f| (f.pane, f.content_rect))
    }

    /// Update the in-progress tab drag: reorder within the origin pane's strip, or
    /// track a drop target (pane + zone) over another pane's content for preview.
    fn drag_tab_update(&mut self, x: u16, y: u16) {
        let Some(drag) = self.tab_drag else {
            return;
        };
        // Over the origin pane's own tab strip: reorder in place, no drop target.
        let on_from_strip = self
            .pane_frames
            .iter()
            .find(|f| f.pane == drag.from_pane)
            .is_some_and(|f| rect_contains(f.tabstrip_rect, (x, y)));
        if on_from_strip {
            self.drag_tab_to(x);
            if let Some(d) = self.tab_drag.as_mut() {
                d.hover = None;
            }
            return;
        }
        let hover = self
            .pane_at_content(x, y)
            .map(|(pane, rect)| (pane, drop_zone(rect, x, y)));
        if let Some(d) = self.tab_drag.as_mut() {
            d.hover = hover;
        }
    }

    /// Finish a tab drag: apply the pending move/split, if any.
    fn drag_tab_drop(&mut self) {
        let Some(drag) = self.tab_drag.take() else {
            return;
        };
        if let Some((target, zone)) = drag.hover {
            self.drop_tab_on(target, zone);
        }
    }

    /// Drop the focused pane's active tab onto `target`'s `zone`: an edge splits
    /// `target` and moves the tab into the new pane; the center moves it into
    /// `target`. Collapses the origin pane if it empties.
    fn drop_tab_on(&mut self, target: PaneId, zone: DropZone) {
        let from = self.focus_pane();
        if self.tabs.is_empty() || (target == from && zone == DropZone::Center) {
            return;
        }
        let idx = self.active.min(self.tabs.len().saturating_sub(1));
        let tab = self.tabs.remove(idx);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));

        // Move all panes into storage so the layout can be mutated freely.
        self.stash_focused();
        let dest = match zone.split_dir() {
            Some(dir) => {
                let new_pane = self.layout.split(target, dir);
                self.stored.insert(
                    new_pane,
                    StoredPane {
                        tabs: Vec::new(),
                        active: 0,
                    },
                );
                new_pane
            },
            None => target,
        };
        if let Some(sp) = self.stored.get_mut(&dest) {
            sp.tabs.push(tab);
            sp.active = sp.tabs.len().saturating_sub(1);
        }
        // If the origin pane emptied, close it (collapsing the split).
        if from != dest && self.stored.get(&from).is_some_and(|sp| sp.tabs.is_empty()) {
            self.stored.remove(&from);
            self.layout.close(from);
        }
        self.layout.set_focus(dest);
        self.load_focused();
        self.focus = Focus::Editor;
        self.reconcile_open_docs();
    }

    /// Whether `tab` is the markdown preview of the source view `source_view`.
    fn previews_view(tab: &Tab, source_view: ViewId) -> bool {
        matches!(&tab.kind, TabKind::MarkdownPreview { source_view: v, .. } if *v == source_view)
    }

    /// The visible (pane-active) tab of some *non-focused* pane matching `pred`.
    fn stored_active(&self, pred: impl Fn(&Tab) -> bool) -> Option<&Tab> {
        self.stored
            .values()
            .filter_map(|pane| pane.tabs.get(pane.active))
            .find(|tab| pred(tab))
    }

    /// As [`stored_active`](Self::stored_active), mutably.
    fn stored_active_mut(&mut self, pred: impl Fn(&Tab) -> bool) -> Option<&mut Tab> {
        self.stored
            .values_mut()
            .filter_map(|pane| pane.tabs.get_mut(pane.active))
            .find(|tab| pred(tab))
    }

    /// Scroll a markdown preview and its source together.
    ///
    /// The focused pane drives and the other follows. Because the driver's own scroll is
    /// never written back, the pair cannot oscillate even though the projections are lossy
    /// — a whole run of source lines can share one wrapped line.
    ///
    /// The preview only pushes once it has actually been scrolled *away* from where the
    /// source projects it. Without that check, merely moving focus onto the preview would
    /// nudge the source by the rounding error of a `source -> wrapped -> source` round trip.
    ///
    /// Runs once per frame just before drawing, so it reads the `wrapped` model the
    /// previous draw cached; a resize therefore takes one extra frame to settle. A pair
    /// whose halves are not both their pane's visible tab is skipped.
    pub(crate) fn sync_markdown_preview(&mut self) {
        let Some(focused) = self.tabs.get(self.active) else {
            return;
        };
        match &focused.kind {
            TabKind::Code { .. } => {
                let view = focused.view;
                let line = focused.editor.scroll_line as usize;
                let Some(preview) = self.stored_active_mut(|t| Self::previews_view(t, view)) else {
                    return;
                };
                if let TabKind::MarkdownPreview {
                    wrapped, scroll, ..
                } = &mut preview.kind
                {
                    let row = wrapped.wrapped_line_for_source(line);
                    *scroll = u16::try_from(row).unwrap_or(u16::MAX);
                }
            },
            TabKind::MarkdownPreview {
                source_view,
                wrapped,
                scroll,
                ..
            } => {
                let view = *source_view;
                let scroll = *scroll;
                let Some(source) = self.stored_active(|t| t.view == view) else {
                    return;
                };
                let source_line = source.editor.scroll_line as usize;
                if wrapped.wrapped_line_for_source(source_line) == usize::from(scroll) {
                    return; // already consistent: a bare focus change must not move it
                }
                let want = wrapped.source_line_for_wrapped(usize::from(scroll));
                let Some(source) = self.stored_active_mut(|t| t.view == view) else {
                    return;
                };
                if let TabKind::Code { buffer, .. } = &source.kind {
                    let last = buffer.line_count().saturating_sub(1);
                    source.editor.scroll_line = u32::try_from(want.min(last)).unwrap_or(u32::MAX);
                }
            },
            _ => {},
        }
    }

    /// Focus the existing preview of `source_view`, wherever it lives. `false` if there
    /// is none.
    fn reveal_markdown_preview(&mut self, source_view: ViewId) -> bool {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| Self::previews_view(t, source_view))
        {
            self.active = index;
            self.focus = Focus::Editor;
            return true;
        }
        let found = self.stored.iter().find_map(|(pane, stored)| {
            stored
                .tabs
                .iter()
                .position(|t| Self::previews_view(t, source_view))
                .map(|index| (*pane, index))
        });
        let Some((pane, index)) = found else {
            return false;
        };
        self.focus_pane_switch(pane);
        self.active = index;
        self.focus = Focus::Editor;
        true
    }

    /// Open a rendered preview of the active Markdown file in a pane to the right.
    ///
    /// Focus deliberately stays in the source editor (unlike [`split_focused`]): the user
    /// invoked this to keep typing and watch the preview follow, and it makes the editor
    /// the scroll master (see [`sync_markdown_preview`](Self::sync_markdown_preview)).
    fn open_markdown_preview_side(&mut self) {
        // Take everything the preview needs up front: the rest of this borrows `self`
        // mutably. The rope clone is O(1), so the preview paints from it on its very
        // first frame, before any snapshot has landed.
        let source = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code {
                path,
                doc,
                buffer,
                text,
                ..
            } => {
                let head = text.as_bytes();
                let head = head.get(..crate::workspace::HEAD_BYTES).unwrap_or(head);
                (karet_filetype::classify_ignoring_size(path, head) == FileKind::Markdown)
                    .then(|| (path.clone(), *doc, tab.view, buffer.content_snapshot()))
            },
            _ => None,
        });
        let Some((path, doc, source_view, buffer)) = source else {
            self.status = Some("markdown preview: not a Markdown file".to_string());
            return;
        };
        if self.reveal_markdown_preview(source_view) {
            return;
        }
        let preview = Tab::markdown_preview(path, doc, source_view, buffer);

        let from = self.focus_pane();
        if !self.layout.can_split(from, SplitDir::Right, self.main_rect) {
            // Too narrow to split: a tab in this pane still beats refusing to preview.
            self.push_tab(preview);
            return;
        }
        let mut preview = preview;
        preview.view = self.alloc_view();
        self.stash_focused();
        let new_pane = self.layout.split(from, SplitDir::Right);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![preview],
                active: 0,
            },
        );
        // `split` focuses the pane it created; hand focus back to the source editor.
        self.layout.set_focus(from);
        self.load_focused();
        self.focus = Focus::Editor;
    }

    /// Split the focused pane in `dir` via the keyboard, opening a second view of the
    /// active document (sharing its session document, with an independent cursor) in
    /// the new pane, which becomes focused.
    fn split_focused(&mut self, dir: SplitDir) {
        let from = self.focus_pane();
        let dup = self.duplicate_active_tab();
        self.stash_focused();
        let new_pane = self.layout.split(from, dir);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![dup],
                active: 0,
            },
        );
        self.layout.set_focus(new_pane);
        self.load_focused();
        self.focus = Focus::Editor;
    }

    /// Build a second view of the active tab for a new pane: the same document
    /// (shared edit log) with a fresh [`ViewId`] and independent editor state. A
    /// non-code (or empty) active tab yields a welcome tab.
    fn duplicate_active_tab(&mut self) -> Tab {
        let view = self.alloc_view();
        let mut tab = match self.tabs.get(self.active) {
            Some(t) => match &t.kind {
                TabKind::Code {
                    path,
                    language,
                    doc,
                    next_version,
                    buffer,
                    text,
                    highlights,
                    semantic_blocks,
                    folds,
                    folded,
                    decos,
                    search_decos,
                    syntax_errors,
                } => Tab::new(
                    t.title.clone(),
                    TabKind::Code {
                        path: path.clone(),
                        language,
                        doc: *doc,
                        next_version: *next_version,
                        buffer: buffer.clone(),
                        text: text.clone(),
                        highlights: highlights.clone(),
                        semantic_blocks: semantic_blocks.clone(),
                        folds: folds.clone(),
                        folded: folded.clone(),
                        decos: decos.clone(),
                        search_decos: search_decos.clone(),
                        syntax_errors: syntax_errors.clone(),
                    },
                ),
                _ => Tab::welcome(),
            },
            None => Tab::welcome(),
        };
        tab.view = view;
        tab
    }

    /// Cycle window focus to the next (`forward`) or previous pane. A no-op with
    /// fewer than two panes.
    fn focus_pane_cycle(&mut self, forward: bool) {
        let panes = self.layout.panes();
        let n = panes.len();
        if n < 2 {
            return;
        }
        let cur = self.layout.focus();
        let i = panes.iter().position(|p| *p == cur).unwrap_or(0);
        let next = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        self.focus_pane_switch(panes[next]);
    }

    /// Move the active tab one slot left (`-1`) or right (`+1`), clamped (no wrap).
    fn move_active_tab(&mut self, delta: i32) {
        let n = self.tabs.len() as i64;
        if n < 2 {
            return;
        }
        let target = (self.active as i64 + i64::from(delta)).clamp(0, n - 1) as usize;
        if target != self.active {
            self.tabs.swap(self.active, target);
            self.active = target;
        }
    }

    /// Record a closed file tab's path so it can be reopened later.
    fn remember_closed(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index)
            && !tab.is_diff()
            && let Some(path) = tab.path()
        {
            let path = path.to_path_buf();
            self.closed.retain(|p| p != &path);
            self.closed.push(path);
        }
    }

    /// Close the focused pane's active tab, routed through the unsaved-changes guard.
    fn request_close_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the focused pane's tab at `index`, routed through the unsaved-changes
    /// guard (the tab is captured by its stable view id).
    fn request_close_tab_at(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get(index) {
            self.guarded_close(CloseRequest::Tab { view: tab.view });
        }
    }

    /// Close the tab at `index`, falling back to a Welcome tab when the last closes.
    fn close_tab_at(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.remember_closed(index);
        if self.tabs.len() == 1 {
            self.tabs = vec![Tab::welcome()];
            self.active = 0;
            self.focus = Focus::Sidebar;
        } else {
            self.tabs.remove(index);
            if index < self.active {
                self.active -= 1;
            }
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len() - 1;
            }
        }
        // The closed tab's own `find` data goes with it; the flag may now be
        // pointing at a different tab, so drop it too rather than risk showing
        // the bar over whatever tab ends up active.
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Close every tab except the active one.
    fn close_other_tabs(&mut self) {
        if self.tabs.len() <= 1 {
            return;
        }
        for i in (0..self.tabs.len()).rev() {
            if i != self.active {
                self.remember_closed(i);
            }
        }
        self.tabs = vec![self.tabs.remove(self.active)];
        self.active = 0;
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Close every tab to the right of the active one.
    fn close_tabs_to_right(&mut self) {
        for i in (self.active + 1..self.tabs.len()).rev() {
            self.remember_closed(i);
        }
        self.tabs.truncate(self.active + 1);
        self.reconcile_open_docs();
    }

    /// Close all tabs, leaving a Welcome tab.
    fn close_all_tabs(&mut self) {
        for i in (0..self.tabs.len()).rev() {
            self.remember_closed(i);
        }
        self.tabs = vec![Tab::welcome()];
        self.active = 0;
        self.focus = Focus::Sidebar;
        self.find_open = false;
        self.reconcile_open_docs();
    }

    /// Reopen the most recently closed file tab whose file still exists.
    fn reopen_closed_tab(&mut self) {
        while let Some(path) = self.closed.pop() {
            if path.is_file() {
                self.open_path(&path);
                return;
            }
        }
    }

    /// Scroll the active tab by `delta` lines/rows (clamped to its content).
    fn scroll_lines(&mut self, delta: i32) {
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
            | TabKind::Blame { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
            | TabKind::CommitLoading { scroll, .. }
            | TabKind::Commit { scroll, .. }
            | TabKind::Compare { scroll, .. } => {
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                *scroll = next as u16;
            },
            TabKind::Hex { bytes, scroll, .. } => {
                let max = bytes.len().div_ceil(16).saturating_sub(1) as i64;
                let next = (*scroll as i64 + i64::from(delta)).clamp(0, max);
                *scroll = next as usize;
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

    /// Scroll the active overflow-mode code tab horizontally by `delta` columns.
    fn scroll_columns(&mut self, delta: i32) {
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
    fn scroll_edge(&mut self, top: bool) {
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
            | TabKind::Blame { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
            | TabKind::CommitLoading { scroll, .. }
            | TabKind::Commit { scroll, .. }
            | TabKind::Compare { scroll, .. } => {
                *scroll = if top { 0 } else { u16::MAX };
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

    /// Toggle the active diff tab between unified and side-by-side.
    fn toggle_diff_layout(&mut self) {
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
    fn toggle_fold(&mut self) {
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
    fn step_changed_file(&mut self, delta: i32) {
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
    fn open_diff_file(&mut self) {
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

    /// Handle a mouse event over a pane's tab strip (click to switch / close, wheel
    /// to cycle). Returns `true` when the event was consumed.
    fn handle_tabstrip_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        let Some((pane, hit)) = self.pane_frames.iter().find_map(|f| {
            rect_contains(f.tabstrip_rect, point)
                .then(|| (f.pane, tab_at(&f.tab_hits, mouse.column)))
        }) else {
            return false;
        };
        // Act on the clicked pane (borrow of `pane_frames` has ended).
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.focus_pane_switch(pane);
                self.next_tab();
            },
            MouseEventKind::ScrollUp => {
                self.focus_pane_switch(pane);
                self.prev_tab();
            },
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus_pane_switch(pane);
                if let Some((i, on_close)) = hit {
                    if on_close {
                        self.request_close_tab_at(i);
                    } else {
                        self.select_tab(i);
                        self.tab_drag = Some(TabDrag {
                            from_pane: pane,
                            hover: None,
                        });
                    }
                }
            },
            MouseEventKind::Down(MouseButton::Middle) => {
                self.focus_pane_switch(pane);
                if let Some((i, _)) = hit {
                    self.request_close_tab_at(i);
                }
            },
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click on a tab selects it and opens the pane context menu
                // for it; the strip's empty tail opens nothing.
                self.focus_pane_switch(pane);
                if let Some((i, _)) = hit {
                    self.select_tab(i);
                    self.open_pane_context_menu(mouse.column, mouse.row);
                }
            },
            _ => {},
        }
        true
    }

    /// Handle a left click on a pane's breadcrumb row: a segment reveals its path
    /// prefix in the Explorer; a separator gap (or an inert segment above the
    /// workspace root) does nothing. Either way the click is consumed so it never
    /// falls through to the tab strip or editor underneath. Returns `true` when
    /// consumed.
    fn handle_breadcrumb_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let Some(hit) = self.pane_frames.iter().find_map(|f| {
            rect_contains(f.breadcrumb_rect, point).then(|| {
                f.breadcrumb_hits
                    .iter()
                    .find(|h| mouse.column >= h.start && mouse.column < h.end)
                    .map(|h| h.path.clone())
            })
        }) else {
            return false;
        };
        if let Some(path) = hit {
            self.reveal_in_explorer(&path);
        }
        true
    }

    /// Handle a left click on a toast card: dismiss it. Returns `true` when the
    /// click landed on a card (so it is not routed elsewhere).
    fn handle_toast_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return false;
        }
        let point = (mouse.column, mouse.row);
        let Some(hit) = self
            .toast_hits
            .iter()
            .find(|h| rect_contains(h.rect, point))
        else {
            return false;
        };
        let id = hit.id;
        self.notifications.dismiss(id);
        true
    }

    /// The command bound to the status-bar segment at column `x`, if any.
    fn status_command_at(&self, x: u16) -> Option<Command> {
        self.status_hits
            .iter()
            .find_map(|&(start, end, cmd)| (x >= start && x < end).then_some(cmd))
    }

    /// Handle a left click on a status-bar segment. Returns `true` when consumed.
    fn handle_status_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !rect_contains(self.status_rect, (mouse.column, mouse.row)) {
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind
            && let Some(cmd) = self.status_command_at(mouse.column)
        {
            self.dispatch(cmd);
        }
        true
    }

    /// Handle mouse interaction with an open context menu.
    fn handle_context_menu_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(menu) = self.context_menu.as_ref() else {
            return false;
        };
        let point = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if rect_contains(menu.rect, point) => {
                let inner_y = mouse.row.saturating_sub(menu.rect.y).saturating_sub(1);
                let idx = usize::from(inner_y);
                if let Some(menu) = self.context_menu.as_mut()
                    && idx < menu.entries.len()
                {
                    menu.selected = idx;
                }
                self.accept_context_menu();
                true
            },
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Down(MouseButton::Right) => {
                self.close_context_menu();
                true
            },
            _ => true,
        }
    }

    /// Handle a mouse event: the tab strip (switch / close / cycle), wheel scrolls
    /// (the sidebar or the active tab), and a left click moves focus.
    /// Resize the sidebar so its right edge sits at column `col`. Dragging narrower
    /// than [`SIDEBAR_MIN_WIDTH`] is read as intent to collapse. The responsive upper
    /// bound (terminal width) is applied when the layout is next computed.
    fn resize_sidebar_to(&mut self, col: u16) {
        let width = col.saturating_sub(self.sidebar_rect.x);
        if width < SIDEBAR_MIN_WIDTH {
            self.sidebar_visible = false;
            self.sidebar_resizing = false;
        } else {
            self.sidebar_width = width;
        }
    }

    /// Resize the pinned Source-Control commit-log region so its top edge (the drag
    /// divider) sits at `row`. The list area's bottom is fixed, so the commit region
    /// grows as the divider moves up; both regions keep at least [`MIN_SCM_REGION`].
    fn resize_scm_commits_to(&mut self, row: u16) {
        let content = self.sidebar_content_rect;
        let bottom = content.y + content.height;
        let list_top = self.scm_changes_rect.y;
        let total = bottom.saturating_sub(list_top);
        // Reserve MIN for the changes region plus the 1-row divider.
        let max_commits = total.saturating_sub(MIN_SCM_REGION + 1).max(MIN_SCM_REGION);
        let h = bottom.saturating_sub(row).saturating_sub(1);
        self.scm_commits_h = h.clamp(MIN_SCM_REGION, max_commits);
    }

    /// Hint the terminal's mouse pointer shape (OSC 22) over a draggable
    /// divider — `col-resize` over the sidebar-width divider, `row-resize` over
    /// the Source-Control commit-log divider, the default shape everywhere
    /// else — mirroring how a GUI editor shows a resize cursor on hover. A
    /// complete no-op when the terminal wasn't confirmed to support it at
    /// startup, and only writes when the shape actually changes (never spams
    /// an escape sequence per mouse-move event).
    fn update_pointer_shape_hint(&mut self, mouse: &MouseEvent) {
        if !self.pointer_shapes_supported {
            return;
        }
        let over_sidebar_divider = self.sidebar_resizing
            || (self.sidebar_visible && mouse.column == self.sidebar_divider_x);
        let over_scm_divider = self.scm_resizing
            || (self.sidebar_panel == SidebarPanel::SourceControl
                && self.scm_divider_y != 0
                && mouse.row == self.scm_divider_y
                && rect_contains(self.sidebar_rect, (mouse.column, mouse.row)));
        let shape = if over_sidebar_divider {
            Some("col-resize")
        } else if over_scm_divider {
            Some("row-resize")
        } else {
            None
        };
        if shape == self.pointer_shape {
            return;
        }
        let _ = write!(io::stdout(), "\x1b]22;{}\x1b\\", shape.unwrap_or("default"));
        let _ = io::stdout().flush();
        self.pointer_shape = shape;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        self.update_pointer_shape_hint(&mouse);
        // Toasts float above everything (including the overlay), so hit-test them
        // first: a left click on a card dismisses it.
        if self.handle_toast_mouse(mouse) {
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        if self.handle_context_menu_mouse(mouse) {
            return;
        }
        // An in-progress tab drag captures motion until the button is released.
        if self.tab_drag.is_some() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_tab_update(mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.drag_tab_drop(),
                _ => {},
            }
            return;
        }
        // An in-progress sidebar resize captures motion until the button is released.
        if self.sidebar_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_sidebar_to(mouse.column),
                MouseEventKind::Up(MouseButton::Left) => self.sidebar_resizing = false,
                _ => {},
            }
            return;
        }
        // An in-progress Source-Control commit-divider resize captures motion likewise.
        if self.scm_resizing {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.resize_scm_commits_to(mouse.row),
                MouseEventKind::Up(MouseButton::Left) => self.scm_resizing = false,
                _ => {},
            }
            return;
        }
        // An in-progress text selection captures motion until the button is released.
        if self.editor_selecting {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_select_to(mouse.column, mouse.row);
                },
                MouseEventKind::Up(MouseButton::Left) => self.editor_selecting = false,
                _ => {},
            }
            return;
        }
        if self.handle_tabstrip_mouse(mouse) {
            return;
        }
        if self.handle_breadcrumb_mouse(mouse) {
            return;
        }
        if self.handle_status_mouse(mouse) {
            return;
        }
        let point = (mouse.column, mouse.row);
        let in_sidebar = self.sidebar_visible && rect_contains(self.sidebar_rect, point);
        let in_outline = self.outline_visible && rect_contains(self.outline_rect, point);
        let in_editor = rect_contains(self.editor_rect, point);
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        match mouse.kind {
            MouseEventKind::ScrollDown if in_outline => self.outline_step(1),
            MouseEventKind::ScrollUp if in_outline => self.outline_step(-1),
            MouseEventKind::ScrollDown if in_sidebar => self.sidebar_wheel(3, mouse.row),
            MouseEventKind::ScrollUp if in_sidebar => self.sidebar_wheel(-3, mouse.row),
            MouseEventKind::ScrollRight if in_editor => self.scroll_columns(3),
            MouseEventKind::ScrollLeft if in_editor => self.scroll_columns(-3),
            MouseEventKind::ScrollDown if in_editor && shift => self.scroll_columns(3),
            MouseEventKind::ScrollUp if in_editor && shift => self.scroll_columns(-3),
            MouseEventKind::ScrollDown => self.scroll_lines(3),
            MouseEventKind::ScrollUp => self.scroll_lines(-3),
            MouseEventKind::Down(MouseButton::Left) if in_outline => {
                self.handle_outline_click(mouse.row);
            },
            MouseEventKind::Down(MouseButton::Right)
                if in_sidebar && self.sidebar_panel == SidebarPanel::Explorer =>
            {
                let row = rect_contains(self.sidebar_content_rect, point)
                    .then(|| {
                        self.explorer
                            .visible_index((mouse.row - self.sidebar_content_rect.y) as usize)
                    })
                    .flatten();
                self.open_context_menu(mouse.column, mouse.row, row);
            },
            MouseEventKind::Down(MouseButton::Right) => {
                // Right-click in a pane's content area opens the pane context menu
                // for that pane's active tab.
                if let Some(pane) = self
                    .pane_frames
                    .iter()
                    .find(|f| rect_contains(f.content_rect, point))
                    .map(|f| f.pane)
                {
                    self.focus_pane_switch(pane);
                    self.focus = Focus::Editor;
                    self.open_pane_context_menu(mouse.column, mouse.row);
                }
            },
            MouseEventKind::Down(MouseButton::Left) => {
                if self.sidebar_visible && mouse.column == self.sidebar_divider_x {
                    // Grab the sidebar-width divider to start a resize drag.
                    self.sidebar_resizing = true;
                } else if in_sidebar
                    && self.sidebar_panel == SidebarPanel::SourceControl
                    && self.scm_divider_y != 0
                    && mouse.row == self.scm_divider_y
                {
                    // Grab the Source-Control changes/commits divider.
                    self.scm_resizing = true;
                } else if in_sidebar {
                    self.handle_sidebar_click(mouse.column, mouse.row, mouse.modifiers);
                } else {
                    self.handle_editor_click(mouse);
                }
            },
            // Track the hover position for the secondary-accent row highlight in the
            // explorer / source-control lists (cleared when off the content area).
            MouseEventKind::Moved => {
                self.hover = rect_contains(self.sidebar_content_rect, point).then_some(point);
                self.sidebar_header_hover =
                    (in_sidebar && mouse.row == self.sidebar_rect.y).then_some(point);
            },
            _ => {},
        }
    }

    /// The absolute explorer row index under the hover cursor, if the mouse is over
    /// the explorer's content (accounts for the current scroll offset).
    pub(crate) fn hovered_explorer_row(&self) -> Option<usize> {
        let (_, hy) = self.hover?;
        let top = self.sidebar_content_rect.y;
        (hy >= top).then(|| self.explorer.offset() + usize::from(hy - top))
    }

    /// The source-control change index under the hover cursor, if any (mirrors the
    /// SCM click hit-testing, using the last frame's row map).
    pub(crate) fn hovered_scm_change(&self) -> Option<usize> {
        let point = self.hover?;
        if !rect_contains(self.scm_changes_rect, point) {
            return None;
        }
        let display = self.scm_offset + usize::from(point.1 - self.scm_changes_rect.y);
        self.scm_row_map.get(display).copied().flatten()
    }

    /// The sidebar panel whose header switcher cell is at `(col, row_y)`, if any.
    fn panel_at(&self, col: u16, row_y: u16) -> Option<SidebarPanel> {
        if row_y != self.sidebar_rect.y {
            return None;
        }
        self.panel_hits
            .iter()
            .find_map(|&(start, end, panel)| (col >= start && col < end).then_some(panel))
    }

    /// Handle a left click inside the sidebar: switch panels via the header, or
    /// select the clicked row. A plain click moves the cursor and activates the
    /// row; Ctrl toggles it in the selection and Shift extends a range to it
    /// (neither activates).
    fn handle_sidebar_click(&mut self, col: u16, row_y: u16, modifiers: KeyModifiers) {
        self.focus = Focus::Sidebar;
        // Explorer header toolbar buttons sit on the header row alongside the switcher.
        if row_y == self.sidebar_rect.y
            && let Some(cmd) = self
                .header_action_hits
                .iter()
                .find_map(|&(start, end, cmd)| (col >= start && col < end).then_some(cmd))
        {
            self.dispatch(cmd);
            return;
        }
        if let Some(panel) = self.panel_at(col, row_y) {
            self.dispatch(Command::SelectPanel(panel));
            return;
        }
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);
        let shift = modifiers.contains(KeyModifiers::SHIFT);
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                if !rect_contains(self.sidebar_content_rect, (col, row_y)) {
                    return;
                }
                let view_row = (row_y - self.sidebar_content_rect.y) as usize;
                let root = self.root.clone();
                self.explorer.ensure_built(&root);
                if self.explorer.visible_index(view_row).is_none() {
                    return;
                }
                if ctrl {
                    self.explorer.toggle_visible(view_row);
                } else if shift {
                    self.explorer.extend_visible(view_row);
                } else {
                    let streak = self.click_streak(col, row_y);
                    self.explorer.select_visible(view_row);
                    if streak >= 2 {
                        // Double-click lands on the SAME view the single-click
                        // preview created, materialized — never a duplicate.
                        self.sidebar_promote_or_open_permanent();
                    } else {
                        self.explorer_preview_with_focus();
                    }
                }
            },
            SidebarPanel::SourceControl => {
                // The pinned commit-log region: click a commit row to open its commit
                // view; the trailing "load more" affordance pages in older history.
                if rect_contains(self.scm_commits_rect, (col, row_y)) {
                    let display =
                        self.scm_commits_offset + (row_y - self.scm_commits_rect.y) as usize;
                    if self.scm_more_row == Some(display) {
                        self.load_more_scm_log();
                    } else if let Some(commit) =
                        display.checked_sub(1).and_then(|i| self.scm.log.get(i))
                    {
                        // Row 0 is the " COMMITS" header; commits begin at display 1.
                        self.open_commit(commit.hash.clone());
                    }
                    return;
                }
                if !rect_contains(self.scm_changes_rect, (col, row_y)) {
                    return;
                }
                let display = self.scm_offset + (row_y - self.scm_changes_rect.y) as usize;
                if let Some(Some(idx)) = self.scm_row_map.get(display).copied() {
                    if ctrl {
                        self.scm.selection.toggle(idx);
                    } else if shift {
                        self.scm.selection.extend_to(idx);
                    } else {
                        let streak = self.click_streak(col, row_y);
                        self.scm.selection.move_to(idx);
                        if streak >= 2 {
                            // Double-click materializes the SAME view the
                            // single-click preview created and moves focus into
                            // it — never a duplicate diff tab.
                            self.open_selected_diff();
                        } else {
                            // A single click browses: the diff shows in the
                            // preview slot while the panel keeps focus, so the
                            // staging keys stay live.
                            self.preview_selected_diff();
                        }
                    }
                }
            },
            SidebarPanel::Search => {
                // Header buttons: option toggles on the find row, replace-all on the
                // replace row.
                if let Some(cmd) =
                    self.search_action_hits
                        .iter()
                        .find_map(|&(start, end, ry, cmd)| {
                            (row_y == ry && col >= start && col < end).then_some(cmd)
                        })
                {
                    self.dispatch(cmd);
                    return;
                }
                // Click a field to edit it.
                if row_y == self.search_query_row {
                    self.search.field = SearchField::Find;
                    self.search.input = true;
                    return;
                }
                if Some(row_y) == self.search_replace_row {
                    self.search.field = SearchField::Replace;
                    self.search.replace_visible = true;
                    self.search.input = true;
                    return;
                }
                if !rect_contains(self.search_results_rect, (col, row_y)) {
                    return;
                }
                let idx = self.search_offset + (row_y - self.search_results_rect.y) as usize;
                if idx < self.search.results.len() {
                    self.search.selected = idx;
                    self.open_selected_result();
                }
            },
        }
    }

    /// Copy `text` to the clipboard, reporting the outcome in the status bar.
    fn copy_to_clipboard(&mut self, text: String, what: &str) {
        self.status = Some(match self.clipboard.set(&text) {
            Ok(()) => format!("copied {what}"),
            Err(e) => format!("copy failed: {e}"),
        });
    }

    /// Copy the active code tab's selection, or its cursor line when nothing is
    /// selected (VS Code behavior).
    fn copy_selection(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_copy_files();
            return;
        }
        let text = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { buffer, text, .. },
                editor,
                ..
            }) => editor.selection_range().map_or_else(
                || {
                    buffer
                        .line(editor.cursor().line as usize)
                        .map(|l| format!("{l}\n"))
                },
                |range| selection_text(buffer, text, range),
            ),
            _ => None,
        };
        match text {
            Some(text) => self.copy_to_clipboard(text, "selection"),
            None => self.status = Some("copy: open a text file".to_string()),
        }
    }

    /// Copy the active file's path (absolute or workspace-relative) to the clipboard.
    fn copy_path(&mut self, relative: bool) {
        let Some(path) = self.tabs.get(self.active).and_then(Tab::path) else {
            self.status = Some("copy path: no file".to_string());
            return;
        };
        let path = if relative {
            path.strip_prefix(&self.root).unwrap_or(path)
        } else {
            path
        };
        let text = path.to_string_lossy().into_owned();
        self.copy_to_clipboard(text, "path");
    }

    /// Reveal the active tab's file in the explorer.
    fn reveal_active_in_explorer(&mut self) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("reveal: no file".to_string());
            return;
        };
        self.reveal_in_explorer(&path);
    }

    /// Gather the repository/remote facts for `path`, synchronously (fast local
    /// reads on a short-lived repository handle, like blame). The `Err` side is a
    /// user-facing reason, doubling as a context-menu disabled note.
    fn remote_facts(&self, path: &Path) -> Result<RemoteFacts, String> {
        // Absolutize first so discovery starts from the file's own directory (a
        // file may live in a different repository than the workspace root).
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let start = abs.parent().unwrap_or(&abs);
        let repo = karet_vcs::Repository::discover(start)
            .map_err(|_| "not in a git repository".to_string())?;
        let origin = repo
            .origin_url()
            .ok_or_else(|| "no origin remote configured".to_string())?;
        let remote = remote::parse_remote(&origin)
            .ok_or_else(|| format!("unrecognized origin remote URL: {origin}"))?;
        let rel_path = repo
            .path_in_worktree(&abs)
            .ok_or_else(|| "file is outside the repository worktree".to_string())?;
        // An unborn branch has no HEAD hash; file_at_rev then errors, reading as
        // untracked — both surface as accurate notes further down.
        let head = repo.head_hash().ok().flatten();
        let branch = repo.current_branch().ok().flatten();
        let tracked = repo.file_at_rev(&abs, "HEAD").ok().flatten().is_some();
        Ok(RemoteFacts {
            remote,
            head,
            branch,
            rel_path,
            tracked,
        })
    }

    /// Copy the `kind` web link for the active file, or surface why it cannot be
    /// built (mirroring the pane menu's disabled notes exactly — both sides run
    /// the same [`remote::link`]).
    fn copy_remote_link(&mut self, kind: remote::LinkKind) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("copy link: no file".to_string());
            return;
        };
        // The caret line only anchors a permalink over a code tab (1-based).
        let line = match (kind, self.tabs.get(self.active)) {
            (remote::LinkKind::GithubPermalink, Some(tab))
                if matches!(tab.kind, TabKind::Code { .. }) =>
            {
                Some(tab.editor.cursor().line.saturating_add(1))
            },
            _ => None,
        };
        let facts = match self.remote_facts(&path) {
            Ok(facts) => facts,
            Err(reason) => {
                self.status = Some(reason);
                return;
            },
        };
        match remote::link(&facts.link_target(), kind, line) {
            Ok(url) => {
                let what = match kind {
                    remote::LinkKind::RemoteFile => "remote file URL",
                    remote::LinkKind::GithubPermalink => "GitHub permalink",
                    remote::LinkKind::GithubHeadLink => "GitHub head link",
                };
                self.copy_to_clipboard(url, what);
            },
            Err(reason) => self.status = Some(reason),
        }
    }

    /// The active tab's file path and, for a code tab, its live buffer text.
    fn active_file_and_text(&self) -> Option<(PathBuf, Option<String>)> {
        let tab = self.tabs.get(self.active)?;
        let path = tab.path()?.to_path_buf();
        let live = match &tab.kind {
            TabKind::Code { text, .. } => Some(text.clone()),
            _ => None,
        };
        Some((path, live))
    }

    /// Why the Open Changes actions do not apply to `path` — outside a repository,
    /// or untracked at `HEAD` (which also covers an unborn branch) — or `None` when
    /// they do. Doubles as the pane menu's disabled note.
    fn open_changes_note(&self, path: &Path) -> Option<String> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let start = abs.parent().unwrap_or(&abs);
        let Ok(repo) = karet_vcs::Repository::discover(start) else {
            return Some("not in a git repository".to_string());
        };
        if repo.file_at_rev(&abs, "HEAD").ok().flatten().is_none() {
            return Some("file is not tracked at HEAD".to_string());
        }
        None
    }

    /// Open a diff tab for the active file: old = its content at `rev`, new = the
    /// working text (the live buffer for a code tab, the file on disk otherwise).
    /// `label` names the old side in the tab title: `name (label ↔ working)`.
    fn open_changes_with(&mut self, rev: &str, label: &str) {
        let Some((path, live)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let start = abs.parent().unwrap_or(&abs);
        let repo = match karet_vcs::Repository::discover(start) {
            Ok(repo) => repo,
            Err(_) => {
                self.status = Some("open changes: not in a git repository".to_string());
                return;
            },
        };
        let old_bytes = match repo.file_at_rev(&abs, rev) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.status = Some(format!("open changes: file does not exist at {label}"));
                return;
            },
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
        let new_text = live.or_else(|| {
            std::fs::read(&abs)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
        });
        let old_text = String::from_utf8(old_bytes).ok();
        // Either side non-text marks the change binary (both texts then empty),
        // matching the FileChange::is_binary contract.
        let is_binary = old_text.is_none() || new_text.is_none();
        let name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let change = FileChange {
            path: abs,
            old_path: None,
            status: StatusKind::Modified,
            is_binary,
            old: if is_binary {
                String::new()
            } else {
                old_text.unwrap_or_default()
            },
            new: if is_binary {
                String::new()
            } else {
                new_text.unwrap_or_default()
            },
        };
        let file = FileView::new(change, Section::Working, self.syntax);
        self.push_tab(Tab::new(
            format!("{name} ({label} \u{2194} working)"),
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        ));
    }

    /// How many commits the With Revision picker lists at most.
    const OPEN_CHANGES_HISTORY_CAP: usize = 200;

    /// Open the diff-target picker over the active file's commit history
    /// (newest first, capped), for "Open Changes: With Revision…".
    fn open_changes_pick_revision(&mut self) {
        let Some((path, _)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let start = abs.parent().unwrap_or(&abs);
        let repo = match karet_vcs::Repository::discover(start) {
            Ok(repo) => repo,
            Err(_) => {
                self.status = Some("open changes: not in a git repository".to_string());
                return;
            },
        };
        let commits = match repo.file_history(&abs, 0, Self::OPEN_CHANGES_HISTORY_CAP) {
            Ok(commits) => commits,
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
        if commits.is_empty() {
            self.status = Some("open changes: no commits touch this file".to_string());
            return;
        }
        let items = commits
            .into_iter()
            .map(|c| {
                let display = format!(
                    "{} {} \u{2014} {}",
                    c.short_hash,
                    c.summary,
                    ui::relative_time(c.time)
                );
                let target = DiffTarget {
                    rev: c.hash,
                    label: c.short_hash,
                };
                (display, target)
            })
            .collect();
        self.overlay = Some(Overlay::diff_target("Open Changes: With Revision", items));
    }

    /// Open the diff-target picker over the repository's local branches, for
    /// "Open Changes: With Branch…".
    fn open_changes_pick_branch(&mut self) {
        let Some((path, _)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let start = abs.parent().unwrap_or(&abs);
        let repo = match karet_vcs::Repository::discover(start) {
            Ok(repo) => repo,
            Err(_) => {
                self.status = Some("open changes: not in a git repository".to_string());
                return;
            },
        };
        let branches = match repo.branches() {
            Ok(branches) => branches,
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
        if branches.is_empty() {
            self.status = Some("open changes: no branches".to_string());
            return;
        }
        let items = branches
            .into_iter()
            .map(|b| {
                let display = if b.is_head {
                    format!("{} (current)", b.name)
                } else {
                    b.name.clone()
                };
                let target = DiffTarget {
                    rev: b.name.clone(),
                    label: b.name,
                };
                (display, target)
            })
            .collect();
        self.overlay = Some(Overlay::diff_target("Open Changes: With Branch", items));
    }

    /// Apply a caret `motion` to the active code tab, extending the selection when
    /// `extend` is set and clearing it otherwise.
    fn caret_motion(&mut self, extend: bool, motion: impl Fn(&mut EditorState, &TextBuffer)) {
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
    fn editor_select_all(&mut self) {
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
    fn add_cursor_vertical(&mut self, above: bool) {
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
    fn add_cursor_next_occurrence(&mut self) {
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
    fn collapse_carets_or_unfocus(&mut self) {
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
    fn click_streak(&mut self, col: u16, row: u16) -> u8 {
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
    fn handle_editor_click(&mut self, mouse: MouseEvent) {
        let point = (mouse.column, mouse.row);
        // Route the click to the pane whose content it landed in, focusing it.
        let Some((pane, area)) = self
            .pane_frames
            .iter()
            .find(|f| rect_contains(f.content_rect, point))
            .map(|f| (f.pane, f.content_rect))
        else {
            return;
        };
        self.focus_pane_switch(pane);
        self.focus = Focus::Editor;
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
    fn drag_select_to(&mut self, col: u16, row: u16) {
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

    /// Transmit or clear the active tab's Kitty image after a frame is drawn.
    fn flush_graphics(&mut self) {
        if self.graphics != GraphicsProtocol::Kitty {
            return;
        }
        let mut stdout = io::stdout();
        // Transmitting a rasterized image/PDF page needs a raster branch compiled in
        // (`images`/`pdf`); the graphical text caret below is independent of it.
        #[cfg(any(feature = "images", feature = "pdf"))]
        {
            // The image, if any, belongs to the focused pane's active tab (keyed by
            // its stable ViewId so a focus switch re-transmits correctly). Documents
            // also key on the current page so paging re-transmits under an unchanged
            // ViewId.
            let current = self.tabs.get(self.active).map(|t| t.view);
            let current_page = match self.tabs.get(self.active).map(|t| &t.kind) {
                #[cfg(feature = "pdf")]
                Some(TabKind::Document { page, .. }) => *page,
                _ => 0,
            };
            // The pixels live directly on an image tab, or in a document's page cache.
            let image = match self.tabs.get(self.active).map(|t| &t.kind) {
                #[cfg(feature = "images")]
                Some(TabKind::Image { image, .. }) => Some(image),
                #[cfg(feature = "pdf")]
                Some(TabKind::Document {
                    rendered: Some((_, image)),
                    ..
                }) => Some(image),
                _ => None,
            };
            match self.image_area {
                Some(area) if self.shown_image != current || self.shown_page != current_page => {
                    let _ = write!(stdout, "{}", image::kitty_delete_all());
                    let _ = write!(stdout, "\x1b[{};{}H", area.y + 1, area.x + 1);
                    if let Some(image) = image {
                        let _ = write!(stdout, "{}", image.kitty_escape(area.width, area.height));
                    }
                    let _ = stdout.flush();
                    self.shown_image = current;
                    self.shown_page = current_page;
                },
                None if self.shown_image.is_some() => {
                    let _ = write!(stdout, "{}", image::kitty_delete_all());
                    let _ = stdout.flush();
                    self.shown_image = None;
                },
                _ => {},
            }
        }

        let caret = self.active_graphics_caret();
        match (caret, self.shown_graphics_caret) {
            (Some(next), shown) if shown != Some(next) => {
                let _ = write!(stdout, "{}", next.escape());
                let _ = stdout.flush();
                self.shown_graphics_caret = Some(next);
            },
            (None, Some(_)) => {
                let _ = write!(stdout, "{}", compat::delete_graphics_caret());
                let _ = stdout.flush();
                self.shown_graphics_caret = None;
            },
            _ => {},
        }
    }

    fn active_graphics_caret(&self) -> Option<GraphicsCaret> {
        if !self.graphics_caret_visible(Instant::now()) {
            return None;
        }
        self.active_graphics_caret_position()
    }

    fn active_graphics_caret_position(&self) -> Option<GraphicsCaret> {
        if !self.graphical_cursor_enabled() || self.focus != Focus::Editor {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
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
        let (x, y) = tab
            .editor
            .primary_caret_cell(self.editor_rect, buffer, &fold_lines)?;
        Some(GraphicsCaret { x, y })
    }

    fn graphics_caret_visible(&self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.graphics_caret_blink_epoch);
        let phase = elapsed.as_millis() / GRAPHICS_CARET_BLINK_INTERVAL.as_millis();
        phase.is_multiple_of(2)
    }

    fn graphics_caret_next_wake(&self, now: Instant) -> Option<Duration> {
        self.active_graphics_caret_position()?;
        let elapsed = now.saturating_duration_since(self.graphics_caret_blink_epoch);
        let interval_ms = GRAPHICS_CARET_BLINK_INTERVAL.as_millis();
        let elapsed_ms = elapsed.as_millis();
        let remaining_ms = interval_ms - (elapsed_ms % interval_ms);
        Some(Duration::from_millis(remaining_ms as u64))
    }

    fn reset_graphics_caret_blink(&mut self) {
        self.graphics_caret_blink_epoch = Instant::now();
    }
}

/// Editing: route edits through the headless session backend and reflect its
/// snapshots back into the active code tab.
impl App {
    /// Register every already-open code tab with the session (called once the
    /// backend is attached at startup).
    fn register_open_tabs(&mut self) {
        for idx in 0..self.tabs.len() {
            self.register_doc(idx);
        }
    }

    /// Register the code tab at `idx` with the session so it can be edited, if it is
    /// an as-yet-unregistered code tab and a backend is attached.
    fn register_doc(&mut self, idx: usize) {
        let path = match self.tabs.get(idx) {
            Some(Tab {
                kind: TabKind::Code {
                    path, doc: None, ..
                },
                ..
            }) => path.clone(),
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
        self.pending_open.insert(id, path);
    }

    /// Build an edit from the active code tab's caret/selection via `build` and
    /// submit it through the session, moving the caret optimistically.
    fn submit_edit<F>(&mut self, build: F)
    where
        F: Fn(LineCol, Option<Range>, &TextBuffer, u64) -> Option<editing::Edit>,
    {
        self.submit_edit_with_cause(EditCause::Replace, build);
    }

    fn submit_edit_with_cause<F>(&mut self, cause: EditCause, build: F)
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
            }
            editor.set_carets(&carets);
            let head = editor.cursor();
            editor.scroll_to(head);
        }
    }

    /// The active tab's session document, if it is a registered code tab.
    fn active_code_doc(&self) -> Option<DocumentId> {
        match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { doc: Some(doc), .. },
                ..
            }) => Some(*doc),
            _ => None,
        }
    }

    /// The active tab's find-in-file state, if any (find-in-file lives per tab so
    /// it survives closing the bar, but not closing the tab).
    fn active_find(&self) -> Option<&FindState> {
        self.tabs.get(self.active)?.find.as_ref()
    }

    /// A mutable handle to the active tab's find-in-file state.
    fn active_find_mut(&mut self) -> Option<&mut FindState> {
        self.tabs.get_mut(self.active)?.find.as_mut()
    }

    /// Send a document command for the active code tab, if any.
    fn send_doc_command(&mut self, make: impl FnOnce(DocumentId) -> SessionCommand) {
        let Some(doc) = self.active_code_doc() else {
            return;
        };
        let result = self.backend.as_ref().map(|backend| {
            let id = backend.next_id();
            backend.send(id, make(doc))
        });
        if let Some(Err(e)) = result {
            self.notify_backend_error(e);
        }
    }

    /// Handle a quit request through the unified close guard.
    fn request_quit(&mut self) {
        self.guarded_close(CloseRequest::Quit);
    }

    /// The stable view ids of the tabs `request` would drop. Tab/pane closes act on
    /// the focused pane only (mirroring the raw close operations); Quit drops every
    /// tab across every pane.
    fn removed_tab_views(&self, request: CloseRequest) -> Vec<ViewId> {
        match request {
            CloseRequest::Quit => self.all_tabs().map(|tab| tab.view).collect(),
            CloseRequest::Tab { view } => vec![view],
            CloseRequest::OtherTabs => self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != self.active)
                .map(|(_, tab)| tab.view)
                .collect(),
            CloseRequest::TabsToRight => self
                .tabs
                .iter()
                .skip(self.active + 1)
                .map(|tab| tab.view)
                .collect(),
            CloseRequest::AllTabs => self.tabs.iter().map(|tab| tab.view).collect(),
        }
    }

    /// The documents `request` would irreversibly lose: the dirty documents whose
    /// **last** referencing view is being dropped. A dirty document still shown in a
    /// surviving tab or another pane (previews count as references, like
    /// [`reconcile_open_docs`](Self::reconcile_open_docs)) is not at risk, so closing
    /// one of its several views must not prompt.
    fn docs_at_risk(&self, request: CloseRequest) -> Vec<DocumentId> {
        let removed: HashSet<ViewId> = self.removed_tab_views(request).into_iter().collect();
        let surviving: HashSet<DocumentId> = self
            .all_tabs()
            .filter(|tab| !removed.contains(&tab.view))
            .filter_map(Self::tab_doc)
            .collect();
        let mut at_risk: Vec<DocumentId> = Vec::new();
        for tab in self.all_tabs().filter(|tab| removed.contains(&tab.view)) {
            let Some(doc) = Self::tab_doc(tab) else {
                continue;
            };
            if surviving.contains(&doc) || at_risk.contains(&doc) {
                continue;
            }
            // The document is fully dropped by this request; prompt only if it is
            // dirty (checked across every view, so per-tab flag skew can't hide it).
            if self
                .all_tabs()
                .any(|t| Self::tab_doc(t) == Some(doc) && t.dirty)
            {
                at_risk.push(doc);
            }
        }
        at_risk
    }

    /// Route an irreversible close through the unified unsaved-changes guard. When it
    /// would drop the last view of one or more dirty documents it arms the
    /// confirmation prompt (default: abort); otherwise it runs immediately.
    ///
    /// Quit additionally honors `files.confirmOnExit`; tab/pane closes are always
    /// guarded — silently discarding unsaved changes is the data-loss bug this fixes.
    fn guarded_close(&mut self, request: CloseRequest) {
        let at_risk = self.docs_at_risk(request);
        let honor_setting =
            !matches!(request, CloseRequest::Quit) || self.settings.files.confirm_on_exit;
        if at_risk.is_empty() || !honor_setting {
            self.execute_close(request);
        } else {
            self.pending_close = Some(request);
            self.status = Some(close_prompt_message(request, at_risk.len()));
        }
    }

    /// Run a confirmed (or unguarded) close, re-resolving a single-tab request by its
    /// view id so a save-then-close that shifted the tab list still closes the right
    /// tab (and harmlessly no-ops if it has since vanished).
    fn execute_close(&mut self, request: CloseRequest) {
        match request {
            CloseRequest::Quit => self.should_quit = true,
            CloseRequest::Tab { view } => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.view == view) {
                    self.close_tab_at(index);
                }
            },
            CloseRequest::OtherTabs => self.close_other_tabs(),
            CloseRequest::TabsToRight => self.close_tabs_to_right(),
            CloseRequest::AllTabs => self.close_all_tabs(),
        }
    }

    /// At the close prompt: save exactly the at-risk documents, then run the parked
    /// request once those saves drain (see [`App::on_backend_event`]). Runs
    /// immediately if nothing needed saving.
    fn close_save(&mut self) {
        let Some(request) = self.pending_close.take() else {
            return;
        };
        let at_risk = self.docs_at_risk(request);
        let saved = self.save_docs(&at_risk);
        if saved == 0 {
            self.execute_close(request);
        } else {
            self.saving_close = Some(request);
            let verb = if matches!(request, CloseRequest::Quit) {
                "quitting"
            } else {
                "closing"
            };
            self.status = Some(format!("saving {saved} file(s) before {verb}…"));
        }
    }

    /// At the close prompt: discard unsaved changes and run the parked request now.
    fn close_discard(&mut self) {
        if let Some(request) = self.pending_close.take() {
            self.execute_close(request);
        }
    }

    /// At the close prompt: an unbound key aborts, leaving every tab untouched.
    fn cancel_close(&mut self) {
        let quitting = matches!(self.pending_close, Some(CloseRequest::Quit));
        self.pending_close = None;
        self.status = Some(if quitting {
            "quit cancelled".to_string()
        } else {
            "close cancelled".to_string()
        });
    }

    /// Issue a save for each of `docs` (skipping any already in flight), tracking it
    /// in `pending_saves` and marking its tabs as saving. Returns the number issued.
    fn save_docs(&mut self, docs: &[DocumentId]) -> usize {
        let Some(backend) = self.backend.clone() else {
            return 0;
        };
        let now = Instant::now();
        let mut issued = 0;
        for &doc in docs {
            if self.pending_saves.values().any(|pending| *pending == doc) {
                continue;
            }
            let id = backend.next_id();
            match backend.send(id, SessionCommand::Save { doc }) {
                Ok(()) => {
                    self.pending_saves.insert(id, doc);
                    issued += 1;
                    for tab in self.all_tabs_mut() {
                        if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                            tab.saving_since = Some(now);
                        }
                    }
                },
                Err(e) => self.notify_backend_error(e),
            }
        }
        issued
    }

    /// Save the active document, or report that there is no file to save. Tracks the
    /// in-flight save so a slow write shows a spinner in the tab.
    fn save_active(&mut self) {
        let Some(doc) = self.active_code_doc() else {
            self.status = Some("save: open a text file".to_string());
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        if self.pending_saves.values().any(|pending| *pending == doc) {
            self.status = Some("save already in progress".to_string());
            return;
        }
        let id = backend.next_id();
        match backend.send(id, SessionCommand::Save { doc }) {
            Ok(()) => {
                self.pending_saves.insert(id, doc);
                let now = Instant::now();
                for tab in self.all_tabs_mut() {
                    if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                        tab.saving_since = Some(now);
                    }
                }
            },
            Err(e) => self.notify_backend_error(e),
        }
    }

    /// Cut the current selection (copy then delete); a no-op without a selection.
    fn cut(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_cut_files();
            return;
        }
        let has_selection = matches!(
            self.tabs.get(self.active),
            Some(Tab { kind: TabKind::Code { .. }, editor, .. })
                if editor.selection_range().is_some_and(|r| !r.is_empty())
        );
        if !has_selection {
            return;
        }
        self.copy_selection();
        self.submit_edit_with_cause(EditCause::Cut, editing::backspace);
    }

    /// Paste the system clipboard at the caret (or the active modal's text field).
    fn paste_from_clipboard(&mut self) {
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_paste_files();
            return;
        }
        match self.clipboard.get() {
            Ok(text) => self.handle_paste(text),
            Err(_) => self.status = Some("paste: clipboard unavailable".to_string()),
        }
    }

    /// Route pasted text (from the paste command or bracketed paste) to whatever
    /// actually owns text input right now: the active modal's field if one is
    /// open, else the editor buffer. Shared by both paste sources, so pasted text
    /// is never interpreted as keys and never lands in the wrong place.
    fn handle_paste(&mut self, text: String) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return;
        }
        if let Some(modal) = self.input_context().modal {
            self.modal_paste(modal, &normalized);
            return;
        }
        self.submit_edit_with_cause(EditCause::Paste, move |caret, sel, _b, base| {
            Some(editing::insert(caret, sel, base, &normalized))
        });
    }

    /// The soonest the event loop should wake for time-based UI: notification expiry,
    /// save-spinner animation, graphical-caret blink, delayed loading states, or an
    /// expiring hover reveal.
    /// `None` when the loop can park on its event sources alone.
    fn next_wake(&self) -> Option<Duration> {
        let now = Instant::now();
        let notif = self.notifications.next_deadline(now);
        let spinner = (!self.pending_saves.is_empty()).then(|| Duration::from_millis(100));
        let caret = self.graphics_caret_next_wake(now);
        let loading = self.loading_reveal_wake(now);
        // Wake to repaint (hiding the tooltip) when the commit-badge reveal expires.
        let reveal = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Commit {
                explain_since: Some(since),
                ..
            }) => COMMIT_REVEAL.checked_sub(since.elapsed()),
            _ => None,
        };
        [notif, spinner, caret, loading, reveal]
            .into_iter()
            .flatten()
            .min()
    }

    fn loading_reveal_wake(&self, now: Instant) -> Option<Duration> {
        let sidebar = (self.sidebar_visible && self.sidebar_panel == SidebarPanel::SourceControl)
            .then_some(self.scm.log_loading_since)
            .flatten()
            .and_then(|since| loading_delay_remaining(since, now));
        let tabs = self.all_tabs().filter_map(|tab| match &tab.kind {
            TabKind::CommitLoading {
                loading_since,
                error,
                ..
            } => error
                .is_none()
                .then(|| loading_delay_remaining(*loading_since, now))
                .flatten(),
            TabKind::Commit {
                files_loading_since,
                ..
            } => files_loading_since.and_then(|since| loading_delay_remaining(since, now)),
            TabKind::CommitGraph {
                loading_since,
                detail_loading_since,
                files_loading_since,
                ..
            } => [
                loading_since.and_then(|since| loading_delay_remaining(since, now)),
                detail_loading_since.and_then(|since| loading_delay_remaining(since, now)),
                files_loading_since.and_then(|since| loading_delay_remaining(since, now)),
            ]
            .into_iter()
            .flatten()
            .min(),
            _ => None,
        });
        std::iter::once(sidebar).flatten().chain(tabs).min()
    }

    /// Push a notification onto the center. Errors and warnings persist until
    /// dismissed; info and success auto-expire after a few seconds.
    fn notify(&mut self, severity: Severity, kind: NotificationKind, title: impl Into<String>) {
        let timeout = match severity {
            Severity::Error | Severity::Warning => None,
            // Info, success (Hint), and any future severity auto-dismiss.
            _ => Some(Duration::from_secs(4)),
        };
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity,
                kind,
                title: title.into(),
                body: None,
                tag: None,
                timeout,
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Surface a dropped backend-submission error as a persistent notification, so a
    /// closed or wedged backend never fails silently.
    fn notify_backend_error(&mut self, error: BackendError) {
        self.notify(
            Severity::Error,
            NotificationKind::System,
            format!("backend: {error}"),
        );
    }

    /// Handle a backend event: correlate opens to tabs, surface save/progress status.
    fn on_backend_event(&mut self, id: Option<RequestId>, event: SessionEvent) {
        // A save's answering event clears its tab spinner. During "save all & quit",
        // only successful Saved responses may let the quit continue; a refused or
        // failed save keeps the app open with the dirty buffer intact.
        let mut save_failed = false;
        if let Some(req) = id
            && let Some(doc) = self.pending_saves.remove(&req)
        {
            save_failed = !matches!(event, SessionEvent::Saved { doc: saved } if saved == doc);
            for tab in self.all_tabs_mut() {
                if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                    tab.saving_since = None;
                }
            }
        }
        if save_failed && let Some(request) = self.saving_close.take() {
            let verb = if matches!(request, CloseRequest::Quit) {
                "quit"
            } else {
                "close"
            };
            self.status = Some(format!("{verb} cancelled: save failed"));
        }
        match event {
            SessionEvent::Opened { doc, .. } => {
                self.open_docs.insert(doc);
                if let Some(req) = id
                    && let Some(path) = self.pending_open.remove(&req)
                {
                    for tab in self.all_tabs_mut() {
                        // A preview opened before its source registered a document binds
                        // here too, by the path the two share.
                        let bound = match &mut tab.kind {
                            TabKind::Code {
                                path: p, doc: d, ..
                            }
                            | TabKind::MarkdownPreview {
                                path: p, doc: d, ..
                            } => Some((p, d)),
                            _ => None,
                        };
                        if let Some((p, d)) = bound
                            && d.is_none()
                            && *p == path
                        {
                            *d = Some(doc);
                        }
                    }
                }
            },
            SessionEvent::Completions {
                doc,
                version,
                items,
            } => self.on_completions(id, doc, version, items),
            SessionEvent::Saved { doc } => {
                for tab in self.all_tabs_mut() {
                    if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                        tab.dirty = false;
                    }
                }
                self.status = Some("saved".to_string());
            },
            // The fresh content arrives via the snapshot stream; just note it.
            SessionEvent::Reloaded { .. } => {
                self.notify(
                    Severity::Information,
                    NotificationKind::Io,
                    "reloaded from disk",
                );
            },
            // A persistent warning: a transient status hint would vanish on the next
            // keystroke, but an unsaved-vs-disk conflict must not be missed.
            SessionEvent::ExternalConflict { .. } => {
                self.notify(
                    Severity::Warning,
                    NotificationKind::Io,
                    "file changed on disk — you have unsaved changes",
                );
            },
            // Full non-UTF-8 editing isn't supported: the tab requested a document
            // that will never arrive (no `Opened` follows), so leaving it as a
            // `doc: None` code tab would make every keystroke silently no-op. Fall
            // back to the same read-only hex view a corrupt CBOR file already uses.
            SessionEvent::NotUtf8 { path } => {
                if let Some(req) = id {
                    self.pending_open.remove(&req);
                }
                for tab in self.all_tabs_mut() {
                    let is_pending_for_path =
                        matches!(&tab.kind, TabKind::Code { path: p, doc: None, .. } if *p == path);
                    if is_pending_for_path && let Ok(bytes) = std::fs::read(&path) {
                        tab.kind = TabKind::Hex {
                            path: path.clone(),
                            bytes,
                            scroll: 0,
                        };
                    }
                }
                self.notify(
                    Severity::Warning,
                    NotificationKind::Io,
                    format!("opened {} read-only: not valid UTF-8", path.display()),
                );
            },
            // Keep a live workspace search current: re-run it (which also
            // refreshes open-pane highlights) whenever something changes on
            // disk. No extra debouncing needed here — the watcher already
            // debounces at the source, and the result cap keeps a re-run cheap.
            SessionEvent::FsChanged { .. } => {
                if !self.search.query.is_empty() {
                    self.run_global_search();
                }
            },
            SessionEvent::ConfigChanged { report } => {
                let report = *report;
                self.apply_loaded_config(report.clone(), false);
                for tab in self.all_tabs_mut() {
                    if let TabKind::LoadedConfig {
                        report: open_report,
                        ..
                    } = &mut tab.kind
                    {
                        *open_report = report.clone();
                    }
                }
                for diag in std::mem::take(&mut self.config_diagnostics) {
                    self.notify(
                        diag.severity,
                        NotificationKind::System,
                        format!("config: {}", diag.message),
                    );
                }
                let graphical_cursor_requested = self.tabs.get(self.active).is_some_and(|tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .graphical_cursor()
                        == Some(true)
                });
                if graphical_cursor_requested && !self.graphical_cursor_compatible() {
                    self.notify(
                        Severity::Error,
                        NotificationKind::System,
                        "graphical cursor is not compatible with this terminal",
                    );
                }
                let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .completion()
                        .enabled()
                });
                if !completion_enabled {
                    self.dismiss_completion();
                }
            },
            SessionEvent::Progress { message, .. } => self.status = Some(message),
            // The single high-up funnel: every backend-reported condition becomes a
            // notification, so nothing is silently dropped.
            SessionEvent::Notification {
                severity,
                kind,
                message,
            } => {
                if let Some(req) = id {
                    self.fail_pending_commit_detail(req, &message);
                }
                self.notify(severity, kind, message);
            },
            SessionEvent::VcsStatus { staged, working } => self.apply_vcs_status(staged, working),
            SessionEvent::VcsLog {
                skip,
                commits,
                has_more,
            } => {
                // A page requested by the graph browser fills it; anything else is the
                // sidebar log.
                if id.is_some() && id == self.graph_log_req {
                    self.graph_log_req = None;
                    self.apply_graph_log(skip, commits, has_more);
                } else {
                    self.apply_vcs_log(skip, commits, has_more);
                }
            },
            SessionEvent::FileHistory {
                skip,
                commits,
                has_more,
                ..
            } => {
                // File history only ever fills the graph browser it was opened for.
                if id.is_some() && id == self.graph_log_req {
                    self.graph_log_req = None;
                    self.apply_graph_log(skip, commits, has_more);
                }
            },
            SessionEvent::VcsCommitsPrepended { commits } => {
                self.apply_vcs_commits_prepended(commits);
            },
            SessionEvent::Committed { oid } => {
                self.commit_input = None;
                let short: String = oid.chars().take(7).collect();
                self.notify(
                    Severity::Information,
                    NotificationKind::Vcs,
                    format!("committed {short}"),
                );
            },
            SessionEvent::CommitMessageGenerated { message } => {
                // Only adopt it if the commit input is still open (the user may have
                // cancelled while the generator ran).
                if self.commit_input.is_some() {
                    self.commit_input = Some(message);
                    self.status = Some("commit message generated".to_string());
                }
            },
            SessionEvent::SwapsFound { swaps } => self.arm_swap_recovery(swaps),
            SessionEvent::CommitDetailReady { detail } => {
                let dest = id.and_then(|i| self.pending_commit_detail.get(&i).cloned());
                match dest {
                    Some(CommitDest::Browser { hash }) if detail.hash == hash => {
                        self.fill_graph_metadata(detail);
                    },
                    Some(CommitDest::Browser { .. }) => {},
                    Some(CommitDest::Tab { view }) => self.fill_commit_metadata(view, detail),
                    _ => self.open_commit_metadata_tab(detail),
                }
            },
            SessionEvent::CommitReady { detail, changes } => {
                match id.and_then(|i| self.pending_commit_detail.remove(&i)) {
                    Some(CommitDest::Browser { hash }) if detail.hash == hash => {
                        self.fill_graph_detail(detail, changes);
                    },
                    Some(CommitDest::Browser { .. }) => {},
                    Some(CommitDest::Tab { view }) => self.fill_commit_tab(view, detail, changes),
                    _ => self.open_commit_tab(detail, changes),
                }
            },
            SessionEvent::RangeReady {
                base_label,
                head_label,
                merge_base,
                changes,
            } => self.open_compare_tab(base_label, head_label, merge_base, changes),
            SessionEvent::CommitVerification { hash, status } => {
                self.apply_commit_verification(&hash, status);
            },
            SessionEvent::GraphReady { title, view, .. } => {
                let count = view.nodes.len();
                self.push_tab(Tab::graph(title, view));
                self.status = Some(format!("dependency graph: {count} package(s)"));
            },
            SessionEvent::LoadedConfig { report } => self.open_loaded_config(*report),
            _ => {},
        }
        // A "save & close" runs the parked request once every issued save succeeds.
        if self.saving_close.is_some()
            && self.pending_saves.is_empty()
            && let Some(request) = self.saving_close.take()
        {
            self.execute_close(request);
        }
    }

    fn open_loaded_config(&mut self, report: LoadedConfig) {
        self.push_tab(Tab::loaded_config(report));
        self.status = Some("loaded settings opened".to_string());
    }

    /// Arm the startup crash-recovery prompt for `swaps` left by a previous session.
    fn arm_swap_recovery(&mut self, swaps: Vec<SwapInfo>) {
        if swaps.is_empty() {
            return;
        }
        let conflicts = swaps.iter().filter(|s| s.conflict).count();
        let suffix = if conflicts > 0 {
            format!(" ({conflicts} changed on disk)")
        } else {
            String::new()
        };
        self.status = Some(format!(
            "recovered {} unsaved file(s) from a previous session{suffix} — \
             press r to recover, d to discard, any other key to dismiss",
            swaps.len()
        ));
        self.pending_swaps = Some(swaps);
    }

    /// Apply a document snapshot to the matching code tab(s): the snapshot is the
    /// render source of truth (buffer, highlights, the search text, and the
    /// unsaved-changes flag).
    fn on_snapshot(&mut self, doc: DocumentId, snap: &DocSnapshot) {
        for tab in self.all_tabs_mut() {
            // A preview mirrors the same document: refresh its buffer and let the next
            // draw notice the version moved and re-render. Nothing else on a preview tab
            // (dirty, cursor, folds) is meaningful.
            if let TabKind::MarkdownPreview {
                doc: Some(d),
                buffer,
                ..
            } = &mut tab.kind
                && *d == doc
                && snap.version >= buffer.version()
            {
                *buffer = snap.buffer.clone();
                continue;
            }
            let matches = matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
            if !matches {
                continue;
            }
            if let TabKind::Code {
                buffer,
                highlights,
                semantic_blocks,
                folds,
                folded,
                text,
                next_version,
                syntax_errors,
                ..
            } = &mut tab.kind
            {
                // A slow-arriving snapshot must not regress a tab that has since
                // advanced further via `submit_edit`'s local speculative apply —
                // only the buffer/text catch up when the snapshot is at least as
                // new as what's already applied locally.
                if snap.version >= buffer.version() {
                    *buffer = snap.buffer.clone();
                    *text = snap.buffer.text();
                }
                *highlights = (*snap.highlights).clone();
                *semantic_blocks = (*snap.semantic_blocks).clone();
                *folds = (*snap.folds).clone();
                *syntax_errors = snap.syntax_error_lines.as_ref().clone();
                *next_version = (*next_version).max(snap.version);
                // Drop collapsed markers whose fold no longer starts where it did (an
                // edit shifted or removed it), so stale hidden lines can't linger.
                let starts: HashSet<u32> = folds.regions().iter().map(|r| r.start).collect();
                folded.retain(|line| starts.contains(line));
            }
            // The clean→dirty transition permanently promotes a preview tab (VS
            // Code behavior): once edited, it survives being navigated away from
            // instead of getting silently replaced by the next preview-opened file.
            if snap.dirty && !tab.dirty {
                tab.is_preview = false;
            }
            tab.dirty = snap.dirty;
            // Undo/redo snapshots carry the caret to jump to; ordinary edits carry
            // `None` so the optimistic placement from `submit_edit` is preserved.
            if let Some(cursor) = &snap.cursor {
                let heads: Vec<LineCol> = cursor.selections.iter().map(|s| s.head).collect();
                if !heads.is_empty() {
                    tab.editor.set_carets(&heads);
                    tab.editor.scroll_to(cursor.primary().head);
                }
            }
        }
        // If the find bar is open, an edit (e.g. a replace) just changed the buffer,
        // so recompute the match highlights against the fresh text.
        if self.find_open {
            self.run_find();
        }
        // Likewise for global search matches: a newly-opened or just-edited tab
        // should show its highlights immediately, not only after the next
        // explicit search re-run.
        if !self.search.query.is_empty() {
            self.refresh_search_decorations();
        }
        // An undo/redo snapshot may have moved the caret away from the popup's
        // anchor; re-validate it.
        self.reconcile_completion();
    }
}

/// Resolve a snapshot's fold regions plus the view's collapsed set into the
/// line-based [`Fold`]s the editor renders and hit-tests against.
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
        | TabKind::Placeholder { path: p, .. }
        | TabKind::Blame { path: p, .. } => Some(p),
        #[cfg(feature = "images")]
        TabKind::Image { path: p, .. } => Some(p),
        #[cfg(feature = "pdf")]
        TabKind::Document { path: p, .. } => Some(p),
        _ => None,
    };
    if let Some(p) = target {
        *p = path.to_path_buf();
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

pub fn run(mut app: App) -> color_eyre::Result<()> {
    let kitty_keyboard_supported = crate::term_caps::supports_kitty_keyboard();
    if !kitty_keyboard_supported {
        return Err(eyre!(
            "karet requires a terminal with kitty keyboard protocol support \
             (kitty, ghostty, WezTerm, foot, …)"
        ));
    }
    app.kitty_keyboard_supported = true;

    // The session backend runs on its own Tokio runtime; the UI task selects over
    // terminal input, backend events, and document snapshots so it never blocks.
    let runtime = tokio::runtime::Runtime::new().map_err(|e| eyre!("tokio runtime: {e}"))?;
    let (session, events, snaps) = Session::new(SessionConfig {
        roots: vec![app.root.clone()],
        settings: app.settings.clone(),
        loaded_config: app.loaded_config.clone(),
        // The real app persists crash-recovery swaps to the user data directory;
        // headless/test sessions leave this unset and keep no backups.
        swap_dir: karet_session::backup::default_swap_dir(),
    });

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        io::stdout(),
        SetTitle(format!("karet - {}", app.root.display()))
    );
    let _keyboard = {
        let _ = crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        );
        KeyboardEnhancementGuard
    };
    // Bracketed paste makes a multi-line paste arrive as one `Event::Paste`, never a
    // storm of keystrokes the keymap would misinterpret.
    let _ = crossterm::execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste);

    // Refine the env-var graphics heuristic with a real handshake (raw mode is on and
    // the input reader thread has not started yet, so we can read the reply here).
    // Upgrade to Kitty when the terminal actually answers; never downgrade a terminal
    // the heuristic already trusts.
    if crate::term_caps::probe_kitty_graphics(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.graphics = GraphicsProtocol::Kitty;
        app.kitty_graphics_supported = true;
    }
    // Same handshake for OSC 22 pointer-shape hints (col-resize/row-resize over
    // the sidebar/SCM dividers) — confirmed support only, never assumed.
    if crate::term_caps::probe_osc22_pointer_shape(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.pointer_shapes_supported = true;
    }

    let result = runtime.block_on(async move {
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        app.backend = Some(backend);
        app.register_open_tabs();
        // Surface any configuration-load problems as startup notifications, now that
        // the notification center will render on the first frame.
        for diag in std::mem::take(&mut app.config_diagnostics) {
            app.notify(
                diag.severity,
                NotificationKind::System,
                format!("config: {}", diag.message),
            );
        }
        let graphical_cursor_requested = app.tabs.get(app.active).is_some_and(|tab| {
            app.settings
                .editor
                .for_language(tab_language(tab))
                .graphical_cursor()
                == Some(true)
        });
        if graphical_cursor_requested && !app.graphical_cursor_compatible() {
            app.notify(
                Severity::Error,
                NotificationKind::System,
                "graphical cursor is not compatible with this terminal",
            );
        }
        event_loop(&mut terminal, &mut app, events, snaps).await
    });

    let _ = write!(io::stdout(), "{}", image::kitty_delete_all());
    let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    drop(_keyboard);
    ratatui::restore();
    result
}

/// The async UI loop: render, then wake on terminal input, a backend event, or a
/// document snapshot — coalescing each burst into a single repaint.
async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut events: EventRx,
    mut snaps: SnapshotRx,
) -> color_eyre::Result<()> {
    // A dedicated thread turns the blocking `event::read` into an async stream.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if input_tx.send(event).is_err() {
                break;
            }
        }
    });

    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.flush_graphics();

        // Wake for notification expiry or a save-spinner frame; park on the event
        // sources when nothing time-based is pending (no idle repaints).
        let deadline = app.next_wake();

        tokio::select! {
            biased;
            input = input_rx.recv() => match input {
                Some(event) => handle_terminal_event(app, event),
                None => app.should_quit = true,
            },
            event = events.recv() => if let Some((id, ev)) = event {
                app.on_backend_event(id, ev);
            },
            snap = snaps.recv() => if let Some((doc, snap)) = snap {
                app.on_snapshot(doc, &snap);
            },
            () = async move {
                match deadline {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {},
        }
        app.notifications.expire(Instant::now());

        // Drain everything else that is ready so a burst collapses into one frame.
        while let Ok(event) = input_rx.try_recv() {
            handle_terminal_event(app, event);
            if app.should_quit {
                break;
            }
        }
        while let Some((id, ev)) = events.try_recv() {
            app.on_backend_event(id, ev);
        }
        while let Some((doc, snap)) = snaps.try_recv() {
            app.on_snapshot(doc, &snap);
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Dispatch one terminal event to the app.
fn handle_terminal_event(app: &mut App, event: Event) {
    app.reset_graphics_caret_blink();
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Paste(text) => app.handle_paste(text),
        _ => {},
    }
}

/// Completion UI (issue #57): triggers, the popup key layer, accept, and the
/// stale-answer bookkeeping. Pure logic lives in [`crate::completion`]; the
/// popup itself is `karet_widgets::completion`. The app talks only through the
/// session seam (`Command::Completion` → `Event::Completions`).
impl App {
    /// The active code tab's completion target: `(document, caret)`.
    fn completion_target(&self) -> Option<(DocumentId, LineCol)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { doc: Some(doc), .. } = &tab.kind else {
            return None;
        };
        Some((*doc, tab.editor.cursor()))
    }

    /// Request completions at the caret. `manual` (Ctrl+Space) bypasses the
    /// syntax-error gate; automatic triggers hold off while the caret's line
    /// has an outright parse error (per issue #57).
    pub(crate) fn trigger_completion(&mut self, manual: bool) {
        let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .completion()
                .enabled()
        });
        if !completion_enabled {
            return;
        }
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let Some((doc, caret)) = self.completion_target() else {
            return;
        };
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            syntax_errors,
            ..
        } = &tab.kind
        else {
            return;
        };
        if !manual && crate::completion::line_has_syntax_error(syntax_errors, caret.line) {
            return; // the line doesn't parse yet: suggesting now is noise
        }
        let (anchor, _) = karet_editor::word_bounds(buffer, caret);
        let id = backend.next_id();
        if backend
            .send(
                id,
                SessionCommand::Completion {
                    doc,
                    position: caret,
                },
            )
            .is_ok()
        {
            self.pending_completion =
                Some(crate::completion::PendingCompletion { id, doc, anchor });
        }
    }

    /// Auto-trigger after typing `c`: identifier characters open the popup (a
    /// one-character prefix suffices), `.` and the second `:` of `::`
    /// re-request at the new completion boundary, anything else does nothing.
    fn maybe_auto_complete(&mut self, c: char) {
        let completion = self.tabs.get(self.active).map(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .completion()
        });
        if !completion.is_some_and(|completion| completion.enabled() && completion.auto_trigger()) {
            return;
        }
        let boundary = c == crate::completion::TRIGGER_DOT
            || (c == crate::completion::TRIGGER_COLON && self.typed_second_colon());
        if boundary {
            self.trigger_completion(false);
            return;
        }
        if self.completion.is_some() {
            return; // already open: typing narrows the filter client-side
        }
        if crate::completion::is_word_char(c) {
            self.trigger_completion(false);
        }
    }

    /// After typing `:`, whether it completed a `::` path separator.
    fn typed_second_colon(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return false;
        };
        let caret = tab.editor.cursor();
        buffer.line(caret.line as usize).is_some_and(|line| {
            let chars: Vec<char> = line.chars().collect();
            let i = caret.col as usize;
            i >= 2 && chars.get(i - 1) == Some(&':') && chars.get(i - 2) == Some(&':')
        })
    }

    /// The live filter: the text typed between the popup's anchor and the
    /// caret. `None` when the popup no longer applies to the active view.
    pub(crate) fn completion_filter(&self) -> Option<String> {
        let ui = self.completion.as_ref()?;
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        if *doc != ui.doc {
            return None;
        }
        let caret = tab.editor.cursor();
        if !crate::completion::caret_still_anchored(ui.anchor, caret) {
            return None;
        }
        let line = buffer.line(ui.anchor.line as usize)?;
        let chars: Vec<char> = line.chars().collect();
        let start = ui.anchor.col as usize;
        let end = (caret.col as usize).min(chars.len());
        (start <= end).then(|| chars[start..end].iter().collect())
    }

    /// The popup's current candidate order (indices into its items), resetting
    /// the selection when the filter changed since the last look.
    pub(crate) fn completion_ranked(&mut self) -> Option<Vec<usize>> {
        let filter = self.completion_filter()?;
        let ui = self.completion.as_mut()?;
        if filter != ui.last_filter {
            ui.list.reset();
            ui.last_filter.clone_from(&filter);
        }
        let mut popup = karet_widgets::CompletionPopup::new(
            &ui.items,
            &mut self.completion_matcher,
            &filter,
            &self.theme,
        );
        Some(popup.ranked())
    }

    /// Handle a key while the popup is open; returns whether it was consumed.
    /// Up/Down navigate, Enter/Tab accept, Esc dismisses; everything else
    /// falls through to normal editing (which refilters).
    fn completion_key(&mut self, key: KeyEvent) -> bool {
        if self.completion.is_none() || !key.modifiers.is_empty() {
            return false;
        }
        let len = self.completion_ranked().map_or(0, |ranked| ranked.len());
        if len == 0 {
            // Nothing matches the typed prefix any more: the popup is over.
            self.dismiss_completion();
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.list.select_prev(len);
                }
                true
            },
            KeyCode::Down => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.list.select_next(len);
                }
                true
            },
            KeyCode::Esc => {
                self.dismiss_completion();
                true
            },
            KeyCode::Enter | KeyCode::Tab => {
                self.accept_completion();
                true
            },
            _ => false,
        }
    }

    /// Accept the selected candidate: replace the typed prefix (anchor to
    /// caret) with the item's resolved insert text, through the ordinary
    /// session edit path. (The item's `insert_text` already carries its
    /// `textEdit.newText` per the LSP precedence applied in karet-lsp.)
    fn accept_completion(&mut self) {
        let Some(ranked) = self.completion_ranked() else {
            self.dismiss_completion();
            return;
        };
        let text = {
            let Some(ui) = self.completion.as_ref() else {
                return;
            };
            let selected = ui.list.selected.min(ranked.len().saturating_sub(1));
            let Some(item) = ranked.get(selected).and_then(|&i| ui.items.get(i)) else {
                self.dismiss_completion();
                return;
            };
            item.insert_text.clone()
        };
        let Some(anchor) = self.completion.as_ref().map(|ui| ui.anchor) else {
            return;
        };
        self.dismiss_completion();
        self.submit_edit_with_cause(EditCause::Replace, move |caret, _sel, _buf, base| {
            // Only carets still on the anchored span complete; others no-op.
            let range = crate::completion::accept_range(anchor, caret)?;
            Some(editing::Edit {
                change: Change::new(
                    base,
                    vec![TextEdit {
                        range,
                        new_text: text.clone(),
                    }],
                ),
                caret: crate::completion::caret_after_insert(range.start, &text),
            })
        });
    }

    /// Close the popup and forget any in-flight request.
    pub(crate) fn dismiss_completion(&mut self) {
        self.completion = None;
        self.pending_completion = None;
    }

    /// Drop the popup / pending request when the caret left the anchored span,
    /// the document changed, or the active tab is no longer a code tab.
    pub(crate) fn reconcile_completion(&mut self) {
        let target = self.completion_target();
        let anchored = |doc: DocumentId, anchor: LineCol| {
            matches!(target, Some((d, caret))
                if d == doc && crate::completion::caret_still_anchored(anchor, caret))
        };
        if let Some(pending) = &self.pending_completion
            && !anchored(pending.doc, pending.anchor)
        {
            self.pending_completion = None;
        }
        if let Some(ui) = &self.completion
            && !anchored(ui.doc, ui.anchor)
        {
            self.completion = None;
        }
    }

    /// Adopt (or drop as stale) an answering `Event::Completions`.
    fn on_completions(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        items: Vec<karet_core::CompletionItem>,
    ) {
        // The request id is the primary staleness key (a newer request
        // supersedes); the anchor check below covers caret movement, which
        // also subsumes the version tag for typed-ahead edits.
        let _ = version;
        let Some(pending) = self.pending_completion else {
            return;
        };
        if id != Some(pending.id) {
            return; // an answer to a superseded request
        }
        self.pending_completion = None;
        if pending.doc != doc {
            return;
        }
        let still_valid = matches!(self.completion_target(), Some((d, caret))
            if d == doc && crate::completion::caret_still_anchored(pending.anchor, caret));
        if !still_valid {
            return;
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(crate::completion::CompletionUi {
            items,
            list: karet_widgets::CompletionState::default(),
            doc,
            anchor: pending.anchor,
            last_filter: String::new(),
        });
        // Seed the filter so the first render doesn't spuriously reset it.
        if let Some(filter) = self.completion_filter()
            && let Some(ui) = self.completion.as_mut()
        {
            ui.last_filter = filter;
        }
    }
}

#[cfg(test)]
mod tests {
    use karet_vcs::StatusKind;

    use super::*;
    use crate::keymap::SidebarPanel;

    fn change(path: &str, status: StatusKind) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status,
            is_binary: false,
            old: String::new(),
            new: "x\n".to_string(),
        }
    }

    fn app() -> App {
        App::new(
            PathBuf::from("."),
            vec![change("a.rs", StatusKind::Modified)],
            vec![change("b.rs", StatusKind::Modified)],
            false,
        )
    }

    fn test_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("karet-{name}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn write_file(root: &Path, rel: &str, contents: &[u8]) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
    }

    fn select_explorer_path(app: &mut App, path: &Path) {
        app.explorer.ensure_built(&app.root);
        let Some(idx) = app.explorer.rows().iter().position(|row| row.path == path) else {
            panic!("missing explorer path {}", path.display());
        };
        app.explorer.select_visible(idx);
    }

    fn refresh_count(backend: &RecordingBackend) -> usize {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| matches!(command, SessionCommand::RefreshVcs))
                    .count()
            })
            .unwrap_or_default()
    }

    fn retarget_commands(backend: &RecordingBackend) -> Vec<(DocumentId, PathBuf)> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(_, command)| match command {
                        SessionCommand::RetargetDocument { doc, path } => {
                            Some((*doc, path.clone()))
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    struct RecordingBackend {
        next: std::sync::atomic::AtomicU64,
        sent: std::sync::Mutex<Vec<(RequestId, SessionCommand)>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            Self {
                next: std::sync::atomic::AtomicU64::new(1),
                sent: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for RecordingBackend {
        fn send(&self, id: RequestId, command: SessionCommand) -> Result<(), BackendError> {
            if let Ok(mut sent) = self.sent.lock() {
                sent.push((id, command));
            }
            Ok(())
        }

        fn next_id(&self) -> RequestId {
            RequestId(self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
        }
    }

    #[test]
    fn quit_with_unsaved_changes_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::Quit);
        assert_eq!(
            app.pending_close,
            Some(CloseRequest::Quit),
            "unsaved changes arm the quit prompt"
        );
        assert!(!app.should_quit);
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::CloseConfirm)
        );

        // Discarding exits.
        app.dispatch(Command::CloseConfirmDiscard);
        assert!(app.pending_close.is_none());
        assert!(app.should_quit);
    }

    #[test]
    fn quit_without_unsaved_changes_exits_immediately() {
        let mut app = app();
        app.dispatch(Command::Quit);
        assert!(app.pending_close.is_none());
        assert!(app.should_quit);
    }

    #[test]
    fn quit_prompt_disabled_by_confirm_on_exit_setting() {
        let mut app = app();
        app.settings.files.confirm_on_exit = false;
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::Quit);
        assert!(
            app.should_quit,
            "confirmOnExit=false quits without prompting"
        );
    }

    #[test]
    fn quit_save_all_with_nothing_dirty_exits() {
        let mut app = app();
        app.pending_close = Some(CloseRequest::Quit);
        app.dispatch(Command::CloseConfirmSave);
        assert!(app.should_quit);
        assert!(app.saving_close.is_none(), "no saves in flight");
    }

    /// Push a dirty code tab backed by `doc`, returning its stable view id. The tab
    /// becomes the focused pane's active tab.
    fn dirty_doc_tab(app: &mut App, name: &str, doc: u64) -> ViewId {
        app.push_tab(text_tab(name, "x"));
        let idx = app.active;
        if let TabKind::Code { doc: d, .. } = &mut app.tabs[idx].kind {
            *d = Some(DocumentId(doc));
        }
        app.tabs[idx].dirty = true;
        app.tabs[idx].view
    }

    /// The documents a backend was asked to save, in order.
    fn saved_docs(backend: &RecordingBackend) -> Vec<DocumentId> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(_, command)| match command {
                        SessionCommand::Save { doc } => Some(*doc),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn close_tab_with_unsaved_changes_arms_the_prompt_and_does_not_close() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        // The close is deferred behind the confirmation, and the tab is untouched.
        assert!(matches!(app.pending_close, Some(CloseRequest::Tab { .. })));
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));
        assert!(app.tabs[0].dirty);
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::CloseConfirm)
        );
    }

    #[test]
    fn close_tab_confirm_discard_closes_and_discards() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        app.dispatch(Command::CloseConfirmDiscard);
        assert!(app.pending_close.is_none());
        // The last tab collapses to a Welcome tab; the dirty buffer is discarded.
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_tab_unbound_key_cancels_and_keeps_the_tab() {
        let mut app = app();
        dirty_doc_tab(&mut app, "t.rs", 1);
        app.dispatch(Command::CloseTab);
        // Any key that is not s/d aborts (the default), leaving the tab open.
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert!(app.pending_close.is_none());
        assert_eq!(app.tabs.len(), 1);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));
        assert_eq!(app.status.as_deref(), Some("close cancelled"));
    }

    #[test]
    fn close_tab_save_parks_request_then_closes_when_saves_drain() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        dirty_doc_tab(&mut app, "t.rs", 7);
        app.dispatch(Command::CloseTab);

        app.dispatch(Command::CloseConfirmSave);
        // The request is parked mid-save; exactly the at-risk doc is saved, and the
        // tab stays open until the save answers.
        assert!(matches!(app.saving_close, Some(CloseRequest::Tab { .. })));
        assert_eq!(saved_docs(&backend), vec![DocumentId(7)]);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));

        // The save drains → the parked close runs.
        let save_id = *app
            .pending_saves
            .keys()
            .next()
            .expect("a save is in flight");
        app.on_backend_event(Some(save_id), SessionEvent::Saved { doc: DocumentId(7) });
        assert!(app.saving_close.is_none());
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_other_tabs_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "keep.rs", 1);
        dirty_doc_tab(&mut app, "other.rs", 2);
        // Keep the first tab active; the dirty second tab would be dropped.
        app.active = 0;
        app.dispatch(Command::CloseOtherTabs);
        assert_eq!(app.pending_close, Some(CloseRequest::OtherTabs));
        assert_eq!(app.tabs.len(), 2, "nothing closes while the prompt is up");
    }

    #[test]
    fn close_tabs_to_right_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "left.rs", 1);
        dirty_doc_tab(&mut app, "right.rs", 2);
        app.active = 0;
        app.dispatch(Command::CloseTabsToRight);
        assert_eq!(app.pending_close, Some(CloseRequest::TabsToRight));
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn close_all_tabs_with_unsaved_arms_the_prompt() {
        let mut app = app();
        dirty_doc_tab(&mut app, "a.rs", 1);
        dirty_doc_tab(&mut app, "b.rs", 2);
        app.dispatch(Command::CloseAllTabs);
        assert_eq!(app.pending_close, Some(CloseRequest::AllTabs));
        assert_eq!(app.tabs.len(), 2);
    }

    #[test]
    fn clean_tab_closes_without_prompting() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(1));
        }
        // Not dirty → close runs immediately.
        app.dispatch(Command::CloseTab);
        assert!(app.pending_close.is_none());
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn close_tab_does_not_prompt_when_doc_open_in_another_tab() {
        let mut app = app();
        // Two tabs of the same dirty document; closing one leaves the other.
        let keep = dirty_doc_tab(&mut app, "dup.rs", 5);
        let drop = dirty_doc_tab(&mut app, "dup.rs", 5);
        assert_ne!(keep, drop);
        app.dispatch(Command::CloseTab); // closes the active (second) view
        assert!(
            app.pending_close.is_none(),
            "the document survives in the first tab, so no data is lost"
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].view, keep);
    }

    #[test]
    fn close_tab_does_not_prompt_when_doc_open_in_another_pane() {
        let mut app = app();
        dirty_doc_tab(&mut app, "shared.rs", 9);
        // Split: the duplicate (same doc) becomes the focused pane; the dirty original
        // moves into a stored pane and keeps the document referenced.
        app.split_focused(SplitDir::Right);
        app.dispatch(Command::CloseTab);
        assert!(
            app.pending_close.is_none(),
            "the dirty document still lives in the other pane"
        );
    }

    #[test]
    fn close_save_targets_only_the_at_risk_documents() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        // Two independent dirty docs; only the one being dropped should be saved.
        dirty_doc_tab(&mut app, "keep.rs", 1);
        let drop = dirty_doc_tab(&mut app, "drop.rs", 2);
        app.guarded_close(CloseRequest::Tab { view: drop });
        app.dispatch(Command::CloseConfirmSave);
        assert_eq!(
            saved_docs(&backend),
            vec![DocumentId(2)],
            "only the at-risk document is saved, not every dirty document"
        );
    }

    #[test]
    fn close_tab_save_revalidates_index_after_drain() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        let _a = dirty_doc_tab(&mut app, "a.rs", 1); // scaffold at index 0, cleaned below
        app.tabs[0].dirty = false;
        let target = dirty_doc_tab(&mut app, "target.rs", 2); // index 1
        let c = dirty_doc_tab(&mut app, "c.rs", 3);
        app.tabs[2].dirty = false;

        app.guarded_close(CloseRequest::Tab { view: target });
        app.dispatch(Command::CloseConfirmSave);
        assert!(matches!(app.saving_close, Some(CloseRequest::Tab { .. })));

        // A tab list mutation before the save drains shifts `target` from index 1 to 0.
        app.tabs.remove(0);

        let save_id = *app
            .pending_saves
            .keys()
            .next()
            .expect("a save is in flight");
        app.on_backend_event(Some(save_id), SessionEvent::Saved { doc: DocumentId(2) });

        // The view-id lookup closes `target` (not whatever now sits at the old index).
        let views: Vec<ViewId> = app.tabs.iter().map(|t| t.view).collect();
        assert!(!views.contains(&target), "the intended tab was closed");
        assert!(views.contains(&c), "the other tab is untouched");
    }

    #[test]
    fn non_code_tab_never_prompts_even_with_other_dirty_docs() {
        let mut app = app();
        dirty_doc_tab(&mut app, "dirty.rs", 1); // a dirty doc lives elsewhere
        app.push_tab(Tab::welcome()); // a non-code tab, now active
        app.dispatch(Command::CloseTab);
        assert!(
            app.pending_close.is_none(),
            "closing a doc-less tab risks no data"
        );
        // The dirty code tab is still open.
        assert!(app.all_tabs().any(|t| t.dirty));
    }

    #[test]
    fn recover_swaps_opens_a_tab_for_each_backed_up_file() {
        let path = std::env::temp_dir().join(format!("karet-recover-{}.rs", std::process::id()));
        if std::fs::write(&path, "fn main() {}\n").is_err() {
            return;
        }
        let mut app = app();
        app.pending_swaps = Some(vec![SwapInfo {
            original: path.clone(),
            updated_unix_ms: 0,
            conflict: false,
        }]);
        app.dispatch(Command::RecoverSwaps);
        assert!(app.pending_swaps.is_none());
        assert!(
            app.all_tabs().any(|t| t.path().is_some_and(|p| p == path)),
            "recovery opens a tab for the backed-up file"
        );
    }

    #[test]
    fn swaps_found_arms_the_recovery_prompt() {
        let mut app = app();
        app.on_backend_event(
            None,
            SessionEvent::SwapsFound {
                swaps: vec![SwapInfo {
                    original: PathBuf::from("/work/a.rs"),
                    updated_unix_ms: 0,
                    conflict: false,
                }],
            },
        );
        assert!(app.pending_swaps.is_some());
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::SwapRecover)
        );
    }

    #[test]
    fn with_settings_applies_the_workbench_slice() {
        use karet_session::config::schema::IconStyleSetting;
        use karet_session::config::schema::StartupPanel;

        let mut settings = Settings::default();
        settings.workbench.icon_style = IconStyleSetting::Ascii;
        settings.workbench.startup_panel = StartupPanel::SourceControl;

        let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
            .with_settings(settings, Vec::new());

        assert_eq!(app.icon_style, IconStyle::Ascii);
        assert_eq!(app.sidebar_panel, SidebarPanel::SourceControl);
        assert!(app.sidebar_visible);
    }

    #[test]
    fn with_settings_none_panel_collapses_the_sidebar() {
        use karet_session::config::schema::StartupPanel;

        let mut settings = Settings::default();
        settings.workbench.startup_panel = StartupPanel::None;
        let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
            .with_settings(settings, Vec::new());
        assert!(!app.sidebar_visible);
    }

    #[test]
    fn live_config_preserves_cli_icons_and_current_sidebar_state() {
        use karet_session::config::schema::IconStyleSetting;
        use karet_session::config::schema::StartupPanel;

        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
            .with_icons(IconStyle::Ascii);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_visible = true;

        let mut settings = Settings::default();
        settings.editor.tab_size = 8;
        settings.workbench.icon_style = IconStyleSetting::Unicode;
        settings.workbench.startup_panel = StartupPanel::None;
        app.on_backend_event(
            None,
            SessionEvent::ConfigChanged {
                report: Box::new(LoadedConfig::from_settings(settings)),
            },
        );

        assert_eq!(app.settings.editor.tab_size, 8);
        assert_eq!(
            app.icon_style,
            IconStyle::Ascii,
            "CLI override remains authoritative"
        );
        assert_eq!(app.sidebar_panel, SidebarPanel::SourceControl);
        assert!(
            app.sidebar_visible,
            "startupPanel is not replayed on reload"
        );
    }

    #[test]
    fn open_startup_goto_positions_caret_and_focuses_editor() {
        let dir = test_dir("goto");
        write_file(
            &dir,
            "src/main.rs",
            b"fn main() {\n    println!(\"hi\");\n}\n",
        );
        let path = dir.join("src/main.rs");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_startup_goto(&path, 2, 5);

        // The file opened as a code tab, focused, with the caret at 0-based (1, 4).
        assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 4));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_goto_clamps_out_of_range_target() {
        let dir = test_dir("goto-clamp");
        write_file(&dir, "a.txt", b"one\ntwo\n");
        let path = dir.join("a.txt");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        // Line far past the end and a large column clamp into the buffer rather than
        // panicking or landing off the end.
        app.open_startup_goto(&path, 9999, 9999);
        let caret = app.tabs[app.active].editor.cursor();
        assert!(
            caret.line <= 2,
            "caret line {} should clamp within the 2-line buffer",
            caret.line
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_split_creates_a_second_pane_with_the_file() {
        let dir = test_dir("split");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.main_rect = Rect::new(0, 0, 120, 40);
        app.open_initial(&dir.join("a.rs"));
        app.open_startup_split(&dir.join("b.rs"));

        assert_eq!(app.layout.panes().len(), 2, "the split adds a second pane");
        // The new pane is focused and holds exactly the split file.
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[app.active].path(),
            Some(dir.join("b.rs")).as_deref()
        );
        // The first pane still holds the originally-opened file.
        let stored: Vec<_> = app
            .stored
            .values()
            .flat_map(|p| p.tabs.iter())
            .filter_map(Tab::path)
            .collect();
        assert_eq!(stored, vec![dir.join("a.rs").as_path()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_split_chains_panes_left_to_right() {
        let dir = test_dir("split-chain");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        write_file(&dir, "c.rs", b"fn c() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.main_rect = Rect::new(0, 0, 200, 40);
        app.open_initial(&dir.join("a.rs"));
        app.open_startup_split(&dir.join("b.rs"));
        app.open_startup_split(&dir.join("c.rs"));

        assert_eq!(app.layout.panes().len(), 3);
        // The last split pane is focused and shows the last file.
        assert_eq!(
            app.tabs[app.active].path(),
            Some(dir.join("c.rs")).as_deref()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_split_falls_back_to_a_tab_when_there_is_no_room() {
        let dir = test_dir("split-narrow");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        // Too narrow for two panes at the minimum pane width.
        app.main_rect = Rect::new(0, 0, 12, 10);
        app.open_initial(&dir.join("a.rs"));
        app.open_startup_split(&dir.join("b.rs"));

        assert_eq!(app.layout.panes().len(), 1, "no second pane is created");
        assert_eq!(app.tabs.len(), 2, "the file still opens, as a tab");
        assert_eq!(
            app.tabs[app.active].path(),
            Some(dir.join("b.rs")).as_deref()
        );
        // The degradation is surfaced, not silent.
        assert!(
            app.notifications
                .active()
                .iter()
                .any(|n| n.title.contains("--split")),
            "a startup notification explains the fallback"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_diff_opens_a_text_diff_tab() {
        let dir = test_dir("cli-diff");
        write_file(&dir, "old.rs", b"fn a() {}\n");
        write_file(&dir, "new.rs", b"fn b() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_startup_diff(
            &dir.join("old.rs"),
            &dir.join("new.rs"),
            Some("fn a() {}\n".to_string()),
            Some("fn b() {}\n".to_string()),
        );

        match &app.tabs[app.active].kind {
            TabKind::Diff { file, .. } => {
                assert!(!file.change.is_binary);
                assert_eq!(file.change.old, "fn a() {}\n");
                assert_eq!(file.change.new, "fn b() {}\n");
                assert_eq!(file.change.path, dir.join("new.rs"));
                assert_eq!(file.change.old_path, Some(dir.join("old.rs")));
                // Both lines differ, so the diff carries one added + one removed line.
                assert_eq!(file.line_stats(), (1, 1));
            },
            _ => panic!("expected a diff tab"),
        }
        assert_eq!(app.tabs[app.active].title, "old.rs ↔ new.rs");
        assert_eq!(app.focus, Focus::Editor);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_diff_marks_a_non_utf8_side_binary() {
        let dir = test_dir("cli-diff-bin");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        // A `None` side is what main.rs passes for non-UTF-8 bytes.
        app.open_startup_diff(
            &dir.join("a.bin"),
            &dir.join("b.bin"),
            None,
            Some("text\n".to_string()),
        );

        match &app.tabs[app.active].kind {
            TabKind::Diff { file, .. } => {
                assert!(file.change.is_binary);
                // The is_binary contract: both texts are empty.
                assert!(file.change.old.is_empty());
                assert!(file.change.new.is_empty());
            },
            _ => panic!("expected a diff tab"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_startup_diff_same_file_name_keeps_a_single_title() {
        let dir = test_dir("cli-diff-title");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_startup_diff(
            &dir.join("v1/config.toml"),
            &dir.join("v2/config.toml"),
            Some("a = 1\n".to_string()),
            Some("a = 2\n".to_string()),
        );
        assert_eq!(app.tabs[app.active].title, "config.toml");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_startup_command_dispatches_in_order() {
        // The pair [SelectPanel(Search), ToggleSidebar] is order-observable: run in
        // this order the panel is Search and the sidebar ends hidden (SelectPanel
        // shows it, ToggleSidebar then hides it); reversed it would end visible.
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        assert!(app.sidebar_visible);
        app.apply_startup_command(Command::SelectPanel(SidebarPanel::Search));
        app.apply_startup_command(Command::ToggleSidebar);
        assert_eq!(app.sidebar_panel, SidebarPanel::Search);
        assert!(
            !app.sidebar_visible,
            "ToggleSidebar must run after SelectPanel"
        );

        // The reversed order ends with the sidebar visible, proving order matters.
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.apply_startup_command(Command::ToggleSidebar);
        app.apply_startup_command(Command::SelectPanel(SidebarPanel::Search));
        assert!(app.sidebar_visible);
    }

    #[test]
    fn apply_startup_command_opens_views() {
        // A view-affecting palette command works from the startup path: SplitRight
        // creates a second pane synchronously (no backend round-trip needed).
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        assert_eq!(app.layout.panes().len(), 1);
        app.apply_startup_command(Command::SplitRight);
        assert_eq!(
            app.layout.panes().len(),
            2,
            "SplitRight should create a second pane"
        );
    }

    #[test]
    fn bad_theme_path_becomes_a_diagnostic_and_keeps_default() {
        let mut settings = Settings::default();
        settings.workbench.color_theme = "/no/such/theme.json".to_string();
        let app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false)
            .with_settings(settings, Vec::new());
        // The default (dark) theme is retained and the failure is queued as a warning.
        assert_eq!(app.config_diagnostics.len(), 1);
        assert!(app.config_diagnostics[0].message.contains("theme"));
    }

    #[test]
    fn load_theme_resolves_the_builtin_dark() {
        assert!(load_theme("dark").is_ok());
        assert!(load_theme("").is_ok());
        assert!(load_theme("/definitely/missing.tmTheme").is_err());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn outline_panel_toggles_and_jumps_to_a_bookmarked_page() {
        // A 2-page PDF whose single bookmark targets the second page (index 1). Like
        // the karet-pdf fixtures, it has no xref table, so hayro parses it via its
        // brute-force fallback.
        const PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 5 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
4 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
5 0 obj<</Type/Outlines/First 6 0 R/Last 6 0 R/Count 1>>endobj\n\
6 0 obj<</Title(Page Two)/Parent 5 0 R/Dest[4 0 R/Fit]>>endobj\n\
trailer<</Size 7/Root 1 0 R>>\n%%EOF";
        let Ok(doc) = karet_pdf::Document::load(PDF.to_vec()) else {
            return;
        };
        let page_count = doc.page_count();
        let outline = doc.outline();
        let mut app = app();
        app.tabs.push(Tab::new(
            "doc.pdf",
            TabKind::Document {
                path: PathBuf::from("doc.pdf"),
                doc,
                page_count,
                page: 0,
                rendered: None,
                outline,
            },
        ));
        app.active = app.tabs.len() - 1;

        // The panel is populated from the bookmark.
        let rows = app.active_outline_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows.first().map(|r| r.label.as_str()), Some("Page Two"));

        // Toggling shows and focuses the panel (it has content).
        app.dispatch(Command::ToggleOutline);
        assert!(app.outline_visible);
        assert_eq!(app.focus, Focus::Outline);

        // Activating the bookmark jumps the document to its page.
        app.dispatch(Command::OutlineActivate);
        let page = match app.tabs.get(app.active).map(|t| &t.kind) {
            Some(TabKind::Document { page, .. }) => Some(*page),
            _ => None,
        };
        assert_eq!(page, Some(1));

        // Toggling again hides the panel and returns focus to the editor.
        app.dispatch(Command::ToggleOutline);
        assert!(!app.outline_visible);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn sidebar_resize_sets_width_and_collapses_below_min() {
        let mut app = app();
        app.sidebar_rect = Rect::new(0, 0, DEFAULT_SIDEBAR_WIDTH, 20);
        app.sidebar_resizing = true;
        // Dragging the divider to column 45 widens the sidebar.
        app.resize_sidebar_to(45);
        assert_eq!(app.sidebar_width, 45);
        assert!(app.sidebar_visible);
        // Dragging narrower than the minimum collapses it and ends the drag, leaving
        // the last valid width intact so re-showing restores a sensible size.
        app.resize_sidebar_to(SIDEBAR_MIN_WIDTH - 1);
        assert!(!app.sidebar_visible);
        assert!(!app.sidebar_resizing);
        assert_eq!(app.sidebar_width, 45);
    }

    #[test]
    fn scm_commit_divider_resizes_and_clamps() {
        let mut app = app();
        // A 20-row list area (rows 2..22); the changes list starts at row 2.
        app.sidebar_content_rect = Rect::new(0, 2, 30, 20);
        app.scm_changes_rect = Rect::new(0, 2, 30, 10);
        // Drag the divider up to row 12 → commits region = rows 13..22 = 9 rows.
        app.resize_scm_commits_to(12);
        assert_eq!(app.scm_commits_h, 9);
        // Dragging past the bottom clamps so the commits region keeps the minimum.
        app.resize_scm_commits_to(30);
        assert_eq!(app.scm_commits_h, MIN_SCM_REGION);
        // Dragging to the very top clamps so the changes region keeps room too.
        app.resize_scm_commits_to(0);
        assert_eq!(app.scm_commits_h, 20 - (MIN_SCM_REGION + 1));
    }

    #[test]
    fn pointer_shape_hint_tracks_divider_hover_when_supported() {
        let mut app = app();
        app.pointer_shapes_supported = true;
        app.sidebar_visible = true;
        app.sidebar_divider_x = 30;

        let moved = |col, row| MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.update_pointer_shape_hint(&moved(30, 5));
        assert_eq!(app.pointer_shape, Some("col-resize"));

        app.update_pointer_shape_hint(&moved(10, 5));
        assert_eq!(
            app.pointer_shape, None,
            "moving off the divider resets to the default shape"
        );
    }

    #[test]
    fn pointer_shape_hint_is_a_no_op_when_unsupported() {
        let mut app = app();
        // `pointer_shapes_supported` defaults to false (never confirmed at startup).
        app.sidebar_visible = true;
        app.sidebar_divider_x = 30;
        app.update_pointer_shape_hint(&MouseEvent {
            kind: MouseEventKind::Moved,
            column: 30,
            row: 5,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            app.pointer_shape, None,
            "an unconfirmed terminal must never get a pointer-shape hint"
        );
    }

    #[test]
    fn graphical_cursor_requires_kitty_keyboard_and_graphics() {
        let mut app = app();
        app.graphics = GraphicsProtocol::Kitty;
        app.kitty_graphics_supported = true;

        assert!(!app.graphical_cursor_compatible());

        app.kitty_keyboard_supported = true;
        assert!(app.graphical_cursor_compatible());

        app.graphics = GraphicsProtocol::Halfblocks;
        assert!(
            !app.graphical_cursor_compatible(),
            "the graphical cursor must only ride the Kitty graphics path"
        );
    }

    #[test]
    fn graphical_cursor_blink_schedules_a_repaint_when_active() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        app.focus = Focus::Editor;
        app.editor_rect = Rect::new(0, 0, 20, 5);
        app.graphics = GraphicsProtocol::Kitty;
        app.kitty_graphics_supported = true;
        app.kitty_keyboard_supported = true;

        let wake = app.next_wake().expect("an active graphical cursor blinks");
        assert!(wake <= GRAPHICS_CARET_BLINK_INTERVAL && wake > Duration::ZERO);
    }

    #[test]
    fn graphical_cursor_is_suppressed_during_the_hidden_blink_phase() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        app.focus = Focus::Editor;
        app.editor_rect = Rect::new(0, 0, 20, 5);
        app.graphics = GraphicsProtocol::Kitty;
        app.kitty_graphics_supported = true;
        app.kitty_keyboard_supported = true;

        assert!(app.active_graphics_caret().is_some());
        app.graphics_caret_blink_epoch = Instant::now() - GRAPHICS_CARET_BLINK_INTERVAL;
        assert_eq!(app.active_graphics_caret(), None);
        assert!(
            app.active_graphics_caret_position().is_some(),
            "blink hides a valid caret without losing its placement"
        );
    }

    #[test]
    fn pending_save_drives_the_animation_tick() {
        let mut app = app();
        assert!(app.next_wake().is_none());
        app.pending_saves.insert(RequestId(1), DocumentId(1));
        assert_eq!(app.next_wake(), Some(Duration::from_millis(100)));
    }

    #[test]
    fn save_completion_clears_the_spinner() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.tabs[app.active].saving_since = Some(Instant::now());
        app.pending_saves.insert(RequestId(5), DocumentId(2));
        app.on_backend_event(
            Some(RequestId(5)),
            SessionEvent::Saved { doc: DocumentId(2) },
        );
        assert!(app.tabs[app.active].saving_since.is_none());
        assert!(app.pending_saves.is_empty());
    }

    #[test]
    fn duplicate_save_command_is_debounced_while_in_flight() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }

        app.save_active();
        app.save_active();

        let sent_saves = backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| matches!(command, SessionCommand::Save { .. }))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(sent_saves, 1, "only one save may be in flight per document");
        assert_eq!(
            app.status.as_deref(),
            Some("save already in progress"),
            "the second shortcut is ignored because the first save is still pending"
        );
    }

    #[test]
    fn save_active_marks_every_view_of_the_document_as_saving() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.split_focused(SplitDir::Right);

        app.save_active();

        assert!(app.tabs[app.active].saving_since.is_some());
        let stored_saving = app
            .stored
            .values()
            .flat_map(|pane| pane.tabs.iter())
            .any(|tab| tab.saving_since.is_some());
        assert!(
            stored_saving,
            "background split view should show save progress"
        );
    }

    #[test]
    fn quit_save_all_conflict_keeps_the_app_open() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(2));
        }
        app.tabs[app.active].dirty = true;
        app.saving_close = Some(CloseRequest::Quit);
        app.pending_saves.insert(RequestId(5), DocumentId(2));

        app.on_backend_event(
            Some(RequestId(5)),
            SessionEvent::ExternalConflict { doc: DocumentId(2) },
        );

        assert!(!app.should_quit);
        assert!(app.saving_close.is_none());
        assert!(app.tabs[app.active].dirty);
    }

    #[test]
    fn saved_event_clears_the_dirty_flag() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "x"));
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(1));
        }
        app.tabs[app.active].dirty = true;
        app.on_backend_event(None, SessionEvent::Saved { doc: DocumentId(1) });
        assert!(!app.tabs[app.active].dirty);
    }

    #[test]
    fn scm_log_pages_replace_then_append() {
        fn commit(hash: &str, summary: &str) -> Commit {
            Commit {
                hash: hash.to_string(),
                short_hash: hash.chars().take(7).collect(),
                summary: summary.to_string(),
                author: "a".to_string(),
                time: 0,
                parents: Vec::new(),
            }
        }
        let mut app = app();
        // The first page replaces and clears the in-flight flag.
        app.scm.log_loading = true;
        app.apply_vcs_log(0, vec![commit("aaaaaaa", "first")], true);
        assert_eq!(app.scm.log.len(), 1);
        assert!(app.scm.log_has_more);
        assert!(!app.scm.log_loading);
        // A page at the right offset appends.
        app.apply_vcs_log(1, vec![commit("bbbbbbb", "second")], false);
        assert_eq!(app.scm.log.len(), 2);
        assert!(!app.scm.log_has_more);
        // A page at the wrong offset is ignored (no duplicate/torn appends).
        app.apply_vcs_log(5, vec![commit("ccccccc", "stale")], false);
        assert_eq!(app.scm.log.len(), 2);
    }

    #[test]
    fn hover_maps_to_explorer_and_scm_rows() {
        let mut app = app();
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 10,
        };
        // Explorer: hover at y=4 with offset 0 → absolute row 2.
        app.hover = Some((5, 4));
        assert_eq!(app.hovered_explorer_row(), Some(2));
        // Above the content area → no hovered row.
        app.hover = Some((5, 1));
        assert_eq!(app.hovered_explorer_row(), None);

        // Source control: display 0 is a section header, 1 and 2 are changes. Hover
        // maps against the changes region rect.
        app.scm_changes_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 10,
        };
        app.scm_offset = 0;
        app.scm_row_map = vec![None, Some(0), Some(1)];
        app.hover = Some((5, 3)); // display = 0 + (3 - 2) = 1 → change 0
        assert_eq!(app.hovered_scm_change(), Some(0));
        app.hover = Some((5, 2)); // display 0 → header → nothing
        assert_eq!(app.hovered_scm_change(), None);
    }

    #[test]
    fn sidebar_header_hover_tracks_header_only() {
        let mut app = app();
        app.sidebar_visible = true;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 20,
            height: 8,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 20,
            height: 7,
        };
        let moved = |column, row| MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(moved(5, 1));
        assert_eq!(app.sidebar_header_hover, Some((5, 1)));
        assert_eq!(app.hover, None);

        app.handle_mouse(moved(5, 3));
        assert_eq!(app.sidebar_header_hover, None);
        assert_eq!(app.hover, Some((5, 3)));

        app.handle_mouse(moved(30, 3));
        assert_eq!(app.sidebar_header_hover, None);
        assert_eq!(app.hover, None);
    }

    #[test]
    fn notify_makes_errors_persistent_and_info_transient() {
        let mut app = app();
        app.notify(Severity::Error, NotificationKind::Io, "save failed");
        app.notify(Severity::Information, NotificationKind::Vcs, "committed");
        let active = app.notifications.active();
        assert_eq!(active.len(), 2);
        // Newest (info) is first; it auto-expires. The error persists.
        assert!(active[0].timeout.is_some());
        assert!(active[1].timeout.is_none());
    }

    #[test]
    fn esc_dismisses_a_toast_before_normal_handling() {
        let mut app = app();
        app.notify(Severity::Error, NotificationKind::Io, "boom");
        assert!(!app.notifications.is_empty());
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(app.notifications.is_empty());
        // A second Esc, with no toast left, falls through to normal handling.
        assert!(!app.should_quit);
    }

    #[test]
    fn starts_explorer_focused_with_welcome_tab() {
        let app = app();
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert!(matches!(app.tabs[0].kind, TabKind::Welcome));
    }

    #[test]
    fn focus_target_tracks_focus_and_panel() {
        let mut app = app();
        assert_eq!(app.focus_target(), FocusTarget::Explorer);
        app.sidebar_panel = SidebarPanel::SourceControl;
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);
        app.focus = Focus::Editor;
        assert_eq!(app.focus_target(), FocusTarget::Editor);
    }

    #[test]
    fn open_anyway_bypasses_the_guard_and_decodes_in_place() {
        // A .cbor file that (per its recorded length) tripped the size guard shows a
        // too-large placeholder; the override re-opens it decoded, in the same tab.
        let dir = std::env::temp_dir().join(format!("karet-anyway-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("big.cbor");
        let value = karet_cbor::CborValue::Array(vec![karet_cbor::CborValue::Integer(1)]);
        let bytes = karet_cbor::encode(&value).unwrap_or_default();
        let _ = std::fs::write(&file, &bytes);

        let mut app = app();
        let len = karet_fileview::viewer::SIZE_GUARD + 1;
        app.tabs = vec![Tab::new(
            "big.cbor",
            TabKind::Placeholder {
                path: file.clone(),
                kind: FileKind::TooLarge { len },
                dims: None,
                len,
            },
        )];
        app.active = 0;
        app.focus = Focus::Editor;
        // A too-large placeholder gets the override layer, so Enter is bound.
        assert_eq!(app.focus_target(), FocusTarget::Oversize);

        app.dispatch(Command::OpenAnyway);
        assert_eq!(
            app.tabs.len(),
            1,
            "the placeholder is replaced, not appended"
        );
        assert!(
            matches!(
                app.tabs[0].kind,
                TabKind::Code {
                    language: "CBOR",
                    ..
                }
            ),
            "open-anyway decodes the CBOR in place"
        );

        // The override is inert on an ordinary tab.
        app.dispatch(Command::OpenAnyway);
        assert!(matches!(app.tabs[0].kind, TabKind::Code { .. }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn send_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
        app.handle_key(KeyEvent::new(code, mods));
    }

    fn commit(hash: &str, summary: &str) -> Commit {
        Commit {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            author: "T".to_string(),
            time: 0,
            parents: Vec::new(),
        }
    }

    fn commit_detail(hash: &str, summary: &str) -> CommitDetail {
        let id = karet_vcs::Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        CommitDetail {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            body: String::new(),
            author: id.clone(),
            committer: id,
            parents: Vec::new(),
            signature: None,
        }
    }

    #[test]
    fn toggle_fold_collapses_at_cursor_and_relocates_caret() {
        use karet_treesitter::ParserPool;
        use karet_treesitter::SyntaxTree;
        use karet_treesitter::language_id_from_path;

        let Some(lang) = language_id_from_path(Path::new("f.rs")) else {
            return; // rust grammar not compiled in
        };
        let src = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src).expect("parse");
        let regions = karet_syntax::fold(&tree);
        let start = regions.regions()[0].start;
        assert_eq!(start, 0, "the function body folds from line 0");

        let mut app = app();
        app.push_tab(text_tab("f.rs", src));
        if let TabKind::Code { folds, .. } = &mut app.tabs[app.active].kind {
            *folds = regions;
        }
        // Cursor inside the region: toggling collapses it and moves the caret to the
        // (still visible) header line.
        app.tabs[app.active].editor.place_caret(LineCol::new(1, 0));
        app.toggle_fold();
        assert_eq!(app.tabs[app.active].editor.cursor().line, 0);
        if let TabKind::Code { folded, .. } = &app.tabs[app.active].kind {
            assert!(folded.contains(&0));
        }
        // Toggling again (cursor now on the header) expands it.
        app.toggle_fold();
        if let TabKind::Code { folded, .. } = &app.tabs[app.active].kind {
            assert!(!folded.contains(&0));
        }
    }

    #[test]
    fn prepended_commits_dedupe_and_preserve_scroll() {
        let mut app = app();
        app.scm.log = vec![commit("aaaaaaa1", "old top"), commit("bbbbbbb2", "older")];
        app.scm_commits_offset = 5;
        // A genuinely-new commit plus a duplicate of the current top: only the new
        // one prepends, and the viewport shifts down by that one inserted row.
        app.apply_vcs_commits_prepended(vec![
            commit("ccccccc3", "new"),
            commit("aaaaaaa1", "old top"),
        ]);
        assert_eq!(app.scm.log.len(), 3);
        assert_eq!(app.scm.log[0].summary, "new");
        assert_eq!(app.scm.log[1].summary, "old top");
        assert_eq!(app.scm_commits_offset, 6);
    }

    #[test]
    fn scm_wheel_scrolls_the_region_under_the_pointer() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        // Changes region on top (rows 0..10), commit-log region below (rows 10..15).
        app.scm_changes_rect = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        };
        app.scm_total_rows = 20;
        app.scm_commits_rect = Rect {
            x: 0,
            y: 10,
            width: 20,
            height: 5,
        };
        app.scm_commits_total = 12;

        // Wheeling over the changes region scrolls it, clamped to total - height.
        app.sidebar_wheel(5, 3);
        assert_eq!(app.scm_offset, 5);
        app.sidebar_wheel(100, 3);
        assert_eq!(app.scm_offset, 10);
        app.sidebar_wheel(-100, 3);
        assert_eq!(app.scm_offset, 0);

        // Wheeling over the commit-log region scrolls it independently.
        app.sidebar_wheel(4, 11);
        assert_eq!(app.scm_commits_offset, 4);
        assert_eq!(app.scm_offset, 0); // changes untouched
        app.sidebar_wheel(100, 11);
        assert_eq!(app.scm_commits_offset, 7); // clamps to 12 - 5
    }

    #[test]
    fn source_control_commit_click_opens_pending_commit_tab_immediately() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_rect = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 12,
        };
        app.scm_commits_rect = Rect {
            x: 0,
            y: 4,
            width: 30,
            height: 6,
        };
        app.scm_commits_offset = 0;
        app.scm.log = vec![commit("aaaaaaa111", "first")];

        app.handle_sidebar_click(2, 5, KeyModifiers::NONE);

        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitLoading { rev, .. } if rev == "aaaaaaa111"
        ));
        let sent = backend
            .sent
            .lock()
            .map(|sent| sent.len())
            .unwrap_or_default();
        assert_eq!(sent, 1, "the detail request is lazy and asynchronous");
    }

    #[test]
    fn commit_detail_response_fills_the_pending_tab_in_place() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);

        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::CommitLoading { .. }
        ));

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: vec![change("a.rs", StatusKind::Modified)],
            },
        );

        assert_eq!(app.tabs[app.active].view, view);
        assert_eq!(app.tabs[app.active].title, "Commit aaaaaaa");
        assert!(!app.tabs[app.active].dirty);
        match &app.tabs[app.active].kind {
            TabKind::Commit { detail, files, .. } => {
                assert_eq!(detail.hash, "aaaaaaa111");
                assert_eq!(files.len(), 1);
            },
            _ => panic!("pending tab should become a loaded commit view"),
        }
    }

    #[test]
    fn commit_metadata_response_progressively_fills_pending_tab() {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend);

        app.open_commit("aaaaaaa111".to_string());
        let view = app.tabs[app.active].view;

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitDetailReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
            },
        );

        assert_eq!(app.tabs[app.active].view, view);
        assert_eq!(app.tabs[app.active].title, "Commit aaaaaaa");
        assert!(!app.tabs[app.active].dirty);
        match &app.tabs[app.active].kind {
            TabKind::Commit {
                detail,
                files,
                files_loading_since,
                ..
            } => {
                assert_eq!(detail.hash, "aaaaaaa111");
                assert!(files.is_empty());
                assert!(files_loading_since.is_some());
            },
            _ => panic!("pending tab should show commit metadata while files load"),
        }

        app.apply_commit_verification(
            "aaaaaaa111",
            GithubVerification {
                verified: true,
                reason: "valid".to_string(),
                signer: Some("Tester".to_string()),
            },
        );

        app.on_backend_event(
            Some(RequestId(1)),
            SessionEvent::CommitReady {
                detail: Box::new(commit_detail("aaaaaaa111", "first")),
                changes: vec![change("a.rs", StatusKind::Modified)],
            },
        );

        match &app.tabs[app.active].kind {
            TabKind::Commit {
                files,
                files_loading_since,
                verification,
                ..
            } => {
                assert_eq!(files.len(), 1);
                assert!(files_loading_since.is_none());
                assert!(verification.as_ref().is_some_and(|v| v.verified));
            },
            _ => panic!("metadata tab should become a complete commit view"),
        }
    }

    #[test]
    fn command_palette_keys_route_through_the_overlay_layer() {
        let mut app = app();
        app.dispatch(Command::OpenCommandPalette);
        assert!(app.overlay.is_some());
        // A printable is a fall-through into the query; Esc resolves to
        // OverlayCancel via the Overlay layer and dismisses the overlay.
        send_key(&mut app, KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(app.overlay.is_some());
        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn loaded_config_command_opens_read_only_tab_without_backend() {
        let mut app = app().with_loaded_config(karet_session::LoadedConfig::from_settings(
            Settings::default(),
        ));
        app.dispatch(Command::ShowLoadedConfig);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::LoadedConfig { .. }
        ));
        assert_eq!(app.tabs[app.active].title, "Loaded Settings");
    }

    #[test]
    fn search_modal_switches_between_input_and_list() {
        let mut app = app();
        app.dispatch(Command::OpenGlobalSearch);
        assert!(app.search.input, "global search starts in query input");
        // Esc in the input modal stops editing (SearchEndInput), it does not quit.
        send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.search.input);
        assert!(!app.should_quit);
        // `/` from the results list re-enters input (SearchBeginInput).
        send_key(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(app.search.input);
        // A Ctrl-chord still resolves globally while in the Search modal.
        send_key(&mut app, KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert!(
            !app.sidebar_visible,
            "Ctrl+B toggled the sidebar from Search"
        );
    }

    #[test]
    fn discard_prompt_confirms_and_cancels_through_the_keymap() {
        let mut app = app();
        // A bound confirm key (Enter) resolves to ConfirmDiscard and clears the arm.
        app.pending_discard = Some(vec![PathBuf::from("a.rs")]);
        send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.pending_discard.is_none());
        // Any unbound key at the prompt cancels (the documented fall-through).
        app.pending_discard = Some(vec![PathBuf::from("a.rs")]);
        send_key(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.pending_discard.is_none());
    }

    #[test]
    fn scm_range_selection_collects_both_paths() {
        // `app()` seeds one staged (a.rs) and one working (b.rs) change.
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SelectExtendDown);
        assert_eq!(app.scm.selection.selected_indices(), vec![0, 1]);
        assert_eq!(app.scm.selected_paths().len(), 2);
    }

    #[test]
    fn scm_plain_move_collapses_range() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SelectExtendDown);
        assert!(app.scm.selection.anchor().is_some());
        // A non-extending move in the SCM panel clears the range.
        app.dispatch(Command::SidebarDown);
        assert!(app.scm.selection.anchor().is_none());
        assert_eq!(app.scm.selection.selected_indices(), vec![1]);
    }

    #[test]
    fn vcs_status_event_repopulates_panel() {
        let mut app = app();
        app.apply_vcs_status(
            vec![change("x.rs", StatusKind::Added)],
            vec![
                change("y.rs", StatusKind::Untracked),
                change("z.rs", StatusKind::Modified),
            ],
        );
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes.len(), 3);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
        assert_eq!(app.scm.selection.anchor(), None);
        assert_eq!(app.scm.selection.len(), 3);
    }

    #[test]
    fn scm_cursor_display_row_accounts_for_both_permanent_headers() {
        let mut app = app();
        app.apply_vcs_status(
            vec![
                change("a.rs", StatusKind::Added),
                change("b.rs", StatusKind::Added),
            ],
            vec![
                change("c.rs", StatusKind::Modified),
                change("d.rs", StatusKind::Modified),
            ],
        );
        // Layout rows: 0 "STAGED CHANGES", 1-2 staged, 3 "CHANGES", 4-5 working.
        let rows: Vec<usize> = (0..4)
            .map(|i| {
                app.scm.selection.move_to(i);
                app.scm_cursor_display_row()
            })
            .collect();
        assert_eq!(rows, vec![1, 2, 4, 5]);
    }

    #[test]
    fn scm_cursor_display_row_reserves_line_for_empty_staged_section() {
        let mut app = app();
        // With nothing staged, the staged section still reserves its header plus one
        // placeholder line, so the first working row lands at display row 3.
        app.apply_vcs_status(
            Vec::new(),
            vec![
                change("c.rs", StatusKind::Modified),
                change("d.rs", StatusKind::Modified),
            ],
        );
        app.scm.selection.move_to(0);
        assert_eq!(app.scm_cursor_display_row(), 3);
        app.scm.selection.move_to(1);
        assert_eq!(app.scm_cursor_display_row(), 4);
    }

    #[test]
    fn commit_input_requires_staged_changes() {
        let mut app = app();
        // a.rs is staged, so the input opens.
        app.dispatch(Command::ScmCommit);
        assert!(app.commit_input.is_some());

        // With nothing staged, it refuses and reports why.
        app.apply_vcs_status(Vec::new(), vec![change("b.rs", StatusKind::Modified)]);
        app.commit_input = None;
        app.dispatch(Command::ScmCommit);
        assert!(app.commit_input.is_none());
        assert!(app.status.is_some());
    }

    #[test]
    fn opening_a_diff_keeps_source_control_focused() {
        // The contract: browsing (arrow moves) previews each change's diff while
        // the SCM pane keeps focus, so stage/unstage/discard/commit and the
        // selection keys keep working; Enter is the explicit "commit into the
        // view" action that focuses the diff editor (see
        // `enter_on_a_change_materializes_and_focuses_the_diff`).
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarDown); // cursor 0 → 1: previews b.rs
        assert!(app.active_is_diff(), "the diff preview is shown");
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.tabs[app.active].title, "b.rs");
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);
        assert_eq!(app.tabs.len(), 1, "welcome tab is replaced, not appended");
        // Arrowing back retargets the SAME preview slot — never one tab per
        // visited change.
        app.dispatch(Command::SidebarUp); // cursor 1 → 0: previews a.rs
        assert_eq!(
            app.tabs.len(),
            1,
            "the preview slot is reused, not appended"
        );
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.tabs[app.active].title, "a.rs");
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn enter_on_a_change_materializes_and_focuses_the_diff() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        // Browse first: the diff shows as a preview without stealing focus.
        app.dispatch(Command::SidebarDown);
        assert!(app.tabs[app.active].is_preview);
        let view = app.tabs[app.active].view;
        // Enter: the SAME previewed view is materialized and focused — the
        // reported bug was a brand-new duplicate diff tab on every Enter.
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "Enter must reuse the previewed diff");
        assert!(!app.tabs[app.active].is_preview, "Enter materializes");
        assert_eq!(app.tabs[app.active].view, view, "the same view, not a copy");
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);
        // Enter again (back from the sidebar): re-focuses, never duplicates.
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "repeat Enter must not duplicate");
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn enter_on_a_focused_diff_opens_the_file_at_its_first_changed_line() {
        let dir = test_dir("diff-enter-into-file");
        write_file(&dir, "a.rs", b"fn a() {}\nfn added() {}\nfn c() {}\n");
        let changed = FileChange {
            path: PathBuf::from("a.rs"),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: "fn a() {}\nfn c() {}\n".to_string(),
            new: "fn a() {}\nfn added() {}\nfn c() {}\n".to_string(),
        };
        let mut app = App::new(dir.clone(), Vec::new(), vec![changed], false);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate); // materialize + focus the diff
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

        // Enter on the focused diff drops into the file, caret on the first
        // changed line (line 2, 0-based 1) — keyboard parity with the mouse.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(app.tabs[app.active].kind, TabKind::Code { .. }),
            "a normal, editable editor tab"
        );
        assert_eq!(
            app.tabs[app.active].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(
            app.tabs[app.active].editor.cursor().line,
            1,
            "caret lands on the first changed line"
        );
        assert_eq!(app.tabs.len(), 2, "the diff stays open alongside the file");

        // Enter again from the diff re-focuses the existing file tab — never a
        // duplicate.
        let file_idx = app.active;
        let diff_idx = app
            .tabs
            .iter()
            .position(Tab::is_diff)
            .expect("the diff tab is still open");
        app.select_tab(diff_idx);
        app.dispatch(Command::OpenDiffFile);
        assert_eq!(app.tabs.len(), 2, "no duplicate editor tab");
        assert_eq!(app.active, file_idx);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enter_on_a_deleted_files_diff_reports_instead_of_opening() {
        let dir = test_dir("diff-enter-deleted");
        let deleted = FileChange {
            path: PathBuf::from("gone.rs"),
            old_path: None,
            status: StatusKind::Deleted,
            is_binary: false,
            old: "fn gone() {}\n".to_string(),
            new: String::new(),
        };
        let mut app = App::new(dir.clone(), Vec::new(), vec![deleted], false);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);

        // The file is gone from the working tree: Enter degrades to a status
        // message — no dead tab, no panic.
        app.dispatch(Command::OpenDiffFile);
        assert_eq!(app.tabs.len(), 1, "nothing new opens for a deleted file");
        assert!(app.active_is_diff(), "the diff stays active");
        assert!(
            app.status.as_deref().is_some_and(|s| s.contains("gone.rs")),
            "a status message names the missing file"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scm_double_click_materializes_the_previewed_diff_without_duplicating() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_visible = true;
        // Seed the layout state a render would have produced: the changes region
        // starts at row 2, whose first display row is change index 0.
        app.sidebar_rect = Rect::new(0, 0, 20, 20);
        app.scm_changes_rect = Rect::new(0, 2, 20, 10);
        app.scm_row_map = vec![Some(0), Some(1)];

        // First click of the double-click: the diff opens as a preview and the
        // panel keeps focus (a plain single click is a browse).
        app.handle_sidebar_click(3, 2, KeyModifiers::NONE);
        assert!(app.active_is_diff());
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.focus, Focus::Sidebar);
        let view = app.tabs[app.active].view;

        // Second click: the SAME view is materialized and focused — the bug was
        // a separate duplicate view on double-click.
        app.handle_sidebar_click(3, 2, KeyModifiers::NONE);
        assert_eq!(
            app.tabs.len(),
            1,
            "double-click must not duplicate the diff"
        );
        assert!(
            !app.tabs[app.active].is_preview,
            "double-click materializes"
        );
        assert_eq!(app.tabs[app.active].view, view, "the same view, not a copy");
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.focus_target(), FocusTarget::DiffEditor);
    }

    #[test]
    fn enter_on_an_explorer_file_materializes_it() {
        let dir = test_dir("explorer-enter-materialize");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        select_explorer_path(&mut app, &dir.join("a.rs"));

        // Enter opens the file materialized (not a preview) and focuses it.
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            !app.tabs[0].is_preview,
            "Enter materializes, never previews"
        );
        assert_eq!(app.focus, Focus::Editor);

        // Enter again re-focuses the same tab — no duplicate.
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1, "repeat Enter must not duplicate");
        assert_eq!(app.focus, Focus::Editor);

        // And Enter on a file currently in the preview slot materializes that
        // same tab in place.
        app.close_all_tabs();
        app.open_path_preview(&dir.join("a.rs"), false);
        assert!(app.tabs[0].is_preview);
        let view = app.tabs[0].view;
        app.focus = Focus::Sidebar;
        app.dispatch(Command::SidebarActivate);
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tabs[0].is_preview);
        assert_eq!(app.tabs[0].view, view, "the same view, not a copy");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_preview_and_a_diff_preview_share_the_panes_one_slot() {
        let dir = test_dir("shared-preview-slot");
        write_file(&dir, "c.rs", b"fn c() {}\n");
        let mut app = App::new(
            dir.clone(),
            vec![change("a.rs", StatusKind::Modified)],
            Vec::new(),
            false,
        );
        // A previewed file occupies the slot…
        app.open_path_preview(&dir.join("c.rs"), false);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);
        assert!(!app.tabs[0].is_diff());

        // …a previewed diff replaces it in place…
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.preview_selected_diff();
        assert_eq!(app.tabs.len(), 1, "one preview slot per pane, any content");
        assert!(app.tabs[0].is_preview);
        assert!(app.tabs[0].is_diff());

        // …and a previewed file takes it back.
        app.open_path_preview(&dir.join("c.rs"), false);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);
        assert!(!app.tabs[0].is_diff());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stepping_changed_files_walks_the_scm_list() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate); // opens a.rs (index 0)
        app.dispatch(Command::NextChangedFile);
        assert_eq!(app.scm.selection.cursor(), 1);
        app.dispatch(Command::PrevChangedFile);
        assert_eq!(app.scm.selection.cursor(), 0);
    }

    #[test]
    fn toggle_diff_layout_flips_view() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate);
        let before = matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::Unified,
                ..
            }
        );
        app.dispatch(Command::ToggleDiffLayout);
        let after = matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::SideBySide,
                ..
            }
        );
        assert!(before && after);
        // The choice persists: the next opened diff adopts the remembered layout.
        assert_eq!(app.diff_layout, ViewMode::SideBySide);
        app.scm.selection.move_to(1);
        app.dispatch(Command::SidebarActivate);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::SideBySide,
                ..
            }
        ));
    }

    #[test]
    fn toggle_sidebar_and_focus() {
        let mut app = app();
        app.dispatch(Command::ToggleSidebar);
        assert!(!app.sidebar_visible);
        app.dispatch(Command::ToggleFocus);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn opening_same_file_focuses_existing_tab() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-open-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");
        let _ = std::fs::write(&b, "fn b() {}\n");

        let mut app = app();
        app.open_path(&a);
        assert_eq!(app.tabs.len(), 1, "first open replaces the welcome tab");
        app.open_path(&a);
        assert_eq!(
            app.tabs.len(),
            1,
            "re-opening the same file focuses, not duplicates"
        );
        app.open_path(&b);
        assert_eq!(app.tabs.len(), 2);
        app.open_path(&a); // focuses a's existing tab rather than appending
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_open_replaces_the_current_preview_tab_in_place() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let b = dir.join("b.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");
        let _ = std::fs::write(&b, "fn b() {}\n");

        let mut app = app();
        app.open_path_preview(&a, true);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            app.tabs[0].is_preview,
            "a preview-opened file is marked preview"
        );
        assert_eq!(app.tabs[0].path(), Some(a.as_path()));

        // Navigating to a second file replaces the preview tab in place — no
        // second tab, and the old one's path is gone.
        app.open_path_preview(&b, true);
        assert_eq!(
            app.tabs.len(),
            1,
            "opening another preview must replace, not append"
        );
        assert!(app.tabs[0].is_preview);
        assert_eq!(app.tabs[0].path(), Some(b.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_open_on_an_already_open_permanent_tab_just_focuses_it() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-focus-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");

        let mut app = app();
        app.open_path(&a); // permanent open (not preview)
        assert!(!app.tabs[0].is_preview);

        app.open_path_preview(&a, true);
        assert_eq!(app.tabs.len(), 1, "must not duplicate an already-open file");
        assert!(
            !app.tabs[0].is_preview,
            "focusing an already-open permanent tab must not turn it into a preview"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn double_click_promotes_the_preview_tab_without_duplicating_it() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-promote-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let a = dir.join("a.rs");
        let _ = std::fs::write(&a, "fn a() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer.ensure_built(&dir);
        app.explorer.select_visible(
            app.explorer
                .rows()
                .iter()
                .position(|r| r.label == "a.rs")
                .expect("a.rs is listed"),
        );
        app.sidebar_promote_or_open_permanent();
        assert_eq!(app.tabs.len(), 1, "not yet open: opens one permanent tab");
        assert!(!app.tabs[0].is_preview);

        // Re-open as preview, then double-click-promote the existing tab: still
        // exactly one tab, now permanent.
        app.close_all_tabs();
        app.open_path_preview(&a, true);
        assert!(app.tabs[0].is_preview);
        app.sidebar_promote_or_open_permanent();
        assert_eq!(app.tabs.len(), 1, "promoting must not open a duplicate tab");
        assert!(
            !app.tabs[0].is_preview,
            "double-click clears the preview flag"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn editing_a_preview_tab_promotes_it_permanently() {
        let dir = std::env::temp_dir().join(format!(
            "karet-preview-edit-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "ab").expect("write temp file");

        let (session, mut events, mut snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path_preview(&path, true);
        pump(&mut app, &mut events).await;
        assert!(app.tabs[app.active].is_preview);

        app.dispatch(Command::InsertChar('x'));
        pump(&mut app, &mut events).await;
        // The dirty flag (and thus the promote-on-edit hook) is only ever set from
        // a document snapshot, not from the optimistic local apply in submit_edit.
        while let Ok(Some((doc, snap))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), snaps.recv()).await
        {
            app.on_snapshot(doc, &snap);
        }
        assert!(
            !app.tabs[app.active].is_preview,
            "the first edit must permanently promote the preview tab"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_dirty_preview_is_not_silently_replaced_by_the_next_preview() {
        // Editing a preview promotes it (see `editing_a_preview_tab_...`), so by the
        // time the next preview opens the edited tab is no longer the preview slot:
        // it survives, its document is not discarded, and the close guard (#51) is
        // never asked to drop unsaved work.
        let dir = test_dir("preview-dirty-safety");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "ab").expect("write a");
        std::fs::write(&b, "cd").expect("write b");

        let (session, mut events, mut snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);

        app.open_path_preview(&a, true);
        pump(&mut app, &mut events).await;
        assert!(app.tabs[app.active].is_preview);

        // Edit a.txt so a snapshot marks it dirty (and thus permanent).
        app.dispatch(Command::InsertChar('x'));
        pump(&mut app, &mut events).await;
        while let Ok(Some((doc, snap))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), snaps.recv()).await
        {
            app.on_snapshot(doc, &snap);
        }
        assert!(!app.tabs[app.active].is_preview, "the edit promoted a.txt");
        assert!(app.tabs[app.active].dirty, "a.txt has unsaved changes");

        // Now preview b.txt. The dirty a.txt tab must NOT be replaced — it has no
        // preview flag — so b.txt opens as a second tab and a.txt is kept safe.
        app.open_path_preview(&b, true);
        pump(&mut app, &mut events).await;
        assert_eq!(app.tabs.len(), 2, "the dirty tab is kept, not replaced");
        let a_tab = app
            .tabs
            .iter()
            .find(|t| t.path().map(canonical) == Some(canonical(&a)))
            .expect("a.txt is still open");
        assert!(a_tab.dirty, "a.txt keeps its unsaved changes");
        assert!(!a_tab.is_preview);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn explorer_row_index(app: &App, label: &str) -> usize {
        app.explorer
            .rows()
            .iter()
            .position(|r| r.label == label)
            .unwrap_or_else(|| panic!("missing explorer row {label}"))
    }

    #[test]
    fn arrowing_the_explorer_previews_files_without_stealing_focus() {
        let dir = test_dir("explorer-arrow-preview");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        let a_idx = explorer_row_index(&app, "a.rs");
        let b_idx = explorer_row_index(&app, "b.rs");
        app.explorer.select_index(a_idx);

        // Arrow onto b.rs: it opens in the pane's preview slot, and the sidebar
        // keeps keyboard focus so the user can keep arrowing.
        app.sidebar_step((b_idx as i32 - a_idx as i32).signum());
        assert_eq!(app.focus, Focus::Sidebar, "arrowing must not steal focus");
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview, "the arrowed-to file is a preview");
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("b.rs")))
        );

        // Arrow back onto a.rs: the single preview slot is reused, never appended.
        app.sidebar_step((a_idx as i32 - b_idx as i32).signum());
        assert_eq!(
            app.tabs.len(),
            1,
            "one preview slot is reused, not appended"
        );
        assert!(app.tabs[0].is_preview);
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );
        assert_eq!(app.focus, Focus::Sidebar);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wheel_scrolling_the_explorer_moves_selection_without_previewing() {
        let dir = test_dir("explorer-wheel-no-preview");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        let before = app.explorer.cursor();

        // A wheel notch moves the selection but must not open anything —
        // scrolling past files must not thrash the preview slot.
        app.sidebar_wheel(1, 3);
        assert_ne!(app.explorer.cursor(), before, "the wheel moves selection");
        assert_eq!(app.tabs.len(), 1);
        assert!(
            matches!(app.tabs[0].kind, TabKind::Welcome),
            "the wheel must not open a preview"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arrowing_onto_a_directory_row_leaves_the_editor_untouched() {
        let dir = test_dir("explorer-arrow-dir");
        write_file(&dir, "sub/nested.rs", b"fn n() {}\n");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        // Preview a.rs first, then arrow onto the `sub` directory row.
        let a_idx = explorer_row_index(&app, "a.rs");
        app.explorer.select_index(a_idx);
        app.preview_selected_explorer_row();
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].is_preview);

        let sub_idx = explorer_row_index(&app, "sub");
        assert!(
            app.explorer.rows()[sub_idx].is_dir,
            "sub is a directory row"
        );
        app.explorer.select_index(sub_idx);
        app.preview_selected_explorer_row();
        // Landing on a directory changes nothing: the a.rs preview stays as-is.
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.tabs[0].path().map(canonical),
            Some(canonical(&dir.join("a.rs")))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn arrowing_onto_an_already_open_file_activates_it_without_a_new_tab() {
        let dir = test_dir("explorer-arrow-permanent");
        write_file(&dir, "a.rs", b"fn a() {}\n");
        write_file(&dir, "b.rs", b"fn b() {}\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.focus = Focus::Sidebar;
        app.explorer.ensure_built(&dir);
        // b.rs is open as a permanent tab (not a preview).
        app.open_path(&dir.join("b.rs"));
        assert!(!app.tabs[0].is_preview);
        app.focus = Focus::Sidebar;

        // Arrow onto b.rs from a.rs: it activates the existing permanent tab
        // rather than opening a preview, and does not steal focus.
        let a_idx = explorer_row_index(&app, "a.rs");
        let b_idx = explorer_row_index(&app, "b.rs");
        app.explorer.select_index(a_idx);
        app.sidebar_step((b_idx as i32 - a_idx as i32).signum());
        assert_eq!(app.tabs.len(), 1, "must not duplicate the open file");
        assert!(
            !app.tabs[0].is_preview,
            "an already-permanent tab stays permanent"
        );
        assert_eq!(app.active, 0);
        assert_eq!(app.focus, Focus::Sidebar);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_highlights_matches_in_a_code_tab() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;

        let mut app = app();
        app.push_tab(Tab::new(
            "t.rs",
            TabKind::Code {
                path: PathBuf::from("t.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text("foo bar foo"),
                text: "foo bar foo".to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        app.dispatch(Command::OpenFind);
        if let Some(find) = app.active_find_mut() {
            find.query = "foo".to_string();
        }
        app.run_find();
        assert_eq!(app.active_find().map(|f| f.count), Some(2));
        if let TabKind::Code { decos, .. } = &app.tabs[app.active].kind {
            assert_eq!(decos.len(), 2);
        } else {
            unreachable!("active tab is a code tab");
        }
        // Closing find clears the highlights.
        app.close_find();
        if let TabKind::Code { decos, .. } = &app.tabs[app.active].kind {
            assert!(decos.is_empty());
        }
    }

    #[test]
    fn global_search_collects_matching_files() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("a.txt"), "needle here\n");
        let _ = std::fs::write(dir.join("b.txt"), "nothing\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 1);
        assert!(app.search.results[0].path.ends_with("a.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn go_to_commit_input_opens_reprompts_and_cancels() {
        let mut app = app();
        app.dispatch(Command::OpenCommitByHash);
        assert_eq!(app.rev_input.as_deref(), Some(""));
        // Submitting an empty revision re-prompts rather than closing.
        app.dispatch(Command::RevInputSubmit);
        assert_eq!(app.rev_input.as_deref(), Some(""));
        // Cancel clears the input.
        app.dispatch(Command::RevInputCancel);
        assert!(app.rev_input.is_none());
    }

    #[test]
    fn file_history_requires_an_open_file() {
        let mut app = app();
        // The Welcome tab has no path — file history has nothing to show.
        app.dispatch(Command::ShowFileHistory);
        assert_eq!(
            app.status.as_deref(),
            Some("file history: open a file first")
        );
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    }

    #[test]
    fn commit_graph_browser_opens_fills_and_clamps_navigation() {
        let mut app = app();
        app.dispatch(Command::ShowCommitGraph);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::CommitGraph { .. }
        ));

        // Backend is None in unit tests, so feed a history page directly.
        let commit = |hash: &str, summary: &str, parents: Vec<String>| Commit {
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: summary.to_string(),
            author: "Tester".to_string(),
            time: 0,
            parents,
        };
        app.apply_graph_log(
            0,
            vec![
                commit("aaaa", "c1", vec!["bbbb".to_string()]),
                commit("bbbb", "c0", Vec::new()),
            ],
            false,
        );
        if let TabKind::CommitGraph { commits, .. } = &app.tabs[app.active].kind {
            assert_eq!(commits.len(), 2);
        }
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 0, .. }
        ));
        // Down twice clamps at the last commit; up past the top clamps at 0.
        app.graph_select(1);
        app.graph_select(1);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 1, .. }
        ));
        app.graph_select(-5);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { selected: 0, .. }
        ));
    }

    #[test]
    fn parse_rev_range_distinguishes_two_and_three_dot() {
        assert_eq!(
            parse_rev_range("main..feature"),
            Some(("main".to_string(), "feature".to_string(), false))
        );
        assert_eq!(
            parse_rev_range("main...feature"),
            Some(("main".to_string(), "feature".to_string(), true))
        );
        // An omitted side defaults to HEAD, and whitespace is trimmed.
        assert_eq!(
            parse_rev_range("origin/main.. "),
            Some(("origin/main".to_string(), "HEAD".to_string(), false))
        );
        assert_eq!(
            parse_rev_range("...HEAD"),
            Some(("HEAD".to_string(), "HEAD".to_string(), true))
        );
        // A plain revision is not a range.
        assert_eq!(parse_rev_range("HEAD~2"), None);
        assert_eq!(parse_rev_range("abc123"), None);
    }

    #[test]
    fn open_compare_tab_builds_a_compare_tab() {
        let mut app = app();
        app.open_compare_tab(
            "main".to_string(),
            "HEAD".to_string(),
            true,
            vec![change("a.rs", StatusKind::Modified)],
        );
        match &app.tabs[app.active].kind {
            TabKind::Compare {
                base_label,
                head_label,
                merge_base,
                files,
                scroll,
            } => {
                assert_eq!(base_label, "main");
                assert_eq!(head_label, "HEAD");
                assert!(*merge_base);
                assert_eq!(files.len(), 1);
                assert_eq!(*scroll, 0);
            },
            _ => panic!("expected a compare tab"),
        }
        // A compare tab scrolls via the shared pager arm.
        app.scroll_lines(2);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Compare { scroll: 2, .. }
        ));
    }

    #[test]
    fn graph_compare_requires_a_marked_base() {
        let mut app = app();
        app.dispatch(Command::ShowCommitGraph);
        app.apply_graph_log(
            0,
            vec![
                Commit {
                    hash: "aaaa".to_string(),
                    short_hash: "aaaa".to_string(),
                    summary: "c1".to_string(),
                    author: "T".to_string(),
                    time: 0,
                    parents: vec!["bbbb".to_string()],
                },
                Commit {
                    hash: "bbbb".to_string(),
                    short_hash: "bbbb".to_string(),
                    summary: "c0".to_string(),
                    author: "T".to_string(),
                    time: 0,
                    parents: Vec::new(),
                },
            ],
            false,
        );
        // Comparing before marking a base only reports a status hint.
        app.dispatch(Command::CommitGraphCompare);
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("mark a compare base")),
            "compare without a base nudges the user"
        );
        // Marking a base records it on the browser tab.
        app.dispatch(Command::CommitGraphMarkBase);
        assert!(matches!(
            &app.tabs[app.active].kind,
            TabKind::CommitGraph { compare_base: Some(b), .. } if b == "aaaa"
        ));
    }

    #[test]
    fn commit_view_scrolls_by_wheel_and_page_and_edges() {
        let mut app = app();
        // Build a standalone commit view with one changed file.
        let detail = CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: karet_vcs::Identity {
                name: "Tester".to_string(),
                email: "t@example.com".to_string(),
                time: 0,
                offset: 0,
            },
            committer: karet_vcs::Identity {
                name: "Tester".to_string(),
                email: "t@example.com".to_string(),
                time: 0,
                offset: 0,
            },
            parents: Vec::new(),
            signature: None,
        };
        let files = vec![FileView::new(
            change("a.rs", StatusKind::Modified),
            crate::render::Section::Staged,
            false,
        )];
        app.push_tab(Tab::commit(Box::new(detail), files));
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit { scroll: 0, .. }
        ));

        // A wheel notch / ScrollDown advances the offset (the draw-time clamp caps it).
        app.scroll_lines(3);
        let scrolled = match app.tabs[app.active].kind {
            TabKind::Commit { scroll, .. } => scroll,
            _ => unreachable!(),
        };
        assert_eq!(scrolled, 3, "the commit view scrolls on a wheel notch");

        // Bottom pins to u16::MAX (clamped against content only during draw); Top returns to 0.
        app.scroll_edge(false);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit {
                scroll: u16::MAX,
                ..
            }
        ));
        app.scroll_edge(true);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit { scroll: 0, .. }
        ));
    }

    #[test]
    fn double_click_badge_reveals_and_wakes_to_hide() {
        let mut app = app();
        let id = || karet_vcs::Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        let detail = CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: id(),
            committer: id(),
            parents: Vec::new(),
            signature: None,
        };
        let files = vec![FileView::new(
            change("a.rs", StatusKind::Modified),
            crate::render::Section::Staged,
            false,
        )];
        app.push_tab(Tab::commit(Box::new(detail), files));
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        };
        app.pane_frames = vec![content_frame(&app, area)];
        // Pretend the last frame placed the badge here.
        let badge = Rect {
            x: 20,
            y: 3,
            width: 8,
            height: 1,
        };
        app.commit_badge_rect = Some(badge);
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // A single click on the badge does not reveal (needs a double-click).
        app.handle_editor_click(click(22, 3));
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit {
                explain_since: None,
                ..
            }
        ));

        // A second, quick click over the same cell reveals the explanation.
        app.handle_editor_click(click(22, 3));
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::Commit {
                explain_since: Some(_),
                ..
            }
        ));

        // The loop is now scheduled to wake within the reveal window so it can repaint
        // and hide the tooltip.
        let wake = app.next_wake().expect("a reveal is pending");
        assert!(wake <= COMMIT_REVEAL && wake > Duration::ZERO);
    }

    #[test]
    fn global_search_highlights_matches_in_an_already_open_tab() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-search-decos-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("a.txt");
        let _ = std::fs::write(&file, "needle here\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&file);

        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 1);
        match &app.tabs[app.active].kind {
            TabKind::Code { search_decos, .. } => assert_eq!(search_decos.len(), 1),
            _ => unreachable!("expected a code tab"),
        }

        // Clearing the query must clear the highlights too, not leave them stale.
        app.search.query.clear();
        app.run_global_search();
        match &app.tabs[app.active].kind {
            TabKind::Code { search_decos, .. } => assert!(search_decos.is_empty()),
            _ => unreachable!("expected a code tab"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fs_changed_event_reruns_a_live_global_search() {
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-app-fschanged-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.search.query = "needle".to_string();
        app.run_global_search();
        assert_eq!(app.search.results.len(), 0, "no matching file exists yet");

        // A file matching the live query appears on disk...
        let file = dir.join("new.txt");
        let _ = std::fs::write(&file, "needle here\n");
        // ...and the watcher's debounced event is what tells the app to look again.
        app.on_backend_event(None, SessionEvent::FsChanged { paths: vec![file] });
        assert_eq!(
            app.search.results.len(),
            1,
            "FsChanged must re-run the live search"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blame_without_a_code_tab_reports_status() {
        let mut app = app();
        // The Welcome tab is active — there is nothing to blame.
        app.dispatch(Command::ShowBlame);
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
        assert_eq!(app.status.as_deref(), Some("blame: open a text file first"));
    }

    #[test]
    fn blame_outside_a_repo_surfaces_an_error() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;

        // A scratch directory that is not a git repository.
        let n = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "karet-blame-{}-{}",
            std::process::id(),
            n.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("orphan.rs");
        let _ = std::fs::write(&file, "fn main() {}\n");

        let mut app = app();
        app.push_tab(Tab::new(
            "orphan.rs",
            TabKind::Code {
                path: file,
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text("fn main() {}\n"),
                text: "fn main() {}\n".to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        app.dispatch(Command::ShowBlame);

        // No Blame tab is created; the failure is surfaced as an error notification.
        assert!(!matches!(app.tabs[app.active].kind, TabKind::Blame { .. }));
        let active = app.notifications.active();
        assert!(
            active
                .iter()
                .any(|n| n.severity == Severity::Error && n.title.starts_with("blame:"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A single focused-pane frame whose content covers `rect`, so editor-click
    /// tests route through the pane hit-testing.
    fn content_frame(app: &App, rect: Rect) -> PaneFrame {
        PaneFrame {
            pane: app.focus_pane(),
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            breadcrumb_rect: Rect::default(),
            breadcrumb_hits: Vec::new(),
            content_rect: rect,
        }
    }

    fn code_tab(name: &str) -> Tab {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        Tab::new(
            name,
            TabKind::Code {
                path: PathBuf::from(name),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text("x\n"),
                text: "x\n".to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        )
    }

    #[test]
    fn tab_navigation_wraps_and_jumps() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.push_tab(code_tab("c.rs"));
        assert_eq!(app.active, 2);
        app.next_tab();
        assert_eq!(app.active, 0, "next wraps to the first tab");
        app.prev_tab();
        assert_eq!(app.active, 2, "prev wraps to the last tab");
        app.go_to_tab(1);
        assert_eq!(app.active, 0);
        app.go_to_tab(9);
        assert_eq!(app.active, 2, "9 selects the last tab");
    }

    #[test]
    fn move_active_tab_reorders_and_clamps() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.active = 0;
        app.move_active_tab(1);
        assert_eq!(app.tabs[1].title, "a.rs");
        assert_eq!(app.active, 1);
        app.move_active_tab(1); // already last: clamped, no change
        assert_eq!(app.active, 1);
    }

    fn text_tab(name: &str, text: &str) -> Tab {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        Tab::new(
            name,
            TabKind::Code {
                path: PathBuf::from(name),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text(text),
                text: text.to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        )
    }

    #[test]
    fn wrap_mode_uses_file_defaults_and_global_overrides() {
        let markdown = text_tab("notes.md", "a long prose line");
        let rust = text_tab("main.rs", "fn main() {}");
        assert!(effective_word_wrap(&markdown, None));
        assert!(!effective_word_wrap(&rust, None));
        assert!(!effective_word_wrap(&markdown, Some(false)));
        assert!(effective_word_wrap(&rust, Some(true)));
    }

    #[test]
    fn word_wrap_resolves_against_the_tab_language() {
        let settings = Settings {
            editor: serde_json::from_str(
                r#"{
                    "wordWrap": false,
                    "[rust]": { "wordWrap": true }
                }"#,
            )
            .unwrap_or_default(),
            ..Settings::default()
        };
        let rust = text_tab("main.rs", "fn main() {}");
        let resolved = settings
            .editor
            .for_language(tab_language(&rust))
            .word_wrap();
        assert!(effective_word_wrap(&rust, resolved));

        let mut python = text_tab("main.py", "print('hi')");
        if let TabKind::Code { language, .. } = &mut python.kind {
            *language = "Python";
        }
        let resolved = settings
            .editor
            .for_language(tab_language(&python))
            .word_wrap();
        assert!(!effective_word_wrap(&python, resolved));
    }

    #[test]
    fn horizontal_mouse_events_scroll_only_overflow_views() {
        let mut app = app();
        app.sidebar_visible = false;
        app.push_tab(text_tab(
            "main.rs",
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz\nsecond\nthird\nfourth",
        ));
        let _ = screen(&mut app, 24, 8);
        let column = app.editor_rect.x.saturating_add(5);
        let row = app.editor_rect.y;
        let mouse = |kind, modifiers| MouseEvent {
            kind,
            column,
            row,
            modifiers,
        };

        app.handle_mouse(mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, KeyModifiers::SHIFT));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 0);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, KeyModifiers::SHIFT));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 3);
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_line, 3);

        app.tabs[app.active] = text_tab("notes.md", "prose that is much wider than the pane");
        let _ = screen(&mut app, 24, 8);
        app.handle_mouse(mouse(MouseEventKind::ScrollRight, KeyModifiers::NONE));
        assert_eq!(app.tabs[app.active].editor.scroll_col, 0);
    }

    /// An app with one Markdown code tab, in a pane wide enough to split.
    fn markdown_app(text: &str) -> App {
        let mut app = app();
        let mut tab = text_tab("notes.md", text);
        if let TabKind::Code { language, .. } = &mut tab.kind {
            *language = "Markdown";
        }
        app.push_tab(tab);
        app.main_rect = Rect::new(0, 0, 80, 24);
        app
    }

    /// The preview tab of the only stored (non-focused) pane, if any.
    fn stored_preview(app: &App) -> Option<&Tab> {
        app.stored
            .values()
            .flat_map(|pane| pane.tabs.iter())
            .find(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
    }

    #[test]
    fn markdown_preview_opens_a_pane_to_the_side_and_keeps_focus_on_the_source() {
        let mut app = markdown_app("# Title\n\nbody\n");
        let source_view = app.tabs[app.active].view;
        let source_pane = app.focus_pane();

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2);
        assert_eq!(app.focus_pane(), source_pane, "focus stays in the editor");
        assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
        let preview = stored_preview(&app).expect("a preview tab in the new pane");
        assert!(App::previews_view(preview, source_view));
        assert_eq!(preview.title, "Preview notes.md");
    }

    #[test]
    fn markdown_preview_is_a_no_op_on_a_non_markdown_tab() {
        let mut app = app();
        app.push_tab(text_tab("main.rs", "fn main() {}"));
        app.main_rect = Rect::new(0, 0, 80, 24);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1, "no pane was opened");
        assert!(stored_preview(&app).is_none());
        assert!(app.status.is_some(), "the refusal is surfaced, not silent");
    }

    #[test]
    fn re_invoking_markdown_preview_reveals_the_existing_one() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        assert_eq!(app.layout.pane_count(), 2);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2, "no second preview pane");
        // Revealing focuses the preview itself.
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
    }

    #[test]
    fn a_pane_too_narrow_to_split_gets_the_preview_as_a_tab() {
        let mut app = markdown_app("# Title\n");
        app.main_rect = Rect::new(0, 0, 4, 2);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
    }

    #[test]
    fn a_markdown_preview_keeps_its_document_open() {
        // `reconcile_open_docs` ref-counts through `tab_doc`, so a preview must report the
        // document it mirrors or closing the source tab would close the document under it.
        let mut app = markdown_app("# Title\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(7));
        }
        app.dispatch(Command::MarkdownPreviewSide);

        let preview = stored_preview(&app).expect("a preview tab");
        assert_eq!(App::tab_doc(preview), Some(DocumentId(7)));
    }

    #[test]
    fn a_markdown_preview_is_a_pager_for_the_keymap() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        app.dispatch(Command::MarkdownPreviewSide); // reveal: focuses the preview
        assert_eq!(app.active_editor_tab(), EditorTab::Pager);
    }

    #[test]
    fn scrolling_a_markdown_preview_moves_it_within_the_wrapped_document() {
        let mut app = markdown_app("# Title\n");
        app.dispatch(Command::MarkdownPreviewSide);
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview

        // Nothing is wrapped until the first draw, so the scroll is pinned at the top.
        app.dispatch(Command::ScrollDown);
        let TabKind::MarkdownPreview {
            wrapped, scroll, ..
        } = &mut app.tabs[app.active].kind
        else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 0, "an unwrapped preview cannot scroll");

        // Stand in for a draw by wrapping the document, then scroll for real.
        *wrapped = karet_markdown::parse("a\n\nb\n\nc\n").wrap(20);
        app.dispatch(Command::ScrollDown);
        app.dispatch(Command::ScrollDown);
        let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 2);

        app.dispatch(Command::ScrollUp);
        let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(*scroll, 1);
    }

    #[test]
    fn a_snapshot_refreshes_the_markdown_preview_buffer() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        let mut app = markdown_app("# Title\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(3));
        }
        app.dispatch(Command::MarkdownPreviewSide);

        let buffer = TextBuffer::from_text("# Changed\n");
        let version = buffer.version();
        app.on_snapshot(
            DocumentId(3),
            &DocSnapshot {
                version,
                buffer,
                highlights: Arc::new(Highlights::default()),
                semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
                folds: Arc::new(FoldRegions::default()),
                decorations: Arc::new(Vec::new()),
                syntax_error_lines: Arc::new(Vec::new()),
                language: Some("Markdown"),
                dirty: true,
                cursor: None,
            },
        );

        let preview = stored_preview(&app).expect("a preview tab");
        let TabKind::MarkdownPreview { buffer, .. } = &preview.kind else {
            panic!("expected a preview tab");
        };
        assert_eq!(buffer.text(), "# Changed\n");
    }

    /// A source doc whose blocks sit on known lines: headings on 0, 2, 4, 6.
    const SYNC_DOC: &str = "# a\n\n# b\n\n# c\n\n# d\n";

    /// Open a preview for `SYNC_DOC` and give it a wrapped model, standing in for the
    /// first draw (which is what normally populates it).
    fn synced_app() -> App {
        let mut app = markdown_app(SYNC_DOC);
        app.dispatch(Command::MarkdownPreviewSide);
        let preview = app
            .stored_active_mut(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
            .expect("a preview tab");
        if let TabKind::MarkdownPreview { wrapped, .. } = &mut preview.kind {
            *wrapped = karet_markdown::parse(SYNC_DOC).wrap(40);
        }
        app
    }

    /// The preview's scroll, wherever the preview currently lives.
    fn preview_scroll(app: &App) -> u16 {
        let find = |t: &Tab| match &t.kind {
            TabKind::MarkdownPreview { scroll, .. } => Some(*scroll),
            _ => None,
        };
        app.tabs
            .iter()
            .chain(app.stored.values().flat_map(|p| p.tabs.iter()))
            .find_map(find)
            .expect("a preview tab")
    }

    /// The source tab's scroll, wherever it lives.
    fn source_scroll(app: &App) -> u32 {
        app.tabs
            .iter()
            .chain(app.stored.values().flat_map(|p| p.tabs.iter()))
            .find(|t| matches!(t.kind, TabKind::Code { .. }))
            .expect("a source tab")
            .editor
            .scroll_line
    }

    #[test]
    fn scrolling_the_source_scrolls_the_preview_to_the_matching_block() {
        let mut app = synced_app();
        // Source line 4 is the third heading; it renders on wrapped line 4 ("# a", "",
        // "# b", "", "# c").
        app.tabs[app.active].editor.scroll_line = 4;
        app.sync_markdown_preview();
        assert_eq!(preview_scroll(&app), 4);
        assert_eq!(source_scroll(&app), 4, "the driver never moves itself");
    }

    #[test]
    fn scrolling_the_preview_scrolls_the_source_back() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));

        for _ in 0..4 {
            app.dispatch(Command::ScrollDown);
        }
        app.sync_markdown_preview();
        assert_eq!(preview_scroll(&app), 4, "the driver never moves itself");
        assert_eq!(source_scroll(&app), 4);
    }

    #[test]
    fn merely_focusing_the_preview_does_not_nudge_the_source() {
        let mut app = synced_app();
        app.tabs[app.active].editor.scroll_line = 3; // a blank line, mid-round-trip
        app.sync_markdown_preview();
        let settled = preview_scroll(&app);

        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        app.sync_markdown_preview();

        assert_eq!(
            source_scroll(&app),
            3,
            "a bare focus change must not move it"
        );
        assert_eq!(preview_scroll(&app), settled);
    }

    #[test]
    fn syncing_is_idempotent_and_cannot_oscillate() {
        let mut app = synced_app();
        app.tabs[app.active].editor.scroll_line = 3;
        for _ in 0..10 {
            app.sync_markdown_preview();
        }
        let (source, preview) = (source_scroll(&app), preview_scroll(&app));

        app.dispatch(Command::MarkdownPreviewSide); // hand the wheel to the preview
        for _ in 0..10 {
            app.sync_markdown_preview();
        }
        assert_eq!(source_scroll(&app), source, "the pair settled, not drifted");
        assert_eq!(preview_scroll(&app), preview);
    }

    #[test]
    fn syncing_a_source_with_no_preview_is_a_no_op() {
        let mut app = markdown_app(SYNC_DOC);
        app.tabs[app.active].editor.scroll_line = 2;
        app.sync_markdown_preview();
        assert_eq!(source_scroll(&app), 2);
    }

    #[test]
    fn a_preview_scrolled_past_the_source_clamps_to_the_last_source_line() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview
        if let TabKind::MarkdownPreview { scroll, .. } = &mut app.tabs[app.active].kind {
            *scroll = u16::MAX;
        }
        app.sync_markdown_preview();
        let last = app
            .stored_active(|t| matches!(t.kind, TabKind::Code { .. }))
            .and_then(|t| match &t.kind {
                TabKind::Code { buffer, .. } => Some(buffer.line_count().saturating_sub(1) as u32),
                _ => None,
            })
            .expect("a source tab");
        assert_eq!(source_scroll(&app), last, "clamped to the last buffer line");
    }

    /// A standalone DOCX-style preview tab over `md`, with a wrapped model (standing
    /// in for the first draw) and an initial `scroll`.
    #[cfg(feature = "docx")]
    fn docx_preview_tab(md: &str, scroll: u16) -> Tab {
        let mut tab = Tab::document_preview(PathBuf::from("report.docx"), md);
        if let TabKind::MarkdownPreview {
            wrapped, scroll: s, ..
        } = &mut tab.kind
        {
            *wrapped = karet_markdown::parse(md).wrap(40);
            *s = scroll;
        }
        tab
    }

    /// A source scrolled to line 4 drives a *real* preview to wrapped line 4 (see
    /// `scrolling_the_source_scrolls_the_preview_to_the_matching_block`); a detached
    /// document preview must never be adopted as that source's preview.
    #[cfg(feature = "docx")]
    #[test]
    fn a_docx_preview_is_never_adopted_by_a_markdown_source() {
        let mut app = synced_app();
        // Swap the stored real preview for a detached docx preview at scroll 0.
        let preview = app
            .stored_active_mut(|t| matches!(t.kind, TabKind::MarkdownPreview { .. }))
            .expect("a preview tab");
        preview.kind = docx_preview_tab(SYNC_DOC, 0).kind;

        app.tabs[app.active].editor.scroll_line = 4;
        app.sync_markdown_preview();

        assert_eq!(
            preview_scroll(&app),
            0,
            "the sentinel source_view must not pair with a real source"
        );
        assert_eq!(source_scroll(&app), 4, "the source itself is unaffected");
    }

    /// A *real* focused preview at scroll 4 writes the source back to line 4 (see
    /// `scrolling_the_preview_scrolls_the_source_back`); a focused detached preview
    /// must drive nothing.
    #[cfg(feature = "docx")]
    #[test]
    fn a_focused_docx_preview_never_drives_a_stored_source() {
        let mut app = synced_app();
        app.dispatch(Command::MarkdownPreviewSide); // focus the preview pane
        // Turn the focused preview into a detached docx preview, scrolled well away
        // from where the stored source (line 0) projects.
        app.tabs[app.active].kind = docx_preview_tab(SYNC_DOC, 4).kind;

        app.sync_markdown_preview();

        assert_eq!(
            source_scroll(&app),
            0,
            "a detached preview must not scroll any source tab"
        );
        // And its own scroll is left alone (nothing wrote it back).
        if let TabKind::MarkdownPreview { scroll, .. } = &app.tabs[app.active].kind {
            assert_eq!(*scroll, 4);
        }
    }

    /// Invoking the preview command over a markdown source must open a fresh real
    /// preview, not reveal/hijack an open docx preview (whose sentinel `source_view`
    /// can never match the source's view).
    #[cfg(feature = "docx")]
    #[test]
    fn preview_side_opens_a_real_preview_instead_of_hijacking_a_docx_tab() {
        let mut app = markdown_app(SYNC_DOC);
        app.push_tab(docx_preview_tab("# doc", 0));
        app.select_tab(0); // back to the markdown source
        let source_view = app.tabs[app.active].view;

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 2, "a new preview pane opened");
        let preview = stored_preview(&app).expect("a preview tab in the new pane");
        assert!(
            App::previews_view(preview, source_view),
            "the new preview pairs with the source"
        );
        // The docx tab is still in the source pane, untouched.
        assert!(app.tabs.iter().any(
            |t| matches!(&t.kind, TabKind::MarkdownPreview { source_view, .. }
                if *source_view == crate::tab::DETACHED_SOURCE_VIEW)
        ));
    }

    /// The preview command refuses politely on a focused docx preview — there is no
    /// markdown source file behind it to preview.
    #[cfg(feature = "docx")]
    #[test]
    fn preview_side_is_a_no_op_on_a_focused_docx_preview() {
        let mut app = app();
        app.push_tab(docx_preview_tab("# doc", 0));
        app.main_rect = Rect::new(0, 0, 80, 24);

        app.dispatch(Command::MarkdownPreviewSide);

        assert_eq!(app.layout.pane_count(), 1, "no pane was opened");
        assert!(app.status.is_some(), "the refusal is surfaced, not silent");
    }

    /// The unified close guard (#51) protects dirty *documents*; a docx preview has
    /// none (`tab_doc` is `None`), so closing it never prompts — even if the dirty
    /// flag were somehow set — and `reconcile_open_docs` has nothing to release.
    #[cfg(feature = "docx")]
    #[test]
    fn closing_a_docx_preview_never_arms_the_close_guard() {
        let mut app = app();
        app.push_tab(docx_preview_tab("# doc", 0));
        let view = app.tabs[app.active].view;
        assert_eq!(App::tab_doc(&app.tabs[app.active]), None);
        app.tabs[app.active].dirty = true; // impossible in practice; the guard still passes

        assert!(app.docs_at_risk(CloseRequest::Tab { view }).is_empty());
        app.guarded_close(CloseRequest::Tab { view });

        assert!(app.pending_close.is_none(), "no confirmation was armed");
        assert!(
            !app.tabs.iter().any(|t| t.view == view),
            "the tab closed immediately"
        );
    }

    /// Document snapshots refresh previews by their bound `DocumentId`; a docx
    /// preview is bound to none, so no snapshot can ever overwrite its content.
    #[cfg(feature = "docx")]
    #[test]
    fn a_snapshot_never_touches_a_docx_preview() {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        let mut app = app();
        app.push_tab(docx_preview_tab("# original", 0));

        let buffer = TextBuffer::from_text("# changed\n");
        let version = buffer.version();
        app.on_snapshot(
            DocumentId(7),
            &DocSnapshot {
                version,
                buffer,
                highlights: Arc::new(Highlights::default()),
                semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
                folds: Arc::new(FoldRegions::default()),
                decorations: Arc::new(Vec::new()),
                syntax_error_lines: Arc::new(Vec::new()),
                language: Some("Markdown"),
                dirty: true,
                cursor: None,
            },
        );

        let TabKind::MarkdownPreview { buffer, .. } = &app.tabs[app.active].kind else {
            panic!("expected the docx preview tab");
        };
        assert_eq!(buffer.text(), "# original");
    }

    /// A minimal DOCX zipped in-memory (no fixture on disk).
    #[cfg(feature = "docx")]
    fn tiny_docx() -> Vec<u8> {
        use std::io::Write as _;
        const DOCUMENT_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("start_file");
        writer
            .write_all(DOCUMENT_XML.as_bytes())
            .expect("write_all");
        writer.finish().expect("finish");
        buf
    }

    #[cfg(feature = "docx")]
    #[test]
    fn reopening_the_same_docx_focuses_the_existing_tab() {
        let dir = test_dir("docx-dedup");
        let file = dir.join("report.docx");
        std::fs::write(&file, tiny_docx()).expect("write the docx");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        app.open_path(&file);
        assert!(matches!(
            app.tabs[app.active].kind,
            TabKind::MarkdownPreview { .. }
        ));
        assert_eq!(app.tabs.len(), 1);
        let view = app.tabs[app.active].view;

        // Move focus elsewhere, then open the same file again.
        app.push_tab(text_tab("other.rs", "fn x() {}"));
        assert_eq!(app.tabs.len(), 2);
        app.open_path(&file);

        assert_eq!(app.tabs.len(), 2, "no duplicate tab for the same document");
        assert_eq!(app.tabs[app.active].view, view, "the existing tab focused");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Draw the whole shell into a test terminal and return the screen, row by row.
    fn screen(app: &mut App, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal
            .draw(|f| crate::ui::draw(f, app))
            .expect("draw the shell");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect()
    }

    /// End-to-end: the shell splits, wraps and paints the preview through `ui::draw`.
    ///
    /// A list is the giveaway — the source pane shows `- one`, the rendered preview shows
    /// a `•` bullet — so this proves the preview is rendered, not echoed source.
    #[test]
    fn the_preview_pane_paints_rendered_markdown_beside_the_source() {
        let mut app = markdown_app("- one\n- two\n");
        app.dispatch(Command::MarkdownPreviewSide);

        let painted = screen(&mut app, 100, 12).join("\n");
        assert!(
            painted.contains("- one"),
            "the source pane still shows markup:\n{painted}"
        );
        assert!(
            painted.contains('\u{2022}'),
            "the preview pane should render a bullet:\n{painted}"
        );
    }

    /// The draw-time render cache is keyed on the document version, so an edit re-renders
    /// the preview on the next frame. Drives the edit through `TextBuffer::apply` — the
    /// same path the session takes — because that is what moves the version.
    #[test]
    fn editing_the_source_re_renders_the_preview_on_the_next_draw() {
        let mut app = markdown_app("# before\n");
        if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
            *doc = Some(DocumentId(11));
        }
        app.dispatch(Command::MarkdownPreviewSide);
        let before = screen(&mut app, 100, 12).join("\n");
        assert!(before.contains("before"), "{before}");

        // "# before" -> "# after": delete "before", insert "after". Applying bumps the
        // version, which is exactly what invalidates the cache.
        let mut edited = karet_text::TextBuffer::from_text("# before\n");
        let change = karet_core::Change::new(
            edited.version(),
            vec![karet_core::TextEdit {
                range: Range {
                    start: LineCol::new(0, 2),
                    end: LineCol::new(0, 8),
                },
                new_text: "after".to_string(),
            }],
        );
        edited.apply_simple(&change).expect("apply the edit");
        assert!(edited.version() > 0, "the edit must move the version");

        for tab in app.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Code { buffer, text, .. } => {
                    *buffer = edited.content_snapshot();
                    *text = edited.text();
                },
                TabKind::MarkdownPreview { buffer, .. } => *buffer = edited.content_snapshot(),
                _ => {},
            }
        }

        let after = screen(&mut app, 100, 12).join("\n");
        assert!(
            after.contains("after") && !after.contains("before"),
            "the preview must re-render once the document version moves:\n{after}"
        );
    }

    #[test]
    fn selection_text_slices_the_source() {
        use karet_text::TextBuffer;
        let src = "foo bar\nbaz";
        let buffer = TextBuffer::from_text(src);
        let range = Range {
            start: LineCol::new(0, 4),
            end: LineCol::new(1, 3),
        };
        assert_eq!(
            selection_text(&buffer, src, range).as_deref(),
            Some("bar\nbaz")
        );
    }

    #[test]
    fn copy_reports_status() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello world"));
        app.focus = Focus::Editor;
        app.dispatch(Command::SelectRight);
        app.dispatch(Command::Copy);
        assert_eq!(app.status.as_deref(), Some("copied selection"));
    }

    #[test]
    fn select_line_end_then_select_all_dispatch_in_editor() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello world\nsecond"));
        app.focus = Focus::Editor;
        // Shift+End selects from the caret to the end of the line.
        app.dispatch(Command::SelectLineEnd);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 11),
            })
        );
        // Ctrl+A selects the whole buffer.
        app.dispatch(Command::EditorSelectAll);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(1, 6),
            })
        );
    }

    #[test]
    fn caret_line_end_moves_without_selecting() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello"));
        app.focus = Focus::Editor;
        app.dispatch(Command::CaretLineEnd);
        assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 5));
        assert_eq!(app.tabs[app.active].editor.selection_range(), None);
    }

    #[test]
    fn add_cursor_below_then_esc_collapses() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "ab\ncd"));
        app.focus = Focus::Editor;
        app.dispatch(Command::AddCursorBelow);
        assert!(app.tabs[app.active].editor.has_multiple_cursors());
        // Esc with several carets collapses to the primary, keeping editor focus.
        app.dispatch(Command::CollapseCarets);
        assert!(!app.tabs[app.active].editor.has_multiple_cursors());
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn esc_with_a_single_caret_keeps_editor_focus() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "ab"));
        app.focus = Focus::Editor;
        app.dispatch(Command::CollapseCarets);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn alt_click_adds_a_second_caret() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        let click = |col, mods| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: mods,
        };
        app.handle_editor_click(click(3, KeyModifiers::NONE));
        app.handle_editor_click(click(8, KeyModifiers::ALT));
        assert!(app.tabs[app.active].editor.has_multiple_cursors());
    }

    #[test]
    fn double_click_selects_the_word() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        // Two quick clicks over the 'a' of "bar" (buffer col 5 -> screen col 8).
        let click = |col| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_editor_click(click(8));
        app.handle_editor_click(click(8));
        let sel = app.tabs[app.active].editor.selection_range();
        assert_eq!(
            sel,
            Some(Range {
                start: LineCol::new(0, 4),
                end: LineCol::new(0, 7),
            })
        );
    }

    #[test]
    fn shift_click_extends_selection_to_the_click() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.pane_frames = vec![content_frame(
            &app,
            Rect {
                x: 0,
                y: 0,
                width: 40,
                height: 5,
            },
        )];
        let click = |col, shift| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 0,
            modifiers: if shift {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            },
        };
        // Place the caret at buffer col 0 (screen col 3 past the gutter), then
        // Shift+click at buffer col 5 (screen col 8) to extend the selection.
        app.handle_editor_click(click(3, false));
        app.handle_editor_click(click(8, true));
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 5),
            })
        );
    }

    #[test]
    fn shift_arrow_extends_then_plain_arrow_clears() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "hello"));
        app.focus = Focus::Editor;
        app.dispatch(Command::SelectRight);
        app.dispatch(Command::SelectRight);
        assert_eq!(
            app.tabs[app.active].editor.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 2),
            })
        );
        app.dispatch(Command::CaretLeft);
        assert_eq!(app.tabs[app.active].editor.selection_range(), None);
    }

    #[test]
    fn tab_at_maps_columns_to_tabs_and_close() {
        let hits = vec![
            TabHit {
                start: 0,
                end: 10,
                close: 8,
            },
            TabHit {
                start: 10,
                end: 20,
                close: 18,
            },
        ];
        assert_eq!(tab_at(&hits, 3), Some((0, false)));
        assert_eq!(tab_at(&hits, 8), Some((0, true)));
        assert_eq!(tab_at(&hits, 12), Some((1, false)));
        assert_eq!(tab_at(&hits, 18), Some((1, true)));
        assert_eq!(tab_at(&hits, 25), None);
    }

    #[test]
    fn status_segment_click_dispatches_its_command() {
        let mut app = app();
        app.status_rect = Rect {
            x: 0,
            y: 9,
            width: 80,
            height: 1,
        };
        app.status_hits = vec![
            (0, 9, Command::ToggleFocus),
            (12, 19, Command::OpenQuickOpen),
        ];
        assert_eq!(app.status_command_at(3), Some(Command::ToggleFocus));
        assert_eq!(app.status_command_at(15), Some(Command::OpenQuickOpen));
        assert_eq!(app.status_command_at(40), None);
        // Clicking the focus segment toggles focus.
        let before = app.focus;
        app.handle_status_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 9,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(app.focus, before);
    }

    #[test]
    fn sidebar_header_click_switches_panel() {
        let mut app = app();
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.panel_hits = vec![
            (23, 25, SidebarPanel::Explorer),
            (25, 27, SidebarPanel::Search),
            (27, 29, SidebarPanel::SourceControl),
        ];
        app.handle_sidebar_click(25, 1, KeyModifiers::NONE); // header row, the "2" cell
        assert_eq!(app.sidebar_panel, SidebarPanel::Search);
    }

    #[test]
    fn sidebar_click_selects_and_opens_scm_change() {
        let mut app = app(); // staged a.rs, working b.rs
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 2,
            width: 30,
            height: 8,
        };
        // Clicks hit-test against the changes region rect.
        app.scm_changes_rect = Rect {
            x: 0,
            y: 2,
            width: 30,
            height: 8,
        };
        app.scm_offset = 0;
        // Display rows: 0 header, 1 a.rs(0), 2 header, 3 b.rs(1).
        app.scm_row_map = vec![None, Some(0), None, Some(1)];
        app.handle_sidebar_click(2, 5, KeyModifiers::NONE); // content row 3 -> change index 1
        assert_eq!(app.scm.selection.cursor(), 1);
        assert!(app.active_is_diff());

        // Ctrl-click a second row adds it to the selection without opening a diff.
        app.handle_sidebar_click(2, 3, KeyModifiers::CONTROL); // content row 1 -> index 0
        assert_eq!(app.scm.selection.selected_indices(), vec![0, 1]);
    }

    #[test]
    fn dragging_moves_the_active_tab() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.push_tab(code_tab("c.rs"));
        app.pane_frames = vec![PaneFrame {
            pane: app.focus_pane(),
            tabstrip_rect: Rect::default(),
            tab_hits: vec![
                TabHit {
                    start: 0,
                    end: 8,
                    close: 6,
                },
                TabHit {
                    start: 8,
                    end: 16,
                    close: 14,
                },
                TabHit {
                    start: 16,
                    end: 24,
                    close: 22,
                },
            ],
            breadcrumb_rect: Rect::default(),
            breadcrumb_hits: Vec::new(),
            content_rect: Rect::default(),
        }];
        app.active = 0;
        app.tab_drag = Some(TabDrag {
            from_pane: app.focus_pane(),
            hover: None,
        });
        app.drag_tab_to(20); // over the third tab
        let titles: Vec<_> = app.tabs.iter().map(|t| t.title.clone()).collect();
        assert_eq!(titles, vec!["b.rs", "c.rs", "a.rs"]);
        assert_eq!(app.active, 2);
    }

    #[test]
    fn drop_tab_on_right_edge_creates_a_second_pane() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        let from = app.focus_pane();
        let dragged = app.tabs[app.active].title.clone();
        app.drop_tab_on(from, DropZone::Right);
        assert_eq!(app.layout.pane_count(), 2);
        // Focus moved to the new pane, holding the dragged tab.
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].title, dragged);
        // The origin pane survives with its remaining tab(s), in storage.
        assert!(app.stored.contains_key(&from));
    }

    #[test]
    fn dropping_the_only_tab_on_an_edge_keeps_one_pane() {
        let mut app = app();
        app.push_tab(code_tab("only.rs"));
        let from = app.focus_pane();
        // The sole tab can't leave an empty origin pane behind, so the split
        // collapses back to a single pane holding it.
        app.drop_tab_on(from, DropZone::Bottom);
        assert_eq!(app.layout.pane_count(), 1);
        assert_eq!(app.tabs[0].title, "only.rs");
    }

    #[test]
    fn keyboard_split_opens_a_second_view_and_focus_cycles() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        let from = app.focus_pane();
        app.dispatch(Command::SplitRight);
        assert_eq!(app.layout.pane_count(), 2);
        let new_pane = app.focus_pane();
        assert_ne!(new_pane, from);
        // The new pane holds a duplicate view of the active document.
        assert_eq!(app.tabs[0].title, "a.rs");
        // Focus cycles to the origin pane and back.
        app.dispatch(Command::FocusPrevPane);
        assert_eq!(app.focus_pane(), from);
        app.dispatch(Command::FocusNextPane);
        assert_eq!(app.focus_pane(), new_pane);
    }

    #[test]
    fn drop_tab_center_on_self_is_a_noop() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        let from = app.focus_pane();
        app.drop_tab_on(from, DropZone::Center);
        assert_eq!(app.layout.pane_count(), 1);
    }

    #[test]
    fn close_other_tabs_keeps_the_active_one() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.push_tab(code_tab("c.rs"));
        app.active = 1;
        app.close_other_tabs();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].title, "b.rs");
        assert_eq!(app.active, 0);
    }

    #[test]
    fn closing_remembers_path_and_reopen_restores_it() {
        let dir = std::env::temp_dir().join(format!("karet-reopen-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("a.rs");
        let _ = std::fs::write(&file, "fn main() {}\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(workspace::open_file(&file));
        app.push_tab(code_tab("scratch"));
        app.active = 0;
        app.close_tab_at(0);
        assert_eq!(app.closed.last(), Some(&file));
        app.reopen_closed_tab();
        assert!(app.tabs.iter().any(|t| t.path() == Some(file.as_path())));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_header_toolbar_click_begins_new_file() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        app.header_action_hits = vec![
            (20, 22, Command::ExplorerNewFile),
            (22, 24, Command::ExplorerNewFolder),
            (24, 26, Command::ExplorerRefresh),
            (26, 28, Command::ExplorerCollapseAll),
        ];
        // Clicking the "new file" button on the header row starts an inline edit.
        app.handle_sidebar_click(20, 1, KeyModifiers::NONE);
        assert!(app.explorer.is_editing());
    }

    #[test]
    fn explorer_blank_area_click_does_not_open_the_last_row() {
        let dir = test_dir("blank-click");
        write_file(&dir, "a.txt", b"a");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_rect = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        };
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 7,
        };
        app.explorer.ensure_built(&dir);

        app.handle_sidebar_click(1, 5, KeyModifiers::NONE);

        assert!(
            !app.tabs
                .iter()
                .any(|tab| tab.path() == Some(dir.join("a.txt").as_path()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_commit_edit_creates_a_file() {
        let dir = std::env::temp_dir().join(format!("karet-newfile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer_begin_new(false);
        for c in "hello.txt".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();
        assert!(dir.join("hello.txt").exists());
        assert!(!app.explorer.is_editing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_explorer_create_keeps_inline_name_for_retry() {
        let dir = std::env::temp_dir().join(format!("karet-newfile-fail-{}", std::process::id()));
        let existing = dir.join("existing");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&existing, "already here");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer_begin_new(true);
        for c in "existing".chars() {
            app.explorer.edit_push(c);
        }

        app.explorer_commit_edit();

        assert!(app.explorer.is_editing());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_copy_paste_file_uses_copy_suffix() {
        let dir = test_dir("copy-file");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        app.dispatch(Command::Copy);
        app.dispatch(Command::Paste);

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_copy_paste_directory_recursively_into_selected_directory() {
        let dir = test_dir("copy-dir");
        write_file(&dir, "src/nested/file.txt", b"nested");
        write_file(&dir, "src/marker.txt", b"marker");
        let _ = std::fs::create_dir_all(dir.join("dst"));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("src"));
        app.dispatch(Command::Copy);
        select_explorer_path(&mut app, &dir.join("dst"));
        app.dispatch(Command::Paste);

        assert_eq!(
            std::fs::read(dir.join("dst/src/nested/file.txt")).unwrap_or_default(),
            b"nested"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_cut_paste_moves_files_and_clears_clipboard() {
        let dir = test_dir("cut-file");
        write_file(&dir, "move.txt", b"move");
        let _ = std::fs::create_dir_all(dir.join("dst"));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("move.txt"));
        app.dispatch(Command::Cut);
        select_explorer_path(&mut app, &dir.join("dst"));
        app.dispatch(Command::Paste);

        assert!(!dir.join("move.txt").exists());
        assert_eq!(
            std::fs::read(dir.join("dst/move.txt")).unwrap_or_default(),
            b"move"
        );
        assert!(app.explorer_clipboard.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_duplicate_file_uses_copy_suffix() {
        let dir = test_dir("duplicate-file");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("a.txt"));
        app.dispatch(Command::ExplorerDuplicate);

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_expands_ancestors_and_selects_nested_file() {
        let dir = test_dir("reveal-nested");
        write_file(&dir, "a/b/c.rs", b"code");
        write_file(&dir, "a/note.txt", b"note");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        // Start from a different panel/focus to prove the reveal switches them.
        app.sidebar_panel = SidebarPanel::Search;
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        let target = dir.join("a/b/c.rs");
        app.reveal_in_explorer(&target);

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_selects_a_directory() {
        let dir = test_dir("reveal-dir");
        write_file(&dir, "a/b/c.rs", b"code");
        write_file(&dir, "a/note.txt", b"note");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        let target = dir.join("a/b");
        app.reveal_in_explorer(&target);

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_outside_root_is_noop_with_status() {
        let dir = test_dir("reveal-outside");
        write_file(&dir, "inside.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        let outside = dir
            .parent()
            .map(|p| p.join("elsewhere.txt"))
            .unwrap_or_else(|| PathBuf::from("/elsewhere.txt"));
        app.reveal_in_explorer(&outside);

        // Nothing changes but a status note.
        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::Editor);
        assert!(
            app.status.as_deref().is_some_and(|s| s.contains("outside")),
            "status: {:?}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_missing_path_reports_status() {
        let dir = test_dir("reveal-missing");
        write_file(&dir, "inside.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        app.reveal_in_explorer(&dir.join("does-not-exist.txt"));

        // A path under the root but absent from the tree does not steal focus.
        assert!(!app.sidebar_visible);
        assert_eq!(app.focus, Focus::Editor);
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("not in the explorer")),
            "status: {:?}",
            app.status
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_scrolls_selection_into_view() {
        let dir = test_dir("reveal-scroll");
        for i in 0..30 {
            write_file(&dir, &format!("d/f{i:02}.txt"), b"x");
        }
        write_file(&dir, "d/target.txt", b"needle");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

        let target = dir.join("d/target.txt");
        app.reveal_in_explorer(&target);
        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));

        // Render a short terminal: the tree clamps its offset to the cursor, so the
        // revealed row scrolls into view even though it sits far below the fold.
        let painted = screen(&mut app, 100, 12).join("\n");
        assert!(
            painted.contains("target.txt"),
            "revealed row not scrolled into view:\n{painted}"
        );
        assert!(
            app.explorer.offset() > 0,
            "the tree did not scroll to reveal the selection"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reveal_in_explorer_of_the_root_focuses_the_explorer_without_reselecting() {
        let dir = test_dir("reveal-root");
        write_file(&dir, "top.txt", b"x");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        select_explorer_path(&mut app, &dir.join("top.txt"));
        app.sidebar_visible = false;
        app.focus = Focus::Editor;

        // The root has no row of its own: revealing it shows and focuses the
        // Explorer but leaves the selection where it was.
        app.reveal_in_explorer(&dir);

        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(
            app.explorer.selected_path(),
            Some(dir.join("top.txt").as_path())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A frame whose breadcrumb row is at `y = 1` (columns 10..50) with one
    /// clickable segment at columns 12..15 resolving to `segment`, over a content
    /// rect that deliberately overlaps the breadcrumb row — so a swallowed click
    /// is distinguishable from one that fell through to the editor.
    fn breadcrumb_frame(app: &App, segment: PathBuf) -> PaneFrame {
        PaneFrame {
            pane: app.focus_pane(),
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            breadcrumb_rect: Rect {
                x: 10,
                y: 1,
                width: 40,
                height: 1,
            },
            breadcrumb_hits: vec![BreadcrumbHit {
                start: 12,
                end: 15,
                path: segment,
            }],
            content_rect: Rect {
                x: 10,
                y: 1,
                width: 40,
                height: 10,
            },
        }
    }

    #[test]
    fn clicking_a_breadcrumb_segment_reveals_its_path_in_the_explorer() {
        let dir = test_dir("breadcrumb-click");
        write_file(&dir, "a/b/c.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_visible = false;
        app.focus = Focus::Editor;
        let target = dir.join("a/b");
        app.pane_frames = vec![breadcrumb_frame(&app, target.clone())];

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 13,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.explorer.selected_path(), Some(target.as_path()));
        assert!(app.sidebar_visible);
        assert_eq!(app.sidebar_panel, SidebarPanel::Explorer);
        assert_eq!(app.focus, Focus::Sidebar);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_breadcrumb_gap_click_is_swallowed_not_forwarded_to_the_editor() {
        let dir = test_dir("breadcrumb-gap");
        write_file(&dir, "a/b/c.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.focus = Focus::Sidebar;
        app.pane_frames = vec![breadcrumb_frame(&app, dir.join("a/b"))];

        // Column 16 is past the segment (a separator gap): the click lands on the
        // breadcrumb row but maps to no segment. Had it fallen through, the editor
        // click handler (whose content rect overlaps the row) would steal focus.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 16,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.focus, Focus::Sidebar, "the gap click fell through");
        assert_eq!(app.explorer.selected_path(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_frame_records_breadcrumb_hits_only_within_the_workspace() {
        let dir = test_dir("breadcrumb-frame");
        write_file(&dir, "a/b.rs", b"code");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.open_path(&dir.join("a/b.rs"));

        let painted = screen(&mut app, 200, 20).join("\n");
        assert!(
            painted.contains('\u{203a}'),
            "the breadcrumb separator did not paint:\n{painted}"
        );

        let frame = app.pane_frames.first().expect("a pane frame");
        assert_eq!(frame.breadcrumb_rect.height, 1);
        let paths: Vec<_> = frame
            .breadcrumb_hits
            .iter()
            .map(|h| h.path.clone())
            .collect();
        // Segments above the workspace root ("/", "tmp", …) are inert: only the
        // root itself and the components below it are recorded.
        assert_eq!(paths, vec![dir.clone(), dir.join("a"), dir.join("a/b.rs")]);
        // Spans are ordered, non-overlapping, and inside the breadcrumb row.
        for pair in frame.breadcrumb_hits.windows(2) {
            assert!(pair[0].end < pair[1].start, "segments overlap or touch");
        }
        for hit in &frame.breadcrumb_hits {
            assert!(hit.start >= frame.breadcrumb_rect.x);
            assert!(hit.end <= frame.breadcrumb_rect.right());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_delete_requires_confirmation() {
        let dir = test_dir("delete-file");
        write_file(&dir, "gone.txt", b"delete");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("gone.txt"));
        app.dispatch(Command::ExplorerDelete);
        assert!(dir.join("gone.txt").exists());
        assert!(app.pending_explorer_delete.is_some());

        app.dispatch(Command::ConfirmExplorerDelete);
        assert!(!dir.join("gone.txt").exists());
        assert!(app.pending_explorer_delete.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_context_menu_accepts_the_selected_file_command() {
        let dir = test_dir("context-duplicate");
        write_file(&dir, "a.txt", b"alpha");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.explorer.ensure_built(&dir);
        let Some(row) = app
            .explorer
            .rows()
            .iter()
            .position(|row| row.path == dir.join("a.txt"))
        else {
            return;
        };

        app.open_context_menu(2, 2, Some(row));
        let Some(menu) = app.context_menu.as_mut() else {
            return;
        };
        let Some(duplicate) = menu
            .entries
            .iter()
            .position(|entry| entry.command == Command::ExplorerDuplicate)
        else {
            return;
        };
        menu.selected = duplicate;
        app.accept_context_menu();

        assert_eq!(
            std::fs::read(dir.join("a copy.txt")).unwrap_or_default(),
            b"alpha"
        );
        assert!(app.context_menu.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_keyboard_context_menu_uses_blank_items_when_empty() {
        let dir = test_dir("context-empty");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;
        app.sidebar_content_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };

        app.open_context_menu_for_selection();

        let Some(menu) = app.context_menu.as_ref() else {
            return;
        };
        let has = |cmd: Command| menu.entries.iter().any(|entry| entry.command == cmd);
        assert!(has(Command::ExplorerNewFile));
        assert!(has(Command::ExplorerNewFolder));
        assert!(!has(Command::SidebarActivate));
        assert!(!has(Command::ExplorerRename));
        assert!(
            menu.entries.iter().all(|entry| entry.enabled),
            "explorer menu entries stay enabled"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_menu_opens_on_the_first_enabled_entry_and_skips_disabled_on_nav() {
        let dir = test_dir("context-skip-disabled");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.context_menu = Some(ContextMenu::new(
            2,
            2,
            vec![
                ContextMenuEntry::disabled(Command::CopyPath, "no file"),
                ContextMenuEntry::enabled(Command::CopyRelativePath),
                ContextMenuEntry::disabled(Command::Quit, "blocked"),
                ContextMenuEntry::enabled(Command::ExplorerRefresh),
            ],
        ));
        let selected = |app: &App| app.context_menu.as_ref().map(|m| m.selected);
        // The initial selection lands on the first enabled row, not row 0.
        assert_eq!(selected(&app), Some(1));
        // Down skips the disabled row 2 and lands on 3; another Down stays put.
        app.dispatch(Command::ContextMenuDown);
        assert_eq!(selected(&app), Some(3));
        app.dispatch(Command::ContextMenuDown);
        assert_eq!(selected(&app), Some(3));
        // Up skips row 2 back to 1; another Up stays (row 0 is disabled).
        app.dispatch(Command::ContextMenuUp);
        assert_eq!(selected(&app), Some(1));
        app.dispatch(Command::ContextMenuUp);
        assert_eq!(selected(&app), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A committed git repo (one tracked `new.rs`) with `origin` set to `remote_url`,
    /// or `None` when `git` is unavailable (the test then skips).
    fn repo_with_remote(remote_url: &str) -> Option<TempRepo> {
        let repo = init_test_repo()?;
        if !git(&repo.path, &["add", "."])
            || !git(&repo.path, &["commit", "-q", "-m", "init"])
            || !git(&repo.path, &["remote", "add", "origin", remote_url])
        {
            return None;
        }
        Some(repo)
    }

    #[test]
    fn pane_context_menu_lists_file_actions_and_disables_links_outside_a_repo() {
        let dir = test_dir("pane-menu-norepo");
        write_file(&dir, "a.rs", b"x\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(dir.join("a.rs").to_string_lossy().as_ref()));

        app.open_pane_context_menu(3, 3);

        let Some(menu) = app.context_menu.as_ref() else {
            panic!("a file-backed tab opens a pane menu");
        };
        let commands: Vec<Command> = menu.entries.iter().map(|e| e.command).collect();
        assert_eq!(
            commands,
            vec![
                Command::CopyPath,
                Command::CopyRelativePath,
                Command::RevealActiveInExplorer,
                Command::CopyRemoteFileUrl,
                Command::OpenChangesWithPrevious,
                Command::OpenChangesWithRevision,
                Command::OpenChangesWithBranch,
                Command::CopyGithubPermalink,
                Command::CopyGithubHeadLink,
            ]
        );
        assert!(menu.entries[..3].iter().all(|e| e.enabled));
        for entry in &menu.entries[3..] {
            assert!(
                !entry.enabled,
                "{:?} is disabled outside a repo",
                entry.command
            );
            assert_eq!(entry.note.as_deref(), Some("not in a git repository"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_context_menu_does_not_open_for_a_pathless_tab() {
        let dir = test_dir("pane-menu-welcome");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));

        app.open_pane_context_menu(3, 3);

        assert!(app.context_menu.is_none(), "a pathless tab opens no menu");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn right_click_opens_the_pane_menu_from_the_tab_strip_and_the_content_area() {
        let dir = test_dir("pane-menu-mouse");
        write_file(&dir, "a.rs", b"x\n");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(dir.join("a.rs").to_string_lossy().as_ref()));
        let mut frame = content_frame(&app, Rect::new(0, 1, 40, 10));
        frame.tabstrip_rect = Rect::new(0, 0, 40, 1);
        frame.tab_hits = vec![TabHit {
            start: 0,
            end: 12,
            close: 11,
        }];
        app.pane_frames = vec![frame];
        let right = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // Over the tab in the strip: selects it and opens the menu.
        app.handle_mouse(right(4, 0));
        assert!(app.context_menu.is_some(), "tab-strip right-click opens");
        app.context_menu = None;

        // In the content area: opens for the pane's active tab.
        app.handle_mouse(right(5, 5));
        assert!(app.context_menu.is_some(), "content right-click opens");
        app.context_menu = None;

        // On the strip's empty tail (past the tab): consumed, no menu.
        app.handle_mouse(right(20, 0));
        assert!(app.context_menu.is_none(), "strip tail opens nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_menu_enables_github_links_for_a_tracked_file_on_github() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("new.rs"));
        assert!(
            entries.iter().all(|e| e.enabled),
            "github + tracked enables every row: {entries:?}"
        );
    }

    #[test]
    fn pane_menu_disables_github_links_for_a_gitlab_remote_with_a_note() {
        let Some(repo) = repo_with_remote("https://gitlab.com/owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("new.rs"));
        let by_cmd = |cmd: Command| entries.iter().find(|e| e.command == cmd);
        // The generic remote URL still works on GitLab…
        assert!(by_cmd(Command::CopyRemoteFileUrl).is_some_and(|e| e.enabled));
        // …while both GitHub links are disabled and name the detected forge.
        for cmd in [Command::CopyGithubPermalink, Command::CopyGithubHeadLink] {
            let Some(entry) = by_cmd(cmd) else {
                panic!("{cmd:?} is listed");
            };
            assert!(!entry.enabled);
            let note = entry.note.as_deref().unwrap_or_default();
            assert!(
                note.contains("GitLab") && note.contains("github.com"),
                "note names the forge: {note}"
            );
        }
    }

    #[test]
    fn pane_menu_disables_links_for_an_untracked_file() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        std::fs::write(repo.path.join("untracked.rs"), "y\n").unwrap_or_default();
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let entries = app.pane_context_entries(&repo.path.join("untracked.rs"));
        for cmd in [
            Command::CopyRemoteFileUrl,
            Command::CopyGithubPermalink,
            Command::CopyGithubHeadLink,
            Command::OpenChangesWithPrevious,
            Command::OpenChangesWithRevision,
            Command::OpenChangesWithBranch,
        ] {
            let Some(entry) = entries.iter().find(|e| e.command == cmd) else {
                panic!("{cmd:?} is listed");
            };
            assert!(!entry.enabled, "{cmd:?} is disabled for an untracked file");
            assert!(
                entry
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .contains("not tracked"),
                "note explains the untracked state"
            );
        }
    }

    #[test]
    fn remote_facts_reads_the_repository_state() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        let Ok(facts) = app.remote_facts(&repo.path.join("new.rs")) else {
            panic!("facts resolve inside a repo with an origin");
        };
        assert_eq!(facts.remote.kind, crate::remote::ForgeKind::GitHub);
        assert_eq!(facts.rel_path, PathBuf::from("new.rs"));
        assert!(facts.tracked);
        assert!(facts.head.is_some());
        assert!(facts.branch.is_some());
    }

    #[test]
    fn copy_github_permalink_reports_success_on_a_github_repo() {
        let Some(repo) = repo_with_remote("git@github.com:owner/repo.git") else {
            return;
        };
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab(
            repo.path.join("new.rs").to_string_lossy().as_ref(),
        ));
        app.dispatch(Command::CopyGithubPermalink);
        assert_eq!(app.status.as_deref(), Some("copied GitHub permalink"));
    }

    /// A git repo whose `new.rs` is committed (no remote), or `None` when `git`
    /// is unavailable (the test then skips).
    fn committed_repo() -> Option<TempRepo> {
        let repo = init_test_repo()?;
        if !git(&repo.path, &["add", "."]) || !git(&repo.path, &["commit", "-q", "-m", "init"]) {
            return None;
        }
        Some(repo)
    }

    /// A code tab over `path` whose live buffer holds `text`.
    fn code_tab_with_text(path: &Path, text: &str) -> Tab {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;
        Tab::new(
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string(),
            TabKind::Code {
                path: path.to_path_buf(),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text(text),
                text: text.to_string(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        )
    }

    #[test]
    fn open_changes_with_previous_diffs_head_against_the_live_buffer() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        // The live buffer differs from both HEAD and the (unchanged) disk file,
        // proving the working side comes from the buffer, not disk.
        app.push_tab(code_tab_with_text(&path, "fn main() { edited }\n"));

        app.dispatch(Command::OpenChangesWithPrevious);

        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("open changes opens a diff tab, got none");
        };
        assert_eq!(title, "new.rs (HEAD \u{2194} working)");
        assert_eq!(
            file.change.old, "fn main() {}\n",
            "old side is HEAD content"
        );
        assert_eq!(
            file.change.new, "fn main() { edited }\n",
            "new side is the live buffer"
        );
    }

    #[test]
    fn open_changes_with_revision_picks_from_the_file_history() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        // A second commit changes the file, so its history has two entries.
        std::fs::write(&path, "fn main() { v1 }\n").unwrap_or_default();
        if !git(&repo.path, &["commit", "-qam", "v1"]) {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "working\n"));

        app.dispatch(Command::OpenChangesWithRevision);
        let Some(overlay) = app.overlay.as_ref() else {
            panic!("the revision picker opens");
        };
        assert_eq!(overlay.title(), "Open Changes: With Revision");
        let rows: Vec<String> = overlay.rows().iter().map(ToString::to_string).collect();
        assert_eq!(rows.len(), 2, "both commits touch the file: {rows:?}");
        assert!(rows[0].contains("v1"), "newest first: {rows:?}");

        // Choose the older commit (the initial content).
        app.dispatch(Command::OverlayDown);
        app.dispatch(Command::OverlayAccept);

        assert!(app.overlay.is_none(), "accept closes the picker");
        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("accepting a revision opens a diff tab");
        };
        assert_eq!(
            file.change.old, "fn main() {}\n",
            "old side is the picked revision's content"
        );
        assert_eq!(file.change.new, "working\n");
        assert!(
            title.contains("\u{2194} working"),
            "title names the comparison: {title}"
        );
    }

    #[test]
    fn open_changes_with_branch_diffs_against_the_branch_tip() {
        let Some(repo) = committed_repo() else {
            return;
        };
        let path = repo.path.join("new.rs");
        // A `feature` branch changes the file; we come back to the default branch.
        if !git(&repo.path, &["checkout", "-q", "-b", "feature"]) {
            return;
        }
        std::fs::write(&path, "fn main() { feature }\n").unwrap_or_default();
        if !git(&repo.path, &["commit", "-qam", "feature change"])
            || !git(&repo.path, &["checkout", "-q", "-"])
        {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "working\n"));

        app.dispatch(Command::OpenChangesWithBranch);
        let Some(overlay) = app.overlay.as_ref() else {
            panic!("the branch picker opens");
        };
        assert_eq!(overlay.title(), "Open Changes: With Branch");
        let rows: Vec<String> = overlay.rows().iter().map(ToString::to_string).collect();
        assert!(
            rows.iter().any(|r| r.ends_with("(current)")),
            "the checked-out branch is marked: {rows:?}"
        );
        // Branches are sorted by name, so `feature` is first regardless of whether
        // the default branch is `main` or `master`.
        assert_eq!(rows[0], "feature");
        app.dispatch(Command::OverlayAccept);

        let Some(Tab {
            title,
            kind: TabKind::Diff { file, .. },
            ..
        }) = app.tabs.last()
        else {
            panic!("accepting a branch opens a diff tab");
        };
        assert_eq!(title, "new.rs (feature \u{2194} working)");
        assert_eq!(
            file.change.old, "fn main() { feature }\n",
            "old side is the branch tip's content"
        );
        assert_eq!(file.change.new, "working\n");
    }

    #[test]
    fn open_changes_reports_a_file_absent_at_the_revision() {
        let Some(repo) = committed_repo() else {
            return;
        };
        // `other.rs` only exists in the second commit, so HEAD~1 has no blob for it.
        let path = repo.path.join("other.rs");
        std::fs::write(&path, "x\n").unwrap_or_default();
        if !git(&repo.path, &["add", "."]) || !git(&repo.path, &["commit", "-qm", "add other"]) {
            return;
        }
        let mut app = App::new(repo.path.clone(), Vec::new(), Vec::new(), false);
        app.push_tab(code_tab_with_text(&path, "x\n"));

        let before = app.tabs.len();
        app.open_changes_with("HEAD~1", "HEAD~1");

        assert_eq!(app.tabs.len(), before, "no diff tab is opened");
        assert_eq!(
            app.status.as_deref(),
            Some("open changes: file does not exist at HEAD~1")
        );
    }

    #[test]
    fn context_menu_refuses_a_disabled_entry_and_surfaces_its_note() {
        let dir = test_dir("context-disabled-accept");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.context_menu = Some(ContextMenu::new(
            2,
            2,
            vec![
                ContextMenuEntry::disabled(Command::ExplorerNewFile, "not available here"),
                ContextMenuEntry::enabled(Command::ExplorerRefresh),
            ],
        ));
        // Force the selection onto the disabled row (as a mouse click would).
        if let Some(menu) = app.context_menu.as_mut() {
            menu.selected = 0;
        }
        app.dispatch(Command::ContextMenuAccept);
        // The command did not run, the menu stays open, and the note is surfaced.
        assert!(!app.explorer.is_editing(), "disabled command must not run");
        assert!(app.context_menu.is_some(), "menu stays open on refusal");
        assert_eq!(app.status.as_deref(), Some("not available here"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_paste_rejects_directory_into_its_descendant() {
        let dir = test_dir("copy-into-self");
        write_file(&dir, "src/child/file.txt", b"child");
        write_file(&dir, "src/marker.txt", b"marker");
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("src"));
        app.dispatch(Command::Copy);
        app.explorer.expand(&dir.join("src"));
        app.explorer.ensure_built(&dir);
        select_explorer_path(&mut app, &dir.join("src/child"));
        app.dispatch(Command::Paste);

        assert!(!dir.join("src/child/src").exists());
        assert_eq!(app.status.as_deref(), Some("paste failed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_rename_refreshes_vcs_status() {
        let dir = test_dir("rename-refresh");
        write_file(&dir, "old.txt", b"old");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;

        select_explorer_path(&mut app, &dir.join("old.txt"));
        app.explorer_begin_rename();
        for c in "new".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();

        assert!(dir.join("new.txt").exists());
        assert_eq!(refresh_count(&backend), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_rename_retargets_open_code_tabs() {
        let dir = test_dir("rename-retarget");
        let old = dir.join("old.txt");
        let new = dir.join("new.txt");
        write_file(&dir, "old.txt", b"old");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;
        let mut tab = code_tab("old.txt");
        if let TabKind::Code { path, doc, .. } = &mut tab.kind {
            *path = old.clone();
            *doc = Some(DocumentId(42));
        }
        app.push_tab(tab);

        select_explorer_path(&mut app, &old);
        app.explorer_begin_rename();
        for c in "new".chars() {
            app.explorer.edit_push(c);
        }
        app.explorer_commit_edit();

        assert!(
            app.tabs
                .iter()
                .any(|tab| tab.title == "new.txt" && tab.path() == Some(new.as_path()))
        );
        assert_eq!(retarget_commands(&backend), vec![(DocumentId(42), new)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explorer_paste_refreshes_vcs_status_after_success() {
        let dir = test_dir("paste-refresh");
        write_file(&dir, "a.txt", b"a");
        let backend = Arc::new(RecordingBackend::new());
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend.clone());
        app.sidebar_panel = SidebarPanel::Explorer;

        app.dispatch(Command::Copy);
        app.dispatch(Command::Paste);

        assert!(dir.join("a copy.txt").exists());
        assert_eq!(refresh_count(&backend), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Push a minimal empty code tab and open Find over it, for tests that only
    /// exercise the find-bar state machine (not real match content).
    fn app_with_find_open() -> App {
        use karet_syntax::Highlights;
        use karet_text::TextBuffer;

        let mut app = app();
        app.push_tab(Tab::new(
            "t.rs",
            TabKind::Code {
                path: PathBuf::from("t.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text(""),
                text: String::new(),
                highlights: Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        app.dispatch(Command::OpenFind);
        app
    }

    #[test]
    fn find_bar_toggle_field_reveals_and_switches_replace() {
        let mut app = app_with_find_open();
        assert!(app.active_find().is_some_and(|f| !f.replace_visible));
        app.find_toggle_field();
        assert!(
            app.active_find()
                .is_some_and(|f| f.replace_visible && f.field == SearchField::Replace)
        );
        app.find_toggle_field();
        assert!(
            app.active_find()
                .is_some_and(|f| f.field == SearchField::Find)
        );
    }

    #[test]
    fn find_input_edits_the_active_field() {
        let mut app = app_with_find_open();
        app.find_input(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.find_toggle_field(); // switch to the replace field
        app.find_input(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "a" && f.replace == "b")
        );
    }

    #[test]
    fn find_toggle_option_flips_the_flags() {
        let mut app = app_with_find_open();
        app.find_toggle_option(SearchOption::Regex);
        app.find_toggle_option(SearchOption::Word);
        assert!(
            app.active_find()
                .is_some_and(|f| f.regex && !f.case_sensitive && f.whole_word)
        );
    }

    #[test]
    fn find_state_survives_esc_and_is_restored_on_reopen() {
        // Regression: closing Find (Esc) used to discard the query/toggles;
        // reopening Find on the same tab must restore them instead of starting
        // blank.
        let mut app = app_with_find_open();
        if let Some(find) = app.active_find_mut() {
            find.query = "needle".to_string();
            find.regex = true;
        }
        app.close_find();
        assert_eq!(
            app.input_context().modal,
            None,
            "closing find must hide the bar"
        );
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "needle" && f.regex),
            "the tab's find data must survive Esc"
        );

        app.open_find();
        assert_eq!(app.input_context().modal, Some(crate::keymap::Modal::Find));
        assert!(
            app.active_find()
                .is_some_and(|f| f.query == "needle" && f.regex),
            "reopening find on the same tab must restore the prior query/toggles"
        );
    }

    #[test]
    fn switching_tabs_does_not_show_a_stale_find_bar() {
        let mut app = app_with_find_open();
        app.push_tab(Tab::new(
            "u.rs",
            TabKind::Code {
                path: PathBuf::from("u.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: karet_text::TextBuffer::from_text(""),
                text: String::new(),
                highlights: karet_syntax::Highlights::default(),
                semantic_blocks: karet_syntax::SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        ));
        assert_eq!(
            app.input_context().modal,
            None,
            "opening a second tab must not carry the first tab's open find bar over"
        );
        app.select_tab(0);
        assert_eq!(
            app.input_context().modal,
            None,
            "switching back must not resurrect the find bar either — only reopening it should"
        );
    }

    #[test]
    fn search_toggle_field_reveals_and_switches_replace() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        assert_eq!(app.search.field, SearchField::Find);
        // Collapse the replace field, then Tab reveals it and moves focus to it.
        app.search_toggle_replace();
        assert!(!app.search.replace_visible);
        app.search_toggle_field();
        assert!(app.search.replace_visible);
        assert_eq!(app.search.field, SearchField::Replace);
        assert!(app.search.input);
        // Tab again returns to the find field.
        app.search_toggle_field();
        assert_eq!(app.search.field, SearchField::Find);
    }

    #[test]
    fn search_edit_targets_the_active_field() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.search.field = SearchField::Find;
        app.search_edit(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        app.search.field = SearchField::Replace;
        app.search_edit(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert_eq!(app.search.query, "a");
        assert_eq!(app.search.replace, "b");
    }

    #[test]
    fn search_replace_all_rewrites_matching_files() {
        let dir = std::env::temp_dir().join(format!("karet-replace-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("a.txt"), "needle and needle\n");

        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Search;
        app.search.query = "needle".to_string();
        app.search.case_sensitive = true;
        app.search.replace = "pin".to_string();
        app.search_replace_all();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap_or_default(),
            "pin and pin\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_option_toggle_button_click_dispatches() {
        let mut app = App::new(PathBuf::from("."), Vec::new(), Vec::new(), false);
        app.sidebar_panel = SidebarPanel::Search;
        app.sidebar_rect = Rect {
            x: 0,
            y: 1,
            width: 30,
            height: 10,
        };
        // A "regex" toggle button on row 2, columns 20..22.
        app.search_action_hits = vec![(20, 22, 2, Command::SearchToggleRegex)];
        assert!(!app.search.regex);
        app.handle_sidebar_click(20, 2, KeyModifiers::NONE);
        assert!(app.search.regex);
    }

    // --- full-stack Source-Control action tests ------------------------------
    //
    // These drive the real `Session` + `local()` backend over a temp git repo, so
    // they exercise the whole key → focus/layer → dispatch → backend actor → git2 →
    // VcsStatus → apply loop that unit tests skip.

    /// A temp directory removed on drop, so a panicking test can't leak it.
    struct TempRepo {
        path: PathBuf,
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Run `git` in `dir`, returning whether it succeeded.
    fn git(dir: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A git repo in a fresh temp dir holding a single untracked file, or `None`
    /// when `git` is unavailable (so the test skips rather than fails).
    fn init_test_repo() -> Option<TempRepo> {
        use std::sync::atomic::AtomicUsize;
        use std::sync::atomic::Ordering;
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "karet-scm-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).ok()?;
        let repo = TempRepo { path };
        if !git(&repo.path, &["init", "-q"])
            || !git(&repo.path, &["config", "user.email", "test@example.com"])
            || !git(&repo.path, &["config", "user.name", "karet test"])
        {
            return None;
        }
        std::fs::write(repo.path.join("new.rs"), "fn main() {}\n").ok()?;
        Some(repo)
    }

    /// A bare key press (no modifiers).
    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Drain backend events into `app`, waiting briefly for the spawned actor.
    async fn pump(app: &mut App, events: &mut EventRx) {
        while let Ok(Some((id, ev))) =
            tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await
        {
            app.on_backend_event(id, ev);
        }
    }

    /// Build an app wired to a real session + local backend, focused on the SCM pane.
    fn scm_app(root: PathBuf) -> (App, EventRx) {
        let (session, events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(root, Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.focus = Focus::Sidebar;
        (app, events)
    }

    fn code_tab_text(app: &App) -> String {
        match &app.tabs[app.active].kind {
            TabKind::Code { text, .. } => text.clone(),
            _ => panic!("expected the active tab to be a code tab"),
        }
    }

    #[tokio::test]
    async fn backspace_and_insert_apply_to_the_local_buffer_without_waiting_for_a_snapshot() {
        // Regression for "editor jumps back / skips characters on backspace": edits
        // used to only move the caret optimistically while the displayed text
        // waited on an async snapshot echo, so a fast burst of keys raced ahead of
        // what was actually applied. Every edit below is dispatched back-to-back
        // with no `pump` in between, so this fails if `submit_edit` regresses to
        // only updating the caret again.
        let dir =
            std::env::temp_dir().join(format!("karet-edit-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "ab").expect("write temp file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await; // registers the doc so submit_edit can act

        app.dispatch(Command::InsertChar('x'));
        assert_eq!(code_tab_text(&app), "xab");
        app.dispatch(Command::InsertChar('y'));
        assert_eq!(code_tab_text(&app), "xyab");
        app.dispatch(Command::DeleteBackward);
        assert_eq!(code_tab_text(&app), "xab");
        app.dispatch(Command::DeleteBackward);
        assert_eq!(
            code_tab_text(&app),
            "ab",
            "two backspaces fired before any snapshot arrives must still land on the \
             locally-applied text"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn paste_while_find_is_open_targets_the_find_query_not_the_editor() {
        // Regression: paste used to always land in the main editor buffer
        // regardless of which text field was actually focused, so pasting while
        // Find was open silently replaced the editor's content/selection instead
        // of the find query.
        let dir = std::env::temp_dir().join(format!(
            "karet-pastefind-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("a.txt");
        std::fs::write(&path, "hello world").expect("write temp file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await;

        app.open_find();
        assert!(app.find_open);

        app.handle_paste("needle".to_string());

        assert_eq!(
            app.active_find().map(|f| f.query.as_str()),
            Some("needle"),
            "pasted text must land in the find query"
        );
        assert_eq!(
            code_tab_text(&app),
            "hello world",
            "paste while Find is open must not touch the editor buffer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_non_utf8_file_opens_read_only_instead_of_a_silently_dead_tab() {
        let dir =
            std::env::temp_dir().join(format!("karet-nonutf8-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("bad.rs");
        // A long valid-ASCII prefix (longer than the classifier's 8 KiB head sample)
        // followed by one invalid byte: the workspace-level classifier sees only
        // clean text and opens a normal code tab, but the session's full-file strict
        // UTF-8 load then fails — this is exactly the "misses a genuinely non-UTF-8
        // file" gap this fallback exists for, not the earlier (already-handled)
        // "obviously binary within the head sample" case.
        let mut bytes = vec![b'a'; 9000];
        bytes.push(0xff);
        std::fs::write(&path, &bytes).expect("write invalid-utf8 file");

        let (session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![dir.clone()],
            ..SessionConfig::default()
        });
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
        app.backend = Some(backend);
        app.open_path(&path);
        pump(&mut app, &mut events).await;

        assert!(
            matches!(app.tabs[app.active].kind, TabKind::Hex { .. }),
            "a non-UTF-8 file must fall back to the read-only hex view, not a dead \
             code tab with doc: None"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn scm_stage_key_stages_through_the_backend() {
        let Some(repo) = init_test_repo() else {
            return;
        };
        let (mut app, mut events) = scm_app(repo.path.clone());

        // The seeded status lists the untracked file.
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Untracked);

        // Pressing 's' in the focused SCM pane stages it, end to end.
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
    }

    #[tokio::test]
    async fn scm_stage_still_works_after_previewing_a_diff() {
        // Regression for "actions do nothing after opening a diff": browsing the
        // change list (arrow moves) previews each diff *without* stealing focus
        // from the Source-Control pane, so the staging keys stay live. (Enter is
        // the explicit "commit into the view" action and does move focus.)
        let Some(repo) = init_test_repo() else {
            return;
        };
        let (mut app, mut events) = scm_app(repo.path.clone());
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 1);

        // Arrow-browse onto the change: its diff previews, focus stays on SCM.
        app.dispatch(Command::SidebarDown);
        assert!(app.active_is_diff());
        assert!(app.tabs[app.active].is_preview);
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);

        // Staging still works while the preview is up.
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 1);
        assert_eq!(app.scm.changes[0].status, StatusKind::Added);
    }

    #[tokio::test]
    async fn scm_stages_a_multi_file_selection() {
        let Some(repo) = init_test_repo() else {
            return;
        };
        if std::fs::write(repo.path.join("second.rs"), b"fn second() {}\n").is_err() {
            return;
        }
        let (mut app, mut events) = scm_app(repo.path.clone());
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 2);

        // Select both changed files, then stage the whole selection at once.
        app.scm.selection.select_all();
        app.handle_key(press('s'));
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.staged_count, 2);
        assert!(
            app.scm
                .changes
                .iter()
                .all(|c| c.status == StatusKind::Added)
        );
    }

    // --- completion UI (issue #57) -----------------------------------------

    fn completion_item_labeled(label: &str, insert: &str) -> karet_core::CompletionItem {
        karet_core::CompletionItem {
            label: label.to_owned(),
            kind: karet_core::CompletionKind::Function,
            detail: None,
            documentation: None,
            insert_text: insert.to_owned(),
            edit: None,
            sort_text: None,
            deprecated: false,
        }
    }

    /// A focused editor over `text` (doc 9) wired to a recording backend, with
    /// the caret at `caret`.
    fn completion_app(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App) {
        let backend = Arc::new(RecordingBackend::new());
        let mut app = app();
        app.backend = Some(backend.clone());
        app.push_tab(text_tab("main.rs", text));
        app.focus = Focus::Editor;
        let idx = app.active;
        if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
            *doc = Some(DocumentId(9));
        }
        app.tabs[idx].editor.set_carets(&[caret]);
        (backend, app)
    }

    /// The completion requests a backend received, as `(id, position)`.
    fn completion_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(id, command)| match command {
                        SessionCommand::Completion { position, .. } => Some((*id, *position)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn open_popup(app: &mut App, items: Vec<karet_core::CompletionItem>, anchor: LineCol) {
        app.completion = Some(crate::completion::CompletionUi {
            items,
            list: karet_widgets::CompletionState::default(),
            doc: DocumentId(9),
            anchor,
            last_filter: String::new(),
        });
    }

    #[test]
    fn ctrl_space_requests_completions_at_the_caret() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let sent = completion_requests(&backend);
        assert_eq!(sent.len(), 1, "one Completion command");
        assert_eq!(sent[0].1, LineCol::new(0, 2));
        let pending = app.pending_completion.expect("a pending request");
        assert_eq!(pending.id, sent[0].0, "answer correlates by request id");
        assert_eq!(pending.anchor, LineCol::new(0, 0), "anchored at word start");
    }

    #[test]
    fn completion_enablement_resolves_against_the_tab_language() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor = serde_json::from_str(
            r#"{
                "completion": { "enabled": true },
                "[rust]": { "completion": { "enabled": false } }
            }"#,
        )
        .unwrap_or_default();

        app.trigger_completion(true);
        assert!(completion_requests(&backend).is_empty());

        if let TabKind::Code { language, .. } = &mut app.tabs[app.active].kind {
            *language = "Python";
        }
        app.trigger_completion(true);
        assert_eq!(completion_requests(&backend).len(), 1);
    }

    #[test]
    fn auto_trigger_fires_on_word_chars_but_the_error_gate_blocks_it() {
        let (backend, mut app) = completion_app("fn main() {}\n", LineCol::new(0, 0));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::empty()));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "a typed identifier char auto-triggers"
        );

        // A syntax error intersecting the caret line suppresses auto-trigger.
        app.dismiss_completion();
        let idx = app.active;
        if let TabKind::Code { syntax_errors, .. } = &mut app.tabs[idx].kind {
            *syntax_errors = vec![(0, 0)];
        }
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "the gate holds while the line has an outright error"
        );
    }

    #[test]
    fn manual_trigger_bypasses_the_error_gate() {
        let (backend, mut app) = completion_app("broken(\n", LineCol::new(0, 7));
        let idx = app.active;
        if let TabKind::Code { syntax_errors, .. } = &mut app.tabs[idx].kind {
            *syntax_errors = vec![(0, 3)];
        }
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(
            completion_requests(&backend).len(),
            1,
            "Ctrl+Space ignores the gate"
        );
    }

    #[test]
    fn trigger_characters_re_request_at_the_boundary() {
        // `.` triggers with an empty prefix …
        let (backend, mut app) = completion_app("self\n", LineCol::new(0, 4));
        app.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::empty()));
        let sent = completion_requests(&backend);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1, LineCol::new(0, 5), "requested after the dot");
        let pending = app.pending_completion.expect("pending");
        assert_eq!(
            pending.anchor,
            LineCol::new(0, 5),
            "empty prefix at a boundary"
        );

        // … a lone `:` does not, the second `:` of `::` does.
        let (backend, mut app) = completion_app("std\n", LineCol::new(0, 3));
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        assert!(
            completion_requests(&backend).is_empty(),
            "single colon is not a boundary"
        );
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::empty()));
        assert_eq!(completion_requests(&backend).len(), 1, "`::` re-requests");
    }

    #[test]
    fn completion_settings_disable_the_paths() {
        // enabled = false kills both manual and automatic completion.
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor.completion.enabled = false;
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(completion_requests(&backend).is_empty());

        // autoTrigger = false keeps manual completion working.
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.settings.editor.completion.auto_trigger = false;
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(completion_requests(&backend).is_empty(), "no auto-trigger");
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        assert_eq!(completion_requests(&backend).len(), 1, "manual still works");
    }

    #[test]
    fn stale_completions_are_ignored_and_fresh_ones_open_the_popup() {
        let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let (id, _) = completion_requests(&backend)[0];

        // An answer to a different (superseded) request id is dropped.
        app.on_backend_event(
            Some(RequestId(id.0 + 100)),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("stale", "stale")],
            },
        );
        assert!(
            app.completion.is_none(),
            "stale answers never open the popup"
        );
        assert!(
            app.pending_completion.is_some(),
            "still awaiting the real one"
        );

        // The matching answer opens the popup.
        app.on_backend_event(
            Some(id),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("foobar", "foobar")],
            },
        );
        let ui = app.completion.as_ref().expect("popup open");
        assert_eq!(ui.items.len(), 1);
        assert!(app.pending_completion.is_none());
    }

    #[test]
    fn a_moved_caret_drops_late_completions() {
        let (backend, mut app) = completion_app("fo\nbar\n", LineCol::new(0, 2));
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
        let (id, _) = completion_requests(&backend)[0];
        // The caret leaves the anchor line before the answer arrives.
        let idx = app.active;
        app.tabs[idx].editor.set_carets(&[LineCol::new(1, 0)]);
        app.on_backend_event(
            Some(id),
            SessionEvent::Completions {
                doc: DocumentId(9),
                version: 0,
                items: vec![completion_item_labeled("foobar", "foobar")],
            },
        );
        assert!(
            app.completion.is_none(),
            "late answers for a moved caret drop"
        );
    }

    #[test]
    fn accepting_replaces_the_typed_prefix() {
        let (_backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        open_popup(
            &mut app,
            vec![completion_item_labeled("foobar", "foobar")],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(code_tab_text(&app), "foobar\n", "the prefix was replaced");
        let idx = app.active;
        assert_eq!(app.tabs[idx].editor.cursor(), LineCol::new(0, 6));
        assert!(app.completion.is_none(), "accepting closes the popup");
    }

    #[test]
    fn popup_keys_navigate_and_escape_dismisses() {
        let (_backend, mut app) = completion_app("\n", LineCol::new(0, 0));
        open_popup(
            &mut app,
            vec![
                completion_item_labeled("alpha", "alpha"),
                completion_item_labeled("beta", "beta"),
            ],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(
            app.completion.as_ref().map(|ui| ui.list.selected),
            Some(1),
            "Down moves the selection"
        );
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
        assert_eq!(app.completion.as_ref().map(|ui| ui.list.selected), Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert!(app.completion.is_none(), "Esc dismisses");
    }

    #[test]
    fn backspacing_past_the_anchor_dismisses_the_popup() {
        let (_backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
        open_popup(
            &mut app,
            vec![completion_item_labeled("foo", "foo")],
            LineCol::new(0, 1),
        );
        // Deleting the char before the anchor moves the caret to (0,0) < anchor.
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()));
        assert!(app.completion.is_none(), "the popup follows its anchor");
    }

    #[test]
    fn typing_keeps_the_popup_filtering_without_a_new_request() {
        let (backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
        open_popup(
            &mut app,
            vec![
                completion_item_labeled("foobar", "foobar"),
                completion_item_labeled("other", "other"),
            ],
            LineCol::new(0, 0),
        );
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::empty()));
        assert!(
            completion_requests(&backend).is_empty(),
            "word chars refilter client-side while open"
        );
        assert!(app.completion.is_some(), "the popup stays open");
        let ranked = app.completion_ranked().unwrap_or_default();
        assert_eq!(
            ranked,
            vec![0],
            "only the matching candidate survives \"fo\""
        );
    }

    #[test]
    fn the_popup_paints_near_the_caret() {
        let (_backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
        open_popup(
            &mut app,
            vec![completion_item_labeled("frobnicate", "frobnicate")],
            LineCol::new(0, 0),
        );
        let painted = screen(&mut app, 80, 16).join("\n");
        assert!(
            painted.contains("frobnicate"),
            "the popup row is painted: {painted}"
        );
    }
}

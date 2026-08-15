//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Command`]s resolved from key events.

mod backend_events;
mod capture;
mod change_view;
mod commands;
mod completion;
mod definition;
mod deps;
mod diffs;
mod editor;
mod explorer;
pub(crate) mod github;
mod graphics;
mod history;
mod hit;
mod hover;
mod inline_macros;
mod input;
mod language_servers;
mod lifecycle;
mod markdown_edit;
mod mouse;
mod notifications;
mod panes;
mod pending;
mod remote_actions;
mod runtime;
mod scm;
mod scroll;
mod search;
mod sidebar;
mod snapshot_events;
mod spellcheck;
mod spelling;
mod startup;
mod state;
mod tabs;
mod todos;
mod util;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

pub(crate) use capture::capture;
use color_eyre::eyre::eyre;
use crossterm::event::DisableBracketedPaste;
use crossterm::event::DisableFocusChange;
use crossterm::event::DisableMouseCapture;
use crossterm::event::EnableBracketedPaste;
use crossterm::event::EnableFocusChange;
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
pub(crate) use hit::*;
use karet_core::BlameAttribution;
use karet_core::BytePos;
use karet_core::Change;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::Notification;
use karet_core::NotificationId;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Severity;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::ThemeRole;
use karet_editor::EditorState;
use karet_editor::editing;
use karet_editor::line_span;
pub(crate) use karet_editor::resolve_folds;
use karet_editor::selection_text;
use karet_filetype::FileKind;
use karet_filetype::IconStyle;
use karet_filetype::WrapMode;
use karet_filetype::file_type_for_path;
use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::{self};
use karet_search::FileHit;
use karet_search::SearchQuery;
use karet_search::search_in_file;
use karet_session::Backend;
use karet_session::BackendError;
use karet_session::ChangeSummary;
use karet_session::Command as SessionCommand;
use karet_session::ConfigDiagnostic;
use karet_session::DocSnapshot;
use karet_session::DocumentId;
use karet_session::DocumentSettings;
use karet_session::Event as SessionEvent;
use karet_session::EventRx;
use karet_session::GithubVerification;
use karet_session::LanguageServerChange;
use karet_session::LanguageServerId;
use karet_session::LanguageServerPlanId;
use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerStatus;
use karet_session::LoadedConfig;
use karet_session::PreparedChange;
use karet_session::PullRequestSummary;
use karet_session::RangeSpec;
use karet_session::RepositorySnapshot;
use karet_session::RequestId;
use karet_session::SessionConfig;
use karet_session::Settings;
use karet_session::SnapshotRx;
use karet_session::SpellingHit;
use karet_session::SwapInfo;
use karet_session::VcsAction;
use karet_session::VcsOutcome;
use karet_session::ViewId;
use karet_session::config::schema::AutoSave;
use karet_session::local;
#[cfg(test)]
use karet_syntax::FoldRegions;
use karet_text::EditCause;
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::RepositorySummary;
use karet_vcs::StatusKind;
use karet_widgets::DropZone;
use karet_widgets::FileTreeState;
use karet_widgets::ListSelection;
use karet_widgets::PaneDivider;
use karet_widgets::PaneId;
use karet_widgets::PaneLayout;
use karet_widgets::PendingEdit;
use karet_widgets::SplitAxis;
use karet_widgets::SplitDir;
use karet_widgets::drop_zone;
use karet_widgets::scroll::PaintedTracks;
use karet_widgets::scroll::ScrollTrack;
use karet_widgets::scroll::TrackHit;
pub(crate) use karet_widgets::textfield::TextFieldState;
pub(crate) use language_servers::LanguageServerBadge;
pub(crate) use pending::Pending;
use ratatui::layout::Rect;
pub(crate) use runtime::run;
pub(crate) use state::*;
use tokio::sync::mpsc;
use util::KeyboardEnhancementGuard;
use util::canonical;
use util::close_prompt_message;
use util::copy_path_recursive;
pub(crate) use util::effective_word_wrap;
use util::load_theme;
use util::move_path;
use util::parse_rev_range;
use util::path_contains_or_equals;
use util::path_under;
use util::rebase_path;
use util::rect_contains;
use util::retarget_tab_path;
use util::row_in_rect;
use util::same_path;
use util::tab_at;
pub(crate) use util::tab_language;
use util::unique_child_path;
use util::word_at;

use crate::clipboard::Clipboard;
use crate::command::Command;
use crate::compat;
use crate::compat::GraphicsCaret;
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
use crate::tab::CommitFiles;
use crate::tab::CommitViewState;
use crate::tab::FindState;
use crate::tab::MarkdownPreviewState;
use crate::tab::MergeConflictState;
use crate::tab::PagerState;
use crate::tab::SearchField;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;
use crate::tab::commit_title;
use crate::ui;
use crate::workspace;

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
    /// What the terminal was confirmed to support at startup.
    pub(crate) caps: TerminalCaps,
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
    /// Per-path repository/remote facts resolved by the backend, or the
    /// user-facing reason they are unavailable. Cleared on every VCS status
    /// refresh (commits and branch switches change the facts).
    pub(crate) remote_facts: HashMap<PathBuf, Result<RemoteFacts, String>>,
    /// Paths whose facts request is in flight (suppresses duplicate requests).
    pub(crate) remote_facts_pending: HashSet<PathBuf>,
    /// Actions parked on a facts answer.
    pub(crate) pending_remote_actions: Vec<PendingRemoteAction>,
    /// The in-flight file-history request for the With Revision diff-target
    /// picker, so its answering [`SessionEvent::FileHistory`] opens the picker.
    pub(crate) pending_history_picker: Option<RequestId>,
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
    /// The workspace-spelling panel state.
    pub(crate) spelling: SpellingPanel,
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
    /// Draggable split boundaries from the last rendered frame.
    pub(crate) pane_dividers: Vec<PaneDivider>,
    /// Current pointer-hovered split boundary.
    pub(crate) pane_divider_hover: Option<PaneDivider>,
    /// Active pane-boundary drag.
    pub(crate) pane_resize: Option<PaneResize>,
    /// Every scrollbar track the last frame painted, in draw order (hit-testing).
    pub(crate) scroll_hits: ScrollHits,
    /// The in-progress scrollbar-thumb drag, if one is under way.
    pub(crate) scroll_drag: Option<ScrollDrag>,
    /// The in-progress tab drag, if the pointer is dragging a tab.
    pub(crate) tab_drag: Option<TabDrag>,
    /// The sidebar's content area (below the header) from the last frame.
    pub(crate) sidebar_content_rect: Rect,
    /// The current mouse position while hovering the sidebar content, for a
    /// secondary-accent row highlight (explorer / source-control lists).
    pub(crate) hover: Option<(u16, u16)>,
    /// Current pointer position over a pane's format-specific action strip.
    pub(crate) pane_action_hover: Option<(u16, u16)>,
    /// The current mouse position while hovering the sidebar header controls.
    pub(crate) sidebar_header_hover: Option<(u16, u16)>,
    /// The header panel-switcher cells (`1 2 3`) from the last frame.
    pub(crate) panel_hits: Vec<(u16, u16, SidebarPanel)>,
    /// The right-side outline panel.
    pub(crate) outline: OutlinePanel,
    /// The explorer header toolbar-button cells `(start, end, command)` from the last
    /// frame (new file / new folder / refresh / collapse all).
    pub(crate) header_action_hits: Vec<(u16, u16, Command)>,

    /// Last completed compact status for nested repositories in the explorer.
    nested_repository_status: HashMap<PathBuf, RepositorySummary>,
    /// In-flight nested-repository requests keyed by request id.
    nested_repository_pending: HashMap<RequestId, (PathBuf, Pending)>,
    /// The Source-Control panel's last-frame render chrome.
    pub(crate) scm_ui: ScmChrome,
    /// Text field currently being extended by a left-button drag.
    pub(crate) text_field_drag: Option<TextFieldTarget>,
    /// The Search panel's last-frame render chrome.
    pub(crate) search_ui: SearchChrome,
    /// The Spelling panel's last-frame render chrome.
    pub(crate) spelling_ui: SpellingChrome,
    /// The Todos panel state.
    pub(crate) todos: TodosPanel,
    /// Today's WakaTime total for the status bar, when tracking is enabled.
    pub(crate) wakatime_status: Option<String>,
    /// The Todos panel's per-frame chrome.
    pub(crate) todos_ui: TodosChrome,
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
    /// The definition request awaiting an answer, if any.
    pub(crate) pending_definition: Option<definition::PendingDefinition>,
    /// Pre-jump positions, most recent last, for "Go Back" after a definition jump.
    pub(crate) definition_jumps: VecDeque<definition::JumpOrigin>,
    /// The Ctrl-hovered symbol the editor should underline as navigable.
    pub(crate) definition_hover: Option<definition::DefinitionHover>,
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
    pending_open: HashMap<RequestId, PendingOpen>,
    /// Opens whose view closed before the backend answered; a late document is released.
    abandoned_open: HashSet<RequestId>,
    /// In-flight save requests, mapping request id → document, so the tab's saving
    /// spinner clears when the answering event (saved or error) arrives.
    pending_saves: HashMap<RequestId, PendingSave>,
    /// Per-document caches fed by backend events.
    pub(crate) docs: DocState,
    /// Repository-scoped lifecycle state used by every LSP presentation surface.
    lsp_runtime: language_servers::LanguageServerRuntimeModel,
    /// Dirty document versions waiting for the configured automatic-save trigger.
    auto_save_pending: HashMap<DocumentId, PendingAutoSave>,
    /// The in-flight completion request, if any (see [`crate::completion`]).
    pub(crate) pending_completion: Option<crate::completion::PendingCompletion>,
    /// The open completion popup, if any.
    pub(crate) completion: Option<crate::completion::CompletionUi>,
    /// The reusable fuzzy matcher backing the completion popup's filtering.
    pub(crate) completion_matcher: karet_fuzzy::Matcher,
    /// The in-flight hover request, if any (see [`crate::hover`]).
    pub(crate) pending_hover: Option<crate::hover::PendingHover>,
    /// The open hover popup, if any.
    pub(crate) hover_ui: Option<crate::hover::HoverUi>,
    /// Parser-backed resolver for the seeded inline-macro catalog.
    inline_macro_engine: karet_syntax::InlineMacroEngine,
    /// In-flight commit-detail requests, mapping request id → where its result goes
    /// (a new standalone commit tab, or the graph browser's detail pane).
    pending_commit_detail: HashMap<RequestId, CommitDest>,
    /// Explicit LaTeX build requests mapped to their reserved preview view.
    latex_previews: HashMap<RequestId, ViewId>,
    /// Lazy forge-verification reads, owned by their exact commit view.
    pending_commit_verification: HashMap<RequestId, (ViewId, String)>,
    /// Conflict-side reads owned by their exact editable view.
    pending_merge_conflicts: HashMap<RequestId, (ViewId, PathBuf)>,
    /// In-flight ad-hoc diff preparations (revision/two-file diffs), owned by the
    /// reserved diff tab's view.
    pending_prepared_diffs: HashMap<RequestId, ViewId>,
    /// In-flight document conversions (DOCX → markdown), owned by the reserved
    /// preview tab's view.
    pending_conversions: HashMap<RequestId, ViewId>,
    /// Two-file diffs from the `--diff` flag, opened as loading tabs before the
    /// backend attaches; their `PrepareDiff` commands are sent on attach.
    pending_startup_diffs: Vec<(ViewId, PathBuf, String, String)>,
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

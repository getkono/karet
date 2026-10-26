//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Command`]s resolved from key events.

mod autopair;
mod backend_events;
mod capture;
mod change_view;
mod clipboard;
pub(crate) mod code_actions;
mod code_lens;
mod commands;
mod comment;
pub(crate) mod commit_console;
mod completion;
pub(crate) mod confirm;
mod debugging;
mod definition;
mod deps;
mod diffs;
mod drag;
mod editor;
mod explorer;
pub(crate) mod github;
mod graphics;
pub(crate) mod hierarchy;
mod history;
mod hit;
mod hover;
mod indent;
mod inlay;
mod inline_completion;
mod inline_macros;
mod input;
mod language_servers;
mod lifecycle;
mod link_color;
mod linked_editing;
mod markdown_edit;
mod mouse;
mod navigation;
mod notifications;
mod occurrences;
mod on_type;
mod panes;
mod pending;
mod problems;
mod quick_open;
mod remote_actions;
pub(crate) mod rename;
mod review;
mod runtime;
pub(crate) mod scm;
mod scroll;
pub(crate) mod seam;
mod search;
pub(crate) mod select;
mod sidebar;
pub(crate) mod signature;
mod snapshot_events;
mod spellcheck;
mod spelling;
mod startup;
mod state;
mod syntax_select;
mod tabs;
#[cfg(feature = "terminal")]
mod terminal;
pub(crate) mod theme_picker;
mod todos;
mod unsupported;
mod util;
pub(crate) mod vi;

#[cfg(test)]
pub(crate) mod tests;

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
pub(crate) use code_lens::code_lens_rows;
use color_eyre::eyre::eyre;
use confirm::ConfirmAction;
use confirm::ConfirmChoice;
use confirm::ConfirmDialog;
use confirm::describe_paths;
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
#[cfg(test)]
pub(crate) use debugging::DebugPanel;
pub(crate) use debugging::DebugRow;
pub(crate) use drag::DragGranularity;
pub(crate) use drag::EditorDrag;
pub(crate) use editor::column::ColumnSelect;
pub(crate) use editor::column::ColumnStep;
use graphics::GraphicsCaret;
pub(crate) use hit::*;
pub(crate) use inlay::InlayHintHit;
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
use karet_editor::class_run_at;
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
use karet_fileview::viewer::human_size;
use karet_search::SearchQuery;
use karet_search::search_chunks;
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
use karet_session::LanguageServerLogEntry;
use karet_session::LanguageServerMessageId;
use karet_session::LanguageServerPlanId;
use karet_session::LanguageServerRuntimeState;
use karet_session::LanguageServerStatus;
use karet_session::LoadedConfig;
use karet_session::LocationKind;
use karet_session::PreparedChange;
use karet_session::PullRequestSummary;
use karet_session::RangeSpec;
use karet_session::RepositorySnapshot;
use karet_session::RequestId;
use karet_session::SaveCause;
use karet_session::SearchHit;
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
use karet_widgets::Spinner;
use karet_widgets::SplitAxis;
use karet_widgets::SplitDir;
use karet_widgets::drop_zone;
use karet_widgets::scroll::PaintedTracks;
use karet_widgets::scroll::ScrollTrack;
use karet_widgets::scroll::TrackHit;
pub(crate) use karet_widgets::textarea::TextAreaState;
pub(crate) use karet_widgets::textfield::TextFieldState;
pub(crate) use language_servers::LanguageServerBadge;
pub(crate) use language_servers::LanguageServerBadgeSummary;
pub(crate) use language_servers::LanguageServerQuestion;
pub(crate) use link_color::HeldColors;
pub(crate) use link_color::current_items;
pub(crate) use notifications::Report;
pub(crate) use pending::Pending;
pub(crate) use problems::ProblemRow;
pub(crate) use problems::ProblemsChrome;
pub(crate) use problems::ProblemsPanel;
pub(crate) use problems::is_problem;
use ratatui::layout::Rect;
pub(crate) use runtime::run;
pub(crate) use search::SearchPanel;
pub(crate) use search::SearchPanelField;
pub(crate) use search::SearchRow;
pub(crate) use select::SelectRegion;
pub(crate) use select::SelectSurface;
pub(crate) use select::SurfaceRow;
pub(crate) use select::SurfaceSelection;
pub(crate) use spelling::SpellingPanel;
pub(crate) use spelling::SpellingRow;
pub(crate) use state::*;
pub(crate) use todos::TodoRow;
pub(crate) use todos::TodosPanel;
use tokio::sync::mpsc;
use util::KeyboardEnhancementGuard;
use util::canonical;
use util::close_prompt_choices;
pub(crate) use util::commit_rails;
use util::copy_path_recursive;
pub(crate) use util::effective_word_wrap;
pub(crate) use util::is_markdown;
pub(crate) use util::keep_visible;
use util::load_theme;
use util::move_path;
use util::parse_rev_range;
use util::path_under;
use util::rebase_path;
use util::rect_contains;
pub(crate) use util::relative_time;
use util::retarget_tab_path;
use util::row_in_rect;
use util::same_path;
use util::tab_at;
pub(crate) use util::tab_language;
use util::unique_child_path;

use crate::clipboard::Clipboard;
use crate::command::Command;
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
use crate::view::View;
use crate::workspace;

/// The maximum number of matching files the workspace search panel collects.
const SEARCH_RESULT_CAP: usize = 500;
/// How many matches a workspace search keeps before it stops walking. Bounds the
/// panel's preview memory, which the file cap alone does not: one generated file
/// can hold thousands of matches.
const SEARCH_MATCH_CAP: usize = 5_000;
/// Expand every file group when a finished search is at most this many matches;
/// a larger result set arrives collapsed so it reads as a scannable file list.
const SEARCH_AUTO_EXPAND: usize = 50;

/// How many commits the source-control log fetches per lazily-loaded page.
const SCM_LOG_PAGE: usize = 25;

/// How many commits the full-screen commit-graph view fetches per page. Much larger
/// than [`SCM_LOG_PAGE`]: that view is meant to stay loaded far beyond the viewport, so
/// it trades a bigger read for not having to page again on every screen of scrolling.
const GRAPH_LOG_PAGE: usize = 200;

/// Columns one keyboard pan moves the commit graph.
const GRAPH_PAN_COLUMNS: i32 = 8;

/// How many screens of history the commit-graph view keeps loaded ahead of the
/// viewport. Prefetching this far out is what keeps the trailing "more" affordance out
/// of sight unless the user outruns the fetch.
const GRAPH_PREFETCH_SCREENS: usize = 2;

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
/// How soon a press must follow the last one to continue its multi-click streak,
/// in the editor, the shell's other surfaces, and the terminal grid alike.
pub(crate) const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(400);
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
    /// The Select Color Theme picker's live preview and the session's pick.
    pub(crate) theme_ui: theme_picker::ThemeUi,
    /// Whether syntax highlighting is enabled.
    pub(crate) syntax: bool,
    /// The icon style for the explorer and activity bar.
    pub(crate) icon_style: IconStyle,
    /// Command-line icon selection, which remains authoritative across config reloads.
    icon_override: Option<IconStyle>,
    /// What the terminal was confirmed to support at startup.
    pub(crate) caps: TerminalCaps,
    /// Which top-level view owns the content area.
    pub(crate) view: View,
    /// The GitHub view's pages. Empty while the workspace is not GitHub-eligible.
    pub(crate) github: github::GithubSurface,
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
    /// The open confirmation dialog, if any. Outranks every other modal but the
    /// shutdown blocker, so a question is never buried under a picker.
    pub(crate) confirm: Option<confirm::ConfirmDialog>,
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
    /// Open views in activation order, oldest first, newest last: the history that
    /// picks which tab — and which pane — to focus when the active one closes.
    /// Every write to `active` goes through [`App::set_active`], which appends here,
    /// so the order cannot drift from what the user actually visited. Ids are only
    /// ever appended: a view whose tab has closed is skipped when the history is
    /// read, so no pruning pass is needed. `ViewId(0)` never enters, being the
    /// unassigned sentinel every welcome tab shares.
    pub(crate) view_history: Vec<ViewId>,
    /// The open modal overlay (quick-open / command palette), if any.
    pub(crate) overlay: Option<Overlay>,
    /// The workspace's file list behind quick-open, walked off the UI thread.
    pub(crate) file_index: workspace::FileIndex,
    /// Quick-open's `#` mode: the workspace-symbol query and its request.
    pub(crate) workspace_symbols: quick_open::WorkspaceSymbolSearch,
    /// Recently opened workspaces and files (persisted only by the live editor).
    pub(crate) recent: workspace::RecentStore,
    /// The workspace to reopen karet on once this window has quit (Open Recent).
    relaunch: Option<PathBuf>,
    /// Whether the find-in-file bar is currently shown. The query/toggle data
    /// itself lives on the active tab (`Tab::find`), so this only tracks
    /// visibility — closing the bar (Esc) clears this without discarding that
    /// data, and it is reset whenever the active tab changes.
    pub(crate) find_open: bool,
    /// The permanent multiline Source-Control commit-message editor.
    pub(crate) commit_input: CommitInput,
    /// The live console a running commit's hooks print into.
    pub(crate) commit_console: commit_console::CommitConsole,
    /// The commit box's AI affordance: what the backend says can run, and what
    /// the last (or current) generation is doing.
    pub(crate) ai_commit: crate::app::scm::aicommit::AiCommitUi,
    /// The in-progress revision text while the go-to-commit input is open.
    pub(crate) rev_input: Option<String>,
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
    /// Modal editing (`editor.keymap` `"vim"`/`"helix"`): the mode, registers,
    /// macros and the command being typed.
    pub(crate) vi: vi::ViState,
    /// The workspace-search panel state.
    pub(crate) search: SearchPanel,
    /// The workspace-spelling panel state.
    pub(crate) spelling: SpellingPanel,
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
    /// The chrome row carrying the top-level view switcher.
    pub(crate) view_chrome_rect: Rect,
    /// Column spans of the view-switcher buttons on the chrome row.
    pub(crate) view_hits: Vec<(u16, u16, View)>,
    /// The right-side outline panel.
    pub(crate) outline: OutlinePanel,
    /// Zen Mode and the maximised pane.
    pub(crate) modes: ScreenModes,
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
    /// Where the find-in-file bar's editable fields landed last frame.
    pub(crate) find_rects: crate::ui::FindBarRects,
    /// The Search panel's last-frame render chrome.
    pub(crate) search_ui: SearchChrome,
    /// The Spelling panel's last-frame render chrome.
    pub(crate) spelling_ui: SpellingChrome,
    /// The Todos panel state.
    pub(crate) todos: TodosPanel,
    /// Today's WakaTime total for the status bar, when tracking is enabled.
    pub(crate) wakatime_status: Option<String>,
    /// Persisted code-review marks (reviewed files per commit).
    pub(crate) review: review::ReviewStore,
    /// The Todos panel's per-frame chrome.
    pub(crate) todos_ui: TodosChrome,
    /// The Problems panel state.
    pub(crate) problems: ProblemsPanel,
    /// The Problems panel's per-frame chrome.
    pub(crate) problems_ui: ProblemsChrome,
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
    /// The open inline rename prompt (F2), if any.
    pub(crate) rename: Option<rename::RenamePrompt>,
    /// The submitted rename awaiting the backend's answer, if any.
    pub(crate) pending_rename: Option<rename::PendingRename>,
    /// The quick-fix menu's pending request, the action being run, and the
    /// gutter lightbulb's probe.
    pub(crate) code_actions: code_actions::CodeActions,
    /// Ghost text: the pending ask and what is on offer (see
    /// [`inline_completion`]).
    pub(crate) inline_completion: inline_completion::InlineCompletions,
    /// The occurrence-highlight probe and what it found (see [`occurrences`]).
    pub(crate) occurrences: occurrences::Occurrences,
    /// Each pane's Go Back / Go Forward history, keyed by the pane it navigates.
    pub(crate) nav_history: HashMap<PaneId, navigation::NavHistory>,
    /// The Ctrl-hovered symbol the editor should underline as navigable.
    pub(crate) definition_hover: Option<definition::DefinitionHover>,
    /// The focused commit view's signature-badge rect (screen coords) from the last
    /// frame, for double-click hit-testing. `None` when no badge is on screen.
    pub(crate) commit_badge_rect: Option<Rect>,
    /// The editor's in-progress pointer selection drag, if one is under way.
    pub(crate) editor_drag: Option<drag::EditorDrag>,
    /// The box selection last made, so the next Shift+Alt press or
    /// Ctrl+Shift+Alt+arrow grows it rather than starting over.
    column_select: Option<ColumnSelect>,
    /// The live pointer selection on a read-only surface (diff, viewer), if any.
    pub(crate) surface_selection: Option<select::SurfaceSelection>,
    /// The surface currently capturing a selection drag, if one is under way.
    pub(crate) surface_selecting: Option<select::SelectSurface>,
    /// The last left-click `(time, column, row)`, for multi-click detection.
    last_click: Option<(Instant, u16, u16)>,
    /// The current multi-click streak (1 = single, 2 = double, 3 = triple and
    /// beyond).
    click_streak: u8,
    /// The surface the last left click landed on and the modifiers it carried: a
    /// click on another surface or with other modifiers starts a new streak.
    click_origin: (editor::ClickSurface, KeyModifiers),
    /// The system clipboard (OSC 52).
    clipboard: Clipboard,
    /// The active Kitty image placement rect (set by the renderer), if any.
    pub(crate) image_area: Option<Rect>,
    /// Local images the markdown preview paints, decoded off the UI thread.
    pub(crate) preview_images: crate::preview_images::PreviewImages,
    /// Starts integrated terminals and owns the channel their output and exit
    /// notices arrive on (taken by the UI loop).
    #[cfg(feature = "terminal")]
    pub(crate) terminals: crate::terminal::TerminalHost,
    /// The frame cap terminal output repaints under.
    #[cfg(feature = "terminal")]
    terminal_frame: terminal::TerminalFrame,
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
    /// Messages to print to stderr once the terminal has been restored.
    ///
    /// The event loop returns without another frame once [`should_quit`] is set,
    /// so a notification raised on the way out would never be painted. Anything
    /// that has to be read after quitting goes here instead.
    ///
    /// [`should_quit`]: Self::should_quit
    farewell: Vec<String>,
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
    pub(crate) lsp_runtime: language_servers::LanguageServerRuntimeModel,
    /// Dirty document versions waiting for the configured automatic-save trigger.
    auto_save_pending: HashMap<DocumentId, PendingAutoSave>,
    /// The closers auto-inserted by the last keystrokes, open to type-over.
    auto_closers: Option<autopair::AutoClosers>,
    /// Completion: the request in flight, the open popup, each document's
    /// trigger and commit characters, an accepted item's follow-up, the snippet
    /// being filled in, and the matcher (see [`completion`]).
    pub(crate) completion: completion::Completion,
    /// The in-flight hover request, if any (see [`crate::hover`]).
    pub(crate) pending_hover: Option<crate::hover::PendingHover>,
    /// The Expand Selection waiting on the server's selection ranges, if any
    /// (see [`syntax_select`]).
    pub(crate) pending_expand: Option<syntax_select::PendingExpand>,
    /// The Start Linked Editing waiting on the server's linked ranges, if
    /// any (see [`linked_editing`]).
    pub(crate) pending_linked: Option<linked_editing::PendingLinked>,
    /// On-type formatting: the request in flight, each document's trigger
    /// characters, and the session toggle (see [`on_type`]).
    pub(crate) on_type: on_type::OnTypeState,
    /// The open hover popup, if any.
    pub(crate) hover_ui: Option<crate::hover::HoverUi>,
    /// Signature help: the request in flight, the open popup, and each
    /// document's trigger characters (see [`signature`]).
    pub(crate) signature: signature::SignatureState,
    /// Inlay-hint tooltips, Ctrl+click, resolving, and the toggle key.
    pub(crate) inlay_ui: inlay::InlayUi,
    /// Each document's code lenses, the requests for them, and a lens run
    /// awaiting its answer (see [`code_lens`]).
    pub(crate) code_lenses: code_lens::CodeLensState,
    /// Each document's language-server links and colours, the requests for
    /// them, and a link resolve awaiting its answer (see [`link_color`]).
    pub(crate) links_colors: link_color::LinkColorState,
    /// The open scrollable diagnostic detail view, if any.
    pub(crate) diagnostic_view: Option<crate::hover::DiagnosticView>,
    /// The debug session: lifecycle, console, Debug panel, and stop location
    /// (see [`debugging`]).
    pub(crate) debugger: debugging::Debugger,
    /// Armed breakpoints per absolute file path: line → verified.
    pub(crate) breakpoints:
        std::collections::HashMap<PathBuf, std::collections::BTreeMap<u32, bool>>,
    /// Palette commands from `--command`, queued until the backend is attached.
    ///
    /// They cannot run at construction time: a command like Show Hover or
    /// Trigger Suggest returns early without a backend, so dispatching before
    /// the attach silently did nothing. [`runtime::attach_backend`] drains
    /// this, which is the one path both the live shell and `--capture` share.
    pub(crate) startup_commands: Vec<Command>,
    /// The last layered tree-sitter parse a UI-thread reader asked for (comment
    /// toggling, Enter and paste indentation, Expand Selection, inline macros), reused while the
    /// document and its buffer version are unchanged.
    layered_trees: syntax_select::LayeredTreeCache,
    /// In-flight commit-detail requests, mapping request id → where its result goes
    /// (a new standalone commit tab, or the graph browser's detail pane).
    pending_commit_detail: HashMap<RequestId, ViewId>,
    /// In-flight range diffs, mapping request id → the compare tab reserved for it.
    pending_compare: HashMap<RequestId, ViewId>,
    /// Explicit LaTeX build requests mapped to their reserved preview view.
    latex_previews: HashMap<RequestId, ViewId>,
    /// Lazy forge-verification reads, owned by their exact commit view.
    pending_commit_verification: HashMap<RequestId, (ViewId, String)>,
    /// Conflict-side reads owned by their exact editable view.
    pending_merge_conflicts: HashMap<RequestId, (ViewId, PathBuf)>,
    /// In-flight ad-hoc diff preparations (revision/two-file diffs), owned by the
    /// reserved diff tab's view, with the 0-based `\n`-delimited line of the
    /// diffed text whose hunk the filled tab scrolls to (a change-marker click),
    /// or `None` to open at the top.
    pending_prepared_diffs: HashMap<RequestId, (ViewId, Option<u32>)>,
    /// In-flight document conversions (DOCX → markdown), owned by the reserved
    /// preview tab's view.
    pending_conversions: HashMap<RequestId, ViewId>,
    /// Two-file diffs from the `--diff` flag, opened as loading tabs before the
    /// backend attaches; their `PrepareDiff` commands are sent on attach.
    pending_startup_diffs: Vec<(ViewId, PathBuf, String, String)>,
    /// Whether the command line asked for a startup view or focus (`--view`,
    /// `--focus`), which a restored workspace layout must leave in place.
    startup_surface_requested: bool,
    /// In-flight commit-graph history pages, each owned by the exact view that asked.
    /// Keyed this way so an answering [`SessionEvent::VcsLog`] fills that view rather
    /// than the sidebar log — or, with several graph views open, the wrong one.
    graph_log_reqs: HashMap<RequestId, ViewId>,
    /// The in-flight seam index request, so a stale answer can be ignored.
    pub(crate) seam_index_req: Option<RequestId>,
    /// The in-flight seam query request.
    pub(crate) seam_query_req: Option<RequestId>,
    /// The in-flight seam node-detail request.
    pub(crate) seam_node_req: Option<RequestId>,
    /// Package discovery behind the Seam start-point picker, run off the UI thread.
    pub(crate) seam_discovery: seam::roots::RootDiscovery,
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

impl App {
    /// Re-read the user and project `keymap.jsonc` after one changed on disk,
    /// make the merged result the live keymap, and say what happened: each
    /// problem as a notification, or a transient "reloaded" when there are none.
    pub(crate) fn reload_keymap(&mut self) {
        let (keymap, report) = keymap::user::load(std::slice::from_ref(&self.root));
        self.apply_keymap(keymap, &report);
    }

    /// Install `keymap` and surface `report` (see [`App::reload_keymap`]).
    fn apply_keymap(&mut self, keymap: keymap::Keymap, report: &keymap::user::KeymapReport) {
        keymap::install(keymap);
        // A half-typed chord was resolved against the old table; drop it.
        self.pending.clear();
        let diags = report.diagnostics();
        if diags.is_empty() {
            self.notify(Report::Outcome, NotificationKind::System, "keymap reloaded");
        }
        for diag in diags {
            self.notify(
                Report::from_severity(diag.severity),
                NotificationKind::System,
                format!("config: {}", diag.message),
            );
        }
    }
}

/// [`App::apply_keymap`]'s side effects. Each test installs the compiled-in
/// defaults, so the process-wide keymap every other test reads never changes.
#[cfg(test)]
mod keymap_reload_tests {
    use super::tests::support::app;
    use super::*;
    use crate::keymap::KeyChord;
    use crate::keymap::user::Issue;
    use crate::keymap::user::KeymapReport;

    /// The active notifications' severities and titles, oldest first.
    fn notes(app: &App) -> Vec<(Severity, String)> {
        let mut notes: Vec<_> = app
            .notifications
            .active()
            .into_iter()
            .map(|note| (note.severity, note.title.clone()))
            .collect();
        notes.reverse();
        notes
    }

    /// A Ctrl+K press, the prefix of the two-key chords.
    fn ctrl_k() -> KeyChord {
        KeyChord::from_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL))
    }

    /// A clean reload drops a half-typed chord and says it reloaded.
    #[test]
    fn a_clean_reload_clears_the_pending_chord_and_reports_success() {
        let mut app = app();
        app.pending.push(ctrl_k());
        app.apply_keymap(keymap::Keymap::defaults(), &KeymapReport::default());
        assert!(app.pending.is_empty());
        assert_eq!(
            notes(&app),
            vec![(Severity::Hint, "keymap reloaded".into())]
        );
    }

    /// A reload with problems reports each one as a `config:` warning, located
    /// at its file, and never also claims a clean reload.
    #[test]
    fn a_reload_with_problems_reports_each_and_not_success() {
        let mut app = app();
        app.pending.push(ctrl_k());
        let issue = |message: &str| Issue {
            path: PathBuf::from("/k/keymap.jsonc"),
            message: message.to_owned(),
        };
        let report = KeymapReport {
            files: Vec::new(),
            issues: vec![issue("1:2: unknown command"), issue("3:4: bad key")],
        };
        app.apply_keymap(keymap::Keymap::defaults(), &report);
        assert!(app.pending.is_empty());
        assert_eq!(
            notes(&app),
            vec![
                (
                    Severity::Warning,
                    "config: /k/keymap.jsonc: 1:2: unknown command".into()
                ),
                (
                    Severity::Warning,
                    "config: /k/keymap.jsonc: 3:4: bad key".into()
                ),
            ]
        );
    }
}

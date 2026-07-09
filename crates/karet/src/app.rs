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
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::FileChange;
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
use crate::compat::CellPixels;
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
use crate::overlay::Overlay;
use crate::overlay::OverlayEvent;
use crate::render::FileView;
use crate::render::Section;
use crate::tab::FindState;
use crate::tab::SearchField;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::tab::ViewMode;
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
enum CommitDest {
    /// Open it as a new standalone commit tab.
    Tab,
    /// Fill the graph browser's detail pane.
    Browser,
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
    /// The detected terminal graphics protocol.
    pub(crate) graphics: GraphicsProtocol,
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
    /// Whether the quit-confirmation prompt (unsaved changes) is armed.
    pub(crate) pending_quit: bool,
    /// Whether a "save all & quit" is in flight: exit once the saves drain.
    pub(crate) quitting: bool,
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
        Self {
            root,
            settings: Settings::default(),
            loaded_config: LoadedConfig::default(),
            config_diagnostics: Vec::new(),
            theme: Theme::dark(),
            syntax,
            icon_style: IconStyle::default(),
            graphics: image::detect_protocol(),
            kitty_keyboard_supported: false,
            pointer_shapes_supported: false,
            pointer_shape: None,
            focus: Focus::Sidebar,
            sidebar_panel: SidebarPanel::Explorer,
            sidebar_visible: true,
            explorer: FileTreeState::new(),
            scm: Scm {
                selection: ListSelection::new(changes.len()),
                changes,
                staged_count,
                log: Vec::new(),
                log_has_more: false,
                log_loading: false,
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
            pending_quit: false,
            quitting: false,
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
            should_quit: false,
            backend: None,
            pending_open: HashMap::new(),
            pending_saves: HashMap::new(),
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
        self
    }

    /// Apply the loaded configuration to the UI shell (builder-style). Stores the
    /// settings (later handed to the session backend) and any load diagnostics (shown
    /// as startup notifications), and applies the `workbench.*` slice: colour theme,
    /// icon style, and the startup sidebar panel.
    #[must_use]
    pub fn with_settings(mut self, settings: Settings, diagnostics: Vec<ConfigDiagnostic>) -> Self {
        self = self.with_loaded_config(LoadedConfig {
            settings,
            diagnostics,
            ..LoadedConfig::default()
        });
        self
    }

    /// Apply a loaded configuration report to the UI shell (builder-style).
    #[must_use]
    pub fn with_loaded_config(mut self, loaded: LoadedConfig) -> Self {
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

        self.icon_style = match settings.workbench.icon_style {
            IconStyleSetting::NerdFont => IconStyle::NerdFont,
            IconStyleSetting::Unicode => IconStyle::Unicode,
            IconStyleSetting::Ascii => IconStyle::Ascii,
        };

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

        self.settings = settings;
        self
    }

    /// Open `path` as the initial tab at startup (used when `karet <file>` is run).
    pub fn open_initial(&mut self, path: &Path) {
        self.open_path(path);
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
                TabKind::Commit { .. }
                | TabKind::Compare { .. }
                | TabKind::Blame { .. }
                | TabKind::Graph { .. }
                | TabKind::LoadedConfig { .. }
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
        match self.settings.editor.graphical_cursor {
            Some(false) => false,
            Some(true) => self.graphical_cursor_compatible(),
            None => self.graphical_cursor_compatible(),
        }
    }

    fn graphical_cursor_compatible(&self) -> bool {
        self.kitty_keyboard_supported
            && self.graphics == GraphicsProtocol::Kitty
            && CellPixels::detect().is_some()
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
            None => self.resolve_key(key),
        }
    }

    /// The current input context: the active modal (if any) over the focused pane.
    /// The precedence mirrors how the shell stacks these overlays. Also drives the
    /// context-aware status hints bar ([`crate::ui`]).
    pub(crate) fn input_context(&self) -> Context {
        let modal = if self.pending_swaps.is_some() {
            // A startup recovery decision blocks everything else until made.
            Some(Modal::SwapRecover)
        } else if self.pending_quit {
            Some(Modal::QuitConfirm)
        } else if self.overlay.is_some() {
            Some(Modal::Overlay)
        } else if self.commit_input.is_some() {
            Some(Modal::CommitInput)
        } else if self.rev_input.is_some() {
            Some(Modal::RevInput)
        } else if self.pending_discard.is_some() {
            Some(Modal::DiscardConfirm)
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
            // An unbound key cancels the quit prompt (stay in the editor)…
            Modal::QuitConfirm => {
                self.pending_quit = false;
                self.status = Some("quit cancelled".to_string());
            },
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
            Modal::SearchList | Modal::DiscardConfirm | Modal::QuitConfirm | Modal::SwapRecover => {
            },
        }
    }

    /// Feed a key to the explorer inline name editor: printable characters extend the
    /// name, Backspace trims it (Enter/Esc are handled as bound commands).
    fn explorer_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => self.explorer.edit_backspace(),
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
            Command::CloseTab => self.close_tab(),
            Command::NextTab => self.next_tab(),
            Command::PrevTab => self.prev_tab(),
            Command::MoveTabLeft => self.move_active_tab(-1),
            Command::MoveTabRight => self.move_active_tab(1),
            Command::GoToTab(n) => self.go_to_tab(n),
            Command::CloseOtherTabs => self.close_other_tabs(),
            Command::CloseTabsToRight => self.close_tabs_to_right(),
            Command::CloseAllTabs => self.close_all_tabs(),
            Command::ReopenClosedTab => self.reopen_closed_tab(),
            Command::OpenAnyway => self.open_active_anyway(),
            Command::DismissNotification => self.notifications.dismiss_latest(),
            Command::DismissAllNotifications => self.notifications.dismiss_all(),
            Command::SplitRight => self.split_focused(SplitDir::Right),
            Command::SplitDown => self.split_focused(SplitDir::Down),
            Command::FocusNextPane => self.focus_pane_cycle(true),
            Command::FocusPrevPane => self.focus_pane_cycle(false),
            Command::Copy => self.copy_selection(),
            Command::CopyPath => self.copy_path(false),
            Command::CopyRelativePath => self.copy_path(true),
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
            Command::InsertChar(c) => {
                let s = c.to_string();
                self.submit_edit(move |caret, sel, _b, base| {
                    Some(editing::insert(caret, sel, base, &s))
                });
            },
            Command::InsertNewline => {
                self.submit_edit(|caret, sel, buf, base| {
                    Some(editing::newline(caret, sel, buf, base))
                });
            },
            Command::DeleteBackward => self.submit_edit(editing::backspace),
            Command::DeleteForward => self.submit_edit(editing::delete_forward),
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
            Command::ExplorerEditSubmit => self.explorer_commit_edit(),
            Command::ExplorerEditCancel => self.explorer.cancel_edit(),
            Command::ConfirmDiscard => self.resolve_discard(true),
            Command::QuitSaveAll => self.quit_save_all(),
            Command::QuitDiscard => {
                self.pending_quit = false;
                self.should_quit = true;
            },
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

    /// Set the active document tab's current page (clamped to the page range).
    fn set_document_page(&mut self, page: usize) {
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
            _ => self.sidebar_step(delta.signum()),
        }
    }

    /// Move the sidebar selection within the active panel.
    fn sidebar_step(&mut self, delta: i32) {
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

    /// Activate the selected sidebar row (open a file, expand a dir, open a diff).
    fn sidebar_activate(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                if let Some(row) = self.explorer.selected() {
                    let path = row.path.clone();
                    if row.is_dir {
                        self.explorer.toggle(&path);
                    } else {
                        self.open_path_preview(&path);
                    }
                }
            },
            SidebarPanel::SourceControl => self.open_selected_diff(),
            SidebarPanel::Search => {},
        }
    }

    /// Double-click on a file in the tree: promote it to a permanent tab instead
    /// of the single-click/Enter preview behavior. If it's already open (as the
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
        match pending {
            PendingEdit::Create { path, folder } => {
                let result = if folder {
                    std::fs::create_dir_all(&path)
                } else {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::File::create(&path).map(|_| ())
                };
                match result {
                    Ok(()) => {
                        self.explorer.rebuild(&self.root);
                        if !folder {
                            self.open_path(&path);
                        }
                    },
                    Err(e) => {
                        self.notify(
                            Severity::Error,
                            NotificationKind::Io,
                            format!("create failed: {e}"),
                        );
                    },
                }
            },
            PendingEdit::Rename { from, to } => match std::fs::rename(&from, &to) {
                Ok(()) => self.explorer.rebuild(&self.root),
                Err(e) => {
                    self.notify(
                        Severity::Error,
                        NotificationKind::Io,
                        format!("rename failed: {e}"),
                    );
                },
            },
        }
    }

    /// Open a diff tab for the selected Source-Control entry.
    fn open_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor) else {
            return;
        };
        let section = self.scm.section(cursor);
        let title = change
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diff")
            .to_string();
        let file = FileView::new(change.clone(), section, self.syntax);
        let tab = Tab::new(
            title,
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        );
        self.push_tab(tab);
        // Previewing a change must not steal focus: `push_tab` focuses the editor
        // (right for opening a file), but here the Source-Control pane stays active
        // so stage/unstage/discard/commit and selection keys keep working while the
        // diff is shown. Press Tab to move into the diff to scroll it.
        self.focus = Focus::Sidebar;
    }

    // --- source control ---------------------------------------------------

    /// Request one page of the commit log starting at `skip`, unless one is already
    /// in flight. The result arrives as [`SessionEvent::VcsLog`].
    fn request_scm_log(&mut self, skip: usize) {
        if self.scm.log_loading {
            return;
        }
        self.scm.log_loading = true;
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

    /// Open the commit view for `rev` (a hash or ref). The detail + diff arrive
    /// asynchronously as [`SessionEvent::CommitReady`], which builds the tab.
    fn open_commit(&mut self, rev: String) {
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev }) {
            self.pending_commit_detail.insert(id, CommitDest::Tab);
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
            if let Some(TabKind::CommitGraph { loading, .. }) = self.active_commit_graph() {
                *loading = true;
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
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev: hash }) {
            self.pending_commit_detail.insert(id, CommitDest::Browser);
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

    /// Fill the graph browser's detail pane from a resolved commit, and fire the lazy
    /// GitHub verification fetch. A no-op if no browser is open.
    fn fill_graph_detail(&mut self, detail: Box<CommitDetail>, changes: Vec<FileChange>) {
        let syntax = self.syntax;
        let hash = detail.hash.clone();
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::CommitGraph {
                detail: slot,
                files,
                verification,
                ..
            } = &mut tab.kind
            {
                *files = changes
                    .iter()
                    .cloned()
                    .map(|c| FileView::new(c, Section::Staged, syntax))
                    .collect();
                *slot = Some(detail.clone());
                *verification = None;
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
                selected,
                ..
            } = &mut tab.kind
            {
                *loading = false;
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
        let tab = workspace::open_file(path, self.syntax);
        self.push_tab(tab);
    }

    /// Open `path` into the focused pane's reusable "preview" tab slot (VS
    /// Code-style): used only by file-tree navigation (single click / arrow +
    /// activate). A file already open (preview or permanent) is just focused,
    /// same as [`open_path`](Self::open_path). Otherwise the current preview
    /// tab, if this pane has one, is replaced in place; if not, a new preview
    /// tab is opened. Every other caller of `open_path` (LSP jumps, the
    /// overlay, reopen-closed, CLI-provided files) keeps opening permanent
    /// tabs — only tree navigation opens previews.
    fn open_path_preview(&mut self, path: &Path) {
        let target = canonical(path);
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| !t.is_diff() && t.path().is_some_and(|p| canonical(p) == target))
        {
            self.select_tab(idx);
            return;
        }
        let mut tab = workspace::open_file(path, self.syntax);
        tab.is_preview = true;
        match self.tabs.iter().position(|t| t.is_preview) {
            Some(idx) => {
                tab.view = self.alloc_view();
                self.tabs[idx] = tab;
                self.active = idx;
                self.focus = Focus::Editor;
                self.find_open = false;
                self.register_doc(self.active);
                // The replaced tab's document (if any) is no longer referenced by
                // any tab; this closes it on the session side.
                self.reconcile_open_docs();
            },
            None => self.push_tab(tab),
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
        let mut tab = workspace::open_file_ignoring_size(&path, self.syntax);
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
            TabKind::Code { doc, .. } => *doc,
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
                    folds,
                    folded,
                    decos,
                    search_decos,
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
                        folds: folds.clone(),
                        folded: folded.clone(),
                        decos: decos.clone(),
                        search_decos: search_decos.clone(),
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

    /// Close the active tab.
    fn close_tab(&mut self) {
        self.close_tab_at(self.active);
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
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } => {
                let max = buffer.line_count().saturating_sub(1) as i64;
                let next = (i64::from(tab.editor.scroll_line) + i64::from(delta)).clamp(0, max);
                tab.editor.scroll_line = next as u32;
            },
            TabKind::Diff { scroll, .. }
            | TabKind::Blame { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
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
            TabKind::Diff { scroll, .. }
            | TabKind::Blame { scroll, .. }
            | TabKind::Graph { scroll, .. }
            | TabKind::LoadedConfig { scroll, .. }
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
                        self.close_tab_at(i);
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
                    self.close_tab_at(i);
                }
            },
            _ => {},
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
        if self.handle_status_mouse(mouse) {
            return;
        }
        let point = (mouse.column, mouse.row);
        let in_sidebar = self.sidebar_visible && rect_contains(self.sidebar_rect, point);
        let in_outline = self.outline_visible && rect_contains(self.outline_rect, point);
        match mouse.kind {
            MouseEventKind::ScrollDown if in_outline => self.outline_step(1),
            MouseEventKind::ScrollUp if in_outline => self.outline_step(-1),
            MouseEventKind::ScrollDown if in_sidebar => self.sidebar_wheel(3, mouse.row),
            MouseEventKind::ScrollUp if in_sidebar => self.sidebar_wheel(-3, mouse.row),
            MouseEventKind::ScrollDown => self.scroll_lines(3),
            MouseEventKind::ScrollUp => self.scroll_lines(-3),
            MouseEventKind::Down(MouseButton::Left) if in_outline => {
                self.handle_outline_click(mouse.row);
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
                if ctrl {
                    self.explorer.toggle_visible(view_row);
                } else if shift {
                    self.explorer.extend_visible(view_row);
                } else {
                    let streak = self.click_streak(col, row_y);
                    self.explorer.select_visible(view_row);
                    if streak >= 2 {
                        self.sidebar_promote_or_open_permanent();
                    } else {
                        self.sidebar_activate();
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
                        self.scm.selection.move_to(idx);
                        self.open_selected_diff();
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

    /// Esc in the editor: collapse multiple carets to the primary; with a single caret
    /// it preserves the former behavior of returning focus to the sidebar.
    fn collapse_carets_or_unfocus(&mut self) {
        let multi = matches!(
            self.tabs.get(self.active),
            Some(Tab {
                kind: TabKind::Code { .. },
                editor,
                ..
            }) if editor.has_multiple_cursors()
        );
        if multi {
            if let Some(Tab { editor, .. }) = self.tabs.get_mut(self.active) {
                editor.collapse_to_primary();
            }
        } else {
            self.toggle_focus();
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
        // The image, if any, belongs to the focused pane's active tab (keyed by its
        // stable ViewId so a focus switch re-transmits correctly). Documents also key
        // on the current page so paging re-transmits under an unchanged ViewId.
        let current = self.tabs.get(self.active).map(|t| t.view);
        let current_page = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Document { page, .. }) => *page,
            _ => 0,
        };
        // The pixels live directly on an image tab, or in a document tab's page cache.
        let image = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Image { image, .. }) => Some(image),
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
        if !self.graphical_cursor_enabled() || self.focus != Focus::Editor {
            return None;
        }
        let cell = CellPixels::detect()?;
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
        Some(GraphicsCaret {
            x,
            y,
            x_offset: 0,
            cell,
        })
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
            if let Ok(applied) = buffer.apply(&change, karet_text::EditContext::default()) {
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

    /// Save the active document, or report that there is no file to save. Tracks the
    /// in-flight save so a slow write shows a spinner in the tab.
    /// Handle a quit request: prompt when there are unsaved changes and
    /// `files.confirmOnExit` is set, otherwise exit immediately. Crash-recovery
    /// backups remain the safety net regardless of the choice.
    fn request_quit(&mut self) {
        let has_unsaved = self.all_tabs().any(|tab| tab.dirty);
        if has_unsaved && self.settings.files.confirm_on_exit {
            self.pending_quit = true;
            self.status = Some(
                "unsaved changes — press s to save all & quit, d to discard & quit, \
                 any other key to cancel"
                    .to_string(),
            );
        } else {
            self.should_quit = true;
        }
    }

    /// At the quit prompt: save every unsaved document, then exit once the saves
    /// drain (see [`App::on_backend_event`]). Exits immediately if nothing is dirty.
    fn quit_save_all(&mut self) {
        self.pending_quit = false;
        let saved = self.save_all_dirty();
        if saved == 0 {
            self.should_quit = true;
        } else {
            self.quitting = true;
            self.status = Some(format!("saving {saved} file(s) before quitting…"));
        }
    }

    /// Save every dirty code document across all panes (deduplicated by document),
    /// returning how many saves were issued. Each is tracked in `pending_saves`.
    fn save_all_dirty(&mut self) -> usize {
        let Some(backend) = self.backend.clone() else {
            return 0;
        };
        let mut docs: Vec<DocumentId> = Vec::new();
        for tab in self.all_tabs() {
            if tab.dirty
                && let TabKind::Code { doc: Some(d), .. } = &tab.kind
                && !docs.contains(d)
            {
                docs.push(*d);
            }
        }
        let now = Instant::now();
        let mut issued = 0;
        for doc in docs {
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

    fn save_active(&mut self) {
        let Some(doc) = self.active_code_doc() else {
            self.status = Some("save: open a text file".to_string());
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        match backend.send(id, SessionCommand::Save { doc }) {
            Ok(()) => {
                self.pending_saves.insert(id, doc);
                let now = Instant::now();
                for tab in &mut self.tabs {
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
        let has_selection = matches!(
            self.tabs.get(self.active),
            Some(Tab { kind: TabKind::Code { .. }, editor, .. })
                if editor.selection_range().is_some_and(|r| !r.is_empty())
        );
        if !has_selection {
            return;
        }
        self.copy_selection();
        self.submit_edit(editing::backspace);
    }

    /// Paste the system clipboard at the caret (or the active modal's text field).
    fn paste_from_clipboard(&mut self) {
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
        self.submit_edit(move |caret, sel, _b, base| {
            Some(editing::insert(caret, sel, base, &normalized))
        });
    }

    /// The soonest the event loop should wake for time-based UI: notification expiry,
    /// or a save-spinner animation frame while any save is in flight. `None` when the
    /// loop can park on its event sources alone.
    fn next_wake(&self) -> Option<Duration> {
        let notif = self.notifications.next_deadline(Instant::now());
        let spinner = (!self.pending_saves.is_empty()).then(|| Duration::from_millis(100));
        // Wake to repaint (hiding the tooltip) when the commit-badge reveal expires.
        let reveal = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Commit {
                explain_since: Some(since),
                ..
            }) => COMMIT_REVEAL.checked_sub(since.elapsed()),
            _ => None,
        };
        [notif, spinner, reveal].into_iter().flatten().min()
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
        // A save's answering event (saved or error) clears its tab spinner.
        if let Some(req) = id
            && let Some(doc) = self.pending_saves.remove(&req)
        {
            for tab in self.all_tabs_mut() {
                if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                    tab.saving_since = None;
                }
            }
        }
        // A "save all & quit" exits once every issued save has been answered.
        if self.quitting && self.pending_saves.is_empty() {
            self.should_quit = true;
        }
        match event {
            SessionEvent::Opened { doc, .. } => {
                self.open_docs.insert(doc);
                if let Some(req) = id
                    && let Some(path) = self.pending_open.remove(&req)
                {
                    for tab in self.all_tabs_mut() {
                        if let TabKind::Code {
                            path: p, doc: d, ..
                        } = &mut tab.kind
                            && d.is_none()
                            && *p == path
                        {
                            *d = Some(doc);
                        }
                    }
                }
            },
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
            SessionEvent::Progress { message, .. } => self.status = Some(message),
            // The single high-up funnel: every backend-reported condition becomes a
            // notification, so nothing is silently dropped.
            SessionEvent::Notification {
                severity,
                kind,
                message,
            } => self.notify(severity, kind, message),
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
            SessionEvent::SwapsFound { swaps } => self.arm_swap_recovery(swaps),
            SessionEvent::CommitReady { detail, changes } => {
                match id.and_then(|i| self.pending_commit_detail.remove(&i)) {
                    Some(CommitDest::Browser) => self.fill_graph_detail(detail, changes),
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
            let matches = matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
            if !matches {
                continue;
            }
            if let TabKind::Code {
                buffer,
                highlights,
                folds,
                folded,
                text,
                next_version,
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
                *folds = (*snap.folds).clone();
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

/// The canonical form of `path` for tab de-duplication, falling back to the path
/// as given when it cannot be resolved (e.g. it no longer exists).
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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

/// Best-effort probe for Kitty graphics protocol support.
///
/// Emits a graphics *query* (`a=q`, which does not display anything) followed by a
/// Primary Device Attributes request (`ESC [ c`) as a terminator, then reads the
/// reply straight from stdin. Returns `Some(true)` when the terminal answers the
/// graphics query, `Some(false)` when it answers DA1 but not the graphics query,
/// and `None` on timeout or I/O error.
///
/// Must run in raw mode and **before** the input reader thread starts, so the
/// query responses are consumed here rather than leaking into the UI as keystrokes.
/// Unlike the env-var [`detect_protocol`](image::detect_protocol) heuristic, this
/// recognizes any graphics-capable terminal, not just an allowlist.
fn probe_kitty_graphics(timeout: Duration) -> Option<bool> {
    use std::io::Read;

    // `i=31` is an arbitrary image id echoed back in the reply; `\x1b[c` (DA1) is
    // answered by every terminal and marks the end of the responses to read.
    let query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";
    let mut stdout = std::io::stdout();
    write!(stdout, "{query}").ok()?;
    stdout.flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut saw_csi = false;
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let b = byte[0];
                    buf.push(b);
                    // Stop once the DA1 reply (CSI … 'c') has been fully consumed.
                    saw_csi |= b == b'[';
                    if saw_csi && b == b'c' {
                        break;
                    }
                },
            }
        }
        let _ = tx.send(buf);
    });

    let buf = rx.recv_timeout(timeout).ok()?;
    // A Kitty graphics acknowledgement looks like: ESC _ G i=31 ; OK ESC \
    let ok = buf.windows(2).any(|w| w == b"_G") && buf.windows(2).any(|w| w == b"OK");
    Some(ok)
}

/// Probe whether the terminal supports OSC 22 (mouse pointer-shape hints, e.g.
/// hovering a resize divider showing the OS's resize cursor) by sending its
/// query form (`ESC ] 22 ; ? ESC \`) and checking for an OSC 22 reply before
/// the DA1 terminator. `Some(true)`/`Some(false)` when the terminal answered
/// before `timeout`, `None` on timeout or I/O error — in which case the
/// caller must not send pointer-shape hints (they'd be silently ignored at
/// best, or misinterpreted at worst). Same raw-mode/before-input-thread
/// constraint and terminating-DA1 trick as [`probe_kitty_graphics`].
fn probe_osc22_pointer_shape(timeout: Duration) -> Option<bool> {
    use std::io::Read;

    let query = "\x1b]22;?\x1b\\\x1b[c";
    let mut stdout = std::io::stdout();
    write!(stdout, "{query}").ok()?;
    stdout.flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut saw_csi = false;
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let b = byte[0];
                    buf.push(b);
                    saw_csi |= b == b'[';
                    if saw_csi && b == b'c' {
                        break;
                    }
                },
            }
        }
        let _ = tx.send(buf);
    });

    let buf = rx.recv_timeout(timeout).ok()?;
    // An OSC 22 reply contains its own introducer echoed back: ESC ] 22 ; ...
    let ok = buf.windows(3).any(|w| w == b"]22");
    Some(ok)
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
    let kitty_keyboard_supported = matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    );
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
    if probe_kitty_graphics(Duration::from_millis(200)) == Some(true) {
        app.graphics = GraphicsProtocol::Kitty;
    }
    // Same handshake for OSC 22 pointer-shape hints (col-resize/row-resize over
    // the sidebar/SCM dividers) — confirmed support only, never assumed.
    if probe_osc22_pointer_shape(Duration::from_millis(200)) == Some(true) {
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
        if app.settings.editor.graphical_cursor == Some(true) && !app.graphical_cursor_compatible()
        {
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
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Paste(text) => app.handle_paste(text),
        _ => {},
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

    #[test]
    fn quit_with_unsaved_changes_arms_the_prompt() {
        let mut app = app();
        app.tabs[app.active].dirty = true;
        app.dispatch(Command::Quit);
        assert!(app.pending_quit, "unsaved changes arm the quit prompt");
        assert!(!app.should_quit);
        assert_eq!(
            app.input_context().modal,
            Some(crate::keymap::Modal::QuitConfirm)
        );

        // Discarding exits.
        app.dispatch(Command::QuitDiscard);
        assert!(!app.pending_quit);
        assert!(app.should_quit);
    }

    #[test]
    fn quit_without_unsaved_changes_exits_immediately() {
        let mut app = app();
        app.dispatch(Command::Quit);
        assert!(!app.pending_quit);
        assert!(app.should_quit);
    }

    #[test]
    fn quit_prompt_disabled_by_confirm_on_exit_setting() {
        let mut app = app();
        app.settings.files.confirm_on_exit = false;
        app.tabs[app.active].dirty = true;
        app.dispatch(Command::Quit);
        assert!(
            app.should_quit,
            "confirmOnExit=false quits without prompting"
        );
    }

    #[test]
    fn quit_save_all_with_nothing_dirty_exits() {
        let mut app = app();
        app.pending_quit = true;
        app.dispatch(Command::QuitSaveAll);
        assert!(app.should_quit);
        assert!(!app.quitting, "no saves in flight");
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
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate);
        assert!(app.active_is_diff(), "the diff tab is shown");
        // Focus stays in the SCM pane so its action/selection keys keep working
        // (the bug was that previewing a diff moved focus to the editor, silently
        // disabling stage/unstage/discard/commit).
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);
        assert_eq!(app.tabs.len(), 1, "welcome tab is replaced, not appended");
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
        app.open_path_preview(&a);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            app.tabs[0].is_preview,
            "a preview-opened file is marked preview"
        );
        assert_eq!(app.tabs[0].path(), Some(a.as_path()));

        // Navigating to a second file replaces the preview tab in place — no
        // second tab, and the old one's path is gone.
        app.open_path_preview(&b);
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

        app.open_path_preview(&a);
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
        app.open_path_preview(&a);
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
        app.open_path_preview(&path);
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
            },
        )
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
    fn esc_with_a_single_caret_returns_focus_to_the_sidebar() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "ab"));
        app.focus = Focus::Editor;
        app.dispatch(Command::CollapseCarets);
        assert_eq!(app.focus, Focus::Sidebar);
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
        app.push_tab(workspace::open_file(&file, false));
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
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
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
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
        // Regression for "actions do nothing after opening a diff": the preview
        // must not steal focus away from the Source-Control pane.
        let Some(repo) = init_test_repo() else {
            return;
        };
        let (mut app, mut events) = scm_app(repo.path.clone());
        pump(&mut app, &mut events).await;
        assert_eq!(app.scm.changes.len(), 1);

        // Preview the change's diff — focus must stay on the SCM layer.
        app.dispatch(Command::SidebarActivate);
        assert!(app.active_is_diff());
        assert_eq!(app.focus_target(), FocusTarget::SourceControl);

        // Staging still works after the preview.
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
}

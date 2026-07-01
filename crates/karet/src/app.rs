//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Command`]s resolved from key events.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use color_eyre::eyre::eyre;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton,
    MouseEvent, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use karet_core::{BytePos, Decoration, DecorationKind, LineCol, Range, ThemeRole};
use karet_editor::EditorState;
use karet_filetype::IconStyle;
use karet_search::{FileHit, SearchQuery, WorkspaceSearch, search_in_file};
use karet_session::{
    Backend, Command as SessionCommand, DocSnapshot, DocumentId, Event as SessionEvent, EventRx,
    RequestId, Session, SessionConfig, SnapshotRx, ViewId, local,
};
use karet_text::TextBuffer;
use karet_theme::Theme;
use karet_vcs::FileChange;
use karet_widgets::FileTreeState;
use karet_widgets::image::{self, GraphicsProtocol};
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use crate::clipboard::Clipboard;
use crate::command::Command;
use crate::editing;
use crate::keymap::{self, Focus, FocusTarget, SidebarPanel};
use crate::overlay::{Overlay, OverlayEvent};
use crate::render::{FileView, Section};
use crate::tab::{Tab, TabKind, ViewMode};
use crate::{ui, workspace};

/// The Source-Control panel state: the changed files (staged first) and selection.
pub(crate) struct Scm {
    /// Changed files: the staged group first, then the working group.
    pub(crate) changes: Vec<FileChange>,
    /// The number of staged files at the front of `changes`.
    pub(crate) staged_count: usize,
    /// The selected entry (index into `changes`).
    pub(crate) selected: usize,
    /// The fixed end of a multi-row range selection, set while extending it.
    pub(crate) anchor: Option<usize>,
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

    /// The inclusive `(lo, hi)` row range currently selected (a single row when
    /// not extending a range).
    pub(crate) fn selected_range(&self) -> (usize, usize) {
        match self.anchor {
            Some(anchor) => (anchor.min(self.selected), anchor.max(self.selected)),
            None => (self.selected, self.selected),
        }
    }

    /// The repository-relative paths of the selected file(s).
    fn selected_paths(&self) -> Vec<PathBuf> {
        let (lo, hi) = self.selected_range();
        self.changes
            .get(lo..=hi)
            .map(|rows| rows.iter().map(|c| c.path.clone()).collect())
            .unwrap_or_default()
    }
}

/// The find-in-file bar state: the query and the match cursor.
#[derive(Default)]
pub(crate) struct FindState {
    /// The search query.
    pub(crate) query: String,
    /// The number of matches.
    pub(crate) count: usize,
    /// The current match (0-based).
    pub(crate) current: usize,
}

/// The workspace-search panel state.
#[derive(Default)]
pub(crate) struct SearchPanel {
    /// The query being typed/run.
    pub(crate) query: String,
    /// The streamed results (one entry per matching file).
    pub(crate) results: Vec<FileHit>,
    /// The selected result.
    pub(crate) selected: usize,
    /// Whether the query input is active (vs. browsing results).
    pub(crate) input: bool,
}

/// The maximum number of matching files the workspace search panel collects.
const SEARCH_RESULT_CAP: usize = 500;

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

/// The IDE shell state.
pub struct App {
    /// The workspace root.
    pub(crate) root: PathBuf,
    /// The active color theme.
    pub(crate) theme: Theme,
    /// Whether syntax highlighting is enabled.
    pub(crate) syntax: bool,
    /// The icon style for the explorer and activity bar.
    pub(crate) icon_style: IconStyle,
    /// The detected terminal graphics protocol.
    pub(crate) graphics: GraphicsProtocol,
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
    /// The open tabs.
    pub(crate) tabs: Vec<Tab>,
    /// The active tab index.
    pub(crate) active: usize,
    /// Paths of recently-closed file tabs, for "reopen closed editor" (newest last).
    pub(crate) closed: Vec<PathBuf>,
    /// The open modal overlay (quick-open / command palette), if any.
    pub(crate) overlay: Option<Overlay>,
    /// The find-in-file bar, if open.
    pub(crate) find: Option<FindState>,
    /// The in-progress commit message while the Source-Control commit input is open.
    pub(crate) commit_input: Option<String>,
    /// Paths awaiting a discard confirmation (set after pressing discard; cleared
    /// when the user confirms or cancels).
    pub(crate) pending_discard: Option<Vec<PathBuf>>,
    /// The workspace-search panel state.
    pub(crate) search: SearchPanel,
    /// A transient status message.
    pub(crate) status: Option<String>,
    /// The sidebar rect from the last frame (mouse hit-testing).
    pub(crate) sidebar_rect: Rect,
    /// The main content rect from the last frame.
    pub(crate) main_rect: Rect,
    /// The tab strip rect from the last frame (mouse hit-testing).
    pub(crate) tabstrip_rect: Rect,
    /// Per-tab clickable regions from the last frame (mouse hit-testing).
    pub(crate) tab_hits: Vec<TabHit>,
    /// Whether the active tab is being dragged to a new position.
    pub(crate) tab_dragging: bool,
    /// The sidebar's content area (below the header) from the last frame.
    pub(crate) sidebar_content_rect: Rect,
    /// The header panel-switcher cells (`1 2 3`) from the last frame.
    pub(crate) panel_hits: Vec<(u16, u16, SidebarPanel)>,
    /// Source-Control display-row → change-index map from the last frame.
    pub(crate) scm_row_map: Vec<Option<usize>>,
    /// The Source-Control list scroll offset from the last frame.
    pub(crate) scm_offset: usize,
    /// The search-results area from the last frame.
    pub(crate) search_results_rect: Rect,
    /// The search-results list scroll offset from the last frame.
    pub(crate) search_offset: usize,
    /// The status bar rect from the last frame (mouse hit-testing).
    pub(crate) status_rect: Rect,
    /// Clickable status-bar segments `(start, end, command)` from the last frame.
    pub(crate) status_hits: Vec<(u16, u16, Command)>,
    /// The active code tab's editor content area from the last frame.
    pub(crate) editor_rect: Rect,
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
    shown_image: Option<usize>,
    /// Whether the app should quit.
    should_quit: bool,
    /// The headless editor backend; edits route through it. `None` in unit tests,
    /// where editing commands are inert.
    backend: Option<Arc<dyn Backend>>,
    /// Open requests awaiting their `Opened` event, mapping request id → file path.
    pending_open: HashMap<RequestId, PathBuf>,
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
            theme: Theme::dark(),
            syntax,
            icon_style: IconStyle::default(),
            graphics: image::detect_protocol(),
            focus: Focus::Sidebar,
            sidebar_panel: SidebarPanel::Explorer,
            sidebar_visible: true,
            explorer: FileTreeState::new(),
            scm: Scm {
                changes,
                staged_count,
                selected: 0,
                anchor: None,
            },
            tabs: vec![Tab::welcome()],
            active: 0,
            closed: Vec::new(),
            overlay: None,
            find: None,
            commit_input: None,
            pending_discard: None,
            search: SearchPanel::default(),
            status: None,
            sidebar_rect: Rect::default(),
            main_rect: Rect::default(),
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            tab_dragging: false,
            sidebar_content_rect: Rect::default(),
            panel_hits: Vec::new(),
            scm_row_map: Vec::new(),
            scm_offset: 0,
            search_results_rect: Rect::default(),
            search_offset: 0,
            status_rect: Rect::default(),
            status_hits: Vec::new(),
            editor_rect: Rect::default(),
            editor_selecting: false,
            last_click: None,
            click_streak: 0,
            clipboard: Clipboard::new(),
            image_area: None,
            shown_image: None,
            should_quit: false,
            backend: None,
            pending_open: HashMap::new(),
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

    /// Open `path` as the initial tab at startup (used when `karet <file>` is run).
    pub fn open_initial(&mut self, path: &Path) {
        self.open_path(path);
    }

    /// Whether the active tab is a diff (enables diff-specific keys).
    fn active_is_diff(&self) -> bool {
        self.tabs.get(self.active).is_some_and(Tab::is_diff)
    }

    /// The pane that currently holds keyboard focus — the single value that
    /// determines which keybinding layer is live.
    pub(crate) fn focus_target(&self) -> FocusTarget {
        FocusTarget::from(self.focus, self.sidebar_panel, self.active_is_diff())
    }

    /// Handle a key press: route to the open overlay, else resolve via the keymap.
    fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        if self.commit_input.is_some() {
            self.handle_commit_key(key);
            return;
        }
        if self.pending_discard.is_some() {
            self.handle_discard_confirm_key(key);
            return;
        }
        if self.find.is_some() {
            self.handle_find_key(key);
            return;
        }
        // The Search panel captures text input, so globals run first, then its own keys.
        if self.focus == Focus::Sidebar && self.sidebar_panel == SidebarPanel::Search {
            if let Some(command) = keymap::global(key) {
                self.dispatch(command);
            } else {
                self.handle_search_key(key);
            }
            return;
        }
        if let Some(command) =
            keymap::resolve(self.focus, self.sidebar_panel, self.active_is_diff(), key)
        {
            self.dispatch(command);
        } else if self.focus == Focus::Editor
            && self.active_code_doc().is_some()
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && let KeyCode::Char(c) = key.code
        {
            // An unbound printable in the editor is text input.
            self.dispatch(Command::InsertChar(c));
        }
    }

    /// Route a key to the open overlay and act on its outcome.
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        match overlay.handle_key(key) {
            OverlayEvent::Consumed => {}
            OverlayEvent::Close => self.overlay = None,
            OverlayEvent::AcceptFile(path) => {
                self.overlay = None;
                self.open_path(&path);
            }
            OverlayEvent::AcceptCommand(cmd) => {
                self.overlay = None;
                self.dispatch(cmd);
            }
        }
    }

    /// Open the quick-open (go-to-file) overlay.
    fn open_quick_open(&mut self) {
        let files = workspace::list_files(&self.root, 2000);
        self.overlay = Some(Overlay::quick_open(files));
    }

    /// Open the find-in-file bar (only over a text/code tab).
    fn open_find(&mut self) {
        if matches!(
            self.tabs.get(self.active).map(|t| &t.kind),
            Some(TabKind::Code { .. })
        ) {
            self.find = Some(FindState::default());
            self.focus = Focus::Editor;
        } else {
            self.status = Some("find: open a text file first".to_string());
        }
    }

    /// Close the find bar and clear the active tab's match highlights.
    fn close_find(&mut self) {
        self.find = None;
        if let Some(Tab {
            kind: TabKind::Code { decos, .. },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            decos.clear();
        }
    }

    /// Handle a key while the find bar is open.
    fn handle_find_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => self.close_find(),
            KeyCode::Enter | KeyCode::Down => self.find_step(1),
            KeyCode::Up => self.find_step(-1),
            KeyCode::Char('g') if ctrl => self.find_step(if shift { -1 } else { 1 }),
            KeyCode::Backspace => {
                if let Some(find) = self.find.as_mut() {
                    find.query.pop();
                }
                self.run_find();
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                if let Some(find) = self.find.as_mut() {
                    find.query.push(c);
                }
                self.run_find();
            }
            _ => {}
        }
    }

    /// Re-run the in-file search and rebuild the active tab's match decorations.
    fn run_find(&mut self) {
        let query = match &self.find {
            Some(find) => find.query.clone(),
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
            if query.is_empty() {
                decos.clear();
            } else {
                let q = SearchQuery {
                    pattern: query,
                    case_sensitive: false,
                    ..Default::default()
                };
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
        if let Some(find) = self.find.as_mut() {
            find.count = count;
            find.current = 0;
        }
    }

    /// Move to the next/previous match (wrapping) and scroll it into view.
    fn find_step(&mut self, delta: i32) {
        let (count, current) = match &self.find {
            Some(find) => (find.count, find.current),
            None => return,
        };
        if count == 0 {
            return;
        }
        let next = (current as i64 + i64::from(delta)).rem_euclid(count as i64) as usize;
        if let Some(find) = self.find.as_mut() {
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

    /// Focus the Search panel and (re)start the query input.
    fn start_global_search(&mut self) {
        self.sidebar_panel = SidebarPanel::Search;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.search.input = true;
    }

    /// Handle a key while the Search panel has focus.
    fn handle_search_key(&mut self, key: KeyEvent) {
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                if self.search.input {
                    self.search.input = false;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Enter => {
                if self.search.input {
                    self.run_global_search();
                    self.search.input = false;
                } else {
                    self.open_selected_result();
                }
            }
            KeyCode::Down => self.search_select(1),
            KeyCode::Up => self.search_select(-1),
            KeyCode::Char('/') if !self.search.input => self.search.input = true,
            KeyCode::Char('j') if !self.search.input => self.search_select(1),
            KeyCode::Char('k') if !self.search.input => self.search_select(-1),
            KeyCode::Backspace if self.search.input => {
                self.search.query.pop();
            }
            KeyCode::Char(c) if self.search.input && plain => self.search.query.push(c),
            _ => {}
        }
    }

    /// Run the workspace search for the current query, collecting up to the cap.
    fn run_global_search(&mut self) {
        self.search.results.clear();
        self.search.selected = 0;
        if self.search.query.is_empty() {
            return;
        }
        let query = SearchQuery {
            pattern: self.search.query.clone(),
            case_sensitive: false,
            ..Default::default()
        };
        let mut results = Vec::new();
        let _ = WorkspaceSearch::new().run(&self.root, &query, |hit| {
            if results.len() < SEARCH_RESULT_CAP {
                results.push(hit);
            }
        });
        self.search.results = results;
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
            Command::Quit => self.should_quit = true,
            Command::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Command::ToggleFocus => self.toggle_focus(),
            Command::SelectPanel(panel) => {
                self.sidebar_panel = panel;
                self.sidebar_visible = true;
                self.focus = Focus::Sidebar;
            }
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
            Command::Copy => self.copy_selection(),
            Command::CopyPath => self.copy_path(false),
            Command::CopyRelativePath => self.copy_path(true),
            Command::SidebarUp => self.sidebar_step(-1),
            Command::SidebarDown => self.sidebar_step(1),
            Command::SidebarActivate => self.sidebar_activate(),
            Command::SidebarCollapse => self.sidebar_collapse(),
            Command::SidebarToggleExpand => self.sidebar_toggle_expand(),
            Command::CaretUp => self.caret_motion(false, EditorState::move_up),
            Command::CaretDown => self.caret_motion(false, EditorState::move_down),
            Command::CaretLeft => self.caret_motion(false, EditorState::move_left),
            Command::CaretRight => self.caret_motion(false, EditorState::move_right),
            Command::SelectUp => self.caret_motion(true, EditorState::move_up),
            Command::SelectDown => self.caret_motion(true, EditorState::move_down),
            Command::SelectLeft => self.caret_motion(true, EditorState::move_left),
            Command::SelectRight => self.caret_motion(true, EditorState::move_right),
            Command::ScrollUp => self.scroll_lines(-1),
            Command::ScrollDown => self.scroll_lines(1),
            Command::PageUp => self.scroll_lines(-i32::from(self.main_rect.height.max(1))),
            Command::PageDown => self.scroll_lines(i32::from(self.main_rect.height.max(1))),
            Command::Top => self.scroll_edge(true),
            Command::Bottom => self.scroll_edge(false),
            Command::ToggleDiffLayout => self.toggle_diff_layout(),
            Command::NextChangedFile => self.step_changed_file(1),
            Command::PrevChangedFile => self.step_changed_file(-1),
            Command::InsertChar(c) => {
                let s = c.to_string();
                self.submit_edit(move |caret, sel, _b, base| {
                    Some(editing::insert(caret, sel, base, &s))
                });
            }
            Command::InsertNewline => {
                self.submit_edit(|caret, sel, buf, base| {
                    Some(editing::newline(caret, sel, buf, base))
                });
            }
            Command::DeleteBackward => self.submit_edit(editing::backspace),
            Command::DeleteForward => self.submit_edit(editing::delete_forward),
            Command::Indent => {
                self.submit_edit(|caret, sel, _b, base| Some(editing::indent(caret, sel, base)));
            }
            Command::Dedent => {
                self.submit_edit(|caret, _sel, buf, base| editing::dedent(caret, buf, base));
            }
            Command::Undo => self.send_doc_command(|doc| SessionCommand::Undo { doc }),
            Command::Redo => self.send_doc_command(|doc| SessionCommand::Redo { doc }),
            Command::Save => self.save_active(),
            Command::Cut => self.cut(),
            Command::Paste => self.paste_from_clipboard(),
            Command::ScmSelectUp => self.scm_extend(-1),
            Command::ScmSelectDown => self.scm_extend(1),
            Command::ScmStage => self.scm_send_paths(|paths| SessionCommand::Stage { paths }),
            Command::ScmUnstage => self.scm_send_paths(|paths| SessionCommand::Unstage { paths }),
            Command::ScmToggleStage => self.scm_toggle_stage(),
            Command::ScmStageAll => self.send_vcs(SessionCommand::StageAll),
            Command::ScmUnstageAll => self.send_vcs(SessionCommand::UnstageAll),
            Command::ScmDiscard => self.scm_arm_discard(),
            Command::ScmCommit => self.scm_open_commit_input(),
            Command::ScmRefresh => self.send_vcs(SessionCommand::RefreshVcs),
        }
    }

    /// Move focus between the sidebar and the editor.
    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Editor,
            Focus::Editor => Focus::Sidebar,
        };
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
            }
            SidebarPanel::SourceControl => {
                let len = self.scm.changes.len();
                if len > 0 {
                    let next =
                        (self.scm.selected as i64 + i64::from(delta)).clamp(0, len as i64 - 1);
                    self.scm.selected = next as usize;
                }
                // A plain move collapses any range selection.
                self.scm.anchor = None;
            }
            SidebarPanel::Search => self.search_select(delta),
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
                        self.open_path(&path);
                    }
                }
            }
            SidebarPanel::SourceControl => self.open_selected_diff(),
            SidebarPanel::Search => {}
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

    /// Open a diff tab for the selected Source-Control entry.
    fn open_selected_diff(&mut self) {
        let Some(change) = self.scm.changes.get(self.scm.selected) else {
            return;
        };
        let section = self.scm.section(self.scm.selected);
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
                view: ViewMode::Unified,
                scroll: 0,
            },
        );
        self.push_tab(tab);
    }

    // --- source control ---------------------------------------------------

    /// Send a fire-and-forget command to the backend (no document context).
    fn send_vcs(&mut self, command: SessionCommand) {
        if let Some(backend) = &self.backend {
            let id = backend.next_id();
            let _ = backend.send(id, command);
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

    /// Stage the selection if it is in the working group, unstage it if it is
    /// staged (the section of the cursor row decides).
    fn scm_toggle_stage(&mut self) {
        let paths = self.scm.selected_paths();
        if paths.is_empty() {
            return;
        }
        if self.scm.selected < self.scm.staged_count {
            self.send_vcs(SessionCommand::Unstage { paths });
        } else {
            self.send_vcs(SessionCommand::Stage { paths });
        }
    }

    /// Extend the Source-Control range selection by `delta` rows.
    fn scm_extend(&mut self, delta: i32) {
        let len = self.scm.changes.len();
        if len == 0 {
            return;
        }
        if self.scm.anchor.is_none() {
            self.scm.anchor = Some(self.scm.selected);
        }
        let next = (self.scm.selected as i64 + i64::from(delta)).clamp(0, len as i64 - 1);
        self.scm.selected = next as usize;
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

    /// Handle a key while the commit-message input is open.
    fn handle_commit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.commit_input = None;
                self.status = Some("commit cancelled".to_string());
            }
            KeyCode::Enter => {
                let message = self.commit_input.take().unwrap_or_default();
                let message = message.trim().to_string();
                if message.is_empty() {
                    self.commit_input = Some(String::new());
                    self.status = Some("commit: message required".to_string());
                } else {
                    self.send_vcs(SessionCommand::Commit { message });
                }
            }
            KeyCode::Backspace => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.pop();
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.push(c);
                }
            }
            _ => {}
        }
    }

    /// Handle a key while a discard confirmation is pending.
    fn handle_discard_confirm_key(&mut self, key: KeyEvent) {
        let confirmed = matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter);
        let paths = self.pending_discard.take();
        if confirmed {
            if let Some(paths) = paths {
                self.send_vcs(SessionCommand::Discard { paths });
                self.status = Some("discarded".to_string());
            }
        } else {
            self.status = Some("discard cancelled".to_string());
        }
    }

    /// Replace the Source-Control panel state from a fresh backend status.
    fn apply_vcs_status(&mut self, staged: Vec<FileChange>, working: Vec<FileChange>) {
        let staged_count = staged.len();
        let mut changes = staged;
        changes.extend(working);
        let selected = self.scm.selected.min(changes.len().saturating_sub(1));
        self.scm = Scm {
            changes,
            staged_count,
            selected,
            anchor: None,
        };
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

    /// Release any session documents no longer shown in a tab (the session
    /// ref-counts opens; the app balances them). Call after closing tabs.
    fn reconcile_open_docs(&mut self) {
        let live: HashSet<DocumentId> = self.tabs.iter().filter_map(Self::tab_doc).collect();
        let stale: Vec<DocumentId> = self.open_docs.difference(&live).copied().collect();
        for doc in stale {
            self.open_docs.remove(&doc);
            if let Some(backend) = &self.backend {
                let id = backend.next_id();
                let _ = backend.send(id, SessionCommand::CloseDocument { doc });
            }
        }
    }

    /// Switch to the tab at `index`, focusing the editor.
    fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.focus = Focus::Editor;
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

    /// While dragging, move the active tab under column `x`.
    fn drag_tab_to(&mut self, x: u16) {
        if let Some((target, _)) = self.tab_at(x)
            && target != self.active
        {
            self.move_tab(self.active, target);
        }
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
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } => {
                let max = buffer.line_count().saturating_sub(1) as i64;
                let next = (i64::from(tab.editor.scroll_line) + i64::from(delta)).clamp(0, max);
                tab.editor.scroll_line = next as u32;
            }
            TabKind::Diff { scroll, .. } => {
                let next = (i64::from(*scroll) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
                *scroll = next as u16;
            }
            TabKind::Hex { bytes, scroll, .. } => {
                let max = bytes.len().div_ceil(16).saturating_sub(1) as i64;
                let next = (*scroll as i64 + i64::from(delta)).clamp(0, max);
                *scroll = next as usize;
            }
            _ => {}
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
            }
            TabKind::Diff { scroll, .. } => *scroll = if top { 0 } else { u16::MAX },
            TabKind::Hex { bytes, scroll, .. } => {
                *scroll = if top {
                    0
                } else {
                    bytes.len().div_ceil(16).saturating_sub(1)
                };
            }
            _ => {}
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
        let next = (self.scm.selected as i64 + i64::from(delta)).clamp(0, len as i64 - 1) as usize;
        self.scm.selected = next;
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

    /// The tab at column `x` and whether `x` is on its close glyph.
    fn tab_at(&self, x: u16) -> Option<(usize, bool)> {
        self.tab_hits
            .iter()
            .enumerate()
            .find_map(|(i, h)| (x >= h.start && x < h.end).then_some((i, x == h.close)))
    }

    /// Handle a mouse event over the tab strip (click to switch / close, wheel to
    /// cycle). Returns `true` when the event was consumed.
    fn handle_tabstrip_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !rect_contains(self.tabstrip_rect, (mouse.column, mouse.row)) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.next_tab(),
            MouseEventKind::ScrollUp => self.prev_tab(),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((i, on_close)) = self.tab_at(mouse.column) {
                    if on_close {
                        self.close_tab_at(i);
                    } else {
                        self.select_tab(i);
                        self.tab_dragging = true;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some((i, _)) = self.tab_at(mouse.column) {
                    self.close_tab_at(i);
                }
            }
            _ => {}
        }
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
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.overlay.is_some() {
            return;
        }
        // An in-progress tab drag captures motion until the button is released.
        if self.tab_dragging {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => self.drag_tab_to(mouse.column),
                MouseEventKind::Up(MouseButton::Left) => self.tab_dragging = false,
                _ => {}
            }
            return;
        }
        // An in-progress text selection captures motion until the button is released.
        if self.editor_selecting {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.drag_select_to(mouse.column, mouse.row);
                }
                MouseEventKind::Up(MouseButton::Left) => self.editor_selecting = false,
                _ => {}
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
        match mouse.kind {
            MouseEventKind::ScrollDown if in_sidebar => self.sidebar_step(1),
            MouseEventKind::ScrollUp if in_sidebar => self.sidebar_step(-1),
            MouseEventKind::ScrollDown => self.scroll_lines(3),
            MouseEventKind::ScrollUp => self.scroll_lines(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                if in_sidebar {
                    self.handle_sidebar_click(mouse.column, mouse.row);
                } else {
                    self.handle_editor_click(mouse);
                }
            }
            _ => {}
        }
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
    /// select and activate the clicked row.
    fn handle_sidebar_click(&mut self, col: u16, row_y: u16) {
        self.focus = Focus::Sidebar;
        if let Some(panel) = self.panel_at(col, row_y) {
            self.dispatch(Command::SelectPanel(panel));
            return;
        }
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                if !rect_contains(self.sidebar_content_rect, (col, row_y)) {
                    return;
                }
                let view_row = (row_y - self.sidebar_content_rect.y) as usize;
                let root = self.root.clone();
                self.explorer.ensure_built(&root);
                self.explorer.select_visible(view_row);
                self.sidebar_activate();
            }
            SidebarPanel::SourceControl => {
                if !rect_contains(self.sidebar_content_rect, (col, row_y)) {
                    return;
                }
                let display = self.scm_offset + (row_y - self.sidebar_content_rect.y) as usize;
                if let Some(Some(idx)) = self.scm_row_map.get(display).copied() {
                    self.scm.selected = idx;
                    self.open_selected_diff();
                }
            }
            SidebarPanel::Search => {
                // The query line sits just above the results; click it to type.
                if row_y == self.sidebar_content_rect.y {
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
            }
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
                        .line(editor.cursor.line as usize)
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
            if extend {
                editor.ensure_anchor();
            } else {
                editor.clear_selection();
            }
            motion(editor, buffer);
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
            }
            _ => 1,
        };
        self.last_click = Some((now, col, row));
        self.click_streak = streak;
        streak
    }

    /// Handle a left click in the editor: focus it and place the caret (single
    /// click), select the word (double) or the line (triple).
    fn handle_editor_click(&mut self, mouse: MouseEvent) {
        self.focus = Focus::Editor;
        if !rect_contains(self.editor_rect, (mouse.column, mouse.row)) {
            return;
        }
        let area = self.editor_rect;
        let streak = self.click_streak(mouse.column, mouse.row);
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let pos = editor.pos_at(area, buffer, mouse.column, mouse.row);
            match streak {
                2 => {
                    let (anchor, head) = word_at(buffer, pos);
                    editor.set_selection(buffer, anchor, head);
                }
                3 => {
                    let (anchor, head) = line_span(buffer, pos.line);
                    editor.set_selection(buffer, anchor, head);
                }
                _ => editor.set_caret(buffer, pos),
            }
        }
        // Only a single click starts a drag-select; word/line clicks are atomic.
        self.editor_selecting = streak == 1;
    }

    /// Extend the editor selection to the cell under `(col, row)` while dragging.
    fn drag_select_to(&mut self, col: u16, row: u16) {
        let area = self.editor_rect;
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let pos = editor.pos_at(area, buffer, col, row);
            editor.extend_to(buffer, pos);
        }
    }

    /// Transmit or clear the active tab's Kitty image after a frame is drawn.
    fn flush_graphics(&mut self) {
        if self.graphics != GraphicsProtocol::Kitty {
            return;
        }
        let mut stdout = io::stdout();
        match self.image_area {
            Some(area) if self.shown_image != Some(self.active) => {
                let _ = write!(stdout, "{}", image::kitty_delete_all());
                let _ = write!(stdout, "\x1b[{};{}H", area.y + 1, area.x + 1);
                if let Some(Tab {
                    kind: TabKind::Image { image, .. },
                    ..
                }) = self.tabs.get(self.active)
                {
                    let _ = write!(stdout, "{}", image.kitty_escape(area.width, area.height));
                }
                let _ = stdout.flush();
                self.shown_image = Some(self.active);
            }
            None if self.shown_image.is_some() => {
                let _ = write!(stdout, "{}", image::kitty_delete_all());
                let _ = stdout.flush();
                self.shown_image = None;
            }
            _ => {}
        }
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
        F: FnOnce(LineCol, Option<Range>, &TextBuffer, u64) -> Option<editing::Edit>,
    {
        if self.backend.is_none() {
            return;
        }
        let idx = self.active;
        let (doc, edit) = match self.tabs.get(idx) {
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
            }) => (
                *doc,
                build(
                    editor.cursor,
                    editor.selection_range(),
                    buffer,
                    *next_version,
                ),
            ),
            _ => return,
        };
        let Some(edit) = edit else {
            return;
        };
        if let Some(backend) = &self.backend {
            let id = backend.next_id();
            let _ = backend.send(
                id,
                SessionCommand::ApplyChange {
                    doc,
                    change: edit.change,
                },
            );
        }
        if let Some(Tab {
            kind: TabKind::Code { next_version, .. },
            editor,
            ..
        }) = self.tabs.get_mut(idx)
        {
            *next_version += 1;
            editor.cursor = edit.caret;
            editor.selection_anchor = None;
            editor.scroll_to(edit.caret);
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

    /// Send a document command for the active code tab, if any.
    fn send_doc_command(&mut self, make: impl FnOnce(DocumentId) -> SessionCommand) {
        let Some(doc) = self.active_code_doc() else {
            return;
        };
        if let Some(backend) = &self.backend {
            let id = backend.next_id();
            let _ = backend.send(id, make(doc));
        }
    }

    /// Save the active document, or report that there is no file to save.
    fn save_active(&mut self) {
        match self.active_code_doc() {
            Some(doc) => {
                if let Some(backend) = &self.backend {
                    let id = backend.next_id();
                    let _ = backend.send(id, SessionCommand::Save { doc });
                }
            }
            None => self.status = Some("save: open a text file".to_string()),
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

    /// Paste the system clipboard at the caret.
    fn paste_from_clipboard(&mut self) {
        match self.clipboard.get() {
            Ok(text) => self.handle_paste(text),
            Err(_) => self.status = Some("paste: clipboard unavailable".to_string()),
        }
    }

    /// Insert pasted text as a single edit (one undo group). Shared by the paste
    /// command and bracketed paste, so pasted text is never interpreted as keys.
    fn handle_paste(&mut self, text: String) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        if normalized.is_empty() {
            return;
        }
        self.submit_edit(move |caret, sel, _b, base| {
            Some(editing::insert(caret, sel, base, &normalized))
        });
    }

    /// Handle a backend event: correlate opens to tabs, surface save/progress status.
    fn on_backend_event(&mut self, id: Option<RequestId>, event: SessionEvent) {
        match event {
            SessionEvent::Opened { doc, .. } => {
                self.open_docs.insert(doc);
                if let Some(req) = id
                    && let Some(path) = self.pending_open.remove(&req)
                {
                    for tab in &mut self.tabs {
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
            }
            SessionEvent::Saved { .. } => self.status = Some("saved".to_string()),
            // The fresh content arrives via the snapshot stream; just note it.
            SessionEvent::Reloaded { .. } => {
                self.status = Some("reloaded from disk".to_string());
            }
            SessionEvent::ExternalConflict { .. } => {
                self.status = Some("⚠ file changed on disk — you have unsaved changes".to_string());
            }
            SessionEvent::Progress { message, .. } => self.status = Some(message),
            SessionEvent::VcsStatus { staged, working } => self.apply_vcs_status(staged, working),
            SessionEvent::Committed { oid } => {
                self.commit_input = None;
                let short: String = oid.chars().take(7).collect();
                self.status = Some(format!("committed {short}"));
            }
            _ => {}
        }
    }

    /// Apply a document snapshot to the matching code tab(s): the snapshot is the
    /// render source of truth (buffer, highlights, and the search text).
    fn on_snapshot(&mut self, doc: DocumentId, snap: &DocSnapshot) {
        for tab in &mut self.tabs {
            if let TabKind::Code {
                doc: Some(d),
                buffer,
                highlights,
                text,
                next_version,
                ..
            } = &mut tab.kind
                && *d == doc
            {
                *buffer = snap.buffer.clone();
                *highlights = (*snap.highlights).clone();
                *text = snap.buffer.text();
                *next_version = (*next_version).max(snap.version);
            }
        }
    }
}

/// Whether the screen point `(x, y)` lies inside `r`.
fn rect_contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// The canonical form of `path` for tab de-duplication, falling back to the path
/// as given when it cannot be resolved (e.g. it no longer exists).
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The (anchor, head) span of the word under `pos`, or the single character there
/// when the cursor is not on a word character.
fn word_at(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    let line = buffer.line(pos.line as usize).unwrap_or_default();
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len() as u32;
    let col = pos.col.min(n);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = col;
    while start > 0 && is_word(chars[start as usize - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < n && is_word(chars[end as usize]) {
        end += 1;
    }
    if start == end {
        (
            LineCol::new(pos.line, col),
            LineCol::new(pos.line, (col + 1).min(n)),
        )
    } else {
        (LineCol::new(pos.line, start), LineCol::new(pos.line, end))
    }
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
pub fn run(mut app: App) -> color_eyre::Result<()> {
    if !matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    ) {
        return Err(eyre!(
            "karet requires a terminal with kitty keyboard protocol support \
             (kitty, ghostty, WezTerm, foot, …)"
        ));
    }

    // The session backend runs on its own Tokio runtime; the UI task selects over
    // terminal input, backend events, and document snapshots so it never blocks.
    let runtime = tokio::runtime::Runtime::new().map_err(|e| eyre!("tokio runtime: {e}"))?;
    let (session, events, snaps) = Session::new(SessionConfig {
        roots: vec![app.root.clone()],
        ..SessionConfig::default()
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

    let result = runtime.block_on(async move {
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        app.backend = Some(backend);
        app.register_open_tabs();
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
        }

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
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::SidebarPanel;
    use karet_vcs::StatusKind;

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
    fn scm_range_selection_collects_both_paths() {
        // `app()` seeds one staged (a.rs) and one working (b.rs) change.
        let mut app = app();
        app.scm.selected = 0;
        app.dispatch(Command::ScmSelectDown);
        assert_eq!(app.scm.selected_range(), (0, 1));
        assert_eq!(app.scm.selected_paths().len(), 2);
    }

    #[test]
    fn scm_plain_move_collapses_range() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::ScmSelectDown);
        assert!(app.scm.anchor.is_some());
        // A non-extending move in the SCM panel clears the range.
        app.dispatch(Command::SidebarDown);
        assert!(app.scm.anchor.is_none());
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
        assert_eq!(app.scm.anchor, None);
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
    fn opening_a_diff_replaces_welcome_and_focuses_editor() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate);
        assert!(app.active_is_diff());
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs.len(), 1, "welcome tab is replaced, not appended");
    }

    #[test]
    fn stepping_changed_files_walks_the_scm_list() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Command::SidebarActivate); // opens a.rs (index 0)
        app.dispatch(Command::NextChangedFile);
        assert_eq!(app.scm.selected, 1);
        app.dispatch(Command::PrevChangedFile);
        assert_eq!(app.scm.selected, 0);
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
        use std::sync::atomic::{AtomicUsize, Ordering};
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
                decos: Vec::new(),
            },
        ));
        app.dispatch(Command::OpenFind);
        if let Some(find) = app.find.as_mut() {
            find.query = "foo".to_string();
        }
        app.run_find();
        assert_eq!(app.find.as_ref().map(|f| f.count), Some(2));
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
                decos: Vec::new(),
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
                decos: Vec::new(),
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
    fn double_click_selects_the_word() {
        let mut app = app();
        app.push_tab(text_tab("t.rs", "foo bar baz"));
        app.editor_rect = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
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
        let mut app = app();
        app.tab_hits = vec![
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
        assert_eq!(app.tab_at(3), Some((0, false)));
        assert_eq!(app.tab_at(8), Some((0, true)));
        assert_eq!(app.tab_at(12), Some((1, false)));
        assert_eq!(app.tab_at(18), Some((1, true)));
        assert_eq!(app.tab_at(25), None);
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
        app.handle_sidebar_click(25, 1); // header row, the "2" cell
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
        app.scm_offset = 0;
        // Display rows: 0 header, 1 a.rs(0), 2 header, 3 b.rs(1).
        app.scm_row_map = vec![None, Some(0), None, Some(1)];
        app.handle_sidebar_click(2, 5); // content row 3 -> change index 1
        assert_eq!(app.scm.selected, 1);
        assert!(app.active_is_diff());
    }

    #[test]
    fn dragging_moves_the_active_tab() {
        let mut app = app();
        app.push_tab(code_tab("a.rs"));
        app.push_tab(code_tab("b.rs"));
        app.push_tab(code_tab("c.rs"));
        app.tab_hits = vec![
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
        ];
        app.active = 0;
        app.tab_dragging = true;
        app.drag_tab_to(20); // over the third tab
        let titles: Vec<_> = app.tabs.iter().map(|t| t.title.clone()).collect();
        assert_eq!(titles, vec!["b.rs", "c.rs", "a.rs"]);
        assert_eq!(app.active, 2);
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
}

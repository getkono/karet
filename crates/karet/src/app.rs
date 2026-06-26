//! The IDE shell: application state, the keymap-driven event loop, and terminal
//! setup. The shell composes the engine/widget crates — it owns the open tabs and
//! the sidebar, and applies [`Action`]s resolved from key events.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use color_eyre::eyre::eyre;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use karet_core::{BytePos, Decoration, DecorationKind, Range, ThemeRole};
use karet_search::{SearchQuery, search_in_file};
use karet_theme::Theme;
use karet_vcs::FileChange;
use karet_widgets::FileTreeState;
use karet_widgets::image::{self, GraphicsProtocol};
use ratatui::layout::Rect;

use crate::keymap::{self, Action, Focus, SidebarPanel};
use crate::overlay::{Overlay, OverlayEvent, PaletteCommand};
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

/// The IDE shell state.
pub struct App {
    /// The workspace root.
    pub(crate) root: PathBuf,
    /// The active color theme.
    pub(crate) theme: Theme,
    /// Whether syntax highlighting is enabled.
    pub(crate) syntax: bool,
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
    /// The open modal overlay (quick-open / command palette), if any.
    pub(crate) overlay: Option<Overlay>,
    /// The find-in-file bar, if open.
    pub(crate) find: Option<FindState>,
    /// A transient status message.
    pub(crate) status: Option<String>,
    /// The sidebar rect from the last frame (mouse hit-testing).
    pub(crate) sidebar_rect: Rect,
    /// The main content rect from the last frame.
    pub(crate) main_rect: Rect,
    /// The active Kitty image placement rect (set by the renderer), if any.
    pub(crate) image_area: Option<Rect>,
    /// The tab index whose image is currently transmitted to the terminal.
    shown_image: Option<usize>,
    /// Whether the app should quit.
    should_quit: bool,
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
            graphics: image::detect_protocol(),
            focus: Focus::Sidebar,
            sidebar_panel: SidebarPanel::Explorer,
            sidebar_visible: true,
            explorer: FileTreeState::new(),
            scm: Scm {
                changes,
                staged_count,
                selected: 0,
            },
            tabs: vec![Tab::welcome()],
            active: 0,
            overlay: None,
            find: None,
            status: None,
            sidebar_rect: Rect::default(),
            main_rect: Rect::default(),
            image_area: None,
            shown_image: None,
            should_quit: false,
        }
    }

    /// Open `path` as the initial tab at startup (used when `karet <file>` is run).
    pub fn open_initial(&mut self, path: &Path) {
        let tab = workspace::open_file(path, self.syntax);
        self.push_tab(tab);
    }

    /// Whether the active tab is a diff (enables diff-specific keys).
    fn active_is_diff(&self) -> bool {
        self.tabs.get(self.active).is_some_and(Tab::is_diff)
    }

    /// Handle a key press: route to the open overlay, else resolve via the keymap.
    fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        if self.find.is_some() {
            self.handle_find_key(key);
            return;
        }
        if let Some(action) = keymap::resolve(self.focus, self.active_is_diff(), key) {
            self.dispatch(action);
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
                let tab = workspace::open_file(&path, self.syntax);
                self.push_tab(tab);
            }
            OverlayEvent::AcceptCommand(cmd) => {
                self.overlay = None;
                self.run_palette_command(cmd);
            }
        }
    }

    /// Open the quick-open (go-to-file) overlay.
    fn open_quick_open(&mut self) {
        let files = workspace::list_files(&self.root, 2000);
        self.overlay = Some(Overlay::quick_open(files));
    }

    /// Apply a command chosen from the command palette.
    fn run_palette_command(&mut self, cmd: PaletteCommand) {
        match cmd {
            PaletteCommand::ToggleSidebar => self.dispatch(Action::ToggleSidebar),
            PaletteCommand::ShowExplorer => {
                self.dispatch(Action::SelectPanel(SidebarPanel::Explorer));
            }
            PaletteCommand::ShowSearch => self.dispatch(Action::SelectPanel(SidebarPanel::Search)),
            PaletteCommand::ShowSourceControl => {
                self.dispatch(Action::SelectPanel(SidebarPanel::SourceControl));
            }
            PaletteCommand::QuickOpen => self.open_quick_open(),
            PaletteCommand::Find => self.dispatch(Action::OpenFind),
            PaletteCommand::GlobalSearch => self.dispatch(Action::OpenGlobalSearch),
            PaletteCommand::CloseTab => self.dispatch(Action::CloseTab),
            PaletteCommand::Quit => self.should_quit = true,
        }
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

    /// Apply a resolved [`Action`].
    fn dispatch(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::ToggleSidebar => self.sidebar_visible = !self.sidebar_visible,
            Action::ToggleFocus => self.toggle_focus(),
            Action::SelectPanel(panel) => {
                self.sidebar_panel = panel;
                self.sidebar_visible = true;
                self.focus = Focus::Sidebar;
            }
            Action::OpenQuickOpen => self.open_quick_open(),
            Action::OpenCommandPalette => self.overlay = Some(Overlay::command_palette()),
            Action::OpenFind => self.open_find(),
            // The global search panel is wired in the next commit.
            Action::OpenGlobalSearch => self.status = Some("not yet available".to_string()),
            Action::CloseTab => self.close_tab(),
            Action::SidebarUp => self.sidebar_step(-1),
            Action::SidebarDown => self.sidebar_step(1),
            Action::SidebarActivate => self.sidebar_activate(),
            Action::SidebarCollapse => self.sidebar_collapse(),
            Action::SidebarToggleExpand => self.sidebar_toggle_expand(),
            Action::ScrollUp => self.scroll_lines(-1),
            Action::ScrollDown => self.scroll_lines(1),
            Action::PageUp => self.scroll_lines(-i32::from(self.main_rect.height.max(1))),
            Action::PageDown => self.scroll_lines(i32::from(self.main_rect.height.max(1))),
            Action::Top => self.scroll_edge(true),
            Action::Bottom => self.scroll_edge(false),
            Action::ToggleDiffLayout => self.toggle_diff_layout(),
            Action::NextChangedFile => self.step_changed_file(1),
            Action::PrevChangedFile => self.step_changed_file(-1),
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
            }
            SidebarPanel::Search => {}
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
                        let tab = workspace::open_file(&path, self.syntax);
                        self.push_tab(tab);
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

    /// Add a tab, replacing a lone Welcome tab, and focus the editor.
    fn push_tab(&mut self, tab: Tab) {
        if self.tabs.len() == 1 && matches!(self.tabs[0].kind, TabKind::Welcome) {
            self.tabs[0] = tab;
            self.active = 0;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
        }
        self.focus = Focus::Editor;
    }

    /// Close the active tab, falling back to a Welcome tab when the last closes.
    fn close_tab(&mut self) {
        if self.tabs.len() <= 1 {
            self.tabs = vec![Tab::welcome()];
            self.active = 0;
            self.focus = Focus::Sidebar;
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
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

    /// Handle a mouse event: wheel scrolls (the sidebar or the active tab) and a
    /// left click moves focus to the clicked region.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.overlay.is_some() {
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
                self.focus = if in_sidebar {
                    Focus::Sidebar
                } else {
                    Focus::Editor
                };
            }
            _ => {}
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

/// Whether the screen point `(x, y)` lies inside `r`.
fn rect_contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
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
    let _ = crossterm::execute!(io::stdout(), EnableMouseCapture);

    let result = event_loop(&mut terminal, &mut app);

    let _ = write!(io::stdout(), "{}", image::kitty_delete_all());
    let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
    drop(_keyboard);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.flush_graphics();
        if !handle_event(app, event::read()?) {
            // Drain queued events so a burst (e.g. wheel ticks) collapses into one frame.
            while event::poll(Duration::ZERO)? {
                handle_event(app, event::read()?);
                if app.should_quit {
                    break;
                }
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Dispatch one input event. Returns `true` when a redraw should happen immediately
/// (a key was handled) so the drain loop knows to continue.
fn handle_event(app: &mut App, event: Event) -> bool {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            app.handle_key(key);
            true
        }
        Event::Mouse(mouse) => {
            app.handle_mouse(mouse);
            false
        }
        _ => false,
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
    fn opening_a_diff_replaces_welcome_and_focuses_editor() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Action::SidebarActivate);
        assert!(app.active_is_diff());
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.tabs.len(), 1, "welcome tab is replaced, not appended");
    }

    #[test]
    fn stepping_changed_files_walks_the_scm_list() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Action::SidebarActivate); // opens a.rs (index 0)
        app.dispatch(Action::NextChangedFile);
        assert_eq!(app.scm.selected, 1);
        app.dispatch(Action::PrevChangedFile);
        assert_eq!(app.scm.selected, 0);
    }

    #[test]
    fn toggle_diff_layout_flips_view() {
        let mut app = app();
        app.sidebar_panel = SidebarPanel::SourceControl;
        app.dispatch(Action::SidebarActivate);
        let before = matches!(
            app.tabs[app.active].kind,
            TabKind::Diff {
                view: ViewMode::Unified,
                ..
            }
        );
        app.dispatch(Action::ToggleDiffLayout);
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
        app.dispatch(Action::ToggleSidebar);
        assert!(!app.sidebar_visible);
        app.dispatch(Action::ToggleFocus);
        assert_eq!(app.focus, Focus::Editor);
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
                buffer: TextBuffer::from_text("foo bar foo"),
                text: "foo bar foo".to_string(),
                highlights: Highlights::default(),
                decos: Vec::new(),
            },
        ));
        app.dispatch(Action::OpenFind);
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
}

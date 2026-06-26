//! Application state and the crossterm event loop.

use std::io;
use std::time::Duration;

use color_eyre::eyre::eyre;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use karet_theme::Theme;
use karet_vcs::FileChange;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::clipboard::Clipboard;
use crate::render::{self, FileView};
use crate::ui;

/// How the diff is laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    /// One column: removals then additions.
    Unified,
    /// Two columns: old on the left, new on the right.
    SideBySide,
}

/// Which Source-Control group a changed file belongs to, mirroring VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree (and untracked files): the working-tree changes.
    Working,
}

/// Which diff pane a selection lives in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    /// The single column of the unified view.
    Unified,
    /// The old (left) column of the side-by-side view.
    Left,
    /// The new (right) column of the side-by-side view.
    Right,
}

/// A caret position in a diff pane: a document line index (independent of scroll)
/// and a character column within that rendered line.
#[derive(Clone, Copy)]
struct Pos {
    line: usize,
    col: usize,
}

/// A normalized selection: the pane plus its `(line, col)` start and end, with
/// `start <= end`.
pub(crate) type SelectionSpan = (Pane, (usize, usize), (usize, usize));

/// An in-progress or completed text selection within one diff pane.
struct Selection {
    pane: Pane,
    anchor: Pos,
    head: Pos,
}

impl Selection {
    /// The selection's endpoints ordered so the first precedes the second.
    fn ordered(&self) -> (Pos, Pos) {
        if (self.anchor.line, self.anchor.col) <= (self.head.line, self.head.col) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// The rendered layout of the last frame, retained so mouse events can hit-test.
#[derive(Default)]
pub(crate) struct Regions {
    /// The file-list panel (left column).
    pub(crate) file_list: Rect,
    /// The list's first visible row, so clicks map through a possible scroll.
    pub(crate) list_offset: usize,
    /// One entry per list display row: `None` for a group header, `Some(i)` for file `i`.
    pub(crate) list_rows: Vec<Option<usize>>,
    /// The diff content area (the whole right column in unified view).
    pub(crate) diff: Rect,
    /// The old/new pane rects when laid out side-by-side.
    pub(crate) sbs: Option<(Rect, Rect)>,
    /// The status bar.
    pub(crate) status: Rect,
}

/// The running viewer state.
pub struct App {
    /// One entry per changed file, the staged group first then the working group.
    pub files: Vec<FileView>,
    /// Index of the focused file.
    pub current: usize,
    /// The current layout.
    pub view: ViewMode,
    /// Vertical scroll offset (in display rows).
    pub scroll: u16,
    /// The active color theme.
    pub theme: Theme,
    /// The active diff text selection, if any.
    selection: Option<Selection>,
    /// Whether a drag-select is currently in progress.
    dragging: bool,
    /// The rendered layout of the last frame, for mouse hit-testing.
    pub(crate) regions: Regions,
    /// The system clipboard (OSC 52).
    clipboard: Clipboard,
}

impl App {
    /// Build the viewer state from the `staged` and `working` change groups, diffing
    /// and highlighting each file.
    pub fn new(staged: Vec<FileChange>, working: Vec<FileChange>, syntax: bool) -> Self {
        let files = staged
            .into_iter()
            .map(|change| FileView::new(change, Section::Staged, syntax))
            .chain(
                working
                    .into_iter()
                    .map(|change| FileView::new(change, Section::Working, syntax)),
            )
            .collect();
        Self {
            files,
            current: 0,
            view: ViewMode::Unified,
            scroll: 0,
            theme: Theme::dark(),
            selection: None,
            dragging: false,
            regions: Regions::default(),
            clipboard: Clipboard::new(),
        }
    }

    /// The number of files in the staged group (the working group is the remainder).
    #[must_use]
    pub fn staged_count(&self) -> usize {
        self.files
            .iter()
            .filter(|fv| fv.section == Section::Staged)
            .count()
    }

    /// Handle one key press. Returns `true` when the app should quit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('c') if ctrl => return true,
            KeyCode::Tab => self.toggle_view(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(20),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(20),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('l') | KeyCode::Char(']') | KeyCode::Right => self.next_file(),
            KeyCode::Char('h') | KeyCode::Char('[') | KeyCode::Left => self.prev_file(),
            _ => {}
        }
        false
    }

    /// Handle one mouse event. The wheel scrolls the diff (or walks the file list
    /// when the pointer is over it); the left button selects files, toggles the
    /// layout, and drag-selects diff text.
    fn handle_mouse(&mut self, mouse: MouseEvent) {
        let p = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollDown if rect_contains(self.regions.file_list, p) => {
                self.next_file();
            }
            MouseEventKind::ScrollUp if rect_contains(self.regions.file_list, p) => {
                self.prev_file();
            }
            MouseEventKind::ScrollDown => self.scroll = self.scroll.saturating_add(3),
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) => self.on_press(p),
            MouseEventKind::Drag(MouseButton::Left) => self.on_drag(p),
            MouseEventKind::Up(MouseButton::Left) => self.on_release(),
            _ => {}
        }
    }

    /// Left-button press: select a file, toggle the layout, or begin a selection.
    fn on_press(&mut self, p: (u16, u16)) {
        if rect_contains(self.regions.file_list, p) {
            if let Some(idx) = self.file_at(p) {
                self.current = idx;
                self.scroll = 0;
                self.selection = None;
            }
        } else if rect_contains(self.regions.status, p) {
            self.toggle_view();
        } else if let Some((pane, rect)) = self.diff_pane_at(p) {
            let pos = pos_in(rect, p, self.scroll);
            self.selection = Some(Selection {
                pane,
                anchor: pos,
                head: pos,
            });
            self.dragging = true;
        }
    }

    /// Left-button drag: extend the active selection, auto-scrolling at the edges.
    fn on_drag(&mut self, p: (u16, u16)) {
        if !self.dragging {
            return;
        }
        let Some(pane) = self.selection.as_ref().map(|s| s.pane) else {
            return;
        };
        let Some(rect) = self.pane_rect(pane) else {
            return;
        };
        // Drag past the top or bottom edge nudges the scroll so the selection grows.
        if p.1 < rect.y {
            self.scroll = self.scroll.saturating_sub(1);
        } else if p.1 >= rect.bottom() {
            self.scroll = self.scroll.saturating_add(1);
        }
        let cx = p.0.clamp(rect.x, rect.right().saturating_sub(1));
        let cy = p.1.clamp(rect.y, rect.bottom().saturating_sub(1));
        let pos = pos_in(rect, (cx, cy), self.scroll);
        if let Some(sel) = self.selection.as_mut() {
            sel.head = pos;
        }
    }

    /// Left-button release: finish a drag and copy the selected text.
    fn on_release(&mut self) {
        if !self.dragging {
            return;
        }
        self.dragging = false;
        self.copy_selection();
    }

    /// Copy the current selection's visible text to the system clipboard.
    fn copy_selection(&self) {
        let Some((pane, start, end)) = self.selection_span() else {
            return;
        };
        let Some(file) = self.files.get(self.current) else {
            return;
        };
        let lines = match pane {
            Pane::Unified => render::unified_lines(file, &self.theme),
            Pane::Left => render::side_by_side_lines(file, &self.theme).0,
            Pane::Right => render::side_by_side_lines(file, &self.theme).1,
        };
        let text = selection_text(&lines, start, end);
        if !text.is_empty() {
            let _ = self.clipboard.set(&text);
        }
    }

    /// The normalized selection as `(pane, start, end)` in `(line, col)` document
    /// coords with `start <= end`, or `None` when nothing is selected.
    pub(crate) fn selection_span(&self) -> Option<SelectionSpan> {
        let sel = self.selection.as_ref()?;
        let (s, e) = sel.ordered();
        Some((sel.pane, (s.line, s.col), (e.line, e.col)))
    }

    /// The file index under a point in the file-list panel, if any (`None` over a
    /// group header or empty row).
    fn file_at(&self, (_, row): (u16, u16)) -> Option<usize> {
        let rel = usize::from(row.checked_sub(self.regions.file_list.y)?);
        let idx = rel + self.regions.list_offset;
        self.regions.list_rows.get(idx).copied().flatten()
    }

    /// The diff pane (and its rect) under a point, if the point is over diff content.
    fn diff_pane_at(&self, p: (u16, u16)) -> Option<(Pane, Rect)> {
        match self.view {
            ViewMode::Unified => {
                rect_contains(self.regions.diff, p).then_some((Pane::Unified, self.regions.diff))
            }
            ViewMode::SideBySide => {
                let (left, right) = self.regions.sbs?;
                if rect_contains(left, p) {
                    Some((Pane::Left, left))
                } else if rect_contains(right, p) {
                    Some((Pane::Right, right))
                } else {
                    None
                }
            }
        }
    }

    /// The rect of `pane` in the current layout, if it is on screen.
    fn pane_rect(&self, pane: Pane) -> Option<Rect> {
        match pane {
            Pane::Unified => Some(self.regions.diff),
            Pane::Left => self.regions.sbs.map(|(l, _)| l),
            Pane::Right => self.regions.sbs.map(|(_, r)| r),
        }
    }

    /// Switch between the unified and side-by-side layouts, dropping any selection
    /// (its coordinates are pane-specific).
    fn toggle_view(&mut self) {
        self.view = match self.view {
            ViewMode::Unified => ViewMode::SideBySide,
            ViewMode::SideBySide => ViewMode::Unified,
        };
        self.selection = None;
    }

    fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.current = (self.current + 1) % self.files.len();
            self.scroll = 0;
            self.selection = None;
        }
    }

    fn prev_file(&mut self) {
        let len = self.files.len();
        if len != 0 {
            self.current = (self.current + len - 1) % len;
            self.scroll = 0;
            self.selection = None;
        }
    }
}

/// Whether the screen point `(x, y)` lies inside `r`.
fn rect_contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Map a screen point inside `rect` to a document position, accounting for `scroll`.
fn pos_in(rect: Rect, (x, y): (u16, u16), scroll: u16) -> Pos {
    Pos {
        line: usize::from(scroll) + usize::from(y.saturating_sub(rect.y)),
        col: usize::from(x.saturating_sub(rect.x)),
    }
}

/// The visible text of `lines` between document positions `start` and `end`
/// (inclusive of `start`, exclusive of `end`), joined with newlines. Columns are
/// character offsets and clamp to each line.
fn selection_text(lines: &[Line<'static>], start: (usize, usize), end: (usize, usize)) -> String {
    let mut out = String::new();
    for line_idx in start.0..=end.0 {
        let Some(line) = lines.get(line_idx) else {
            break;
        };
        let chars: Vec<char> = line.spans.iter().flat_map(|s| s.content.chars()).collect();
        let len = chars.len();
        let from = if line_idx == start.0 {
            start.1.min(len)
        } else {
            0
        };
        let to = if line_idx == end.0 {
            end.1.min(len)
        } else {
            len
        };
        if from < to {
            out.extend(chars[from..to].iter());
        }
        if line_idx != end.0 {
            out.push('\n');
        }
    }
    out
}

/// Pops the kitty keyboard-enhancement flags on drop, so they are cleared even if
/// the event loop panics (ratatui's panic hook restores the rest of the terminal).
struct KeyboardEnhancementGuard;

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Run the viewer: require the kitty keyboard protocol, set up the terminal (mouse
/// capture + keyboard enhancement), loop until quit, then restore it.
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

    let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
    drop(_keyboard);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        // Block for the next event, then drain any already-queued events before the next
        // redraw so a burst of mouse-wheel ticks collapses into one frame (no scroll lag).
        if handle_event(app, event::read()?) {
            return Ok(());
        }
        while event::poll(Duration::ZERO)? {
            if handle_event(app, event::read()?) {
                return Ok(());
            }
        }
    }
}

/// Dispatch one input event, returning `true` when the app should quit.
fn handle_event(app: &mut App, event: Event) -> bool {
    match event {
        Event::Key(key) => key.kind == KeyEventKind::Press && app.handle_key(key),
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
    use karet_vcs::StatusKind;
    use std::path::PathBuf;

    fn change(path: &str, status: StatusKind, new: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status,
            is_binary: false,
            old: String::new(),
            new: new.to_string(),
        }
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A three-file app (1 staged, 2 working) with the file list laid out at the
    /// origin and the diff to its right.
    fn three_file_app() -> App {
        let staged = vec![change("a.txt", StatusKind::Modified, "a\n")];
        let working = vec![
            change("b.txt", StatusKind::Modified, "b\n"),
            change("c.txt", StatusKind::Modified, "c\n"),
        ];
        let mut app = App::new(staged, working, false);
        app.regions.file_list = Rect::new(0, 0, 20, 20);
        app.regions.diff = Rect::new(20, 0, 60, 20);
        // Display rows: [STAGED hdr, a, CHANGES hdr, b, c].
        app.regions.list_rows = vec![None, Some(0), None, Some(1), Some(2)];
        app.regions.list_offset = 0;
        app
    }

    #[test]
    fn click_in_file_list_selects_that_file() {
        let mut app = three_file_app();
        // Row 4 is file c (index 2); a group-header row selects nothing.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 4));
        assert_eq!(app.current, 2);
        app.scroll = 9;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 0)); // header row
        assert_eq!(app.current, 2, "clicking a header keeps the selection");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1)); // file a
        assert_eq!(app.current, 0);
        assert_eq!(app.scroll, 0, "selecting a file resets the scroll");
    }

    #[test]
    fn file_list_click_respects_scroll_offset() {
        let mut app = three_file_app();
        app.regions.list_offset = 2; // first two display rows scrolled off
        // Screen row 1 maps to list row 1 + offset 2 = 3 -> Some(1).
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 1));
        assert_eq!(app.current, 1);
    }

    #[test]
    fn wheel_navigates_the_list_but_scrolls_the_diff() {
        let mut app = three_file_app();
        // Over the file list, the wheel walks files.
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 5, 3));
        assert_eq!(app.current, 1);
        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 5, 3));
        assert_eq!(app.current, 0);
        // Over the diff, the wheel scrolls.
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 3));
        assert_eq!(app.scroll, 3);
    }

    #[test]
    fn clicking_the_status_bar_toggles_the_layout() {
        let mut app = three_file_app();
        app.regions.status = Rect::new(0, 20, 80, 1);
        assert!(matches!(app.view, ViewMode::Unified));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 20));
        assert!(matches!(app.view, ViewMode::SideBySide));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 20));
        assert!(matches!(app.view, ViewMode::Unified));
    }

    #[test]
    fn dragging_in_the_diff_builds_a_selection() {
        let mut app = three_file_app();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 22, 1));
        assert!(app.dragging);
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 26, 1));
        let (pane, start, end) = app.selection_span().expect("a selection exists");
        assert!(matches!(pane, Pane::Unified));
        // diff rect is at x=20, y=0; scroll is 0.
        assert_eq!(start, (1, 2));
        assert_eq!(end, (1, 6));
    }

    #[test]
    fn releasing_ends_the_drag() {
        let mut app = three_file_app();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 22, 1));
        assert!(app.dragging);
        // Release without moving: the selection is empty, so nothing is copied.
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 22, 1));
        assert!(!app.dragging);
    }

    #[test]
    fn selection_text_slices_across_lines() {
        use ratatui::text::Span;
        let lines = vec![
            Line::from(vec![Span::raw("abc"), Span::raw("def")]),
            Line::from(vec![Span::raw("ghijkl")]),
            Line::from(vec![Span::raw("mnopqr")]),
        ];
        // Multi-line: from line 0 col 2 through line 2 col 3.
        assert_eq!(selection_text(&lines, (0, 2), (2, 3)), "cdef\nghijkl\nmno");
        // Single line.
        assert_eq!(selection_text(&lines, (1, 1), (1, 4)), "hij");
        // Out-of-range columns clamp to the line.
        assert_eq!(selection_text(&lines, (0, 4), (0, 100)), "ef");
    }

    #[test]
    fn groups_files_with_untracked_in_the_working_section() {
        let staged = vec![change("a.txt", StatusKind::Modified, "a\n")];
        let working = vec![
            change("b.txt", StatusKind::Modified, "b\n"),
            change("new.txt", StatusKind::Untracked, "n\n"),
        ];
        let app = App::new(staged, working, false);

        assert_eq!(app.files.len(), 3);
        // Staged files come first, then the working group.
        assert_eq!(app.staged_count(), 1);
        assert_eq!(app.files[0].section, Section::Staged);
        assert_eq!(app.files[1].section, Section::Working);
        // The untracked file lands in the working ("Changes") group, like VS Code.
        assert_eq!(app.files[2].section, Section::Working);
        assert_eq!(app.files[2].change.status, StatusKind::Untracked);
    }
}

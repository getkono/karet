//! Application state and the crossterm event loop.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use karet_theme::Theme;
use karet_vcs::FileChange;

use crate::render::FileView;
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree (and untracked files): the working-tree changes.
    Working,
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
            KeyCode::Tab => {
                self.view = match self.view {
                    ViewMode::Unified => ViewMode::SideBySide,
                    ViewMode::SideBySide => ViewMode::Unified,
                };
            }
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

    fn next_file(&mut self) {
        if !self.files.is_empty() {
            self.current = (self.current + 1) % self.files.len();
            self.scroll = 0;
        }
    }

    fn prev_file(&mut self) {
        let len = self.files.len();
        if len != 0 {
            self.current = (self.current + len - 1) % len;
            self.scroll = 0;
        }
    }
}

/// Run the viewer: set up the terminal, loop until quit, then restore it.
pub fn run(mut app: App) -> color_eyre::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> color_eyre::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && app.handle_key(key)
        {
            return Ok(());
        }
    }
}

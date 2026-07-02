//! A minimal read-only pager over [`karet_editor::Editor::read_only`], validating
//! the read-only rendering mode in isolation (no file-view dispatch).
//!
//! Run against a file, or with no argument for built-in sample text:
//!
//! ```console
//! cargo run -p karet-editor --example read_only -- src/lib.rs
//! ```
//!
//! Keys: `j`/`k` or ↓/↑ scroll a line, `Space`/`b` page, `g`/`G` top/bottom,
//! `q`/`Esc` quit.

use std::io;

use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use karet_editor::Editor;
use karet_editor::EditorState;
use karet_text::TextBuffer;
use karet_theme::Theme;
use ratatui::DefaultTerminal;

fn main() -> io::Result<()> {
    let text = match std::env::args_os().nth(1) {
        Some(path) => std::fs::read_to_string(path)?,
        None => (1..=200).map(|i| format!("line {i}\n")).collect(),
    };
    let buffer = TextBuffer::from_text(&text);
    let theme = Theme::dark();

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &buffer, &theme);
    ratatui::restore();
    result
}

/// The draw/input loop: render `buffer` read-only and page through it.
fn run(terminal: &mut DefaultTerminal, buffer: &TextBuffer, theme: &Theme) -> io::Result<()> {
    let mut state = EditorState::new();
    let last_line = (buffer.line_count().max(1) - 1) as u32;
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_stateful_widget(
                Editor::new(buffer).theme(theme).read_only(true),
                area,
                &mut state,
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('j') | KeyCode::Down => {
                state.scroll_line = state.scroll_line.saturating_add(1);
            },
            KeyCode::Char('k') | KeyCode::Up => {
                state.scroll_line = state.scroll_line.saturating_sub(1);
            },
            KeyCode::Char(' ') | KeyCode::PageDown => state.scroll_page_down(),
            KeyCode::Char('b') | KeyCode::PageUp => state.scroll_page_up(),
            KeyCode::Char('g') | KeyCode::Home => state.scroll_line = 0,
            KeyCode::Char('G') | KeyCode::End => state.center_on(last_line),
            _ => {},
        }
    }
}

//! A tiny standalone pager built on `karet-fileview` — the "external consumer" of
//! this crate, mirroring what a directory browser like tree-tui does: prepare a
//! file once, then render it read-only and page through it.
//!
//! Run it with a path to any file:
//!
//! ```console
//! cargo run -p karet-fileview --example render_file --features all-languages -- src/lib.rs
//! ```
//!
//! (The `all-languages` feature — enabled automatically here via a dev-dependency —
//! compiles in the tree-sitter grammars so code highlights.)
//!
//! Keys: `j`/`k` or ↓/↑ scroll a line, `Space`/`b` page, `g`/`G` top/bottom,
//! `q`/`Esc` quit.

use std::io;
use std::path::Path;

use crossterm::event;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use karet_fileview::FileDoc;
use karet_fileview::FileView;
use karet_fileview::FileViewState;
use karet_fileview::Limits;
use karet_fileview::flush_kitty_image;
use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::detect_protocol;
use ratatui::DefaultTerminal;

fn main() -> io::Result<()> {
    let Some(arg) = std::env::args_os().nth(1) else {
        eprintln!("usage: render_file <path>");
        return Ok(());
    };
    let path = Path::new(&arg);
    let bytes = std::fs::read(path)?;
    let len = bytes.len() as u64;
    // Full-reader budgets: open files up to 4 MiB, highlight up to 20k lines.
    let limits = Limits::new(4 * 1024 * 1024, 20_000);
    // The expensive step, run exactly once.
    let doc = FileDoc::prepare(path, &bytes, len, &limits);
    let protocol = detect_protocol();

    let mut terminal = ratatui::init();
    // Always restore the terminal, even if the loop returns an error.
    let result = run(&mut terminal, &doc, protocol);
    ratatui::restore();
    result
}

/// The draw/input loop: render the prepared doc and page through it until quit.
fn run(
    terminal: &mut DefaultTerminal,
    doc: &FileDoc,
    protocol: GraphicsProtocol,
) -> io::Result<()> {
    let mut state = FileViewState::new();
    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            frame.render_stateful_widget(FileView::new(doc).graphics(protocol), area, &mut state);
        })?;
        // The Kitty graphics path transmits pixels after the frame is drawn.
        if protocol == GraphicsProtocol::Kitty {
            flush_kitty_image(doc, &state, &mut io::stdout())?;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
            KeyCode::Char('j') | KeyCode::Down => state.scroll_down(1),
            KeyCode::Char('k') | KeyCode::Up => state.scroll_up(1),
            KeyCode::Char(' ') | KeyCode::PageDown => state.page_down(),
            KeyCode::Char('b') | KeyCode::PageUp => state.page_up(),
            KeyCode::Char('g') | KeyCode::Home => state.scroll_to_top(),
            // Jump to the end; the scroll clamps to the document at render.
            KeyCode::Char('G') | KeyCode::End => state.scroll_down(u32::MAX),
            _ => {},
        }
    }
}

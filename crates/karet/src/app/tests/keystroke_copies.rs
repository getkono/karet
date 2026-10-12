//! Typing must not cost a whole-document copy (issue #394).
//!
//! A code tab keeps its contiguous text as a lazy [`karet_text::TextSnapshot`].
//! These tests drive the keystroke, Ctrl+D, and render paths and then check the
//! snapshot was never materialized: any of them falling back to
//! `buffer.text()`-style copying would build it.

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::support::*;
use crate::app::*;

/// A focused Markdown editor (doc 9) over `text`, wired to a recording backend.
fn markdown_app(text: &str) -> App {
    let mut app = app();
    app.backend = Some(Arc::new(RecordingBackend::new()));
    app.push_tab(text_tab("notes.md", text));
    app.focus = Focus::Editor;
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DocumentId(9));
    }
    app
}

/// Whether the active tab's contiguous text has been built.
fn materialized(app: &App) -> bool {
    match &app.tabs[app.active].kind {
        TabKind::Code { text, .. } => text.is_materialized(),
        _ => panic!("expected the active tab to be a code tab"),
    }
}

fn draw(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, app))
        .expect("draw shell");
}

#[test]
fn typing_and_rendering_never_copy_the_document() {
    let mut app = markdown_app("# Notes\n\nfoo and foo\n");
    draw(&mut app);
    for c in "abc".chars() {
        app.dispatch(Command::InsertChar(c));
        draw(&mut app);
    }
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 1)]);
    app.add_cursor_next_occurrence(); // select the word
    app.add_cursor_next_occurrence(); // add the next "foo"
    draw(&mut app);
    assert!(app.tabs[app.active].editor.has_multiple_cursors());
    assert!(
        !materialized(&app),
        "a keystroke, Ctrl+D, or a frame copied the whole document"
    );
    // The lazy text still tracks every edit for the readers that need it.
    assert_eq!(code_tab_text(&app), "abc# Notes\n\nfoo and foo\n");
}

#[test]
fn a_markdown_table_still_reaches_the_renderer() {
    let mut app = markdown_app("| a | b |\n|---|---|\n| 1 | 2 |\n");
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 9)]);
    app.dispatch(Command::InsertChar('\n'));
    draw(&mut app);
    // A pipe gates the table scan in, which reads the shared snapshot.
    assert!(materialized(&app));
    assert_eq!(
        app.tabs[app.active]
            .markdown_table_lines
            .as_ref()
            .map(|(_, ranges)| ranges.clone()),
        Some(vec![0..=2])
    );
}

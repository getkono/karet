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
    app.dispatch(Command::AddCursorNextOccurrence); // select the word
    app.dispatch(Command::AddCursorNextOccurrence); // add the next "foo"
    draw(&mut app);
    assert!(app.tabs[app.active].editor.has_multiple_cursors());
    assert!(
        !materialized(&app),
        "a keystroke, Ctrl+D, or a frame copied the whole document"
    );
    // The lazy text still tracks every edit for the readers that need it.
    assert_eq!(code_tab_text(&app), "abc# Notes\n\nfoo and foo\n");
}

/// Where the active tab's find (`global == false`) or global-search match
/// highlights start, as `(line, col)`.
fn match_starts(app: &App, global: bool) -> Vec<(u32, u32)> {
    match &app.tabs[app.active].kind {
        TabKind::Code {
            decos,
            search_decos,
            ..
        } => if global { search_decos } else { decos }
            .iter()
            .map(|d| (d.range.start.line, d.range.start.col))
            .collect(),
        _ => panic!("expected the active tab to be a code tab"),
    }
}

/// An open find bar and a live global search re-run on every edit (#420); both
/// read the rope's chunks, so neither copies the document.
#[test]
fn typing_with_find_and_search_active_never_copies_the_document() {
    let mut app = markdown_app("# Notes\n\nfoo and foo\n");
    app.dispatch(Command::OpenFind);
    for c in "FOO".chars() {
        app.find_input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.search.query = "foo".to_owned();
    app.search.hits = vec![karet_session::SearchHit {
        path: PathBuf::from("notes.md"),
        matches: Vec::new(),
    }];
    app.refresh_search_decorations();
    app.focus = Focus::Editor;
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(0, 0)]);
    app.dispatch(Command::InsertChar('x'));
    app.refresh_find();
    app.refresh_search_decorations();
    draw(&mut app);
    assert_eq!(match_starts(&app, false), vec![(2, 0), (2, 8)]);
    assert_eq!(match_starts(&app, true), vec![(2, 0), (2, 8)]);
    assert!(
        !materialized(&app),
        "find or search highlighting copied the whole document"
    );
}

/// The highlight worker's table lines ride the document snapshot (#433), so a
/// Markdown file with a table costs no copy per keystroke either.
#[test]
fn a_markdown_table_still_reaches_the_renderer() {
    let mut app = markdown_app("| a | b |\n|---|---|\n| 1 | 2 |\n");
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(2, 9)]);
    app.dispatch(Command::InsertChar('\n'));
    draw(&mut app);
    let buffer = match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => (**buffer).clone(),
        _ => panic!("expected the active tab to be a code tab"),
    };
    app.on_snapshot(
        DocumentId(9),
        &DocSnapshot {
            version: buffer.version(),
            buffer,
            highlights: Arc::default(),
            semantic_blocks: Arc::default(),
            folds: Arc::default(),
            decorations: Arc::default(),
            syntax_error_lines: Arc::default(),
            table_lines: Arc::new(vec![0..=2]),
            language: Some("Markdown"),
            dirty: true,
            cursor: None,
        },
    );
    draw(&mut app);
    assert_eq!(*app.tabs[app.active].markdown_table_lines, [0..=2]);
    assert!(
        !materialized(&app),
        "a Markdown table made a keystroke or a frame copy the whole document"
    );
}

/// A split view of a Markdown tab keeps the worker's table lines it was split
/// from, rather than wrapping the table until the next snapshot arrives.
#[test]
fn a_split_view_keeps_the_markdown_table_lines() {
    let mut app = markdown_app("| a | b |\n|---|---|\n| 1 | 2 |\n");
    app.tabs[app.active].markdown_table_lines = Arc::new(vec![0..=2]);
    let original = app.tabs[app.active].view;
    app.split_focused(karet_widgets::SplitDir::Right);
    let split = &app.tabs[app.active];
    assert_ne!(split.view, original, "the split should focus the new view");
    assert_eq!(*split.markdown_table_lines, [0..=2]);
}

/// Every view's buffer of doc 9, across every pane.
fn doc_buffers(app: &App) -> Vec<Arc<karet_text::TextBuffer>> {
    app.all_tabs()
        .filter_map(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(DocumentId(9)),
                buffer,
                ..
            } => Some(Arc::clone(buffer)),
            _ => None,
        })
        .collect()
}

/// Whether every view of doc 9 holds one and the same buffer.
fn one_buffer(app: &App) -> bool {
    let buffers = doc_buffers(app);
    buffers.len() == 2 && Arc::ptr_eq(&buffers[0], &buffers[1])
}

/// The snapshot echoing the focused view's buffer as it now stands.
fn echo(app: &App) -> DocSnapshot {
    let buffer = match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => buffer.content_snapshot(),
        _ => karet_text::TextBuffer::default(),
    };
    DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::default(),
        semantic_blocks: Arc::default(),
        folds: Arc::default(),
        decorations: Arc::default(),
        syntax_error_lines: Arc::default(),
        table_lines: Arc::default(),
        language: Some("Markdown"),
        dirty: true,
        cursor: None,
    }
}

/// A split view holds the document's buffer itself, not a copy of it (#397):
/// splitting shares it, an edit in one view copies it for that view alone, and
/// the next snapshot hands every view the same buffer again.
#[test]
fn split_views_share_one_buffer() {
    let mut app = markdown_app("abc\n");
    app.split_focused(karet_widgets::SplitDir::Right);
    assert!(one_buffer(&app), "the split copied the buffer");

    app.dispatch(Command::InsertChar('x'));
    let buffers = doc_buffers(&app);
    assert_eq!(buffers.len(), 2);
    assert!(
        !Arc::ptr_eq(&buffers[0], &buffers[1]),
        "an edit in one view must not reach into the other's buffer"
    );
    let texts: Vec<String> = buffers.iter().map(|b| b.text()).collect();
    assert!(texts.contains(&"xabc\n".to_owned()), "{texts:?}");
    assert!(texts.contains(&"abc\n".to_owned()), "{texts:?}");

    let snap = echo(&app);
    app.on_snapshot(DocumentId(9), &snap);
    assert!(one_buffer(&app), "the snapshot left the views apart");
    assert_eq!(doc_buffers(&app)[0].text(), "xabc\n");

    // A republish of the same version reunites views that already caught up.
    let again = echo(&app);
    app.on_snapshot(DocumentId(9), &again);
    assert!(one_buffer(&app));
}

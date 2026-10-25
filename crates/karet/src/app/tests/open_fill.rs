//! A text file opens as a reserved, empty code tab that the session's first
//! snapshot fills: the caret lands where the open asked, caches drawn from the
//! placeholder are dropped, and nothing is edited against the placeholder.

use karet_core::Change;
use karet_core::Range;
use karet_core::TextEdit;

use super::support::*;
use crate::app::*;

const DOC: DocumentId = DocumentId(9);

/// An app with `name` (holding `text`) open as a reserved tab already bound to
/// [`DOC`], waiting for its first snapshot, over a recording backend.
fn reserved(name: &str, text: &str) -> (App, Arc<RecordingBackend>, PathBuf) {
    let dir = test_dir(name);
    write_file(&dir, "notes.md", text.as_bytes());
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    app.open_path(&dir.join("notes.md"));
    let idx = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[idx].kind {
        *doc = Some(DOC);
    }
    (app, backend, dir)
}

/// `text` as the session holds it after restoring an undo history: the same
/// content, at a version past the placeholder's 0.
fn restored(text: &str) -> TextBuffer {
    let mut buffer = TextBuffer::from_text("");
    let change = Change::new(
        0,
        vec![TextEdit {
            range: Range::default(),
            new_text: text.to_string(),
        }],
    );
    let _ = buffer.apply_simple(&change);
    buffer
}

fn code_text(app: &App) -> String {
    match &app.tabs[app.active].kind {
        TabKind::Code { text, .. } => text.to_string(),
        _ => String::new(),
    }
}

#[test]
fn the_first_fill_places_the_caret_where_the_open_asked_not_at_the_end() {
    let (mut app, _backend, dir) = reserved("fill-caret", "one\ntwo\nthree\n");
    let path = dir.join("notes.md");
    app.focus_by_file_line(&path, LineCol::new(1, 2), true);

    let filled = restored("one\ntwo\nthree\n");
    assert!(filled.version() > 0, "the fill arrives at a later version");
    app.on_snapshot(DOC, &buffer_snapshot(filled));

    assert_eq!(code_text(&app), "one\ntwo\nthree\n");
    assert_eq!(app.tabs[app.active].fill_caret, None);
    // Carried through the insertion of the whole text, the caret would sit at
    // its end; the fill is a load, so it goes where the jump asked.
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plain_open_fills_with_the_caret_at_the_top() {
    let (mut app, _backend, dir) = reserved("fill-top", "one\ntwo\n");
    app.on_snapshot(DOC, &buffer_snapshot(restored("one\ntwo\n")));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_first_snapshot_places_the_caret() {
    let (mut app, _backend, dir) = reserved("fill-once", "one\ntwo\n");
    app.on_snapshot(DOC, &buffer_snapshot(TextBuffer::from_text("one\ntwo\n")));
    let tab = app.active;
    let buffer = match &app.tabs[tab].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => Arc::default(),
    };
    app.tabs[tab].editor.goto(&buffer, LineCol::new(1, 1));
    // A later republish (a highlight pass) leaves the user's caret alone.
    app.on_snapshot(DOC, &buffer_snapshot(TextBuffer::from_text("one\ntwo\n")));
    assert_eq!(app.tabs[tab].editor.cursor(), LineCol::new(1, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edit_before_the_fill_is_refused_rather_than_sent_against_the_placeholder() {
    let (mut app, backend, dir) = reserved("fill-edit", "ab");
    app.focus = Focus::Editor;
    app.dispatch(Command::InsertChar('x'));

    assert_eq!(code_text(&app), "", "the placeholder is not edited");
    let sent_change = backend.sent.lock().is_ok_and(|sent| {
        sent.iter()
            .any(|(_, command)| matches!(command, SessionCommand::ApplyChange { .. }))
    });
    assert!(
        !sent_change,
        "no change built on the placeholder reaches the session"
    );

    app.on_snapshot(DOC, &buffer_snapshot(TextBuffer::from_text("ab")));
    app.dispatch(Command::InsertChar('x'));
    assert_eq!(code_text(&app), "xab", "once filled, typing lands");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_fill_redraws_a_markdown_preview_drawn_from_the_placeholder() {
    let (mut app, _backend, dir) = reserved("fill-preview", "# Loaded heading\n");
    app.focus = Focus::Editor;
    app.main_rect = Rect::new(0, 0, 100, 20);
    app.dispatch(Command::MarkdownPreviewSide);
    // Drawn once from the empty placeholder, at version 0.
    let before = screen(&mut app, 100, 20).join("\n");
    assert!(!before.contains("Loaded heading"), "{before}");

    // The loaded text arrives at that same version 0, so only dropping what
    // was drawn from the placeholder lets the preview catch up.
    app.on_snapshot(
        DOC,
        &buffer_snapshot(TextBuffer::from_text("# Loaded heading\n")),
    );
    let after = screen(&mut app, 100, 20).join("\n");
    assert!(after.contains("Loaded heading"), "{after}");
    let _ = std::fs::remove_dir_all(&dir);
}

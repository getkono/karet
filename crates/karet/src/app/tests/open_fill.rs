//! A text file opens as a reserved, empty code tab that the session's first
//! snapshot fills: the caret lands where the open asked, caches drawn from the
//! placeholder are dropped, and nothing is edited against the placeholder.

use karet_core::Change;
use karet_core::CursorState;
use karet_core::Range;
use karet_core::Selection;
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
fn the_first_fill_puts_back_a_held_view_and_a_jump_replaces_it() {
    let text = "one\ntwo\nthree\nfour\n";
    let held = || crate::tab::FillView {
        cursors: CursorState {
            selections: vec![
                Selection::caret(LineCol::new(1, 1)),
                Selection::caret(LineCol::new(3, 2)),
            ],
            primary: 1,
        },
        scroll_line: 9,
        scroll_col: 1,
    };
    let (mut app, _backend, dir) = reserved("fill-view", text);
    app.tabs[app.active].fill_view = Some(held());
    app.on_snapshot(DOC, &buffer_snapshot(restored(text)));
    let tab = &app.tabs[app.active];
    assert!(tab.fill_view.is_none());
    assert_eq!(
        tab.editor.cursors(),
        &held().cursors,
        "both carets come back"
    );
    assert_eq!(
        (tab.editor.scroll_line(), tab.editor.scroll_col()),
        (4, 1),
        "the scroll is clamped to the loaded text's last line"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // A jump while the tab waits is newer than the held view.
    let (mut app, _backend, dir) = reserved("fill-view-jump", text);
    app.tabs[app.active].fill_view = Some(held());
    app.focus_by_file_line(&dir.join("notes.md"), LineCol::new(2, 0), true);
    app.on_snapshot(DOC, &buffer_snapshot(restored(text)));
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 0));
    assert_eq!(app.tabs[app.active].editor.cursors().selections.len(), 1);
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

#[test]
fn a_tab_waiting_past_the_reveal_delay_shows_a_muted_loading_placeholder() {
    let (mut app, _backend, dir) = reserved("fill-placeholder", "loaded body\n");
    let tab = app.active;
    let since = app.tabs[tab].fill_since;
    assert!(since.is_some(), "a reserved tab anchors its wait");
    // The wait sends no event, so the loop has to schedule the reveal itself.
    assert!(since.is_some_and(|since| app.pendings().contains(&since)));

    // A fast load never flashes the placeholder.
    let fresh = screen(&mut app, 80, 10).join("\n");
    assert!(!fresh.contains("Loading file…"), "{fresh}");

    app.tabs[tab].fill_since = Some(Pending::revealed());
    let waiting = screen(&mut app, 80, 10).join("\n");
    assert!(waiting.contains("Loading file…"), "{waiting}");

    app.on_snapshot(
        DOC,
        &buffer_snapshot(TextBuffer::from_text("loaded body\n")),
    );
    assert_eq!(app.tabs[tab].fill_since, None);
    assert!(app.pendings().is_empty(), "the filled tab waits on nothing");
    let filled = screen(&mut app, 80, 10).join("\n");
    assert!(!filled.contains("Loading file…"), "{filled}");
    assert!(filled.contains("loaded body"), "{filled}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_first_fill_recomputes_conflict_markers_drawn_from_the_placeholder() {
    let dir = test_dir("fill-conflict");
    let text = "<<<<<<< HEAD\nfn current() {}\n=======\nfn incoming() {}\n>>>>>>> incoming\n";
    write_file(&dir, "a.rs", text.as_bytes());
    let backend = Arc::new(RecordingBackend::new());
    let conflicted = change("a.rs", karet_vcs::StatusKind::Conflicted);
    let mut app = App::new(dir.clone(), Vec::new(), vec![conflicted], false);
    app.backend = Some(backend.clone());
    app.open_path(&dir.join("a.rs"));
    let tab = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[tab].kind {
        *doc = Some(DOC);
    }
    app.sidebar_panel = SidebarPanel::SourceControl;
    app.preview_selected_diff();
    assert!(app.tabs[tab].merge_conflict.is_some());

    // Drawn once from the empty placeholder at version 0: no markers to find.
    let _ = screen(&mut app, 120, 12);
    let markers = |app: &App| {
        app.tabs[tab]
            .conflict_decorations
            .as_ref()
            .map(|(version, decorations)| (*version, decorations.len()))
    };
    assert_eq!(markers(&app), Some((0, 0)));

    // The loaded text arrives at that same version 0, so only dropping what
    // was drawn from the placeholder lets the markers appear.
    let loaded = TextBuffer::from_text(text);
    assert_eq!(loaded.version(), 0);
    app.on_snapshot(DOC, &buffer_snapshot(loaded));
    let _ = screen(&mut app, 120, 12);
    assert!(
        markers(&app).is_some_and(|(version, count)| version == 0 && count > 0),
        "{:?}",
        markers(&app)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_open_stops_the_wait_and_shows_why() {
    let dir = test_dir("fill-failed");
    write_file(&dir, "notes.md", b"text\n");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    app.open_path(&dir.join("notes.md"));
    let tab = app.active;
    let open = backend.sent.lock().ok().and_then(|sent| {
        sent.iter().find_map(|(id, command)| {
            matches!(command, SessionCommand::OpenDocument { .. }).then_some(*id)
        })
    });
    assert!(open.is_some(), "the tab asked the session for its text");
    app.tabs[tab].fill_since = Some(Pending::revealed());

    // A warning tagged with the open (a bad `.editorconfig`) is not a failure.
    let warn = |severity| SessionEvent::Notification {
        severity,
        kind: NotificationKind::Io,
        message: "could not open notes.md: denied".to_string(),
    };
    app.on_backend_event(open, warn(Severity::Warning));
    assert!(app.tabs[tab].fill_error.is_none());
    assert!(!app.pending_open.is_empty());

    app.on_backend_event(open, warn(Severity::Error));
    assert!(app.pending_open.is_empty(), "no `Opened` will follow");
    assert_eq!(app.tabs[tab].fill_since, None);
    assert!(app.pendings().is_empty(), "nothing is left to reveal");
    let shown = screen(&mut app, 80, 10).join("\n");
    assert!(!shown.contains("Loading file…"), "{shown}");
    assert!(shown.contains("could not open notes.md: denied"), "{shown}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_split_of_a_waiting_tab_keeps_waiting_and_fills_with_its_held_view() {
    let text = "one\ntwo\nthree\nfour\n";
    let (mut app, _backend, dir) = reserved("fill-split", text);
    let held = crate::tab::FillView {
        cursors: CursorState::single(Selection::caret(LineCol::new(2, 1))),
        scroll_line: 1,
        scroll_col: 0,
    };
    app.tabs[app.active].fill_view = Some(held.clone());
    let since = Pending::revealed();
    app.tabs[app.active].fill_since = Some(since);
    let source = app.tabs[app.active].view;

    app.split_focused(SplitDir::Right);
    let copy = &app.tabs[app.active];
    assert_ne!(copy.view, source, "the split is a second view");
    assert_eq!(copy.fill_caret, Some(LineCol::default()));
    assert_eq!(copy.fill_since, Some(since));
    assert!(copy.fill_view.is_some());
    let waiting = screen(&mut app, 80, 10).join("\n");
    assert!(waiting.contains("Loading file…"), "{waiting}");

    // Typing into the copy is refused, not built against the placeholder.
    app.focus = Focus::Editor;
    app.dispatch(Command::InsertChar('x'));
    assert_eq!(code_text(&app), "", "the placeholder is not edited");

    app.on_snapshot(DOC, &buffer_snapshot(restored(text)));
    let copy = &app.tabs[app.active];
    assert_eq!(code_text(&app), text);
    assert_eq!((copy.fill_caret, copy.fill_since), (None, None));
    assert_eq!(
        copy.editor.cursors(),
        &held.cursors,
        "the held caret comes back"
    );
    assert_eq!(copy.editor.scroll_line(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_split_carries_the_failed_open_it_was_made_from() {
    let (mut app, _backend, dir) = reserved("fill-split-error", "text\n");
    app.tabs[app.active].fill_since = None;
    app.tabs[app.active].fill_error = Some("could not open notes.md: denied".to_string());

    app.split_focused(SplitDir::Right);
    assert_eq!(
        app.tabs[app.active].fill_error.as_deref(),
        Some("could not open notes.md: denied")
    );
    let shown = screen(&mut app, 160, 10).join("\n");
    assert_eq!(
        shown.matches("could not open notes.md: denied").count(),
        2,
        "both views show why: {shown}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ids of every `OpenDocument` sent so far.
fn opens(backend: &RecordingBackend) -> Vec<RequestId> {
    backend.sent.lock().map_or_else(
        |_| Vec::new(),
        |sent| {
            sent.iter()
                .filter(|(_, command)| matches!(command, SessionCommand::OpenDocument { .. }))
                .map(|(id, _)| *id)
                .collect()
        },
    )
}

fn doc_of(tab: &Tab) -> Option<DocumentId> {
    match &tab.kind {
        TabKind::Code { doc, .. } => *doc,
        _ => None,
    }
}

#[test]
fn a_split_made_before_the_open_is_answered_asks_for_its_own_document() {
    let dir = test_dir("fill-split-unbound");
    write_file(&dir, "notes.md", b"one\ntwo\n");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    app.open_path(&dir.join("notes.md"));
    assert_eq!(opens(&backend).len(), 1, "the source asked for its text");

    app.split_focused(SplitDir::Right);
    let copy = app.tabs[app.active].view;
    let both = opens(&backend);
    assert_eq!(both.len(), 2, "the copy asked for its own document");

    let opened = SessionEvent::Opened {
        doc: DOC,
        version: 0,
    };
    app.on_backend_event(both.first().copied(), opened.clone());
    app.on_backend_event(both.last().copied(), opened);
    let copy_doc = app.all_tabs().find(|tab| tab.view == copy).and_then(doc_of);
    assert_eq!(copy_doc, Some(DOC), "the copy is bound");
    let code_views: Vec<_> = app
        .all_tabs()
        .filter(|tab| matches!(tab.kind, TabKind::Code { .. }))
        .map(doc_of)
        .collect();
    assert_eq!(
        code_views,
        vec![Some(DOC), Some(DOC)],
        "both views share it"
    );
    // The app holds one reference per document, so the second open's extra
    // one goes straight back to the session.
    let closes = backend.sent.lock().map_or(0, |sent| {
        sent.iter()
            .filter(|(_, command)| {
                matches!(command, SessionCommand::CloseDocument { doc } if *doc == DOC)
            })
            .count()
    });
    assert_eq!(closes, 1);

    app.on_snapshot(DOC, &buffer_snapshot(TextBuffer::from_text("one\ntwo\n")));
    assert_eq!(code_text(&app), "one\ntwo\n", "the copy fills");
    let _ = std::fs::remove_dir_all(&dir);
}

/// How many active notifications read exactly `title`.
fn cards(app: &App, title: &str) -> usize {
    app.notifications
        .active()
        .iter()
        .filter(|note| note.title == title)
        .count()
}

/// A split made before the open is answered sends a second open, so a file
/// that cannot open fails twice: once per view, but as one card either way.
#[test]
fn a_split_made_before_a_failed_open_shows_the_failure_once() {
    let dir = test_dir("fill-split-failed");
    write_file(&dir, "notes.md", b"text\n");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    app.open_path(&dir.join("notes.md"));
    app.split_focused(SplitDir::Right);
    let both = opens(&backend);
    assert_eq!(both.len(), 2, "each view asked for its own document");

    let message = "could not open notes.md: denied";
    for open in &both {
        let failed = SessionEvent::Notification {
            severity: Severity::Error,
            kind: NotificationKind::Io,
            message: message.to_string(),
        };
        app.on_backend_event(Some(*open), failed);
    }
    assert!(app.pending_open.is_empty(), "both opens are answered");
    let failed_views = app
        .all_tabs()
        .filter(|tab| tab.fill_error.as_deref() == Some(message))
        .count();
    assert_eq!(failed_views, 2, "both views show why");
    assert_eq!(cards(&app, message), 1, "one card for one file");

    // A failure that answers no open is not an open's, and stays its own card.
    let unrelated = SessionEvent::Notification {
        severity: Severity::Error,
        kind: NotificationKind::Io,
        message: message.to_string(),
    };
    app.on_backend_event(None, unrelated);
    assert_eq!(cards(&app, message), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same for a file that turns out not to be UTF-8: both views fall back to
/// the hex view, under one card.
#[test]
fn a_split_made_before_a_non_utf8_open_shows_the_fallback_once() {
    let dir = test_dir("fill-split-not-utf8");
    // Text in the sampled head, so it opens as a code tab; the session's full
    // decode is what answers `NotUtf8`, simulated below.
    write_file(&dir, "blob.txt", b"text\n");
    let path = dir.join("blob.txt");
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.backend = Some(backend.clone());
    app.open_path(&path);
    app.split_focused(SplitDir::Right);
    let both = opens(&backend);
    assert_eq!(both.len(), 2, "each view asked for its own document");

    for open in &both {
        let not_utf8 = SessionEvent::NotUtf8 { path: path.clone() };
        app.on_backend_event(Some(*open), not_utf8);
    }
    assert!(app.pending_open.is_empty(), "both opens are answered");
    let hex_views = app
        .all_tabs()
        .filter(|tab| matches!(tab.kind, TabKind::Hex { .. }))
        .count();
    assert_eq!(hex_views, 2, "both views fall back to hex");
    let title = format!("opened {} read-only: not valid UTF-8", path.display());
    assert_eq!(cards(&app, &title), 1, "one card for one file");
    let _ = std::fs::remove_dir_all(&dir);
}

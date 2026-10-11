//! Rename Symbol: F2 opens the inline prompt, the backend's answers shape it,
//! Enter sends the rename, and the applied edit's tabs and notice follow.

use karet_core::ApplyWorkspaceEditResult;
use karet_core::Range;
use karet_session::AppliedWorkspaceEdit;
use karet_session::RenameTarget;

use super::support::*;
use crate::app::*;

const SOURCE: &str = "let some_value = 1;\nprint(some_value);\n";

/// The commands of one kind a backend received, in order.
fn sent<T>(
    backend: &RecordingBackend,
    pick: impl Fn(RequestId, &SessionCommand) -> Option<T>,
) -> Vec<T> {
    backend
        .sent
        .lock()
        .map(|sent| sent.iter().filter_map(|(id, c)| pick(*id, c)).collect())
        .unwrap_or_default()
}

fn prepare_ids(backend: &RecordingBackend) -> Vec<RequestId> {
    sent(backend, |id, command| {
        matches!(command, SessionCommand::PrepareRename { .. }).then_some(id)
    })
}

fn renames(backend: &RecordingBackend) -> Vec<(RequestId, LineCol, String)> {
    sent(backend, |id, command| match command {
        SessionCommand::Rename {
            position, new_name, ..
        } => Some((id, *position, new_name.clone())),
        _ => None,
    })
}

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        send_key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

#[test]
fn f2_opens_the_prompt_at_once_with_the_name_selected() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    send_key(&mut app, KeyCode::F(2), KeyModifiers::NONE);

    assert_eq!(prepare_ids(&backend).len(), 1, "the backend is asked");
    let prompt = app
        .rename
        .as_ref()
        .expect("the prompt opens before the answer");
    assert!(prompt.shown);
    assert_eq!(prompt.range, span(0, 4, 14));
    assert_eq!(prompt.text, "some_value");
    assert_eq!(prompt.edit.selected_text(&prompt.text), Some("some_value"));
    assert_eq!(app.input_context().modal, Some(Modal::Rename));

    // Typing replaces the selected name, and Enter renames at the symbol.
    type_text(&mut app, "total");
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.rename.is_none());
    assert_eq!(
        renames(&backend),
        vec![(
            app.pending_rename.as_ref().expect("pending").id,
            LineCol::new(0, 4),
            "total".to_owned()
        )]
    );
}

#[test]
fn the_server_answer_corrects_the_prompt_until_the_user_types() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    let id = prepare_ids(&backend)[0];

    app.on_backend_event(
        Some(id),
        SessionEvent::RenamePrepared {
            doc: DocumentId(9),
            version: 0,
            target: Some(RenameTarget {
                range: span(0, 4, 8),
                placeholder: "some".into(),
            }),
            reason: None,
        },
    );
    let prompt = app.rename.as_ref().expect("prompt");
    assert_eq!(
        (prompt.range, prompt.text.as_str()),
        (span(0, 4, 8), "some")
    );

    // Once the user has typed, a late answer keeps what they typed.
    app.dispatch(Command::RenameSymbol);
    let id = prepare_ids(&backend)[1];
    type_text(&mut app, "mine");
    app.on_backend_event(
        Some(id),
        SessionEvent::RenamePrepared {
            doc: DocumentId(9),
            version: 0,
            target: Some(RenameTarget {
                range: span(0, 4, 14),
                placeholder: "server".into(),
            }),
            reason: None,
        },
    );
    let prompt = app.rename.as_ref().expect("prompt");
    assert_eq!(prompt.text, "mine");
    assert_eq!(prompt.range, span(0, 4, 14));
}

#[test]
fn a_refusal_closes_the_prompt_with_the_reason() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    let id = prepare_ids(&backend)[0];
    app.on_backend_event(
        Some(id),
        SessionEvent::RenamePrepared {
            doc: DocumentId(9),
            version: 0,
            target: None,
            reason: Some("cannot rename a keyword".into()),
        },
    );
    assert!(app.rename.is_none());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("rename: cannot rename a keyword")
    );
}

#[test]
fn off_a_name_the_prompt_waits_for_the_server() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 16));
    app.dispatch(Command::RenameSymbol);
    // Nothing the editor recognizes: no prompt, and keys still edit.
    assert_eq!(app.input_context().modal, None);
    let id = prepare_ids(&backend)[0];
    app.on_backend_event(
        Some(id),
        SessionEvent::RenamePrepared {
            doc: DocumentId(9),
            version: 0,
            target: Some(RenameTarget {
                range: span(0, 15, 16),
                placeholder: "=".into(),
            }),
            reason: None,
        },
    );
    assert_eq!(app.input_context().modal, Some(Modal::Rename));
}

#[test]
fn esc_and_an_unchanged_name_rename_nothing() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.rename.is_none());

    app.dispatch(Command::RenameSymbol);
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(renames(&backend).is_empty());
    assert!(app.pending_rename.is_none());
}

#[test]
fn the_applied_rename_is_reported_and_opens_background_tabs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let other = dir.path().join("other.rs");
    std::fs::write(&other, "use some_value;\n").expect("write");
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    let active = app.active;
    app.dispatch(Command::RenameSymbol);
    type_text(&mut app, "total");
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let (id, _, _) = renames(&backend)[0].clone();

    app.on_backend_event(
        Some(id),
        SessionEvent::WorkspaceEdit {
            applied: AppliedWorkspaceEdit {
                result: ApplyWorkspaceEditResult::applied(),
                edited: vec![PathBuf::from("main.rs"), other.clone()],
                opened: vec![(DocumentId(40), other.clone())],
                ..AppliedWorkspaceEdit::default()
            },
        },
    );

    assert_eq!(
        last_message(&app).as_deref(),
        Some("renamed to total in 2 files")
    );
    // A background tab for the file, the focus left where it was.
    assert_eq!(app.active, active);
    assert!(
        app.tabs
            .iter()
            .any(|tab| tab.path() == Some(other.as_path()))
    );
    // The tab opens the document like any other, and the reference the
    // backend held on our behalf is released.
    let opens = sent(&backend, |_, command| match command {
        SessionCommand::OpenDocument { path, .. } if *path == other => Some(path.clone()),
        _ => None,
    });
    assert_eq!(opens.len(), 1);
    let closes = sent(&backend, |_, command| match command {
        SessionCommand::CloseDocument { doc } => Some(*doc),
        _ => None,
    });
    assert_eq!(closes, vec![DocumentId(40)]);
}

#[test]
fn a_failed_rename_says_why_and_a_moved_file_retargets_its_tab() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    type_text(&mut app, "total");
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let (id, _, _) = renames(&backend)[0].clone();
    app.on_backend_event(
        Some(id),
        SessionEvent::WorkspaceEdit {
            applied: AppliedWorkspaceEdit {
                result: ApplyWorkspaceEditResult::failed("main.rs changed", None),
                ..AppliedWorkspaceEdit::default()
            },
        },
    );
    assert_eq!(
        last_report(&app).map(|(_, message)| message).as_deref(),
        Some("rename: main.rs changed")
    );

    // A server's own edit that moved the open file carries its tab along.
    app.on_backend_event(
        None,
        SessionEvent::WorkspaceEdit {
            applied: AppliedWorkspaceEdit {
                result: ApplyWorkspaceEditResult::applied(),
                renamed: vec![(PathBuf::from("main.rs"), PathBuf::from("lib.rs"))],
                ..AppliedWorkspaceEdit::default()
            },
        },
    );
    let tab = &app.tabs[app.active];
    assert_eq!(tab.path(), Some(Path::new("lib.rs")));
    assert_eq!(tab.title, "lib.rs");
}

#[test]
fn switching_tabs_closes_the_prompt() {
    let (_backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    app.push_tab(text_tab("other.rs", "x\n"));
    app.reconcile_rename();
    assert!(app.rename.is_none());
}

#[test]
fn the_prompt_renders_under_the_symbol_with_the_name() {
    let (_backend, mut app) = completion_app(SOURCE, LineCol::new(0, 6));
    app.dispatch(Command::RenameSymbol);
    let rows = screen(&mut app, 80, 20);
    let title = rows
        .iter()
        .position(|row| row.contains(" Rename "))
        .expect("the prompt is drawn");
    assert!(
        rows[title + 1].contains("some_value"),
        "the name sits inside the box: {rows:#?}"
    );
    // Below the symbol's line, which stays readable.
    let symbol = rows
        .iter()
        .position(|row| row.contains("let some_value"))
        .expect("the symbol line is visible");
    assert_eq!(title, symbol + 1);
}

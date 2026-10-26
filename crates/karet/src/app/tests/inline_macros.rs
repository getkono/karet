use super::support::*;
use crate::app::*;

fn inline_macro_app(name: &str, text: &str) -> (App, Arc<RecordingBackend>) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab(name, text));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(152));
    }
    (app, backend)
}

/// The last edit sent: each one is followed by the selection set it left.
fn last_edit(sent: &[(RequestId, SessionCommand)]) -> Option<&(RequestId, SessionCommand)> {
    sent.iter()
        .rev()
        .find(|(_, command)| matches!(command, SessionCommand::ApplyChange { .. }))
}

#[test]
fn markdown_selection_character_runs_the_inline_macro_atomically() {
    let (mut app, backend) = inline_macro_app("notes.md", "café is good");
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        editor.set_selection(buffer, LineCol::new(0, 0), LineCol::new(0, 4));
    }

    app.dispatch(Command::InsertChar('['));

    let TabKind::Code { text, .. } = &app.tabs[active].kind else {
        return;
    };
    assert_eq!(text, "[café]() is good");
    assert_eq!(app.tabs[active].editor.cursor(), LineCol::new(0, 7));
    let sent = backend
        .sent
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some((_, SessionCommand::ApplyChange { change, cause, .. })) = last_edit(&sent) else {
        panic!("macro did not submit an edit");
    };
    assert_eq!(*cause, EditCause::Replace);
    assert_eq!(change.edits.len(), 1);
    assert_eq!(change.edits[0].new_text, "[café]()");
}

#[test]
fn markdown_code_selection_falls_back_to_plain_typing() {
    let (mut app, _) = inline_macro_app("notes.md", "`code`");
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        editor.set_selection(buffer, LineCol::new(0, 1), LineCol::new(0, 5));
    }

    app.dispatch(Command::InsertChar('['));

    let TabKind::Code { text, .. } = &app.tabs[active].kind else {
        return;
    };
    // No link macro inside code: ordinary typing wraps the selection in the
    // bracket pair instead of replacing it.
    assert_eq!(text, "`[code]`");
}

#[test]
fn inline_macros_read_the_shared_layered_parse_only_when_they_need_one() {
    // Tab after a word that is not `fn` never asks for a parse.
    let (mut app, _) = inline_macro_app("lib.rs", "let");
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        editor.set_selection(buffer, LineCol::new(0, 3), LineCol::new(0, 3));
    }
    assert!(!app.try_inline_macro(karet_syntax::InlineMacroTrigger::Tab));
    assert!(app.layered_trees.held().is_none());

    // A selection the macro must check parses once, into the cache every other
    // reader of this version shares.
    let (mut app, _) = inline_macro_app("notes.md", "`code`");
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        editor.set_selection(buffer, LineCol::new(0, 1), LineCol::new(0, 5));
    }
    assert!(!app.try_inline_macro(karet_syntax::InlineMacroTrigger::Character('[')));
    let held = app
        .layered_trees
        .held()
        .expect("the macro parsed through the cache");
    assert_eq!(held.text, "`code`");
    let shared = app.layered_tree().expect("the shared parse");
    assert!(Arc::ptr_eq(&held, &shared));
}

#[test]
fn markdown_macro_expands_every_selected_caret_in_one_change() {
    let (mut app, backend) = inline_macro_app("notes.md", "one two");
    let active = app.active;
    if let Tab {
        kind: TabKind::Code { buffer, .. },
        editor,
        ..
    } = &mut app.tabs[active]
    {
        editor.set_cursor_state(
            buffer,
            karet_core::CursorState {
                selections: vec![
                    karet_core::Selection {
                        anchor: LineCol::new(0, 0),
                        head: LineCol::new(0, 3),
                    },
                    karet_core::Selection {
                        anchor: LineCol::new(0, 4),
                        head: LineCol::new(0, 7),
                    },
                ],
                primary: 1,
            },
        );
    }

    app.dispatch(Command::InsertChar('['));

    let TabKind::Code { text, .. } = &app.tabs[active].kind else {
        return;
    };
    assert_eq!(text, "[one]() [two]()");
    assert_eq!(app.tabs[active].editor.cursors().selections.len(), 2);
    let sent = backend
        .sent
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some((_, SessionCommand::ApplyChange { change, .. })) = last_edit(&sent) else {
        panic!("macro did not submit an edit");
    };
    assert_eq!(change.edits.len(), 2);
}

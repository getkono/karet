//! What a language server's completion items carry beyond a label (issue
//! #371): their edit ranges, additional edits, deferred documentation,
//! commit and trigger characters, and incomplete lists.

use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;
use karet_core::Markup;
use karet_core::MarkupKind;

use super::support::*;
use crate::app::*;

fn span(from: (u32, u32), to: (u32, u32)) -> Range {
    Range {
        start: LineCol::new(from.0, from.1),
        end: LineCol::new(to.0, to.1),
    }
}

/// An item that replaces `range` with `text`.
fn edit_item(label: &str, range: Range, text: &str) -> CompletionItem {
    CompletionItem {
        edit: Some(TextEdit {
            range,
            new_text: text.to_owned(),
        }),
        insert_text: text.to_owned(),
        ..CompletionItem::new(label, CompletionKind::Method)
    }
}

/// Every command the backend received, in order.
fn sent(backend: &RecordingBackend) -> Vec<(RequestId, SessionCommand)> {
    backend
        .sent
        .lock()
        .map(|sent| sent.clone())
        .unwrap_or_default()
}

/// The `(id, item label)` of every resolve the backend received.
fn resolves(backend: &RecordingBackend) -> Vec<(RequestId, String)> {
    sent(backend)
        .into_iter()
        .filter_map(|(id, command)| match command {
            SessionCommand::Lsp(LspCommand::ResolveCompletion { item, .. }) => {
                Some((id, item.label))
            },
            _ => None,
        })
        .collect()
}

/// The trigger of every completion request the backend received.
fn triggers(backend: &RecordingBackend) -> Vec<CompletionTrigger> {
    sent(backend)
        .into_iter()
        .filter_map(|(_, command)| match command {
            SessionCommand::Lsp(LspCommand::Completion { trigger, .. }) => Some(trigger),
            _ => None,
        })
        .collect()
}

/// Ask with Ctrl+Space, and answer with `items` as `options`' server would.
fn answer(
    app: &mut App,
    backend: &RecordingBackend,
    items: Vec<CompletionItem>,
    incomplete: bool,
    options: CompletionOptions,
) {
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL));
    let (id, _) = *completion_requests(backend)
        .last()
        .expect("a completion request");
    app.on_backend_event(
        Some(id),
        SessionEvent::Completions {
            doc: DocumentId(9),
            version: 0,
            items,
            incomplete,
            options,
        },
    );
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

/// Falsified by an accept that replaces only the typed word: `.` would stay,
/// giving `v.?.push`.
#[test]
fn an_accept_replaces_the_servers_edit_range() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    let item = edit_item("push", span((0, 1), (0, 4)), "?.push");
    answer(
        &mut app,
        &backend,
        vec![item],
        false,
        CompletionOptions::default(),
    );
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "v?.push\n");
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 7));
}

/// What was typed after the request is part of what the accept replaces.
#[test]
fn an_edit_range_covers_what_was_typed_after_the_request() {
    let (backend, mut app) = completion_app("pu\n", LineCol::new(0, 2));
    let item = edit_item("push", span((0, 0), (0, 2)), "push");
    answer(
        &mut app,
        &backend,
        vec![item],
        false,
        CompletionOptions::default(),
    );
    key(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "push\n", "not `pushs`");
}

/// Enter inserts over the word up to the caret; Shift+Enter replaces the
/// rest of it too.
#[test]
fn shift_enter_takes_the_replace_range() {
    for (modifiers, expected) in [
        (KeyModifiers::NONE, "push_front_back\n"),
        (KeyModifiers::SHIFT, "push_front\n"),
    ] {
        let (backend, mut app) = completion_app("pu_back\n", LineCol::new(0, 2));
        let item = CompletionItem {
            replace_range: Some(span((0, 0), (0, 7))),
            ..edit_item("push_front", span((0, 0), (0, 2)), "push_front")
        };
        answer(
            &mut app,
            &backend,
            vec![item],
            false,
            CompletionOptions::default(),
        );
        key(&mut app, KeyCode::Enter, modifiers);
        assert_eq!(code_tab_text(&app), expected, "with {modifiers:?}");
    }
}

/// Falsified by an auto-import that is dropped, or applied without moving
/// the caret down past the line it adds.
#[test]
fn additional_edits_apply_with_the_accept() {
    let (backend, mut app) = completion_app("fn f() {\n    Ha\n}\n", LineCol::new(1, 6));
    let item = CompletionItem {
        additional_edits: vec![TextEdit {
            range: span((0, 0), (0, 0)),
            new_text: "use std::collections::HashMap;\n".to_owned(),
        }],
        ..CompletionItem::new("HashMap", CompletionKind::Struct)
    };
    answer(
        &mut app,
        &backend,
        vec![item],
        false,
        CompletionOptions::default(),
    );
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        code_tab_text(&app),
        "use std::collections::HashMap;\nfn f() {\n    HashMap\n}\n"
    );
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 11));
}

/// The selected item is resolved once, and its documentation then shows
/// beside the popup, rendered as markdown.
#[test]
fn the_selected_item_resolves_and_shows_its_documentation() {
    let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    let item = CompletionItem {
        payload: Some("{}".to_owned()),
        ..CompletionItem::new("foobar", CompletionKind::Function)
    };
    answer(
        &mut app,
        &backend,
        vec![item],
        false,
        CompletionOptions::default(),
    );
    let asked = resolves(&backend);
    assert_eq!(asked.len(), 1, "the selection is resolved on opening");
    let (id, label) = asked[0].clone();
    assert_eq!(label, "foobar");
    // Rendering again does not ask again while the answer is on its way.
    let _ = screen(&mut app, 100, 12);
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(resolves(&backend).len(), 1);

    app.on_backend_event(
        Some(id),
        SessionEvent::CompletionResolved {
            doc: DocumentId(9),
            resolved: Some(CompletionItem {
                documentation: Some(Markup {
                    kind: MarkupKind::Markdown,
                    value: "Frobs the **bar**.".to_owned(),
                }),
                ..CompletionItem::new("foobar", CompletionKind::Function)
            }),
        },
    );
    let ui = app.completion.popup.as_ref().expect("still open");
    assert_eq!(ui.items[0].payload, None, "the payload is spent");
    let painted = screen(&mut app, 100, 12).join("\n");
    assert!(
        painted.contains("Frobs the bar."),
        "the docs pane renders the markdown: {painted}"
    );
}

/// An item accepted before its resolve answered still gets the additional
/// edits the server deferred, as soon as they arrive -- unless the user has
/// edited since.
#[test]
fn deferred_additional_edits_follow_an_early_accept() {
    for edit_in_between in [false, true] {
        let (backend, mut app) = completion_app("x\nHa\n", LineCol::new(1, 2));
        let item = CompletionItem {
            payload: Some("{}".to_owned()),
            ..CompletionItem::new("HashMap", CompletionKind::Struct)
        };
        answer(
            &mut app,
            &backend,
            vec![item],
            false,
            CompletionOptions::default(),
        );
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(code_tab_text(&app), "x\nHashMap\n");
        let (id, _) = resolves(&backend)
            .last()
            .cloned()
            .expect("the accept asks for what was deferred");
        if edit_in_between {
            key(&mut app, KeyCode::Char(';'), KeyModifiers::NONE);
        }
        app.on_backend_event(
            Some(id),
            SessionEvent::CompletionResolved {
                doc: DocumentId(9),
                resolved: Some(CompletionItem {
                    additional_edits: vec![TextEdit {
                        range: span((0, 0), (0, 0)),
                        new_text: "use m::HashMap;\n".to_owned(),
                    }],
                    ..CompletionItem::new("HashMap", CompletionKind::Struct)
                }),
            },
        );
        if edit_in_between {
            assert_eq!(code_tab_text(&app), "x\nHashMap;\n", "stale: dropped");
        } else {
            assert_eq!(code_tab_text(&app), "use m::HashMap;\nx\nHashMap\n");
            assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(2, 7));
        }
    }
}

/// An unresolved item that listed its additional edits up front applies them
/// at the accept and asks for nothing more, so a server echoing them on
/// resolve cannot apply the import twice.
#[test]
fn eager_additional_edits_are_not_asked_for_again() {
    let (backend, mut app) = completion_app("x\nHa\n", LineCol::new(1, 2));
    let item = CompletionItem {
        payload: Some("{}".to_owned()),
        additional_edits: vec![TextEdit {
            range: span((0, 0), (0, 0)),
            new_text: "use m::HashMap;\n".to_owned(),
        }],
        ..CompletionItem::new("HashMap", CompletionKind::Struct)
    };
    answer(
        &mut app,
        &backend,
        vec![item],
        false,
        CompletionOptions::default(),
    );
    let asked = resolves(&backend).len();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "use m::HashMap;\nx\nHashMap\n");
    assert_eq!(resolves(&backend).len(), asked, "no follow-up resolve");
    assert!(app.completion.follow_up.is_none());
}

/// Each document keeps the characters its own server last answered with:
/// another document's answer does not revert this one to the built-in ones.
#[test]
fn completion_characters_are_kept_per_document() {
    let (backend, mut app) = completion_app("a\n", LineCol::new(0, 1));
    let here = CompletionOptions {
        trigger_characters: vec!['<'],
        commit_characters: vec![';'],
    };
    answer(&mut app, &backend, Vec::new(), false, here.clone());
    let elsewhere = CompletionOptions {
        trigger_characters: vec!['#'],
        commit_characters: Vec::new(),
    };
    let id = RequestId(u64::MAX);
    app.completion.pending = Some(crate::completion::PendingCompletion {
        id,
        doc: DocumentId(10),
        anchor: LineCol::new(0, 0),
        position: LineCol::new(0, 0),
    });
    app.on_backend_event(
        Some(id),
        SessionEvent::Completions {
            doc: DocumentId(10),
            version: 0,
            items: Vec::new(),
            incomplete: false,
            options: elsewhere.clone(),
        },
    );
    assert_eq!(app.completion_characters(DocumentId(9)), here);
    assert_eq!(app.completion_characters(DocumentId(10)), elsewhere);
}

/// A server's commit character accepts the selection and is then typed; a
/// character it did not name is just typed.
#[test]
fn a_commit_character_accepts_then_types_itself() {
    let options = CompletionOptions {
        trigger_characters: Vec::new(),
        commit_characters: vec![';'],
    };
    let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    let item = CompletionItem::new("foobar", CompletionKind::Function);
    answer(
        &mut app,
        &backend,
        vec![item.clone()],
        false,
        options.clone(),
    );
    key(&mut app, KeyCode::Char(';'), KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "foobar;\n");

    let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    answer(&mut app, &backend, vec![item], false, options);
    key(&mut app, KeyCode::Char(','), KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "fo,\n");
}

/// Once a server named its trigger characters, those -- and only those --
/// re-request, saying which character did it.
#[test]
fn a_servers_trigger_characters_replace_the_built_in_ones() {
    let (backend, mut app) = completion_app("a\n", LineCol::new(0, 1));
    let options = CompletionOptions {
        trigger_characters: vec!['<'],
        commit_characters: Vec::new(),
    };
    answer(&mut app, &backend, Vec::new(), false, options);
    key(&mut app, KeyCode::Char('<'), KeyModifiers::NONE);
    assert_eq!(
        triggers(&backend).last(),
        Some(&CompletionTrigger::Character('<'))
    );
    let asked = triggers(&backend).len();
    app.dismiss_completion();
    key(&mut app, KeyCode::Char(' '), KeyModifiers::NONE);
    key(&mut app, KeyCode::Char('.'), KeyModifiers::NONE);
    assert_eq!(triggers(&backend).len(), asked, "`.` is not this server's");
}

/// Typing over an incomplete list asks again; over a complete one it only
/// filters.
#[test]
fn an_incomplete_list_is_asked_again_as_the_word_grows() {
    for incomplete in [true, false] {
        let (backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
        let items = vec![
            CompletionItem::new("foo", CompletionKind::Function),
            CompletionItem::new("fob", CompletionKind::Function),
        ];
        answer(
            &mut app,
            &backend,
            items,
            incomplete,
            CompletionOptions::default(),
        );
        let before = triggers(&backend).len();
        key(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
        let after = triggers(&backend);
        if incomplete {
            assert_eq!(after.len(), before + 1);
            assert_eq!(after.last(), Some(&CompletionTrigger::Incomplete));
            assert!(
                app.completion.popup.is_some(),
                "the old list stays up meanwhile"
            );
        } else {
            assert_eq!(after.len(), before, "a complete list only filters");
        }
    }
}

//! Snippet completions (issue #373): an accept selects the first stop, Tab and
//! Shift+Tab move between stops, mirrors are edited together, Esc leaves.

use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::CompletionOptions;
use karet_core::Tabstop;

use super::support::*;
use crate::app::*;

fn stop(index: u32, spans: &[(u32, u32)]) -> Tabstop {
    Tabstop {
        index,
        spans: spans.to_vec(),
        choices: Vec::new(),
    }
}

/// A snippet item inserting `text` over the typed word, with `tabstops`.
fn snippet_item(label: &str, text: &str, tabstops: Vec<Tabstop>) -> CompletionItem {
    CompletionItem {
        insert_text: text.to_owned(),
        tabstops,
        ..CompletionItem::new(label, CompletionKind::Snippet)
    }
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

/// Open the popup with `item` and accept it with Enter.
fn accept(app: &mut App, backend: &RecordingBackend, item: CompletionItem) {
    key(app, KeyCode::Char(' '), KeyModifiers::CONTROL);
    let (id, _) = *completion_requests(backend)
        .last()
        .expect("a completion request");
    app.on_backend_event(
        Some(id),
        SessionEvent::Completions {
            doc: DocumentId(9),
            version: 0,
            items: vec![item],
            incomplete: false,
            options: CompletionOptions::default(),
        },
    );
    key(app, KeyCode::Enter, KeyModifiers::NONE);
}

/// The active editor's selections, as ranges.
fn selections(app: &App) -> Vec<Range> {
    app.tabs[app.active]
        .editor
        .cursors()
        .selections
        .iter()
        .map(|selection| selection.range())
        .collect()
}

fn span(line: u32, from: u32, to: u32) -> Range {
    Range {
        start: LineCol::new(line, from),
        end: LineCol::new(line, to),
    }
}

/// `push(${1:ch}, ${2:n})$0`, typed after `v.pu`.
fn push_item() -> CompletionItem {
    snippet_item(
        "push",
        "push(ch, n)",
        vec![
            stop(1, &[(5, 7)]),
            stop(2, &[(9, 10)]),
            stop(0, &[(11, 11)]),
        ],
    )
}

/// Falsified by an accept that leaves the caret after the text, or a Tab
/// that indents instead of moving on, or a session outliving its final stop.
#[test]
fn tab_walks_the_stops_to_the_final_one() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    assert_eq!(code_tab_text(&app), "v.push(ch, n)\n");
    assert_eq!(selections(&app), vec![span(0, 7, 9)], "`ch` is selected");
    typed(&mut app, "c");
    assert_eq!(code_tab_text(&app), "v.push(c, n)\n");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(selections(&app), vec![span(0, 10, 11)], "`n` is selected");
    typed(&mut app, "42");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "v.push(c, 42)\n");
    assert_eq!(selections(&app), vec![span(0, 13, 13)]);
    assert!(
        app.completion.snippet.is_none(),
        "the final stop ends the session"
    );
}

/// Typing in a mirrored stop edits every copy of it.
#[test]
fn mirrors_are_edited_together() {
    let (backend, mut app) = completion_app("le\n", LineCol::new(0, 2));
    let item = snippet_item(
        "let",
        "let v = v;",
        vec![stop(1, &[(4, 5), (8, 9)]), stop(0, &[(10, 10)])],
    );
    accept(&mut app, &backend, item);
    assert_eq!(selections(&app).len(), 2);
    typed(&mut app, "total");
    assert_eq!(code_tab_text(&app), "let total = total;\n");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(selections(&app), vec![span(0, 18, 18)]);
}

#[test]
fn shift_tab_goes_back_a_stop() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(selections(&app), vec![span(0, 7, 9)]);
    assert!(app.completion.snippet.is_some());
}

/// Esc leaves the session; the next Tab is an ordinary Tab again.
#[test]
fn esc_leaves_the_snippet() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    assert!(app.completion.snippet.is_some());
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.completion.snippet.is_none());
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_ne!(
        selections(&app),
        vec![span(0, 10, 11)],
        "Tab no longer moves to the next stop"
    );
}

/// A Tab from a caret that left the snippet ends the session and does what
/// Tab does there.
#[test]
fn a_tab_outside_the_snippet_is_an_ordinary_tab() {
    let (backend, mut app) = completion_app("v.pu\nx\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    assert!(app.completion.snippet.is_some());
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(1, 0)]);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(app.completion.snippet.is_none());
    assert_ne!(code_tab_text(&app), "v.push(ch, n)\nx\n", "Tab indented");
}

/// An undo is a change the session did not follow: it ends there.
#[test]
fn an_unfollowed_change_ends_the_session() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    if let TabKind::Code { next_version, .. } = &mut app.tabs[app.active].kind {
        *next_version += 7;
    }
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(app.completion.snippet.is_none());
}

/// A snippet whose only stop is `$0` places the caret there and starts no
/// session; a plain item starts none either.
#[test]
fn a_final_stop_alone_only_places_the_caret() {
    let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    let item = snippet_item("foo", "foo()", vec![stop(0, &[(4, 4)])]);
    accept(&mut app, &backend, item);
    assert_eq!(selections(&app), vec![span(0, 4, 4)]);
    assert!(app.completion.snippet.is_none());

    let (backend, mut app) = completion_app("fo\n", LineCol::new(0, 2));
    accept(
        &mut app,
        &backend,
        CompletionItem::new("foo", CompletionKind::Function),
    );
    assert_eq!(selections(&app), vec![span(0, 3, 3)]);
    assert!(app.completion.snippet.is_none());
}

/// An import the item brings above the snippet moves its stops down a line.
#[test]
fn additional_edits_move_the_stops() {
    let (backend, mut app) = completion_app("x\npu\n", LineCol::new(1, 2));
    let item = CompletionItem {
        additional_edits: vec![TextEdit {
            range: span(0, 0, 0),
            new_text: "use x;\n".to_owned(),
        }],
        ..snippet_item(
            "push",
            "push(ch)",
            vec![stop(1, &[(5, 7)]), stop(0, &[(8, 8)])],
        )
    };
    accept(&mut app, &backend, item);
    assert_eq!(code_tab_text(&app), "use x;\nx\npush(ch)\n");
    assert_eq!(selections(&app), vec![span(2, 5, 7)]);
}

/// Falsified by a copy's stops placed at its caret's position before the
/// other copies' inserts shifted it (the second copy on a line selecting
/// inside the first), or by a copy left unselected.
#[test]
fn every_caret_gets_a_copy_whose_stops_are_selected() {
    let (backend, mut app) = completion_app("pu pu\n", LineCol::new(0, 5));
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(0, 2), LineCol::new(0, 5)]);
    accept(&mut app, &backend, push_item());
    assert_eq!(code_tab_text(&app), "push(ch, n) push(ch, n)\n");
    assert_eq!(selections(&app), vec![span(0, 5, 7), span(0, 17, 19)]);
    typed(&mut app, "c");
    assert_eq!(code_tab_text(&app), "push(c, n) push(c, n)\n");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(selections(&app), vec![span(0, 8, 9), span(0, 19, 20)]);
}

/// The same across lines, with an import above both copies moving each of
/// them down a line.
#[test]
fn copies_on_several_lines_follow_the_additional_edits() {
    let (backend, mut app) = completion_app("x\npu\npu\n", LineCol::new(2, 2));
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(1, 2), LineCol::new(2, 2)]);
    let item = CompletionItem {
        additional_edits: vec![TextEdit {
            range: span(0, 0, 0),
            new_text: "use x;\n".to_owned(),
        }],
        ..push_item()
    };
    accept(&mut app, &backend, item);
    assert_eq!(code_tab_text(&app), "use x;\nx\npush(ch, n)\npush(ch, n)\n");
    assert_eq!(selections(&app), vec![span(2, 5, 7), span(3, 5, 7)]);
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(selections(&app), vec![span(2, 9, 10), span(3, 9, 10)]);
}

/// Falsified by the old session's later stops (`n`) surviving the new
/// item's accept, or by the new item's first stop not being selected.
#[test]
fn a_snippet_accepted_in_a_session_replaces_it() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    typed(&mut app, "ne");
    let item = snippet_item(
        "new",
        "new(a)",
        vec![stop(1, &[(4, 5)]), stop(0, &[(6, 6)])],
    );
    accept(&mut app, &backend, item);
    assert_eq!(code_tab_text(&app), "v.push(new(a), n)\n");
    assert_eq!(selections(&app), vec![span(0, 11, 12)], "`a` is selected");
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(
        selections(&app),
        vec![span(0, 13, 13)],
        "the new item's final stop, not the old session's `n`"
    );
    assert!(
        app.completion.snippet.is_none(),
        "the old session went with the accept"
    );
}

/// A plain completion accepted inside a placeholder fills it in, and the
/// session carries on past it.
#[test]
fn a_plain_completion_inside_a_stop_keeps_the_session() {
    let (backend, mut app) = completion_app("v.pu\n", LineCol::new(0, 4));
    accept(&mut app, &backend, push_item());
    typed(&mut app, "my");
    accept(
        &mut app,
        &backend,
        CompletionItem::new("my_char", CompletionKind::Variable),
    );
    assert_eq!(code_tab_text(&app), "v.push(my_char, n)\n");
    assert!(app.completion.snippet.is_some());
    key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(selections(&app), vec![span(0, 16, 17)], "`n` is selected");
}

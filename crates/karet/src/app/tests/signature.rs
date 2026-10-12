//! The signature-help wiring: what opens it, the request out, the answer in,
//! the popup on screen, and what closes it. The call-finding scan and the
//! widget are tested where they live; what is covered here is the seam
//! between them, the keyboard, and the backend.

use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::CompletionOptions;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::ParamInfo;
use karet_core::Signature;
use karet_core::SignatureHelp;
use karet_core::SignatureHelpOptions;

use super::support::*;
use crate::app::*;

/// The signature-help requests a backend received, as `(id, position, manual)`.
fn requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol, bool)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::SignatureHelp {
                        position, manual, ..
                    } => Some((*id, *position, *manual)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn signature(label: &str, parameters: &[&str]) -> Signature {
    Signature {
        label: label.to_owned(),
        documentation: None,
        parameters: parameters
            .iter()
            .map(|label| ParamInfo {
                label: (*label).to_owned(),
                documentation: None,
            })
            .collect(),
    }
}

fn add_help(active_parameter: usize) -> SignatureHelp {
    SignatureHelp {
        signatures: vec![signature(
            "fn add(a: i32, b: i32) -> i32",
            &["a: i32", "b: i32"],
        )],
        active_signature: 0,
        active_parameter,
    }
}

fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_key(KeyEvent::new(code, modifiers));
}

/// Answer the latest request with `help`.
fn answer(app: &mut App, backend: &RecordingBackend, help: Option<SignatureHelp>) {
    answer_with(app, backend, help, SignatureHelpOptions::default());
}

fn answer_with(
    app: &mut App,
    backend: &RecordingBackend,
    help: Option<SignatureHelp>,
    options: SignatureHelpOptions,
) {
    let Some(&(id, _, _)) = requests(backend).last() else {
        panic!("nothing was asked");
    };
    app.on_backend_event(
        Some(id),
        SessionEvent::SignatureHelp {
            doc: DocumentId(9),
            help,
            options,
        },
    );
}

/// An app whose caret sits inside `add(`, with the popup open over it.
fn open_popup(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = completion_app(text, caret);
    app.dispatch(Command::SignatureHelp);
    answer(&mut app, &backend, Some(add_help(0)));
    assert!(app.signature.ui.is_some(), "the answer opens the popup");
    (backend, app)
}

#[test]
fn typing_an_open_paren_asks_at_the_caret_without_reporting_refusals() {
    let (backend, mut app) = completion_app("add\n", LineCol::new(0, 3));
    key(&mut app, KeyCode::Char('('), KeyModifiers::NONE);
    let asked = requests(&backend);
    assert_eq!(asked.len(), 1);
    assert_eq!((asked[0].1, asked[0].2), (LineCol::new(0, 4), false));
}

#[test]
fn ctrl_shift_space_asks_by_hand() {
    let (backend, mut app) = completion_app("add(1, 2)\n", LineCol::new(0, 4));
    key(
        &mut app,
        KeyCode::Char(' '),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    let asked = requests(&backend);
    assert_eq!(asked.len(), 1);
    assert!(asked[0].2, "a request by hand hears why it is refused");
}

/// Before the server names its characters `(` and `,` trigger; once it
/// has, its own do, and a character it did not name no longer does.
#[test]
fn the_servers_trigger_characters_replace_the_defaults() {
    let (backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
    key(&mut app, KeyCode::Char('('), KeyModifiers::NONE);
    let options = SignatureHelpOptions {
        trigger_characters: vec!['<'],
        retrigger_characters: Vec::new(),
    };
    answer_with(&mut app, &backend, None, options);
    assert_eq!(requests(&backend).len(), 1);

    key(&mut app, KeyCode::Char('('), KeyModifiers::NONE);
    assert_eq!(requests(&backend).len(), 1, "`(` is no longer a trigger");
    key(&mut app, KeyCode::Char('<'), KeyModifiers::NONE);
    assert_eq!(requests(&backend).len(), 2, "`<` now is");
}

#[test]
fn a_superseded_answer_opens_nothing() {
    let (backend, mut app) = completion_app("add(\n", LineCol::new(0, 4));
    app.dispatch(Command::SignatureHelp);
    let (id, _, _) = requests(&backend)[0];
    app.on_backend_event(
        Some(RequestId(id.0 + 1)),
        SessionEvent::SignatureHelp {
            doc: DocumentId(9),
            help: Some(add_help(0)),
            options: SignatureHelpOptions::default(),
        },
    );
    assert!(app.signature.ui.is_none());
}

/// While it is open, a typed retrigger character asks again where the caret
/// now is, and the answer moves the highlight on.
#[test]
fn typing_a_comma_asks_again_and_moves_the_highlight() {
    let (backend, mut app) = open_popup("add()\n", LineCol::new(0, 4));
    key(&mut app, KeyCode::Char('1'), KeyModifiers::NONE);
    key(&mut app, KeyCode::Char(','), KeyModifiers::NONE);
    let asked = requests(&backend);
    assert_eq!(asked.last().map(|ask| ask.1), Some(LineCol::new(0, 6)));
    answer(&mut app, &backend, Some(add_help(1)));
    let active = app.signature.ui.as_ref().map(|ui| ui.help.active_parameter);
    assert_eq!(active, Some(1));
}

/// The server saying the caret is in no call closes the popup.
#[test]
fn an_empty_answer_closes_the_popup() {
    let (backend, mut app) = open_popup("add(x)\n", LineCol::new(0, 4));
    key(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
    answer(&mut app, &backend, None);
    assert!(app.signature.ui.is_none());
}

/// Typing the `)` that closes the call leaves it, and the popup closes
/// without asking.
#[test]
fn typing_the_closing_paren_closes_without_asking() {
    let (backend, mut app) = open_popup("add(1\n", LineCol::new(0, 5));
    let before = requests(&backend).len();
    key(&mut app, KeyCode::Char(')'), KeyModifiers::NONE);
    assert!(app.signature.ui.is_none());
    assert_eq!(requests(&backend).len(), before);
}

/// Moving the caret back out before the `(` closes it too.
#[test]
fn moving_out_of_the_call_closes_the_popup() {
    let (_backend, mut app) = open_popup("add(1)\n", LineCol::new(0, 4));
    key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert!(app.signature.ui.is_none());
}

#[test]
fn esc_closes_the_popup_and_is_consumed() {
    let (_backend, mut app) = open_popup("add(1)\n", LineCol::new(0, 4));
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.signature.ui.is_none());
}

/// With the completion popup open over it, Esc and the arrows are the
/// completion popup's: the first Esc closes that one only.
#[test]
fn the_completion_popup_takes_the_keys_first() {
    let (backend, mut app) = open_popup("add(v)\n", LineCol::new(0, 5));
    let id = RequestId(9_000);
    app.pending_completion = Some(crate::completion::PendingCompletion {
        id,
        doc: DocumentId(9),
        anchor: LineCol::new(0, 4),
        position: LineCol::new(0, 5),
    });
    app.on_backend_event(
        Some(id),
        SessionEvent::Completions {
            doc: DocumentId(9),
            version: 0,
            items: vec![CompletionItem::new("value", CompletionKind::Variable)],
            incomplete: false,
            options: CompletionOptions::default(),
        },
    );
    assert!(app.completion.is_some(), "both popups are open");
    let _ = backend;

    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.completion.is_none());
    assert!(
        app.signature.ui.is_some(),
        "the first Esc is the completion's"
    );
    key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(app.signature.ui.is_none());
}

/// Up/Down cycle overloads, wrapping, and a refresh offering the same
/// overloads keeps the pick; with one signature the arrows move the caret.
#[test]
fn arrows_cycle_overloads_and_the_pick_survives_a_refresh() {
    let (backend, mut app) = completion_app("f(\n", LineCol::new(0, 2));
    app.dispatch(Command::SignatureHelp);
    let overloads = SignatureHelp {
        signatures: vec![signature("f(a)", &["a"]), signature("f(a, b)", &["a", "b"])],
        active_signature: 0,
        active_parameter: 0,
    };
    answer(&mut app, &backend, Some(overloads.clone()));
    key(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.signature.ui.as_ref().map(|ui| ui.shown()), Some(1));
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.signature.ui.as_ref().map(|ui| ui.shown()), Some(0));
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);

    key(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
    answer(&mut app, &backend, Some(overloads));
    assert_eq!(app.signature.ui.as_ref().map(|ui| ui.shown()), Some(1));

    let (_backend, mut single) = open_popup("add(\n\n", LineCol::new(0, 4));
    key(&mut single, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(single.tabs[single.active].editor.cursor().line, 1);
}

/// With the hover popup over it the signature popup is hidden, so the
/// arrows are not spent cycling an overload nobody sees: they move the caret.
#[test]
fn an_open_hover_leaves_the_arrows_to_the_caret() {
    let (backend, mut app) = completion_app("f(\n\n", LineCol::new(0, 2));
    app.dispatch(Command::SignatureHelp);
    let overloads = SignatureHelp {
        signatures: vec![signature("f(a)", &["a"]), signature("f(a, b)", &["a", "b"])],
        active_signature: 0,
        active_parameter: 0,
    };
    answer(&mut app, &backend, Some(overloads));
    app.hover_ui = Some(crate::hover::HoverUi {
        markup: Markup {
            kind: MarkupKind::Markdown,
            value: "docs".to_owned(),
        },
        doc: DocumentId(9),
        at: LineCol::new(0, 2),
        scroll: 0,
        rect: Rect::default(),
    });
    key(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(app.tabs[app.active].editor.cursor().line, 1);
    let chosen = app.signature.ui.as_ref().and_then(|ui| ui.chosen);
    assert_eq!(chosen, None, "no overload was cycled under the hover");
}

/// Switching to another document's tab closes the popup and forgets the
/// request in flight: its answer belongs to the document left behind.
#[test]
fn switching_tab_closes_the_popup() {
    let (backend, mut app) = completion_app("add(1)\n", LineCol::new(0, 4));
    app.push_tab(text_tab("other.rs", "add(2)\n"));
    let other = app.active;
    if let TabKind::Code { doc, .. } = &mut app.tabs[other].kind {
        *doc = Some(DocumentId(10));
    }
    app.tabs[other].editor.set_carets(&[LineCol::new(0, 4)]);
    app.set_active(0);
    app.dispatch(Command::SignatureHelp);
    answer(&mut app, &backend, Some(add_help(0)));
    assert!(app.signature.ui.is_some(), "the answer opens the popup");
    key(&mut app, KeyCode::Char('2'), KeyModifiers::NONE);
    assert!(app.signature.pending.is_some(), "a refresh is in flight");

    app.dispatch(Command::NextTab);
    app.reconcile_completion();
    assert_eq!(app.active, other);
    assert!(app.signature.ui.is_none());
    assert!(app.signature.pending.is_none());
}

/// Moving the keyboard off the editor closes the popup.
#[test]
fn the_editor_losing_the_keyboard_closes_the_popup() {
    let (_backend, mut app) = open_popup("add(1)\n", LineCol::new(0, 4));
    app.focus = Focus::Sidebar;
    app.reconcile_completion();
    assert!(app.signature.ui.is_none());
}

/// Closing a document's tab drops the trigger characters its server named.
#[test]
fn closing_the_tab_forgets_its_trigger_characters() {
    let (backend, mut app) = completion_app("f\n", LineCol::new(0, 1));
    app.open_docs.insert(DocumentId(9));
    key(&mut app, KeyCode::Char('('), KeyModifiers::NONE);
    let options = SignatureHelpOptions {
        trigger_characters: vec!['<'],
        retrigger_characters: vec![','],
    };
    answer_with(&mut app, &backend, None, options);
    assert!(app.signature.chars.contains_key(&DocumentId(9)));

    // The typed `(` dirtied it; close without the unsaved-changes prompt.
    let idx = app.active;
    app.tabs[idx].dirty = false;
    app.dispatch(Command::CloseTab);
    assert!(matches!(app.tabs[app.active].kind, TabKind::Welcome));
    assert!(!app.signature.chars.contains_key(&DocumentId(9)));
}

#[test]
fn the_popup_paints_above_the_caret() {
    let text = "fn add(a: i32, b: i32) -> i32 { a + b }\n\nlet x = add(1, \n";
    let (_backend, mut app) = open_popup(text, LineCol::new(2, 15));
    let rows = screen(&mut app, 90, 24);
    let caret_row = rows
        .iter()
        .position(|row| row.contains("let x = add(1,"))
        .unwrap_or_else(|| panic!("the caret line is on screen:\n{}", rows.join("\n")));
    let popup_row = rows
        .iter()
        .position(|row| row.contains("fn add(a: i32, b: i32) -> i32") && !row.contains('{'))
        .unwrap_or_else(|| panic!("the signature is on screen:\n{}", rows.join("\n")));
    assert!(
        popup_row < caret_row,
        "above the caret:\n{}",
        rows.join("\n")
    );
}

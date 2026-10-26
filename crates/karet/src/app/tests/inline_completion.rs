//! Ghost text (issue #393): asking once typing pauses, showing only the
//! newest ask's answer while it still matches what was typed, and Tab,
//! Ctrl+Right, Esc and Alt+] / Alt+[ acting on it only while it is on show.

use std::time::Duration;
use std::time::Instant;

use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::support::*;
use crate::app::*;

/// The inline-completion asks a backend received, as `(id, position,
/// trigger)`.
fn asks(backend: &RecordingBackend) -> Vec<(RequestId, LineCol, InlineCompletionTrigger)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::InlineCompletion {
                        position, trigger, ..
                    } => Some((*id, *position, *trigger)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The accepts a backend received, as `(server, item text)`.
fn accepts(backend: &RecordingBackend) -> Vec<(String, String)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::AcceptInlineCompletion { server, item, .. } => {
                        Some((server.key().to_owned(), item.insert_text.clone()))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn copilot() -> LanguageServerId {
    LanguageServerId::new("copilot-language-server")
}

/// An item replacing columns `start..end` of `line` with `text`.
fn item(text: &str, line: u32, (start, end): (u32, u32)) -> InlineCompletionItem {
    InlineCompletionItem {
        range: Range {
            start: LineCol::new(line, start),
            end: LineCol::new(line, end),
        },
        ..InlineCompletionItem::new(text, LineCol::new(line, start))
    }
}

fn type_char(app: &mut App, ch: char) {
    send_key(app, KeyCode::Char(ch), KeyModifiers::NONE);
}

/// An editor over `text` with the caret at `caret`, already seen once by the
/// probe -- so the next edit is what asks.
fn ghost_app(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App, Instant) {
    let (backend, mut app) = completion_app(text, caret);
    let now = Instant::now();
    app.probe_inline_completion(now);
    (backend, app, now)
}

/// Type `ch`, let typing pause, and return the ask it made.
fn type_and_ask(app: &mut App, backend: &RecordingBackend, ch: char, now: Instant) -> RequestId {
    type_char(app, ch);
    app.probe_inline_completion(now);
    app.probe_inline_completion(now + LSP_CHANGE_DEBOUNCE);
    asks(backend).last().map(|(id, ..)| *id).expect("an ask")
}

fn answer(app: &mut App, id: RequestId, items: Vec<InlineCompletionItem>) {
    app.on_backend_event(
        Some(id),
        SessionEvent::InlineCompletions {
            doc: DocumentId(9),
            version: 0,
            server: copilot(),
            items,
        },
    );
}

fn ghost_text(app: &App) -> Option<String> {
    app.inline_ghost().map(|(ghost, ..)| ghost.text)
}

/// Typing asks once it pauses, at the caret, and not before; moving the
/// caret asks nothing, and cancels an ask still waiting for the pause.
#[test]
fn an_edit_asks_once_typing_pauses_and_a_caret_move_does_not() {
    let (backend, mut app, now) = ghost_app("fn ma\n", LineCol::new(0, 5));
    assert!(asks(&backend).is_empty(), "opening a document asks nothing");

    type_char(&mut app, 'i');
    app.probe_inline_completion(now);
    app.probe_inline_completion(now + LSP_CHANGE_DEBOUNCE / 2);
    assert!(asks(&backend).is_empty(), "asked before typing paused");
    assert_eq!(
        app.inline_completion_next_wake(now),
        Some(LSP_CHANGE_DEBOUNCE),
        "the loop must wake to send the ask without another key"
    );
    app.probe_inline_completion(now + LSP_CHANGE_DEBOUNCE);
    assert_eq!(
        asks(&backend)
            .into_iter()
            .map(|(_, at, trigger)| (at, trigger))
            .collect::<Vec<_>>(),
        [(LineCol::new(0, 6), InlineCompletionTrigger::Automatic)]
    );
    assert_eq!(app.inline_completion_next_wake(now), None);

    send_key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    app.probe_inline_completion(now + LSP_CHANGE_DEBOUNCE * 4);
    assert_eq!(asks(&backend).len(), 1, "a caret move asked");

    type_char(&mut app, 'x');
    app.probe_inline_completion(now);
    send_key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    app.probe_inline_completion(now + LSP_CHANGE_DEBOUNCE * 4);
    assert_eq!(
        asks(&backend).len(),
        1,
        "an ask waiting for the pause outlived a caret move"
    );
}

/// `editor.inlineCompletion` off for the language asks nothing, however much
/// is typed.
#[test]
fn the_setting_switches_asking_off() {
    let (backend, mut app, now) = ghost_app("fn ma\n", LineCol::new(0, 5));
    app.settings.editor.inline_completion.enabled = false;
    type_char(&mut app, 'i');
    app.probe_inline_completion(now + Duration::from_secs(5));
    assert!(asks(&backend).is_empty());
    assert_eq!(app.inline_completion_next_wake(now), None);
}

/// The answer shows as ghost text after the caret, dimmed, its further lines
/// on rows below; Tab inserts it -- replacing the text it spans, so the
/// auto-closed bracket is not doubled -- and tells the server.
#[test]
fn tab_accepts_the_ghost_over_the_text_it_replaces_and_tells_the_server() {
    let (backend, mut app, now) = ghost_app("call()\n", LineCol::new(0, 5));
    let id = type_and_ask(&mut app, &backend, 'a', now);
    answer(&mut app, id, vec![item("call(a, b)\nnext();", 0, (0, 7))]);
    assert_eq!(ghost_text(&app).as_deref(), Some(", b)\nnext();"));

    let screen = screen(&mut app, 80, 12);
    let row = screen
        .iter()
        .position(|row| row.contains("call(a, b)"))
        .expect("the ghost's first line reads on from the caret");
    assert!(
        screen[row + 1].contains("next();"),
        "the ghost's further line is a row below"
    );
    let buffer = frame(&mut app, 80, 12);
    // A column, not a byte offset: the gutter may hold multi-byte glyphs.
    let byte = screen[row].find(", b").expect("ghost cells");
    let x = u16::try_from(screen[row][..byte].chars().count()).expect("column");
    let y = u16::try_from(row).expect("row");
    assert_eq!(
        buffer[(x, y)].fg,
        app.theme.role(ThemeRole::Muted).to_ratatui(),
        "ghost text is dimmed"
    );

    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "call(a, b)\nnext();\n");
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(1, 7));
    assert_eq!(
        accepts(&backend),
        [(copilot().key().to_owned(), "call(a, b)\nnext();".to_owned())]
    );
    assert!(app.inline_ghost().is_none());
}

/// Regression: a caret move types nothing, so Left then Tab still replaces
/// the `)` the item spans, and Right then Tab eats nothing after it.
#[test]
fn moving_the_caret_then_tab_replaces_only_what_the_item_spans() {
    let (backend, mut app, now) = ghost_app("call() + x\n", LineCol::new(0, 5));
    let id = type_and_ask(&mut app, &backend, 'a', now);
    answer(&mut app, id, vec![item("call(a, b)", 0, (0, 7))]);
    send_key(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(ghost_text(&app).as_deref(), Some("a, b)"));
    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(code_tab_text(&app), "call(a, b) + x\n");

    let (backend, mut app, now) = ghost_app("call() + x\n", LineCol::new(0, 5));
    let id = type_and_ask(&mut app, &backend, 'a', now);
    answer(&mut app, id, vec![item("call(a, b)", 0, (0, 7))]);
    send_key(&mut app, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(ghost_text(&app), None, "past the `)` nothing still applies");
    assert_eq!(code_tab_text(&app), "call(a) + x\n");
}

/// Without a ghost, Tab is Tab, Esc is Esc and Ctrl+Right moves by word.
#[test]
fn keys_keep_their_meaning_while_no_ghost_is_shown() {
    let (backend, mut app, _) = ghost_app("ab cd\n", LineCol::new(0, 0));
    send_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_ne!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 0));
    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_ne!(
        code_tab_text(&app),
        "ab cd\n",
        "Tab did not indent or insert"
    );
    assert!(accepts(&backend).is_empty());
}

/// Typing what the ghost proposes narrows it; typing anything else drops it
/// at once, without waiting for an answer.
#[test]
fn typing_narrows_the_ghost_and_diverging_drops_it() {
    let (backend, mut app, now) = ghost_app("let \n", LineCol::new(0, 4));
    let id = type_and_ask(&mut app, &backend, 'x', now);
    answer(&mut app, id, vec![item("let x = 1;", 0, (0, 5))]);
    assert_eq!(ghost_text(&app).as_deref(), Some(" = 1;"));
    type_char(&mut app, ' ');
    assert_eq!(ghost_text(&app).as_deref(), Some("= 1;"));
    type_char(&mut app, '!');
    app.probe_inline_completion(now);
    assert_eq!(ghost_text(&app), None);
}

/// Esc dismisses the ghost, and an answer still on its way for the ask
/// behind it is not shown when it lands; nor is an answer to an older ask.
#[test]
fn esc_dismisses_and_stale_answers_are_dropped() {
    let (backend, mut app, now) = ghost_app("fo\n", LineCol::new(0, 2));
    let first = type_and_ask(&mut app, &backend, 'o', now);
    let second = type_and_ask(&mut app, &backend, '(', now);
    answer(&mut app, first, vec![item("foo(bar)", 0, (0, 4))]);
    assert_eq!(
        ghost_text(&app),
        None,
        "an answer to an older ask was shown"
    );
    answer(&mut app, second, vec![item("foo(bar)", 0, (0, 4))]);
    assert_eq!(ghost_text(&app).as_deref(), Some("bar)"));

    send_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(ghost_text(&app), None);
    answer(&mut app, second, vec![item("foo(baz)", 0, (0, 4))]);
    assert_eq!(ghost_text(&app), None, "a dismissed ask's answer came back");
}

/// Ctrl+Right takes one word and keeps the rest on show, asking nothing that
/// could replace it; the last word is a whole accept.
#[test]
fn ctrl_right_accepts_a_word_at_a_time() {
    let (backend, mut app, now) = ghost_app("le\n", LineCol::new(0, 2));
    let id = type_and_ask(&mut app, &backend, 't', now);
    answer(&mut app, id, vec![item("let total = 1", 0, (0, 3))]);
    send_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(code_tab_text(&app), "let total\n");
    assert_eq!(ghost_text(&app).as_deref(), Some(" = 1"));
    app.probe_inline_completion(now + Duration::from_secs(5));
    assert_eq!(asks(&backend).len(), 1, "a partial accept asked again");
    assert!(accepts(&backend).is_empty());

    send_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(ghost_text(&app).as_deref(), Some(" 1"));
    send_key(&mut app, KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(code_tab_text(&app), "let total = 1\n");
    assert_eq!(
        accepts(&backend).len(),
        1,
        "the last word is a whole accept"
    );
}

/// Alt+] asks at once when nothing is shown -- as the user's own ask, so a
/// server may offer several -- and then cycles through what came back, as
/// Alt+[ does backwards.
#[test]
fn alt_brackets_ask_then_cycle() {
    let (backend, mut app, _) = ghost_app("x\n", LineCol::new(0, 1));
    send_key(&mut app, KeyCode::Char(']'), KeyModifiers::ALT);
    let (id, at, trigger) = *asks(&backend).last().expect("an ask");
    assert_eq!(
        (at, trigger),
        (LineCol::new(0, 1), InlineCompletionTrigger::Invoked)
    );
    answer(
        &mut app,
        id,
        vec![
            item("x1", 0, (0, 1)),
            item("y", 0, (0, 1)),
            item("x2", 0, (0, 1)),
        ],
    );
    assert_eq!(
        ghost_text(&app).as_deref(),
        Some("1"),
        "an item that no longer matches is never shown"
    );
    send_key(&mut app, KeyCode::Char(']'), KeyModifiers::ALT);
    assert_eq!(ghost_text(&app).as_deref(), Some("2"));
    send_key(&mut app, KeyCode::Char(']'), KeyModifiers::ALT);
    assert_eq!(ghost_text(&app).as_deref(), Some("1"));
    send_key(&mut app, KeyCode::Char('['), KeyModifiers::ALT);
    assert_eq!(ghost_text(&app).as_deref(), Some("2"));
    assert_eq!(asks(&backend).len(), 1, "cycling asked again");
}

/// A second caret, or a completion popup opening, hides the ghost: Tab is
/// then the popup's, and an accept could not say which caret it is for.
#[test]
fn a_second_caret_hides_the_ghost() {
    let (backend, mut app, now) = ghost_app("ab\nab\n", LineCol::new(0, 2));
    let id = type_and_ask(&mut app, &backend, 'c', now);
    answer(&mut app, id, vec![item("abcd", 0, (0, 3))]);
    assert!(app.inline_ghost().is_some());
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(0, 3), LineCol::new(1, 2)]);
    assert!(app.inline_ghost().is_none());
    send_key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(accepts(&backend).is_empty());
}

/// "Copilot: Sign In" asks Copilot's server; the device code it answers goes
/// on the clipboard and stays on screen, and a failure says why.
#[test]
fn copilot_sign_in_puts_the_device_code_on_the_clipboard_and_on_screen() {
    let (backend, mut app) = completion_app("x\n", LineCol::new(0, 0));
    app.clipboard = crate::clipboard::Clipboard::in_memory();
    app.dispatch(Command::CopilotSignIn);
    let asked = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter().any(|(_, command)| {
                matches!(command, SessionCommand::SignInLanguageServer { server }
                    if *server == copilot())
            })
        })
        .unwrap_or_default();
    assert!(asked, "the sign-in was not asked of Copilot's server");

    app.on_backend_event(
        None,
        SessionEvent::LanguageServerAccount {
            server: copilot(),
            outcome: karet_session::api::AccountOutcome::DeviceCode {
                user_code: "ABCD-EFGH".to_owned(),
                verification_uri: Some("https://github.com/login/device".to_owned()),
            },
        },
    );
    assert_eq!(app.clipboard.get().unwrap_or_default(), "ABCD-EFGH");
    let (severity, message) = last_report(&app).expect("a notification");
    assert_eq!(
        severity,
        Severity::Warning,
        "the code must stay until dismissed"
    );
    assert!(message.contains("ABCD-EFGH") && message.contains("github.com/login/device"));

    app.on_backend_event(
        None,
        SessionEvent::LanguageServerAccount {
            server: copilot(),
            outcome: karet_session::api::AccountOutcome::Failed {
                message: "not running".to_owned(),
            },
        },
    );
    assert_eq!(
        last_report(&app),
        Some((
            Severity::Error,
            "Copilot: sign-in failed: not running".to_owned()
        ))
    );
}

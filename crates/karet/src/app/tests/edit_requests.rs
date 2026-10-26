//! Start Linked Editing and on-type formatting through the session seam: what
//! goes out, and what an answer does to the carets and the text -- including
//! the answers that must do nothing because the text or the carets moved on.

use karet_core::Range;
use karet_core::TextEdit;

use super::support::*;
use crate::app::*;

const DOC: DocumentId = DocumentId(9);

/// The linked-editing requests a backend received, as `(id, position)`.
fn linked_asks(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::LinkedEditingRanges { position, .. } => Some((*id, *position)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The on-type requests a backend received, as `(id, position, ch)`.
fn on_type_asks(backend: &RecordingBackend) -> Vec<(RequestId, LineCol, char)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::FormatOnType { position, ch, .. } => {
                        Some((*id, *position, *ch))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn version(app: &App) -> u64 {
    match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => buffer.version(),
        _ => panic!("expected a code tab"),
    }
}

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

fn carets(app: &App) -> Vec<LineCol> {
    app.tabs[app.active]
        .editor
        .cursors()
        .selections
        .iter()
        .map(|selection| selection.head)
        .collect()
}

const HTML: &str = "<div>\n  x\n</div>\n";

fn answer_linked(app: &mut App, id: RequestId, ranges: Option<Vec<Range>>) {
    let version = version(app);
    app.on_backend_event(
        Some(id),
        SessionEvent::LinkedEditingRanges {
            doc: DOC,
            version,
            ranges,
        },
    );
}

#[test]
fn start_linked_editing_puts_a_caret_in_every_linked_range() {
    let (backend, mut app) = completion_app(HTML, LineCol::new(0, 2));
    app.dispatch(Command::StartLinkedEditing);
    let asked = linked_asks(&backend);
    assert_eq!(
        asked.iter().map(|(_, at)| *at).collect::<Vec<_>>(),
        [LineCol::new(0, 2)]
    );

    answer_linked(
        &mut app,
        asked[0].0,
        Some(vec![span(0, 1, 4), span(2, 2, 5)]),
    );

    assert_eq!(carets(&app), [LineCol::new(0, 2), LineCol::new(2, 3)]);
    assert_eq!(app.tabs[app.active].editor.cursors().primary, 0);
}

/// Falsifier: an answer for text that has changed since would put carets
/// into ranges that no longer hold the name.
#[test]
fn an_answer_for_text_that_moved_on_places_nothing() {
    let (backend, mut app) = completion_app(HTML, LineCol::new(0, 2));
    app.dispatch(Command::StartLinkedEditing);
    let (id, _) = linked_asks(&backend)[0];
    let stale = version(&app);
    app.dispatch(Command::InsertChar('v'));

    app.on_backend_event(
        Some(id),
        SessionEvent::LinkedEditingRanges {
            doc: DOC,
            version: stale,
            ranges: Some(vec![span(0, 1, 4), span(2, 2, 5)]),
        },
    );

    assert_eq!(carets(&app), [LineCol::new(0, 3)]);
}

#[test]
fn nothing_linked_says_so() {
    let (backend, mut app) = completion_app(HTML, LineCol::new(1, 2));
    app.dispatch(Command::StartLinkedEditing);
    let (id, _) = linked_asks(&backend)[0];

    answer_linked(&mut app, id, None);

    assert_eq!(carets(&app), [LineCol::new(1, 2)]);
    assert_eq!(
        last_message(&app).as_deref(),
        Some("linked editing: nothing is linked to the cursor")
    );
}

#[test]
fn a_refusal_already_said_why_so_the_empty_answer_adds_nothing() {
    let (backend, mut app) = completion_app(HTML, LineCol::new(0, 2));
    app.dispatch(Command::StartLinkedEditing);
    let (id, _) = linked_asks(&backend)[0];
    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: karet_session::LanguageServerId::RustAnalyzer,
            feature: karet_core::ServerFeature::LinkedEditingRange,
        },
    );

    answer_linked(&mut app, id, None);

    let message = last_message(&app).unwrap_or_default();
    assert!(
        message.ends_with("does not support linked editing"),
        "{message}"
    );
}

fn on_type_app(text: &str, caret: LineCol) -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = completion_app(text, caret);
    app.settings.editor.format_on_type = true;
    (backend, app)
}

fn answer_on_type(app: &mut App, id: RequestId, version: u64, edits: Vec<TextEdit>) {
    answer_triggers(app, id, version, (edits, Some(vec![';', '\n'])));
}

fn answer_triggers(
    app: &mut App,
    id: RequestId,
    version: u64,
    (edits, triggers): (Vec<TextEdit>, Option<Vec<char>>),
) {
    app.on_backend_event(
        Some(id),
        SessionEvent::OnTypeFormatting {
            doc: DOC,
            version,
            edits,
            triggers,
        },
    );
}

/// A server that formats on nothing is offered nothing more. Falsifier: an
/// empty list left unremembered sends every keystroke, and each one flushes
/// the debounced `didChange` ahead of its request.
#[test]
fn a_server_with_no_triggers_is_offered_nothing_more() {
    let (backend, mut app) = on_type_app("a\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar('b'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let version = version(&app);
    answer_triggers(&mut app, id, version, (Vec::new(), Some(Vec::new())));

    app.dispatch(Command::InsertChar(';'));
    app.dispatch(Command::InsertNewline);
    assert_eq!(on_type_asks(&backend).len(), 1, "nothing is a trigger");
}

/// Reload the settings with `edit` applied, keeping on-type formatting on.
fn reload_settings(app: &mut App, edit: impl FnOnce(&mut karet_session::config::Settings)) {
    let mut config = app.loaded_config.clone();
    config.settings.editor.format_on_type = true;
    edit(&mut config.settings);
    app.apply_loaded_config(config, false);
}

/// Changed `lsp` settings may move the document to a server that does format
/// on type, so a remembered empty list is dropped and the document is offered
/// typed characters again. Falsifier: kept until the tab closes, the list
/// leaves on-type formatting off for the document under its new server.
#[test]
fn an_lsp_settings_change_offers_typed_characters_again() {
    let (backend, mut app) = on_type_app("a\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar('b'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let version = version(&app);
    answer_triggers(&mut app, id, version, (Vec::new(), Some(Vec::new())));

    reload_settings(&mut app, |settings| {
        settings.lsp.enabled = !settings.lsp.enabled;
    });
    app.dispatch(Command::InsertChar(';'));
    let asked = on_type_asks(&backend);
    assert_eq!(
        asked.len(),
        2,
        "the new server has not said what it formats on"
    );
    assert_eq!(asked[1].2, ';');
}

/// A reload that leaves `lsp` alone leaves the servers as they were, so what
/// they said they format on still holds. Falsifier: dropped on every reload,
/// a server that formats on nothing is offered every keystroke again.
#[test]
fn a_reload_that_leaves_lsp_alone_keeps_the_triggers() {
    let (backend, mut app) = on_type_app("a\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar('b'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let version = version(&app);
    answer_triggers(&mut app, id, version, (Vec::new(), Some(Vec::new())));

    reload_settings(&mut app, |settings| settings.editor.tab_size += 1);
    app.dispatch(Command::InsertChar(';'));
    assert_eq!(on_type_asks(&backend).len(), 1, "nothing is a trigger");
}

/// An answer no server gave says nothing about the triggers, so the next
/// character is still offered: a server may yet start serving the file.
#[test]
fn an_unknown_trigger_list_keeps_offering() {
    let (backend, mut app) = on_type_app("a\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar('b'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let version = version(&app);
    answer_triggers(&mut app, id, version, (Vec::new(), None));

    app.dispatch(Command::InsertChar('c'));
    assert_eq!(on_type_asks(&backend).len(), 2);
}

#[test]
fn on_type_formatting_is_off_by_default() {
    let (backend, mut app) = completion_app("x\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar(';'));
    assert!(on_type_asks(&backend).is_empty());
}

#[test]
fn a_typed_trigger_is_offered_and_its_edits_are_applied() {
    let (backend, mut app) = on_type_app("let x = 1\n", LineCol::new(0, 9));
    app.dispatch(Command::InsertChar(';'));
    let asked = on_type_asks(&backend);
    assert_eq!(asked.len(), 1);
    let (id, at, ch) = asked[0];
    assert_eq!((at, ch), (LineCol::new(0, 10), ';'));

    let edit = TextEdit {
        range: span(0, 0, 0),
        new_text: "    ".to_owned(),
    };
    let version = version(&app);
    answer_on_type(&mut app, id, version, vec![edit]);

    assert_eq!(code_tab_text(&app), "    let x = 1;\n");
    assert_eq!(
        carets(&app),
        [LineCol::new(0, 14)],
        "the caret moves with the text"
    );
}

/// Once the server has named its triggers, nothing else is offered.
#[test]
fn after_the_first_answer_only_the_servers_triggers_are_offered() {
    let (backend, mut app) = on_type_app("a\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar('b'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let version = version(&app);
    answer_on_type(&mut app, id, version, Vec::new());

    app.dispatch(Command::InsertChar('c'));
    assert_eq!(on_type_asks(&backend).len(), 1, "`c` is not a trigger");
    app.dispatch(Command::InsertNewline);
    let asked = on_type_asks(&backend);
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].2, '\n');
}

/// Falsifier: applied against text the user has typed into since, the edit
/// would land in the wrong place -- and the session would refuse the
/// keystroke that followed.
#[test]
fn edits_for_text_typed_on_since_are_dropped() {
    let (backend, mut app) = on_type_app("x\n", LineCol::new(0, 1));
    app.dispatch(Command::InsertChar(';'));
    let (id, _, _) = on_type_asks(&backend)[0];
    let asked_at = version(&app);
    app.dispatch(Command::InsertChar(' '));

    let edit = TextEdit {
        range: span(0, 0, 0),
        new_text: "!".to_owned(),
    };
    answer_on_type(&mut app, id, asked_at, vec![edit]);

    assert_eq!(code_tab_text(&app), "x; \n");
}

#[test]
fn several_carets_offer_nothing() {
    let (backend, mut app) = on_type_app("a\nb\n", LineCol::new(0, 1));
    let active = app.active;
    app.tabs[active]
        .editor
        .set_carets(&[LineCol::new(0, 1), LineCol::new(1, 1)]);
    app.dispatch(Command::InsertChar(';'));
    assert!(on_type_asks(&backend).is_empty());
}

#[test]
fn toggle_format_on_type_flips_the_setting_for_the_session() {
    let (backend, mut app) = completion_app("x\n", LineCol::new(0, 1));
    app.dispatch(Command::ToggleFormatOnType);
    assert_eq!(last_message(&app).as_deref(), Some("format on type: on"));
    app.dispatch(Command::InsertChar(';'));
    assert_eq!(on_type_asks(&backend).len(), 1);

    app.dispatch(Command::ToggleFormatOnType);
    assert_eq!(last_message(&app).as_deref(), Some("format on type: off"));
    app.dispatch(Command::InsertChar(';'));
    assert_eq!(on_type_asks(&backend).len(), 1);
}

#[test]
fn the_palette_offers_both_commands() {
    for command in [Command::StartLinkedEditing, Command::ToggleFormatOnType] {
        assert_eq!(crate::command::resolve_named(command.label()), Ok(command));
    }
}

//! Code actions: Ctrl+. asks, the answer opens a grouped menu under the caret,
//! accepting a row runs it, the palette's source actions run the first fit,
//! and the gutter lightbulb follows a settled caret.

use karet_core::ApplyWorkspaceEditResult;
use karet_core::CodeAction;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_session::AppliedWorkspaceEdit;
use karet_session::CodeActionTrigger;
use karet_session::LSP_CHANGE_DEBOUNCE;
use karet_session::LanguageServerId;

use super::support::*;
use crate::app::*;

const SOURCE: &str = "use std::fmt;\nlet value = 1;\n";
const DOC: DocumentId = DocumentId(9);

/// Every `CodeActions` request a backend received.
fn listings(backend: &RecordingBackend) -> Vec<(RequestId, Range, Vec<String>, CodeActionTrigger)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::CodeActions {
                        range,
                        kinds,
                        trigger,
                        ..
                    } => Some((*id, *range, kinds.clone(), *trigger)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `ApplyCodeAction` a backend received, as `(id, version, title)`.
fn runs(backend: &RecordingBackend) -> Vec<(RequestId, u64, String)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::ApplyCodeAction {
                        version, action, ..
                    } => Some((*id, *version, action.title.clone())),
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

fn action(title: &str, kind: &str) -> CodeAction {
    CodeAction {
        title: title.to_owned(),
        kind: Some(kind.to_owned()),
        ..CodeAction::default()
    }
}

fn answer(app: &mut App, id: RequestId, actions: Vec<CodeAction>, reason: Option<&str>) {
    let version = version(app);
    app.on_backend_event(
        Some(id),
        SessionEvent::CodeActions {
            doc: DOC,
            version,
            actions,
            reason: reason.map(str::to_owned),
        },
    );
}

fn menu_labels(app: &App) -> Vec<String> {
    app.context_menu
        .as_ref()
        .map(|menu| {
            menu.entries()
                .iter()
                .map(|entry| entry.label.clone().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn ctrl_dot_lists_actions_at_the_caret_and_runs_the_one_picked() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(1, 5));
    send_key(&mut app, KeyCode::Char('.'), KeyModifiers::CONTROL);

    let [(id, range, ref kinds, trigger)] = listings(&backend)[..] else {
        panic!("one listing expected");
    };
    let caret = LineCol::new(1, 5);
    assert_eq!(
        range,
        Range {
            start: caret,
            end: caret
        }
    );
    assert!(kinds.is_empty());
    assert_eq!(trigger, CodeActionTrigger::Invoked);
    assert!(
        app.context_menu.is_none(),
        "nothing shows before the answer"
    );

    answer(
        &mut app,
        id,
        vec![
            action("Extract variable", "refactor.extract"),
            action("Remove unused import", "quickfix"),
        ],
        None,
    );
    assert_eq!(
        menu_labels(&app),
        [
            "Quick Fix",
            "Remove unused import",
            "Refactor",
            "Extract variable"
        ]
    );
    assert_eq!(app.input_context().modal, Some(Modal::ContextMenu));

    // The first row that can run is selected; Enter runs it.
    send_key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.context_menu.is_none());
    let version = version(&app);
    let [(run, at, ref title)] = runs(&backend)[..] else {
        panic!("one run expected");
    };
    assert_eq!((at, title.as_str()), (version, "Remove unused import"));

    // A failure is reported with the action's title; a success says nothing,
    // the change being on screen.
    app.on_backend_event(
        Some(run),
        SessionEvent::WorkspaceEdit {
            applied: AppliedWorkspaceEdit {
                result: ApplyWorkspaceEditResult::failed("the server refused", None),
                ..AppliedWorkspaceEdit::default()
            },
        },
    );
    assert_eq!(
        last_message(&app).as_deref(),
        Some("Remove unused import: the server refused")
    );
}

#[test]
fn alt_enter_opens_the_same_menu() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 0));
    send_key(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert_eq!(listings(&backend).len(), 1);
    assert_eq!(code_tab_text(&app), SOURCE, "no newline was inserted");
}

#[test]
fn an_empty_or_stale_answer_opens_no_menu() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 0));
    app.dispatch(Command::CodeActions);
    let id = listings(&backend)[0].0;
    answer(&mut app, id, Vec::new(), Some("nothing to fix"));
    assert!(app.context_menu.is_none());
    assert_eq!(
        last_message(&app).as_deref(),
        Some("code actions: nothing to fix")
    );

    // Typed since asking: the answer describes text that is gone.
    app.dispatch(Command::CodeActions);
    let id = listings(&backend)[1].0;
    let asked_at = version(&app);
    app.dispatch(Command::InsertChar('x'));
    app.on_backend_event(
        Some(id),
        SessionEvent::CodeActions {
            doc: DOC,
            version: asked_at,
            actions: vec![action("Fix", "quickfix")],
            reason: None,
        },
    );
    assert!(app.context_menu.is_none());
}

#[test]
fn a_server_without_code_actions_is_reported_once() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 0));
    app.dispatch(Command::CodeActions);
    let id = listings(&backend)[0].0;
    app.on_backend_event(
        Some(id),
        SessionEvent::FeatureUnsupported {
            server: LanguageServerId::RustAnalyzer,
            feature: ServerFeature::CodeAction,
        },
    );
    let notices = app.notifications.active().len();
    answer(&mut app, id, Vec::new(), Some("unsupported"));
    assert_eq!(app.notifications.active().len(), notices);
    assert!(
        last_message(&app).is_some_and(|message| message.contains("does not support code actions"))
    );
}

#[test]
fn organize_imports_runs_the_preferred_source_action() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 0));
    app.dispatch(Command::OrganizeImports);
    let (id, _, kinds, _) = listings(&backend)[0].clone();
    assert_eq!(kinds, ["source.organizeImports"]);

    let mut preferred = action("Organize imports (preferred)", "source.organizeImports");
    preferred.preferred = true;
    answer(
        &mut app,
        id,
        vec![
            action("Organize imports", "source.organizeImports"),
            preferred,
        ],
        None,
    );
    assert!(app.context_menu.is_none(), "no menu: it just runs");
    assert_eq!(runs(&backend)[0].2, "Organize imports (preferred)");

    // Nothing to run says so, under the command's name.
    app.dispatch(Command::FixAll);
    let (id, _, kinds, _) = listings(&backend)[1].clone();
    assert_eq!(kinds, ["source.fixAll"]);
    answer(&mut app, id, Vec::new(), None);
    assert_eq!(runs(&backend).len(), 1);
    assert_eq!(
        last_message(&app).as_deref(),
        Some("fix all: nothing to do here")
    );
}

#[test]
fn the_source_action_menu_asks_for_source_kinds_only() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 0));
    app.dispatch(Command::SourceAction);
    assert_eq!(listings(&backend)[0].2, ["source"]);
}

#[test]
fn the_lightbulb_waits_for_the_caret_to_settle_and_follows_its_line() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(0, 4));
    let start = Instant::now();
    app.probe_code_actions(start);
    assert!(listings(&backend).is_empty(), "not while the caret moves");
    assert_eq!(app.code_action_next_wake(start), Some(LSP_CHANGE_DEBOUNCE));

    let settled = start + LSP_CHANGE_DEBOUNCE;
    app.probe_code_actions(settled);
    let [(id, _, _, trigger)] = listings(&backend)[..] else {
        panic!("one probe expected");
    };
    assert_eq!(trigger, CodeActionTrigger::Automatic);
    assert_eq!(app.code_action_next_wake(settled), None, "sent once");
    app.probe_code_actions(settled + LSP_CHANGE_DEBOUNCE);
    assert_eq!(listings(&backend).len(), 1, "not re-sent while it stands");

    answer(
        &mut app,
        id,
        vec![action("Remove import", "quickfix")],
        None,
    );
    let bulb = app.code_action_bulb_decoration().expect("a bulb");
    assert_eq!(bulb.range.start.line, 0);
    assert!(app.context_menu.is_none(), "a probe opens nothing");

    // Along the same line the bulb stays until the next answer...
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(0, 6)]);
    app.probe_code_actions(settled);
    assert!(app.code_action_bulb_decoration().is_some());
    // ...but another line takes it down at once.
    app.tabs[app.active]
        .editor
        .set_carets(&[LineCol::new(1, 0)]);
    app.probe_code_actions(settled);
    assert!(app.code_action_bulb_decoration().is_none());

    // An answer with only disabled actions shows no bulb.
    app.probe_code_actions(settled + LSP_CHANGE_DEBOUNCE);
    let id = listings(&backend).last().expect("a probe").0;
    let mut disabled = action("Inline", "refactor.inline");
    disabled.disabled = Some("not here".into());
    answer(&mut app, id, vec![disabled], None);
    assert!(app.code_action_bulb_decoration().is_none());
}

#[test]
fn the_lightbulb_is_drawn_in_the_caret_lines_gutter() {
    let (backend, mut app) = completion_app(SOURCE, LineCol::new(1, 0));
    let start = Instant::now();
    app.probe_code_actions(start);
    app.probe_code_actions(start + LSP_CHANGE_DEBOUNCE);
    let id = listings(&backend)[0].0;
    answer(&mut app, id, vec![action("Fix", "quickfix")], None);

    let glyph = match app.icon_style {
        karet_filetype::IconStyle::NerdFont => "\u{f0eb}",
        karet_filetype::IconStyle::Unicode => "\u{2726}",
        karet_filetype::IconStyle::Ascii => "*",
    };
    let rows = screen(&mut app, 80, 20);
    let with_bulb: Vec<&String> = rows.iter().filter(|row| row.contains(glyph)).collect();
    assert_eq!(with_bulb.len(), 1, "{rows:#?}");
    assert!(
        with_bulb[0].contains("let value"),
        "on the caret line: {with_bulb:?}"
    );
}

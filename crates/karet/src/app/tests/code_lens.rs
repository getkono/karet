//! Code lenses: asked for over the viewport as hints are, drawn on rows of
//! their own above the lines they describe, carried with the text, and run by
//! a click on a title.

use std::sync::Arc;

use karet_core::ApplyWorkspaceEditResult;
use karet_core::CodeLens;
use karet_core::CommandId;
use karet_core::LineCol;
use karet_core::Range;

use super::support::*;
use crate::app::*;

const DOC: DocumentId = DocumentId(9);
const TEXT: &str = "use x;\n\n    fn main() {}\n";

/// The code-lens requests a backend received, as `(id, lines)`.
fn lens_requests(backend: &RecordingBackend) -> Vec<(RequestId, (u32, u32))> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::CodeLenses { lines, .. } => Some((*id, *lines)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The id of the newest code-lens request a backend received.
fn last_lens_request(backend: &RecordingBackend) -> RequestId {
    lens_requests(backend)
        .last()
        .map_or(RequestId(0), |(id, _)| *id)
}

/// The lens runs a backend received.
fn runs(backend: &RecordingBackend) -> Vec<CodeLens> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::RunCodeLens { lens, .. } => Some(lens.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn lens(line: u32, title: &str, command: Option<&str>) -> CodeLens {
    CodeLens {
        range: Range {
            start: LineCol::new(line, 4),
            end: LineCol::new(line, 11),
        },
        title: Some(title.to_owned()),
        command: command.map(|name| CommandId(name.to_owned())),
        payload: None,
    }
}

/// An app that asked for lenses once and was answered with `lenses`.
fn answered(lenses: Vec<CodeLens>) -> (Arc<RecordingBackend>, App) {
    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.request_code_lenses();
    let id = last_lens_request(&backend);
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.on_code_lenses(Some(id), DOC, version, lenses);
    (backend, app)
}

/// Where `needle` is drawn on `screen`, as `(column, row)`.
fn find(screen: &[String], needle: &str) -> Option<(u16, u16)> {
    screen.iter().enumerate().find_map(|(row, text)| {
        let byte = text.find(needle)?;
        let column = text[..byte].chars().count();
        Some((u16::try_from(column).ok()?, u16::try_from(row).ok()?))
    })
}

fn left_click(app: &mut App, (column, row): (u16, u16)) {
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn a_frame_asks_for_the_visible_lines_once_and_not_when_disabled() {
    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.request_code_lenses();
    app.request_code_lenses();
    let asked = lens_requests(&backend);
    assert_eq!(asked.len(), 1, "an identical request in flight re-asked");
    assert_eq!(asked.first().map(|(_, lines)| lines.0), Some(0));

    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.settings.editor.code_lens = false;
    app.request_code_lenses();
    assert!(lens_requests(&backend).is_empty());
}

/// The lenses on one line share a row above it, indented like it, and the
/// line itself moves down to make room.
#[test]
fn an_answer_is_drawn_on_a_row_above_its_line() {
    let lenses = vec![lens(2, "Run", Some("run")), lens(2, "Debug", Some("debug"))];
    let (backend, mut app) = answered(lenses);
    let screen = screen(&mut app, 80, 12);
    let title = find(&screen, "Run | Debug").unwrap_or_default();
    let code = find(&screen, "fn main").unwrap_or_default();
    assert_eq!(code.1, title.1 + 1, "the row sits right above its line");
    assert_eq!(title.0, code.0, "indented like the line it describes");
    // Covered now: the next frame asks for nothing.
    app.request_code_lenses();
    assert_eq!(lens_requests(&backend).len(), 1);
}

#[test]
fn an_answer_the_buffer_has_moved_past_is_dropped() {
    let (backend, mut app) = completion_app(TEXT, LineCol::new(0, 0));
    app.request_code_lenses();
    let id = last_lens_request(&backend);
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.submit_edit(|caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, "x"))
    });
    app.on_code_lenses(Some(id), DOC, version, vec![lens(2, "Run", None)]);
    assert!(app.code_lenses.held.get(&DOC).is_none_or(Vec::is_empty));
}

/// A failure keeps what is shown and waits before asking again.
#[test]
fn a_failure_keeps_the_lenses_shown_and_backs_off() {
    let (backend, mut app) = answered(vec![lens(2, "Run", None)]);
    // An edit makes the text new; once typing pauses it is asked again.
    app.submit_edit(|caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, "x"))
    });
    app.request_code_lenses();
    assert_eq!(lens_requests(&backend).len(), 1, "asked while still typing");
    let failed_at = Instant::now() + karet_session::LSP_CHANGE_DEBOUNCE * 2;
    app.request_code_lenses_at(failed_at);
    assert_eq!(
        lens_requests(&backend).len(),
        2,
        "the edited document was not re-asked"
    );
    let id = last_lens_request(&backend);
    let version = app.buffer_version(DOC).unwrap_or_default();
    app.on_code_lenses_failed_at(Some(id), DOC, version, failed_at);
    assert_eq!(app.code_lenses.held.get(&DOC).map(Vec::len), Some(1));
    app.request_code_lenses_at(failed_at);
    assert_eq!(lens_requests(&backend).len(), 2, "re-asked without waiting");
    assert!(app.code_lens_next_wake(failed_at).is_some());
    app.request_code_lenses_at(failed_at + Duration::from_secs(31));
    assert_eq!(lens_requests(&backend).len(), 3, "never re-asked");
}

/// A held lens moves with the text until its replacement arrives.
#[test]
fn an_edit_above_a_lens_carries_it_down() {
    let (_, mut app) = answered(vec![lens(2, "Run", None)]);
    app.submit_edit(|caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, "\n"))
    });
    let lines: Vec<u32> = app
        .code_lenses
        .held
        .get(&DOC)
        .into_iter()
        .flatten()
        .map(CodeLens::line)
        .collect();
    assert_eq!(lines, [3]);
}

/// A click on a title runs that lens; a click between titles, or on a
/// lens that only informs, runs nothing and moves no caret.
#[test]
fn a_click_on_a_title_runs_its_lens() {
    let lenses = vec![
        lens(2, "Run", Some("run")),
        lens(2, "Debug", Some("debug")),
        lens(2, "3 references", None),
    ];
    let (backend, mut app) = answered(lenses);
    let screen = screen(&mut app, 80, 12);
    let (x, y) = find(&screen, "Debug").unwrap_or_default();
    left_click(&mut app, (x + 1, y));
    let ran: Vec<Option<String>> = runs(&backend).into_iter().map(|l| l.title).collect();
    assert_eq!(ran, [Some("Debug".to_owned())]);

    let caret = app.tabs[app.active].editor.cursors().primary().head;
    let (x, y) = find(&screen, " | Debug").unwrap_or_default();
    left_click(&mut app, (x + 1, y));
    let (x, y) = find(&screen, "3 references").unwrap_or_default();
    left_click(&mut app, (x, y));
    assert_eq!(runs(&backend).len(), 1, "a separator or an inert lens ran");
    assert_eq!(app.tabs[app.active].editor.cursors().primary().head, caret);
}

/// A run that fails says so under the lens's title; one that works is quiet.
#[test]
fn a_failed_run_is_reported_under_its_title() {
    let (backend, mut app) = answered(vec![lens(2, "Run test", Some("run"))]);
    let screen = screen(&mut app, 80, 12);
    let at = find(&screen, "Run test").unwrap_or_default();
    left_click(&mut app, at);
    let id = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| {
            sent.iter()
                .rev()
                .find(|(_, command)| matches!(command, SessionCommand::RunCodeLens { .. }))
                .map(|(id, _)| *id)
        })
        .unwrap_or(RequestId(0));
    let applied = karet_session::AppliedWorkspaceEdit {
        label: Some("Run test".into()),
        result: ApplyWorkspaceEditResult::failed("the server does not run it", None),
        ..karet_session::AppliedWorkspaceEdit::default()
    };
    app.on_workspace_edit(Some(id), applied);
    assert_eq!(
        last_message(&app).as_deref(),
        Some("Run test: the server does not run it")
    );
}

/// A server whose runtime state changes may answer what it could not before,
/// so a covered document is asked about again.
#[test]
fn a_server_state_change_asks_a_covered_document_again() {
    let (backend, mut app) = answered(vec![lens(2, "Run", None)]);
    app.request_code_lenses();
    assert_eq!(
        lens_requests(&backend).len(),
        1,
        "a covered document re-asked"
    );
    app.on_backend_event(
        None,
        SessionEvent::LanguageServerRuntimeChanged {
            server: LanguageServerId::RustAnalyzer,
            root: PathBuf::from("/workspace"),
            state: karet_session::LanguageServerRuntimeState::Running,
            error: None,
        },
    );
    app.request_code_lenses();
    assert_eq!(
        lens_requests(&backend).len(),
        2,
        "the server's new state did not re-ask"
    );
}

/// Closing a document drops its lenses and its request in flight, so a late
/// answer to that request finds nothing to land on.
#[test]
fn closing_a_document_drops_its_lenses_and_pending_request() {
    let (backend, mut app) = answered(vec![lens(2, "Run", None)]);
    app.submit_edit(|caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, "x"))
    });
    app.request_code_lenses_at(Instant::now() + karet_session::LSP_CHANGE_DEBOUNCE * 2);
    assert_eq!(
        lens_requests(&backend).len(),
        2,
        "the edit was not re-asked"
    );
    let id = last_lens_request(&backend);
    let version = app.buffer_version(DOC).unwrap_or_default();

    app.on_backend_event(None, SessionEvent::Closed { doc: DOC });
    assert!(!app.code_lenses.held.contains_key(&DOC));

    app.on_code_lenses(Some(id), DOC, version, vec![lens(2, "Late", None)]);
    assert!(
        !app.code_lenses.held.contains_key(&DOC),
        "an answer to a request dropped on close was adopted"
    );
}

/// Turning `editor.codeLens` off clears what is shown at once.
#[test]
fn turning_lenses_off_clears_them() {
    let (_, mut app) = answered(vec![lens(2, "Run", None)]);
    app.settings.editor.code_lens = false;
    app.apply_code_lens_visibility();
    assert!(app.code_lenses.held.is_empty());
    let screen = screen(&mut app, 80, 12);
    assert_eq!(find(&screen, "Run"), None);
}

#[test]
fn rows_indent_like_their_line_with_tabs_at_tab_width() {
    let buffer = karet_text::TextBuffer::from_text("\t  fn f() {}\n");
    let rows = code_lens_rows(&[lens(0, "Run", None)], &buffer, 4);
    assert_eq!(
        rows.first().map(|row| row.line.text.as_str()),
        Some("      Run")
    );
}

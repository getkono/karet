//! Uncommitted-line gutter markers: request coalescing, answer adoption, the
//! `git.decorations` switch, and painting into the editor's marker lane.

use super::support::*;
use crate::app::*;

fn line_change_requests(backend: &RecordingBackend) -> Vec<(RequestId, DocumentId)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::LineChanges { doc } => Some((*id, *doc)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn added(line: u32) -> Decoration {
    Decoration {
        range: Range {
            start: LineCol::new(line, 0),
            end: LineCol::new(line, 0),
        },
        kind: DecorationKind::GutterMarker { glyph: '\u{258e}' },
        role: Some(ThemeRole::GutterAdded),
    }
}

/// An app on a recording backend with one code tab backed by document 9.
fn app_with_doc() -> (App, Arc<RecordingBackend>) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.git.decorations = true;
    app.push_tab(text_tab("main.rs", "zero\none\ntwo\n"));
    app.focus = Focus::Editor;
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(9));
    }
    (app, backend)
}

#[test]
fn edits_during_a_request_coalesce_into_one_follow_up() {
    let (mut app, backend) = app_with_doc();
    app.request_line_changes(DocumentId(9));
    app.request_line_changes(DocumentId(9));
    app.request_line_changes(DocumentId(9));
    let first = line_change_requests(&backend);
    assert_eq!(first.len(), 1, "one comparison in flight per document");

    app.on_backend_event(
        Some(first[0].0),
        SessionEvent::LineChanges {
            doc: DocumentId(9),
            version: 0,
            markers: vec![added(1)],
        },
    );
    assert_eq!(
        app.docs.line_changes.get(&DocumentId(9)),
        Some(&vec![added(1)])
    );
    let requests = line_change_requests(&backend);
    assert_eq!(
        requests.len(),
        2,
        "the edits made meanwhile are asked about once"
    );

    app.on_backend_event(
        Some(requests[1].0),
        SessionEvent::LineChanges {
            doc: DocumentId(9),
            version: 1,
            markers: Vec::new(),
        },
    );
    assert_eq!(
        line_change_requests(&backend).len(),
        2,
        "nothing left to re-ask"
    );
    assert_eq!(app.docs.line_changes.get(&DocumentId(9)), Some(&Vec::new()));
}

#[test]
fn an_answer_nobody_is_waiting_for_is_dropped() {
    let (mut app, backend) = app_with_doc();
    app.request_line_changes(DocumentId(9));
    let requests = line_change_requests(&backend);
    app.forget_line_changes(Some(DocumentId(9)));
    app.on_backend_event(
        Some(requests[0].0),
        SessionEvent::LineChanges {
            doc: DocumentId(9),
            version: 0,
            markers: vec![added(0)],
        },
    );
    assert!(
        app.docs.line_changes.is_empty(),
        "a closed document keeps nothing"
    );
}

#[test]
fn a_status_change_refreshes_every_open_document_once() {
    let (mut app, backend) = app_with_doc();
    app.push_tab(text_tab("lib.rs", "a\n"));
    if let TabKind::Code { doc, .. } = &mut app.tabs[app.active].kind {
        *doc = Some(DocumentId(4));
    }
    app.on_backend_event(
        None,
        SessionEvent::VcsStatus {
            staged: Vec::new(),
            working: Vec::new(),
        },
    );
    let mut docs: Vec<DocumentId> = line_change_requests(&backend)
        .into_iter()
        .map(|(_, doc)| doc)
        .collect();
    docs.sort();
    assert_eq!(docs, vec![DocumentId(4), DocumentId(9)]);
}

#[test]
fn turning_decorations_off_asks_for_and_paints_nothing() {
    let (mut app, backend) = app_with_doc();
    app.docs.line_changes.insert(DocumentId(9), vec![added(1)]);
    app.settings.git.decorations = false;
    app.request_line_changes(DocumentId(9));
    assert!(line_change_requests(&backend).is_empty());
    let rows = screen(&mut app, 60, 12);
    assert!(
        !rows.iter().any(|row| row.contains('\u{258e}')),
        "no marker is painted while the setting is off: {rows:#?}"
    );
}

#[test]
fn held_markers_paint_in_the_gutter_lane_in_their_role() {
    let (mut app, _backend) = app_with_doc();
    app.docs.line_changes.insert(DocumentId(9), vec![added(1)]);
    let buffer = frame(&mut app, 60, 12);
    let markers: Vec<(u16, u16)> = buffer
        .content()
        .iter()
        .enumerate()
        .filter(|(_, cell)| cell.symbol() == "\u{258e}")
        .map(|(index, _)| buffer.pos_of(index))
        .collect();
    assert_eq!(markers.len(), 1, "exactly the one changed line is marked");
    let (x, y) = markers[0];
    assert_eq!(
        buffer[(x, y)].fg,
        app.theme.role(ThemeRole::GutterAdded).to_ratatui()
    );
    // The marker sits on the row whose line number is 2 (zero-based line 1).
    let row: String = (x..x + 4).map(|col| buffer[(col, y)].symbol()).collect();
    assert!(row.contains('2'), "marker row reads {row:?}");
}

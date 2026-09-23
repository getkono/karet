//! Asking for inlay hints, and refusing to paint a stale answer.
//!
//! The request is issued once per frame, so the interesting behaviour is all
//! about *not* asking: an unchanged viewport, an identical request already in
//! flight, and an answer the buffer has moved past.

use std::sync::Arc;

use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::LineCol;

use super::support::*;
use crate::app::*;

/// The inlay-hint requests a backend received, as `(id, start line, end line)`.
fn inlay_requests(backend: &RecordingBackend) -> Vec<(RequestId, u32, u32)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::InlayHints { range, .. } => {
                        Some((*id, range.start.line, range.end.line))
                    },
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn hint(line: u32, col: u32, label: &str) -> InlayHint {
    InlayHint {
        position: LineCol::new(line, col),
        label: label.to_owned(),
        kind: InlayHintKind::Type,
        padding_left: false,
        padding_right: false,
    }
}

/// An app whose one code tab has been "painted" with `visible` rows on screen.
fn hinted_app(text: &str) -> (Arc<RecordingBackend>, App) {
    completion_app(text, LineCol::new(0, 0))
}

#[test]
fn a_frame_asks_for_the_visible_range_once() {
    let (backend, mut app) = hinted_app("let a = 1;\nlet b = 2;\n");
    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    assert_eq!(asked.len(), 1, "one request for the one code tab");

    // Nothing changed, so a second frame asks for nothing. Without the
    // coverage check this would be a round trip per frame, forever.
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        1,
        "an unchanged viewport re-asked"
    );
}

#[test]
fn a_disabled_setting_asks_for_nothing() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.settings.editor.inlay_hints.enabled = false;
    app.request_inlay_hints();
    assert!(inlay_requests(&backend).is_empty());
}

#[test]
fn an_answer_is_painted_and_stops_further_asking() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    let Some(&(id, ..)) = asked.first() else {
        unreachable!("a request was just issued");
    };

    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    let painted = app.docs.inlay_hints.get(&DocumentId(9));
    assert_eq!(painted.map(Vec::len), Some(1));

    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        1,
        "an answered range re-asked"
    );
}

#[test]
fn an_answer_to_a_superseded_request_is_dropped() {
    // Two requests can be outstanding across an edit. Painting the first
    // answer would annotate the new text at the old text's columns.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    let Some(&(id, ..)) = asked.first() else {
        unreachable!("a request was just issued");
    };

    // An answer carrying a version the request was not made against.
    app.on_inlay_hints(Some(id), DocumentId(9), 99, vec![hint(0, 5, ": i32")]);
    assert!(
        !app.docs.inlay_hints.contains_key(&DocumentId(9)),
        "a hint set for the wrong version was painted"
    );

    // And an answer tagged with a request id nobody is waiting on.
    app.on_inlay_hints(
        Some(RequestId(4242)),
        DocumentId(9),
        0,
        vec![hint(0, 5, "x")],
    );
    assert!(!app.docs.inlay_hints.contains_key(&DocumentId(9)));
}

#[test]
fn an_edit_re_asks_but_leaves_the_current_hints_on_screen() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);

    // The text moved. The hints are now positioned against the old revision.
    app.stale_inlay_hints(DocumentId(9));

    // They stay painted -- blanking them per keystroke would strobe --
    assert_eq!(
        app.docs.inlay_hints.get(&DocumentId(9)).map(Vec::len),
        Some(1),
        "hints were cleared mid-edit instead of being left in place"
    );
    // -- but the next frame asks for their replacement.
    app.request_inlay_hints();
    assert_eq!(inlay_requests(&backend).len(), 2, "an edit did not re-ask");
}

#[test]
fn closing_a_document_forgets_everything_about_it() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);

    app.forget_inlay_hints(DocumentId(9));
    assert!(!app.docs.inlay_hints.contains_key(&DocumentId(9)));
    assert!(!app.docs.inlay_covered.contains_key(&DocumentId(9)));
    assert!(!app.docs.inlay_pending.contains_key(&DocumentId(9)));
}

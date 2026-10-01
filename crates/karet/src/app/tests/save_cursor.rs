//! The caret a save reports to the backend. The backend restores it when the
//! save's own rewrite is undone, so it has to arrive before the save does, and
//! it has to come from a tab that shows the saved document.

use super::support::*;
use crate::app::*;

/// Push a code tab for `doc` with its caret at `caret`, returning its view.
fn caret_tab(app: &mut App, doc: u64, caret: LineCol) -> ViewId {
    app.push_tab(text_tab("t.rs", "one\ntwo\n"));
    let tab = &mut app.tabs[app.active];
    if let TabKind::Code { doc: d, .. } = &mut tab.kind {
        *d = Some(DocumentId(doc));
    }
    tab.editor.set_carets(&[caret]);
    tab.view
}

/// Whether the last two commands sent are `SetCursor` for `doc` from `view`
/// with its caret at `caret`, then `Save`.
fn reported_before_save(
    backend: &RecordingBackend,
    doc: u64,
    view: ViewId,
    caret: LineCol,
) -> bool {
    let sent = backend.sent.lock().map(|s| s.clone()).unwrap_or_default();
    let kinds: Vec<_> = sent.iter().map(|(_, c)| c).collect();
    matches!(
        kinds.as_slice(),
        [.., SessionCommand::SetCursor { doc: d, view: v, cursors }, SessionCommand::Save { .. }]
            if *d == DocumentId(doc) && *v == view && cursors.primary().head == caret
    )
}

#[test]
fn a_save_reports_the_caret_before_it_saves() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    let view = caret_tab(&mut app, 2, LineCol::new(1, 2));

    app.save_active();

    assert!(reported_before_save(&backend, 2, view, LineCol::new(1, 2)));
}

/// An auto-save of a document the focused tab does not show reports the caret
/// of a tab that does, not the focused tab's.
#[test]
fn a_save_of_a_background_document_reports_the_tab_showing_it() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.settings.files.auto_save = AutoSave::OnFocusChange;
    let background = caret_tab(&mut app, 2, LineCol::new(1, 2));
    caret_tab(&mut app, 3, LineCol::new(0, 1));
    app.focus = Focus::Sidebar;

    app.schedule_auto_save(DocumentId(2), 1, Instant::now());

    assert_eq!(saved_docs(&backend), [DocumentId(2)]);
    assert!(reported_before_save(
        &backend,
        2,
        background,
        LineCol::new(1, 2)
    ));
}

/// With two tabs on the saved document, the focused one's caret is reported
/// even though the other comes first in tab order.
#[test]
fn the_focused_tab_wins_over_another_tab_on_the_same_document() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    caret_tab(&mut app, 2, LineCol::new(0, 1));
    let focused = caret_tab(&mut app, 2, LineCol::new(1, 2));

    app.save_active();

    assert!(reported_before_save(
        &backend,
        2,
        focused,
        LineCol::new(1, 2)
    ));
}

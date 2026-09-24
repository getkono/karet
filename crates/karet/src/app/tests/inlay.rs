//! Asking for inlay hints, and refusing to paint a stale answer.
//!
//! The request is issued once per frame, so the interesting behaviour is all
//! about *not* asking: an unchanged viewport, an identical request already in
//! flight, a document still being typed in, a tab nobody can see, and an
//! answer the buffer has moved past.

use std::sync::Arc;
use std::time::Duration;

use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::LineCol;
use karet_session::LSP_CHANGE_DEBOUNCE;

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

/// The documents inlay hints were requested for, in order.
fn inlay_request_docs(backend: &RecordingBackend) -> Vec<DocumentId> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::InlayHints { doc, .. } => Some(*doc),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An app whose one code tab has been "painted" with `visible` rows on screen.
fn hinted_app(text: &str) -> (Arc<RecordingBackend>, App) {
    completion_app(text, LineCol::new(0, 0))
}

/// Type `text` at the caret, through the same path a keystroke takes.
fn type_text(app: &mut App, text: &str) {
    app.submit_edit(|caret, selection, _, base| {
        Some(karet_editor::editing::insert(caret, selection, base, text))
    });
}

/// A snapshot republishing the active tab's buffer exactly as it is.
fn snapshot_of(app: &App) -> DocSnapshot {
    let buffer = match &app.tabs[app.active].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => karet_text::TextBuffer::from_text(""),
    };
    DocSnapshot {
        version: buffer.version(),
        buffer,
        highlights: Arc::new(karet_syntax::Highlights::default()),
        semantic_blocks: Arc::new(karet_syntax::SemanticBlocks::default()),
        folds: Arc::new(FoldRegions::default()),
        decorations: Arc::new(Vec::new()),
        syntax_error_lines: Arc::new(Vec::new()),
        language: Some("Rust"),
        dirty: false,
        cursor: None,
    }
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
fn an_edit_waits_for_a_pause_then_re_asks_leaving_the_hints_on_screen() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);

    type_text(&mut app, "x");
    let now = Instant::now();

    // Mid-typing: nothing is asked -- every answer would be obsolete before
    // it arrived, and asking per keystroke is the load this debounce removes.
    app.request_inlay_hints_at(now);
    assert_eq!(inlay_requests(&backend).len(), 1, "asked while typing");
    // The hints stay painted rather than strobing off per keystroke.
    assert_eq!(
        app.docs.inlay_hints.get(&DocumentId(9)).map(Vec::len),
        Some(1),
        "hints were cleared mid-edit instead of being left in place"
    );
    // The loop is told when to wake, so the request needs no further input.
    let wake = app.inlay_next_wake(now).unwrap_or_default();
    assert!(
        !wake.is_zero() && wake <= LSP_CHANGE_DEBOUNCE,
        "no wake scheduled for the pause: {wake:?}"
    );

    let quiet = now + LSP_CHANGE_DEBOUNCE + Duration::from_millis(1);
    app.request_inlay_hints_at(quiet);
    assert_eq!(
        inlay_requests(&backend).len(),
        2,
        "the pause did not re-ask"
    );
    // And once asked, there is nothing left to wake for.
    assert_eq!(app.inlay_next_wake(quiet), None);
}

#[test]
fn a_snapshot_that_does_not_move_the_text_asks_for_nothing() {
    // A highlight pass republishes the same version. Treating every snapshot
    // as an edit re-asked the server after each one.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);

    app.on_snapshot(DocumentId(9), &snapshot_of(&app));
    assert!(
        app.docs.inlay_quiet_until.is_empty(),
        "a no-op snapshot debounced"
    );
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        1,
        "a no-op snapshot re-asked"
    );
}

#[test]
fn a_background_tab_is_not_asked_about_until_it_comes_forward() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    let mut other = text_tab("other.rs", "let b = 2;\n");
    if let TabKind::Code { doc, .. } = &mut other.kind {
        *doc = Some(DocumentId(10));
    }
    app.push_tab(other);

    app.request_inlay_hints();
    assert_eq!(
        inlay_request_docs(&backend),
        vec![DocumentId(10)],
        "only the front tab is painted, so only it is asked about"
    );

    // Brought forward, the first tab is asked about on the next frame.
    let first = app
        .tabs
        .iter()
        .position(|tab| {
            matches!(
                tab.kind,
                TabKind::Code {
                    doc: Some(DocumentId(9)),
                    ..
                }
            )
        })
        .unwrap_or_default();
    app.set_active(first);
    app.request_inlay_hints();
    assert_eq!(
        inlay_request_docs(&backend),
        vec![DocumentId(10), DocumentId(9)]
    );
}

#[test]
fn one_document_in_two_panes_is_asked_about_once_over_both_viewports() {
    let text = "let a = 1;\n".repeat(1000);
    let (backend, mut app) = hinted_app(&text);
    app.split_focused(karet_widgets::SplitDir::Right);
    // The new, focused pane scrolls far down; the stored one stays at the top.
    let active = app.active;
    app.tabs[active].editor.scroll_line = 500;

    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    assert_eq!(asked.len(), 1, "one document, one request: {asked:?}");
    // Top pane: lines 0..=65 (one guessed row plus 64 overscan). Bottom
    // pane: 436..=565. Their union is what the one cached set must cover,
    // or the annotations vanish from one pane whenever focus moves.
    assert_eq!((asked[0].1, asked[0].2), (0, 565));
}

#[test]
fn an_answer_the_buffer_has_moved_past_is_not_painted() {
    // The request is still the outstanding one and its version matches what
    // it asked, but the buffer was edited while the server was thinking: its
    // hints annotate text that is no longer there.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    type_text(&mut app, "x");

    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    assert!(!app.docs.inlay_hints.contains_key(&DocumentId(9)));
    assert!(!app.docs.inlay_covered.contains_key(&DocumentId(9)));
    assert!(
        !app.docs.inlay_pending.contains_key(&DocumentId(9)),
        "a settled request stayed outstanding"
    );
}

#[test]
fn a_request_lost_across_a_restart_does_not_suppress_the_re_ask() {
    // A settings reload restarts the server under a new manager generation,
    // and an answer from the old one is dropped by the session. The request
    // it was answering is then never answered -- and while it stayed
    // "in flight" it suppressed every re-ask, forever.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        1,
        "an in-flight request re-asked"
    );

    app.invalidate_inlay_coverage();
    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    assert_eq!(
        asked.len(),
        2,
        "a request from before the restart suppressed the re-ask"
    );

    // The lost request answering after all is superseded, not painted...
    let (old, new) = (asked[0].0, asked[1].0);
    app.on_inlay_hints(Some(old), DocumentId(9), 0, vec![hint(0, 5, "stale")]);
    assert!(!app.docs.inlay_hints.contains_key(&DocumentId(9)));
    // ...and the live one is adopted and covers the viewport.
    app.on_inlay_hints(Some(new), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    assert!(app.docs.inlay_covered.contains_key(&DocumentId(9)));
}

#[test]
fn an_answer_from_before_an_invalidation_is_painted_but_not_trusted() {
    // Startup is exactly this race: the first frame's request is answered
    // empty because no server is running yet. The answer is worth painting,
    // but it must not count as coverage, or it is never asked again.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.invalidate_inlay_coverage();

    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    assert_eq!(
        app.docs.inlay_hints.get(&DocumentId(9)).map(Vec::len),
        Some(1),
        "a real answer was thrown away"
    );
    assert!(!app.docs.inlay_covered.contains_key(&DocumentId(9)));
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        2,
        "the suspect answer was trusted"
    );
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

#[test]
fn a_servers_refresh_re_asks_for_a_covered_document() {
    // Editing `-> u32` to `-> u64` in another file changes the hints here
    // without changing this buffer, so the version-keyed coverage cannot see
    // it. The server's refresh is the only signal, and it has to re-ask.
    let (backend, mut app) = hinted_app("let a = f();\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": u32")]);
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        1,
        "covered, so not re-asked"
    );

    app.on_backend_event(
        None,
        SessionEvent::InlayHintsRefresh {
            server: karet_session::LanguageServerId::RustAnalyzer,
        },
    );
    // The stale set stays up until its replacement lands -- blanking it
    // would flash every annotation on screen -- but it is asked for again.
    assert_eq!(
        app.docs.inlay_hints.get(&DocumentId(9)).map(Vec::len),
        Some(1)
    );
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        2,
        "a refresh did not re-ask for a covered document"
    );
}

/// An app holding one answered hint at `(line, col)` of `text`, with the
/// caret at `caret`.
fn app_with_hint_at(text: &str, caret: LineCol, line: u32, col: u32) -> App {
    let (backend, mut app) = completion_app(text, caret);
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(line, col, ": u32")]);
    app
}

/// Where the held hints now sit.
fn held_positions(app: &App) -> Vec<LineCol> {
    app.docs
        .inlay_hints
        .get(&DocumentId(9))
        .map(|hints| hints.iter().map(|hint| hint.position).collect())
        .unwrap_or_default()
}

#[test]
fn lines_inserted_above_a_hint_carry_it_down() {
    let mut app = app_with_hint_at("fn f() {}\nlet a = f();\n", LineCol::new(0, 0), 1, 5);
    type_text(&mut app, "use x;\nuse y;\n");
    assert_eq!(held_positions(&app), vec![LineCol::new(3, 5)]);
}

#[test]
fn typing_before_a_hint_on_its_line_shifts_its_column() {
    let mut app = app_with_hint_at("let a = f();\n", LineCol::new(0, 4), 0, 5);
    type_text(&mut app, "mut ");
    assert_eq!(held_positions(&app), vec![LineCol::new(0, 9)]);
}

#[test]
fn deleting_the_text_around_a_hint_drops_it() {
    let mut app = app_with_hint_at("let a = f();\nlet b = 2;\n", LineCol::new(0, 0), 0, 5);
    // Select the whole first line and delete it.
    let active = app.active;
    let buffer = match &app.tabs[active].kind {
        TabKind::Code { buffer, .. } => buffer.clone(),
        _ => unreachable!("a code tab"),
    };
    app.tabs[active]
        .editor
        .set_selection(&buffer, LineCol::new(0, 0), LineCol::new(1, 0));
    type_text(&mut app, "");
    assert!(
        held_positions(&app).is_empty(),
        "a hint outlived the text it annotated"
    );
}

#[test]
fn a_change_seen_only_as_a_snapshot_carries_the_hints_too() {
    // An undo, a formatter, a reload: the app never saw the edits, only the
    // text after them. The hints still move with it.
    let mut app = app_with_hint_at("let a = f();\n", LineCol::new(0, 0), 0, 5);
    let mut snapshot = snapshot_of(&app);
    let inserted = karet_core::Change::new(
        snapshot.version,
        vec![karet_core::TextEdit {
            range: karet_core::Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 0),
            },
            new_text: "// note\n".to_owned(),
        }],
    );
    let applied = snapshot
        .buffer
        .apply(&inserted, karet_text::EditContext::default())
        .map(|applied| applied.version)
        .unwrap_or_default();
    snapshot.version = applied;

    app.on_snapshot(DocumentId(9), &snapshot);
    assert_eq!(held_positions(&app), vec![LineCol::new(1, 5)]);
}

#[test]
fn a_document_in_two_panes_is_carried_once_per_edit() {
    // The edit reaches the focused pane by its local apply and the other by
    // the snapshot echo. Shifting on both would move the hint twice.
    let mut app = app_with_hint_at("fn f() {}\nlet a = f();\n", LineCol::new(0, 0), 1, 5);
    app.split_focused(karet_widgets::SplitDir::Right);
    type_text(&mut app, "use x;\n");
    let echo = snapshot_of(&app);
    app.on_snapshot(DocumentId(9), &echo);
    assert_eq!(held_positions(&app), vec![LineCol::new(2, 5)]);
}

#[test]
fn a_failed_request_keeps_what_is_painted_and_asks_again_after_a_pause() {
    // rust-analyzer cancels a hint request whenever any file changes under it.
    // Adopting that as an empty set blanked an idle pane's hints, and its
    // coverage kept them blank until the buffer moved.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(first, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(first), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    app.invalidate_inlay_coverage();
    app.request_inlay_hints();
    let Some(&(second, ..)) = inlay_requests(&backend).get(1) else {
        unreachable!("an invalidated range is re-asked");
    };

    let now = Instant::now();
    app.on_inlay_hints_failed_at(Some(second), DocumentId(9), 0, now);
    assert_eq!(
        app.docs.inlay_hints.get(&DocumentId(9)).map(Vec::len),
        Some(1),
        "a failure cleared the painted hints"
    );
    app.request_inlay_hints_at(now);
    assert_eq!(
        inlay_requests(&backend).len(),
        2,
        "a failing server was re-asked without a pause"
    );
    assert!(
        app.inlay_next_wake(now).is_some(),
        "no wake scheduled to re-ask"
    );
    let quiet = now + LSP_CHANGE_DEBOUNCE + Duration::from_millis(1);
    app.request_inlay_hints_at(quiet);
    assert_eq!(
        inlay_requests(&backend).len(),
        3,
        "a failure counted as coverage"
    );
}

#[test]
fn a_failure_for_a_superseded_request_is_ignored() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    app.invalidate_inlay_coverage();
    app.request_inlay_hints();
    let asked = inlay_requests(&backend);
    let (Some(&(stale, ..)), Some(&(current, ..))) = (asked.first(), asked.get(1)) else {
        unreachable!("two requests were just issued");
    };

    let now = Instant::now();
    app.on_inlay_hints_failed_at(Some(stale), DocumentId(9), 0, now);
    assert!(
        app.docs
            .inlay_pending
            .get(&DocumentId(9))
            .is_some_and(|pending| pending.id == current),
        "a stale failure retired the current request"
    );
    assert_eq!(app.inlay_next_wake(now), None, "a stale failure backed off");
}

/// Reload the configuration with hints `enabled` or not, as the watcher does.
fn reload_with_hints(app: &mut App, enabled: bool) {
    let mut settings = app.settings.clone();
    settings.editor.inlay_hints.enabled = enabled;
    app.on_backend_event(
        None,
        SessionEvent::ConfigChanged {
            report: Box::new(karet_session::LoadedConfig::from_settings(settings)),
        },
    );
}

#[test]
fn turning_hints_off_clears_them_and_turning_them_on_asks_again() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);

    reload_with_hints(&mut app, false);
    assert!(
        app.docs.inlay_hints.is_empty(),
        "hints stayed painted after being turned off"
    );
    app.request_inlay_hints();
    assert_eq!(inlay_requests(&backend).len(), 1, "asked while turned off");

    reload_with_hints(&mut app, true);
    app.request_inlay_hints();
    assert_eq!(
        inlay_requests(&backend).len(),
        2,
        "turning hints back on did not ask for them"
    );
}

#[test]
fn an_answer_arriving_after_hints_are_turned_off_is_discarded() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    app.request_inlay_hints();
    let Some(&(id, ..)) = inlay_requests(&backend).first() else {
        unreachable!("a request was just issued");
    };
    reload_with_hints(&mut app, false);
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    assert!(
        app.docs.inlay_hints.is_empty(),
        "a late answer was painted with hints turned off"
    );
}

#[test]
fn the_retry_wait_doubles_per_failure_and_is_capped() {
    use crate::app::inlay::INLAY_RETRY_MAX;
    use crate::app::inlay::inlay_retry_delay;

    assert_eq!(inlay_retry_delay(1), LSP_CHANGE_DEBOUNCE);
    assert_eq!(inlay_retry_delay(2), LSP_CHANGE_DEBOUNCE * 2);
    assert_eq!(inlay_retry_delay(3), LSP_CHANGE_DEBOUNCE * 4);
    assert_eq!(inlay_retry_delay(50), INLAY_RETRY_MAX);
    assert_eq!(inlay_retry_delay(u32::MAX), INLAY_RETRY_MAX);
}

/// Ask, fail at `now`, and return the id of the request that failed.
fn ask_and_fail(backend: &RecordingBackend, app: &mut App, now: Instant) -> Option<RequestId> {
    app.request_inlay_hints_at(now);
    let &(id, ..) = inlay_requests(backend).last()?;
    app.on_inlay_hints_failed_at(Some(id), DocumentId(9), 0, now);
    Some(id)
}

#[test]
fn a_server_that_keeps_failing_is_asked_less_and_less_often() {
    // Re-asking at a fixed pause made a server that rejects every request
    // take ~7 requests a second, forever.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    let mut now = Instant::now();
    let mut waits = Vec::new();
    for _ in 0..10 {
        let _ = ask_and_fail(&backend, &mut app, now);
        let wait = app.inlay_next_wake(now).unwrap_or_default();
        waits.push(wait);
        now += wait + Duration::from_millis(1);
    }
    assert!(
        waits.windows(2).all(|pair| pair[1] >= pair[0]),
        "the wait shrank: {waits:?}"
    );
    assert_eq!(
        waits.last().copied(),
        Some(crate::app::inlay::INLAY_RETRY_MAX),
        "the wait never reached its cap: {waits:?}"
    );
    assert_eq!(inlay_requests(&backend).len(), 10, "one ask per wait");
}

#[test]
fn a_provider_change_ends_a_failure_back_off() {
    // A server back from a restart must not wait out a back-off earned while
    // it was away.
    let (backend, mut app) = hinted_app("let a = 1;\n");
    let now = Instant::now();
    for step in 0..5 {
        let _ = ask_and_fail(&backend, &mut app, now + INLAY_STEP * step);
    }
    let last = now + INLAY_STEP * 4;
    let asked = inlay_requests(&backend).len();
    // Still inside the back-off the last failure earned: nothing is asked.
    app.request_inlay_hints_at(last);
    assert_eq!(inlay_requests(&backend).len(), asked, "asked mid back-off");
    app.invalidate_inlay_coverage();
    app.request_inlay_hints_at(last);
    assert_eq!(
        inlay_requests(&backend).len(),
        asked + 1,
        "a provider change did not end the back-off"
    );
}

#[test]
fn an_answer_ends_a_failure_back_off() {
    let (backend, mut app) = hinted_app("let a = 1;\n");
    let now = Instant::now();
    for step in 0..5 {
        let _ = ask_and_fail(&backend, &mut app, now + INLAY_STEP * step);
    }
    assert!(app.docs.inlay_failures.contains_key(&DocumentId(9)));
    app.invalidate_inlay_coverage();
    app.request_inlay_hints_at(now + INLAY_STEP * 5);
    let Some(&(id, ..)) = inlay_requests(&backend).last() else {
        unreachable!("a request was just issued");
    };
    app.on_inlay_hints(Some(id), DocumentId(9), 0, vec![hint(0, 5, ": i32")]);
    assert_eq!(
        app.docs.inlay_failures.get(&DocumentId(9)),
        None,
        "an answer left the failure streak standing"
    );
}

/// Far enough apart that every failure's back-off has expired, so each step
/// asks.
const INLAY_STEP: Duration = Duration::from_secs(31);

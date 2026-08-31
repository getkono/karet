//! Debugger UI-glue tests: breakpoint toggling and full-replace sends,
//! F5 state routing, acknowledgement merging, and the status segment.

use std::sync::Arc;

use karet_session::DebugBreakpoint;
use karet_session::DebugSessionState;

use super::support::*;
use crate::app::*;

fn debug_commands(backend: &RecordingBackend) -> Vec<SessionCommand> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| {
                    matches!(
                        command,
                        SessionCommand::DebugStart { .. }
                            | SessionCommand::DebugStop
                            | SessionCommand::DebugContinue
                            | SessionCommand::DebugPause
                            | SessionCommand::DebugStepOver
                            | SessionCommand::DebugStepIn
                            | SessionCommand::DebugStepOut
                            | SessionCommand::DebugSetBreakpoints { .. }
                    )
                })
                .map(|(_, command)| command.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn toggling_breakpoints_sends_the_full_set_per_file() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.push_tab(text_tab("main.rs", "a\nb\nc\nd\n"));
    app.debug_toggle_breakpoint_at(PathBuf::from("main.rs"), 1);
    app.debug_toggle_breakpoint_at(PathBuf::from("main.rs"), 3);
    app.debug_toggle_breakpoint_at(PathBuf::from("main.rs"), 1);
    let sent = debug_commands(&backend);
    assert_eq!(sent.len(), 3);
    assert!(matches!(
        &sent[1],
        SessionCommand::DebugSetBreakpoints { path, lines }
            if path == &PathBuf::from("main.rs") && lines == &[1, 3]
    ));
    assert!(matches!(
        &sent[2],
        SessionCommand::DebugSetBreakpoints { path, lines }
            if path == &PathBuf::from("main.rs") && lines == &[3]
    ));
}

#[test]
fn f5_starts_when_idle_and_continues_when_stopped() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.debug_start_or_continue();
    app.debug_state = DebugSessionState::Stopped;
    app.debug_start_or_continue();
    app.debug_state = DebugSessionState::Running;
    app.debug_start_or_continue();
    let sent = debug_commands(&backend);
    assert_eq!(sent.len(), 2);
    assert!(matches!(
        sent[0],
        SessionCommand::DebugStart {
            configuration: None
        }
    ));
    assert!(matches!(sent[1], SessionCommand::DebugContinue));
}

#[test]
fn steps_are_gated_on_the_stopped_state() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.debug_step(SessionCommand::DebugStepOver);
    assert!(debug_commands(&backend).is_empty());
    app.debug_state = DebugSessionState::Stopped;
    app.debug_step(SessionCommand::DebugStepOver);
    let sent = debug_commands(&backend);
    assert_eq!(sent.len(), 1);
    assert!(matches!(sent[0], SessionCommand::DebugStepOver));
}

#[test]
fn acknowledgements_replace_and_late_verifications_merge() {
    let mut app = app();
    let path = PathBuf::from("main.rs");
    app.debug_toggle_breakpoint_at(path.clone(), 2);
    app.debug_toggle_breakpoint_at(path.clone(), 5);
    // The adapter's full answer verifies both.
    app.on_debug_breakpoints(
        path.clone(),
        &[
            DebugBreakpoint {
                line: 2,
                verified: true,
            },
            DebugBreakpoint {
                line: 5,
                verified: false,
            },
        ],
    );
    // A late single-entry verification merges by line.
    app.on_debug_breakpoints(
        path.clone(),
        &[DebugBreakpoint {
            line: 5,
            verified: true,
        }],
    );
    let file = app.breakpoints.get(&path).cloned().unwrap_or_default();
    assert_eq!(file.get(&2), Some(&true));
    assert_eq!(file.get(&5), Some(&true));
    assert_eq!(file.len(), 2);
}

#[test]
fn the_status_segment_tracks_the_lifecycle() {
    let mut app = app();
    assert_eq!(app.debug_status_segment(), None);
    app.on_debug_state(DebugSessionState::Starting, "Run".to_owned());
    assert_eq!(app.debug_status_segment(), Some("⏳ debug".to_owned()));
    app.on_debug_state(DebugSessionState::Stopped, "breakpoint".to_owned());
    assert_eq!(app.debug_status_segment(), Some("⏸ breakpoint".to_owned()));
    app.on_debug_state(DebugSessionState::Idle, String::new());
    assert_eq!(app.debug_status_segment(), None);
}

#[test]
fn debug_output_buffers_and_caps() {
    let mut app = app();
    app.on_debug_output("stdout".to_owned(), "one\ntwo\n".to_owned());
    assert_eq!(app.debug_output.len(), 2);
    for _ in 0..600 {
        app.on_debug_output("stdout".to_owned(), "spam\n".to_owned());
    }
    assert_eq!(app.debug_output.len(), 500);
}

fn frame(id: i64, name: &str, line: u32) -> karet_session::DebugFrame {
    karet_session::DebugFrame {
        id,
        name: name.to_owned(),
        line,
        column: 0,
        path: Some(PathBuf::from("/w/main.rs")),
    }
}

#[test]
fn the_inspection_waterfall_is_lazy_and_stale_answers_drop() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    // A stop populates: the panel clears and the stack is requested.
    app.on_debug_state(DebugSessionState::Stopped, "breakpoint".to_owned());
    app.on_debug_stopped("breakpoint", Some(PathBuf::from("/w/main.rs")), Some(3));
    let stack_id = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| {
            sent.iter()
                .find(|(_, c)| matches!(c, SessionCommand::DebugStackTrace))
                .map(|(id, _)| *id)
        })
        .unwrap_or(RequestId(0));
    // The stack answer auto-selects the top frame and requests its scopes.
    app.on_debug_stack(
        Some(stack_id),
        vec![frame(41, "main", 3), frame(42, "callee", 9)],
    );
    assert_eq!(app.debug_panel.selected_frame, Some(41));
    let scopes_id = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| {
            sent.iter()
                .find(|(_, c)| matches!(c, SessionCommand::DebugScopes { frame: 41 }))
                .map(|(id, _)| *id)
        })
        .unwrap_or(RequestId(0));
    // Scopes auto-expand the first cheap scope and fetch its variables.
    app.on_debug_scopes(
        Some(scopes_id),
        41,
        vec![
            karet_session::DebugScope {
                name: "Registers".to_owned(),
                reference: 7,
                expensive: true,
            },
            karet_session::DebugScope {
                name: "Locals".to_owned(),
                reference: 11,
                expensive: false,
            },
        ],
    );
    assert!(app.debug_panel.expanded.contains(&11));
    assert!(
        !app.debug_panel.expanded.contains(&7),
        "expensive stays lazy"
    );
    let vars_id = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| {
            sent.iter()
                .find(|(_, c)| matches!(c, SessionCommand::DebugVariables { reference: 11 }))
                .map(|(id, _)| *id)
        })
        .unwrap_or(RequestId(0));
    // A stale answer (not in pending) is dropped whole.
    app.on_debug_variables(
        Some(RequestId(9999)),
        11,
        vec![karet_session::DebugVariable {
            name: "stale".to_owned(),
            value: "x".to_owned(),
            ty: None,
            reference: 0,
        }],
    );
    assert!(app.debug_panel.variables.is_empty());
    app.on_debug_variables(
        Some(vars_id),
        11,
        vec![karet_session::DebugVariable {
            name: "answer".to_owned(),
            value: "42".to_owned(),
            ty: None,
            reference: 0,
        }],
    );
    // The tree flattens: Locals expanded shows its child row.
    assert!(app.debug_panel.rows.iter().any(|row| matches!(
        row,
        crate::app::DebugRow::Variable {
            parent: 11,
            index: 0,
            depth: 1
        }
    )));
    // Resuming clears every per-stop artifact.
    app.on_debug_state(DebugSessionState::Running, String::new());
    assert!(app.debug_panel.stack.is_empty());
    assert!(app.debug_panel.variables.is_empty());
    assert!(app.debug_panel.pending.is_empty());
    assert_eq!(app.debug_stopped, None);
}

#[test]
fn evaluate_logs_the_expression_and_its_answer() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.debug_state = DebugSessionState::Stopped;
    app.debug_evaluate("1 + 41".to_owned());
    let id = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| {
            sent.iter()
                .find(|(_, c)| matches!(c, SessionCommand::DebugEvaluate { .. }))
                .map(|(id, _)| *id)
        })
        .unwrap_or(RequestId(0));
    app.on_debug_evaluated(Some(id), "42".to_owned());
    assert_eq!(
        app.debug_panel.repl,
        vec!["› 1 + 41".to_owned(), "  = 42".to_owned()]
    );
    assert!(
        app.debug_panel
            .rows
            .iter()
            .any(|row| matches!(row, crate::app::DebugRow::Repl(_)))
    );
}

#[test]
fn rebuild_rows_sections_and_console_tail() {
    let mut panel = crate::app::DebugPanel::default();
    panel.rebuild_rows(0);
    assert!(matches!(
        panel.rows[0],
        crate::app::DebugRow::Section("CALL STACK")
    ));
    assert!(
        panel
            .rows
            .iter()
            .any(|row| matches!(row, crate::app::DebugRow::Note("not stopped")))
    );
    // A long console shows only the tail.
    panel.rebuild_rows(250);
    let outputs: Vec<usize> = panel
        .rows
        .iter()
        .filter_map(|row| match row {
            crate::app::DebugRow::Output(index) => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 100);
    assert_eq!(outputs.first(), Some(&150));
    assert_eq!(outputs.last(), Some(&249));
}

#[test]
fn ending_a_debug_session_the_user_asked_to_end_is_not_an_error() {
    // `stop` and a debuggee exiting are the two normal ends, and the backend
    // words them exactly this way. Tiering them as failures left a permanent red
    // card behind every ordinary debug run.
    for detail in ["stopped", "session ended"] {
        let mut ended = app();
        ended.on_debug_state(DebugSessionState::Idle, detail.to_string());
        let (severity, title) = last_report(&ended).expect("the end is reported");
        assert_eq!(severity, Severity::Hint, "{title}");
        assert!(
            ended
                .notifications
                .active()
                .iter()
                .all(|note| note.timeout.is_some()),
            "an ordinary end leaves nothing to dismiss"
        );
    }

    // A session that could not start still reports as a failure that waits.
    let mut failed = app();
    failed.on_debug_state(
        DebugSessionState::Idle,
        "no adapter for this language".to_string(),
    );
    let (severity, title) = last_report(&failed).expect("the failure is reported");
    assert_eq!(severity, Severity::Error, "{title}");
    assert!(failed.notifications.active()[0].timeout.is_none());
}

#[test]
fn stepping_through_a_debuggee_keeps_one_card_rather_than_one_per_line() {
    // `DebugStopped` fires with reason "step" for every stepped line, so an
    // untagged card would stack one per keypress and saturate the stack.
    let mut app = app();
    for _ in 0..6 {
        app.on_debug_stopped("step", None, None);
    }
    let stops = app
        .notifications
        .active()
        .iter()
        .filter(|note| note.title == "stopped: step")
        .count();
    assert_eq!(stops, 1, "the card is rewritten in place, not restacked");
}

#[test]
fn saving_many_documents_leaves_one_card_rather_than_one_per_file() {
    // Save-all writes one document per event, and auto-save fires on a timer
    // while the user types; both would otherwise stack identical cards.
    let mut app = app();
    for _ in 0..5 {
        app.notify_tagged(
            Report::Outcome,
            NotificationKind::Io,
            "saved",
            Some(App::SAVED_TAG.to_string()),
        );
    }
    let saved = app
        .notifications
        .active()
        .iter()
        .filter(|note| note.title == "saved")
        .count();
    assert_eq!(saved, 1);
}

#[test]
fn a_notebook_cell_failure_is_not_painted_as_a_success() {
    // The event carries no severity, so the tier is read off the producer's
    // wording — a run that failed must not come out teal like one that worked.
    let mut failed = app();
    failed.on_backend_event(
        None,
        SessionEvent::NotebookKernelStatus {
            path: PathBuf::from("nb.ipynb"),
            text: "cell failed: boom".to_string(),
        },
    );
    let (severity, title) = last_report(&failed).expect("the failure is reported");
    assert_eq!(severity, Severity::Error, "{title}");

    // Progress under the same feed stays transient and collapses to one card.
    let mut running = app();
    for n in 1..=4 {
        running.on_backend_event(
            None,
            SessionEvent::NotebookKernelStatus {
                path: PathBuf::from("nb.ipynb"),
                text: format!("running cell {n}/4"),
            },
        );
    }
    assert_eq!(running.notifications.active().len(), 1);
}

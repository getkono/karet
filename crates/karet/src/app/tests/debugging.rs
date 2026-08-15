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

use karet_core::Location;
use karet_core::Range;

use super::support::*;
use crate::app::*;

/// The definition requests a backend received, as `(id, position)`.
fn definition_requests(backend: &RecordingBackend) -> Vec<(RequestId, LineCol)> {
    backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(id, command)| match command {
                    SessionCommand::Definition { position, .. } => Some((*id, *position)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn at(line: u32, col: u32) -> Range {
    Range {
        start: LineCol::new(line, col),
        end: LineCol::new(line, col + 3),
    }
}

/// A workspace with `main.rs` open and `other.rs` on disk beside it.
fn workspace(name: &str) -> Option<(Arc<RecordingBackend>, App, PathBuf)> {
    let root = test_dir(name);
    write_file(&root, "other.rs", b"pub fn target() {}\nsecond line\n");
    let (backend, mut app) = completion_app("target();\n", LineCol::new(0, 1));
    app.root = root.clone();
    Some((backend, app, root.join("other.rs")))
}

#[test]
fn the_command_requests_the_definition_at_the_caret() {
    let (backend, mut app) = completion_app("let x = target();\n", LineCol::new(0, 9));
    app.dispatch(Command::GoToDefinition);

    let requests = definition_requests(&backend);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1, LineCol::new(0, 9));
}

#[test]
fn the_command_without_a_code_file_explains_itself_and_sends_nothing() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.dispatch(Command::GoToDefinition);

    assert!(definition_requests(&backend).is_empty());
    assert_eq!(
        app.status.as_deref(),
        Some("go to definition: open a code file first")
    );
}

#[test]
fn a_single_location_opens_the_file_and_lands_on_the_range_start() {
    let Some((backend, mut app, other)) = workspace("definition-single") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(0, 7),
            }],
        },
    );

    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
    // The caret lands on the definition's start, as a bare caret — the end of the
    // range is deliberately not selected.
    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 7));
    assert!(app.tabs[app.active].editor.selection_range().is_none());
}

#[test]
fn an_empty_answer_reports_why_and_leaves_the_caret_alone() {
    let (backend, mut app) = completion_app("target();\n", LineCol::new(0, 2));
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    app.on_backend_event(Some(id), SessionEvent::Definitions { locations: vec![] });

    assert_eq!(app.tabs[app.active].editor.cursor(), LineCol::new(0, 2));
    // No server is attached in a test app, so the status says so rather than
    // claiming the symbol has no definition.
    assert_eq!(
        app.status.as_deref(),
        Some("no language server for this file")
    );
}

#[test]
fn a_superseded_answer_is_ignored_and_the_live_request_still_lands() {
    let Some((backend, mut app, other)) = workspace("definition-superseded") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    app.dispatch(Command::GoToDefinition);
    let requests = definition_requests(&backend);
    assert_eq!(requests.len(), 2);

    // The first request answers late. `Event::Definitions` carries no document or
    // version, so the id is the only thing distinguishing it — and dropping the
    // pending record here would strand the live request forever.
    app.on_backend_event(
        Some(requests[0].0),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );
    assert_ne!(app.tabs[app.active].path(), Some(other.as_path()));

    app.on_backend_event(
        Some(requests[1].0),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(1, 0),
            }],
        },
    );
    assert_eq!(app.tabs[app.active].path(), Some(other.as_path()));
}

#[test]
fn an_answer_for_a_view_the_user_left_is_dropped() {
    let Some((backend, mut app, other)) = workspace("definition-stale-view") else {
        return;
    };
    app.dispatch(Command::GoToDefinition);
    let id = definition_requests(&backend)[0].0;

    // Move to a different tab while the request is in flight.
    app.push_tab(text_tab("elsewhere.rs", "unrelated\n"));
    app.on_backend_event(
        Some(id),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: other.clone(),
                range: at(0, 7),
            }],
        },
    );

    assert_eq!(
        app.tabs[app.active].path().and_then(|p| p.file_name()),
        Some(std::ffi::OsStr::new("elsewhere.rs")),
        "a jump must not yank the user out of the tab they moved to"
    );
}

#[test]
fn an_unsolicited_definition_event_is_ignored() {
    let (_backend, mut app) = completion_app("target();\n", LineCol::new(0, 1));
    let before = app.active;

    app.on_backend_event(
        Some(RequestId(77)),
        SessionEvent::Definitions {
            locations: vec![Location {
                path: PathBuf::from("/nowhere.rs"),
                range: at(0, 0),
            }],
        },
    );

    assert_eq!(app.active, before);
}

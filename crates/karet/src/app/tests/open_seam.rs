//! Opening a file entirely through the backend.
//!
//! These cover the shell's half of the contract: a tab is reserved from the
//! path's name, the backend is asked what the file really is, and the answer
//! either confirms the guess or corrects it. Nothing here touches the filesystem,
//! which is the property that lets the shell render a workspace on another
//! machine.

use karet_fileview::viewer::FileKind;
use karet_session::api::FileChunk;
use karet_session::api::PathClass;

use super::support::*;
use crate::app::*;

/// An app with a recording backend, rooted at a path that does not exist here.
///
/// Deliberately absent: if any of this reached the filesystem, these tests would
/// be testing the wrong machine.
fn remote_app() -> (Arc<RecordingBackend>, App) {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = App::new(
        PathBuf::from("/not/on/this/machine"),
        Vec::new(),
        Vec::new(),
        false,
    );
    app.backend = Some(backend.clone());
    (backend, app)
}

/// The commands `backend` recorded, paired with their request ids.
fn sent(backend: &RecordingBackend) -> Vec<(RequestId, SessionCommand)> {
    backend
        .sent
        .lock()
        .map(|sent| sent.clone())
        .unwrap_or_default()
}

fn find_request(
    backend: &RecordingBackend,
    matches: impl Fn(&SessionCommand) -> bool,
) -> Option<RequestId> {
    sent(backend)
        .into_iter()
        .find(|(_, command)| matches(command))
        .map(|(id, _)| id)
}

#[test]
fn opening_a_source_file_reserves_a_tab_and_registers_the_document() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/main.rs");

    app.open_path(&path);
    app.register_open_tabs();

    assert!(matches!(app.tabs[app.active].kind, TabKind::Code { .. }));
    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::OpenDocument { path: asked, .. } if asked == &path
        ))
        .is_some(),
        "the document is registered with the backend, not read from disk"
    );
}

/// Media is reserved from its extension and then confirmed: only the machine
/// holding the file can read its magic bytes or its length.
#[test]
fn opening_media_asks_the_backend_what_it_really_is() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");

    app.open_path(&path);
    app.register_open_tabs();

    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::ClassifyPath { path: asked, .. } if asked == &path
        ))
        .is_some(),
        "only the machine holding the file can classify its bytes"
    );
}

/// A text-shaped name needs no classification round trip: opening the document
/// answers the same question, and answers `NotUtf8` when the bytes disagree.
#[test]
fn opening_a_text_shaped_file_costs_no_classification_round_trip() {
    let (backend, mut app) = remote_app();

    app.open_path(Path::new("/not/on/this/machine/blob.bin"));
    app.register_open_tabs();

    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::ClassifyPath { .. }
        ))
        .is_none()
    );
}

/// Attaching a backend re-walks every open tab, and a user can reopen a file
/// already loading. Asking twice would double the traffic for no answer.
#[test]
fn a_second_registration_does_not_ask_again() {
    let (backend, mut app) = remote_app();
    app.open_path(Path::new("/not/on/this/machine/logo.png"));

    app.register_open_tabs();
    app.register_open_tabs();

    let asks = sent(&backend)
        .iter()
        .filter(|(_, command)| matches!(command, SessionCommand::ClassifyPath { .. }))
        .count();
    assert_eq!(asks, 1);
}

#[test]
fn a_classified_binary_file_has_its_bytes_requested_and_rendered() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");
    app.open_path(&path);
    app.register_open_tabs();
    let Some(classify) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ClassifyPath { .. })
    }) else {
        return;
    };

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 4,
                head: vec![0, 1, 2, 3],
            }),
        },
    );
    let Some(read) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ReadFileBytes { .. })
    }) else {
        return;
    };
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path: path.clone(),
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0, 1, 2, 3],
                total_len: 4,
            }),
        },
    );

    let TabKind::Hex { bytes, .. } = &app.tabs[app.active].kind else {
        return;
    };
    assert_eq!(bytes, &vec![0, 1, 2, 3]);
}

/// A file arriving in pieces must assemble, not restart: a large image or PDF
/// crosses a connection in chunks and every one of them counts.
#[test]
fn a_chunked_file_assembles_across_reads() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/logo.png");
    app.open_path(&path);
    app.register_open_tabs();
    let Some(classify) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ClassifyPath { .. })
    }) else {
        return;
    };
    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 6,
                head: vec![1, 2, 3],
            }),
        },
    );

    // Two chunks: the first short of the end, the second completing the file.
    for (offset, bytes) in [(0_u64, vec![1_u8, 2, 3]), (3, vec![4, 5, 6])] {
        let Some(read) = sent(&backend)
            .into_iter()
            .rev()
            .find(|(_, command)| matches!(command, SessionCommand::ReadFileBytes { .. }))
            .map(|(id, _)| id)
        else {
            return;
        };
        app.on_backend_event(
            Some(read),
            SessionEvent::FileBytes {
                path: path.clone(),
                result: Ok(FileChunk {
                    offset,
                    bytes,
                    total_len: 6,
                }),
            },
        );
    }

    let TabKind::Hex { bytes, .. } = &app.tabs[app.active].kind else {
        return;
    };
    assert_eq!(bytes, &vec![1, 2, 3, 4, 5, 6]);
}

/// An extension can lie. The backend sees the bytes, so its verdict re-routes the
/// tab to the renderer the content actually warrants.
#[test]
fn a_lying_extension_is_corrected_by_the_backends_verdict() {
    let (backend, mut app) = remote_app();
    // Named like a PDF, but the bytes say otherwise.
    let path = PathBuf::from("/not/on/this/machine/report.pdf");
    app.open_path(&path);
    app.register_open_tabs();
    let Some(classify) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ClassifyPath { .. })
    }) else {
        return;
    };

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path: path.clone(),
            result: Ok(PathClass {
                kind: FileKind::Binary,
                len: 2,
                head: vec![0, 1],
            }),
        },
    );
    let Some(read) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ReadFileBytes { .. })
    }) else {
        return;
    };
    app.on_backend_event(
        Some(read),
        SessionEvent::FileBytes {
            path,
            result: Ok(FileChunk {
                offset: 0,
                bytes: vec![0, 1],
                total_len: 2,
            }),
        },
    );

    assert!(matches!(app.tabs[app.active].kind, TabKind::Hex { .. }));
}

/// An unreadable file must leave the placeholder the open reserved, not wedge the
/// tab or crash the shell.
#[test]
fn an_unreadable_file_keeps_its_placeholder() {
    let (backend, mut app) = remote_app();
    let path = PathBuf::from("/not/on/this/machine/gone.png");
    app.open_path(&path);
    app.register_open_tabs();
    let Some(classify) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ClassifyPath { .. })
    }) else {
        return;
    };

    app.on_backend_event(
        Some(classify),
        SessionEvent::PathClassified {
            path,
            result: Err("No such file or directory".to_owned()),
        },
    );

    assert!(matches!(
        app.tabs[app.active].kind,
        TabKind::Hex { .. } | TabKind::Placeholder { .. }
    ));
}

#[test]
fn quick_open_shows_its_picker_before_the_file_list_arrives() {
    let (backend, mut app) = remote_app();

    app.open_quick_open();

    assert!(app.overlay.is_some(), "the picker opens at once");
    assert!(
        find_request(&backend, |command| matches!(
            command,
            SessionCommand::ListFiles { .. }
        ))
        .is_some(),
        "the workspace walk runs on the machine holding the files"
    );
}

#[test]
fn the_file_list_fills_the_open_picker_with_workspace_relative_rows() {
    let (backend, mut app) = remote_app();
    app.open_quick_open();
    let Some(request) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ListFiles { .. })
    }) else {
        return;
    };

    app.on_backend_event(
        Some(request),
        SessionEvent::FilesListed {
            files: vec![
                PathBuf::from("/not/on/this/machine/src/main.rs"),
                PathBuf::from("/not/on/this/machine/README.md"),
            ],
            truncated: false,
        },
    );

    let Some(overlay) = app.overlay.as_ref() else {
        return;
    };
    assert_eq!(overlay.rows(), vec!["src/main.rs", "README.md"]);
}

/// A stale answer must not repopulate a picker the user has already moved past —
/// the same rule every other request in the shell follows.
#[test]
fn a_stale_file_list_is_ignored() {
    let (backend, mut app) = remote_app();
    app.open_quick_open();
    let Some(request) = find_request(&backend, |command| {
        matches!(command, SessionCommand::ListFiles { .. })
    }) else {
        return;
    };
    // The user reopened quick-open, superseding the first request.
    app.open_quick_open();

    app.on_backend_event(
        Some(request),
        SessionEvent::FilesListed {
            files: vec![PathBuf::from("/not/on/this/machine/stale.rs")],
            truncated: false,
        },
    );

    let Some(overlay) = app.overlay.as_ref() else {
        return;
    };
    assert!(overlay.rows().is_empty(), "{:?}", overlay.rows());
}

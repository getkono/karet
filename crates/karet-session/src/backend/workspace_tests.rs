//! The workspace-filesystem seam's `Command`/`Event` contract, driven through a
//! real backend.
//!
//! [`fs_worker`](crate::fs_worker) already covers the filesystem behaviour
//! directly. What is tested here is the part only a whole backend can show: that
//! a command reaches the worker, its answer carries the submitting request id,
//! and it comes back on the ordinary event stream.

use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::PathMutation;
use crate::api::ViewId;
use crate::session::Session;
use crate::session::SessionConfig;

/// Submit `command` and return the first event tagged with its request id.
///
/// Startup producers (VCS status, language-server inventory) publish unsolicited
/// events, so correlating by id is the only reliable way to find an answer.
async fn answer(backend: &LocalBackend, events: &mut EventRx, command: Command) -> Option<Event> {
    let id = backend.next_id();
    backend.send(id, command).ok()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id == Some(id) {
                return Some(event);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// A session rooted at a scratch workspace containing `files`.
fn workspace(files: &[(&str, &[u8])]) -> Option<(tempfile::TempDir, LocalBackend, EventRx)> {
    let dir = tempfile::tempdir().ok()?;
    for (name, bytes) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, bytes).ok()?;
    }
    let config = SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    };
    let (session, events, _snaps) = Session::new(config);
    Some((dir, local_session(session, None), events))
}

/// Open `path` in `session` and return the id the `Opened` event announced.
///
/// The document id is minted by the session, so a test that needs one has to
/// read it off the stream rather than guess.
fn open_document(session: &mut Session, events: &mut EventRx, path: PathBuf) -> Option<DocumentId> {
    session.handle(
        RequestId(1),
        Command::OpenDocument {
            path,
            language: None,
        },
    );
    while let Some((_, event)) = events.try_recv() {
        if let Event::Opened { doc, .. } = event {
            return Some(doc);
        }
    }
    None
}

#[tokio::test]
async fn classifying_a_path_answers_the_submitting_request() {
    let Some((dir, backend, mut events)) = workspace(&[("main.rs", b"fn main() {}\n")]) else {
        return;
    };

    let event = answer(
        &backend,
        &mut events,
        Command::ClassifyPath {
            path: dir.path().join("main.rs"),
            ignore_size: false,
        },
    )
    .await;

    let Some(Event::PathClassified { result, .. }) = event else {
        return;
    };
    assert_eq!(
        result.ok().map(|class| class.kind),
        Some(karet_filetype::FileKind::Text)
    );
}

#[tokio::test]
async fn reading_bytes_answers_with_a_chunk() {
    let Some((dir, backend, mut events)) = workspace(&[("data.bin", &[1, 2, 3, 4])]) else {
        return;
    };

    let event = answer(
        &backend,
        &mut events,
        Command::ReadFileBytes {
            path: dir.path().join("data.bin"),
            offset: 0,
            len: 4,
        },
    )
    .await;

    let Some(Event::FileBytes { result, .. }) = event else {
        return;
    };
    let Ok(chunk) = result else {
        return;
    };
    assert_eq!(chunk.bytes, vec![1, 2, 3, 4]);
    assert!(chunk.is_final());
}

#[tokio::test]
async fn listing_files_walks_the_session_root_without_being_told_it() {
    let Some((_dir, backend, mut events)) = workspace(&[("a.rs", b"x"), ("b.rs", b"x")]) else {
        return;
    };

    let event = answer(&backend, &mut events, Command::ListFiles { limit: 100 }).await;

    let Some(Event::FilesListed { files, truncated }) = event else {
        return;
    };
    assert_eq!(files.len(), 2);
    assert!(!truncated);
}

/// A session with no root cannot walk anything, but a client still has an
/// outstanding request. It must be answered — emptily — rather than dropped.
#[tokio::test]
async fn listing_files_without_a_root_still_answers() {
    let (session, mut events, _snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);

    let event = answer(&backend, &mut events, Command::ListFiles { limit: 100 }).await;

    let Some(Event::FilesListed { files, truncated }) = event else {
        return;
    };
    assert!(files.is_empty());
    assert!(!truncated);
}

#[tokio::test]
async fn reading_a_directory_answers_with_its_children() {
    let Some((dir, backend, mut events)) = workspace(&[("src/lib.rs", b"x"), ("top.rs", b"x")])
    else {
        return;
    };

    let event = answer(
        &backend,
        &mut events,
        Command::ReadDirectory {
            path: dir.path().to_path_buf(),
            show_hidden: false,
            respect_gitignore: false,
        },
    )
    .await;

    let Some(Event::DirectoryListed { result, .. }) = event else {
        return;
    };
    let Ok(entries) = result else {
        return;
    };
    let labels: Vec<String> = entries
        .iter()
        .map(|entry| entry.label().to_owned())
        .collect();
    assert_eq!(labels, ["src", "top.rs"]);
}

/// The mutation rides back on the answer so a client can refresh the right
/// directories without keeping its own request-to-path map.
#[tokio::test]
async fn a_mutation_is_echoed_back_with_its_result() {
    let Some((dir, backend, mut events)) = workspace(&[]) else {
        return;
    };
    let path = dir.path().join("created.rs");

    let event = answer(
        &backend,
        &mut events,
        Command::MutatePath {
            mutation: PathMutation::CreateFile { path: path.clone() },
        },
    )
    .await;

    let Some(Event::PathMutated { mutation, result }) = event else {
        return;
    };
    assert_eq!(result, Ok(()));
    assert_eq!(mutation.target(), &path);
    assert!(path.is_file());
}

#[tokio::test]
async fn a_failed_mutation_reports_why_rather_than_going_silent() {
    let Some((dir, backend, mut events)) = workspace(&[]) else {
        return;
    };

    let event = answer(
        &backend,
        &mut events,
        Command::MutatePath {
            mutation: PathMutation::Delete {
                path: dir.path().join("never-existed.rs"),
            },
        },
    )
    .await;

    let Some(Event::PathMutated { result, .. }) = event else {
        return;
    };
    assert!(result.is_err());
}

/// `SetViewport` is fire-and-forget: it must not answer, and above all must not
/// wedge a session when it names a document that was never opened.
#[tokio::test]
async fn setting_a_viewport_for_an_unknown_document_is_harmless() {
    let Some((_dir, backend, mut events)) = workspace(&[("a.rs", b"x")]) else {
        return;
    };
    let id = backend.next_id();
    assert!(
        backend
            .send(
                id,
                Command::SetViewport {
                    doc: DocumentId(9999),
                    view: ViewId(1),
                    first_line: 0,
                    last_line: 40,
                },
            )
            .is_ok()
    );

    // The session must still serve the next command.
    let event = answer(&backend, &mut events, Command::ListFiles { limit: 8 }).await;

    assert!(
        matches!(event, Some(Event::FilesListed { .. })),
        "{event:?}"
    );
}

/// View state is opaque durability, not interpretation: whatever bytes a client
/// checkpoints come back byte-for-byte, including none at all.
#[test]
fn checkpointed_view_state_is_stored_verbatim() {
    let (mut session, _events, _snaps) = Session::new(SessionConfig::default());

    assert!(session.view_state().is_empty());
    session.handle(
        RequestId(1),
        Command::CheckpointViewState {
            blob: b"\x00\xffnot utf-8".to_vec(),
        },
    );

    assert_eq!(session.view_state(), b"\x00\xffnot utf-8");
}

#[test]
fn a_later_checkpoint_replaces_the_earlier_one() {
    let (mut session, _events, _snaps) = Session::new(SessionConfig::default());

    session.handle(
        RequestId(1),
        Command::CheckpointViewState {
            blob: b"first".to_vec(),
        },
    );
    session.handle(
        RequestId(2),
        Command::CheckpointViewState {
            blob: b"second".to_vec(),
        },
    );

    assert_eq!(session.view_state(), b"second");
}

#[test]
fn an_undeclared_viewport_means_whole_document_highlights() {
    let (session, _events, _snaps) = Session::new(SessionConfig::default());

    assert_eq!(session.viewport_lines(DocumentId(1)), None);
}

/// Two views of one document scroll independently; the backend must cover the
/// union, or whichever view lost the race renders unhighlighted.
#[test]
fn two_views_of_one_document_widen_the_range_to_their_union() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path: PathBuf = dir.path().join("wide.rs");
    if std::fs::write(&path, "fn main() {}\n").is_err() {
        return;
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };

    session.handle(
        RequestId(2),
        Command::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 1_000,
            last_line: 1_040,
        },
    );
    session.handle(
        RequestId(3),
        Command::SetViewport {
            doc,
            view: ViewId(2),
            first_line: 2_000,
            last_line: 2_040,
        },
    );

    let margin = crate::session::VIEWPORT_MARGIN;
    assert_eq!(
        session.viewport_lines(doc),
        Some((1_000 - margin, 2_040 + margin))
    );
}

/// A viewport at the top of a file would underflow its margin. Saturating is the
/// difference between "start at line 0" and a panic.
#[test]
fn a_viewport_at_the_top_of_a_file_clamps_its_margin() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("top.rs");
    if std::fs::write(&path, "fn main() {}\n").is_err() {
        return;
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };

    session.handle(
        RequestId(2),
        Command::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 0,
            last_line: 40,
        },
    );

    assert_eq!(session.viewport_lines(doc).map(|lines| lines.0), Some(0));
}

/// Declaring a viewport must republish: the spans the client holds were scoped to
/// the *previous* window, so without a fresh snapshot the newly revealed lines
/// stay unhighlighted until something else happens to touch the document.
#[test]
fn declaring_a_new_viewport_republishes_the_document() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("scroll.rs");
    if std::fs::write(&path, "fn main() {}\n".repeat(500)).is_err() {
        return;
    }
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };
    while snaps.try_recv().is_some() {}

    session.handle(
        RequestId(2),
        Command::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 0,
            last_line: 40,
        },
    );

    assert!(
        snaps.try_recv().is_some(),
        "a new viewport should publish a snapshot scoped to it"
    );
}

/// A client sends its viewport on every scroll, and most scrolls land inside the
/// margin already covered. Republishing each time would spend a frame — and, on a
/// connection, a payload — to say nothing new.
#[test]
fn re_declaring_an_unchanged_viewport_does_not_republish() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("scroll.rs");
    if std::fs::write(&path, "fn main() {}\n".repeat(500)).is_err() {
        return;
    }
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };
    let viewport = Command::SetViewport {
        doc,
        view: ViewId(1),
        first_line: 10,
        last_line: 50,
    };
    session.handle(RequestId(2), viewport.clone());
    while snaps.try_recv().is_some() {}

    session.handle(RequestId(3), viewport);

    assert!(snaps.try_recv().is_none());
}

/// A viewport reversed by a drag-select upward must still describe a range, not an
/// empty one — otherwise selecting backwards blanks the highlighting.
#[test]
fn a_reversed_viewport_is_normalized_rather_than_empty() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("reversed.rs");
    if std::fs::write(&path, "fn main() {}\n").is_err() {
        return;
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };

    session.handle(
        RequestId(2),
        Command::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 900,
            last_line: 400,
        },
    );

    let margin = crate::session::VIEWPORT_MARGIN;
    assert_eq!(
        session.viewport_lines(doc),
        Some((400 - margin, 900 + margin))
    );
}

/// Closing a document must drop its viewports, or the next document to reuse that
/// id inherits a window nobody declared for it.
#[test]
fn closing_a_document_forgets_its_viewport() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("closed.rs");
    if std::fs::write(&path, "fn main() {}\n").is_err() {
        return;
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let Some(doc) = open_document(&mut session, &mut events, path) else {
        return;
    };
    session.handle(
        RequestId(2),
        Command::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 0,
            last_line: 40,
        },
    );
    assert!(session.viewport_lines(doc).is_some());

    session.handle(RequestId(3), Command::CloseDocument { doc });

    assert_eq!(session.viewport_lines(doc), None);
}

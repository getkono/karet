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
use crate::api::DocumentCommand;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::PathMutation;
use crate::api::ViewId;
use crate::api::WorkspaceCommand;
use crate::session::Session;
use crate::session::SessionConfig;

/// Submit `command` and return the first event tagged with its request id.
///
/// Startup producers (VCS status, language-server inventory) publish unsolicited
/// events, so correlating by id is the only reliable way to find an answer.
async fn answer(
    backend: &LocalBackend,
    events: &mut EventRx,
    command: impl Into<Command>,
) -> Option<Event> {
    let id = backend.next_id();
    backend.send(id, command.into()).ok()?;
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

/// What every test returns: a missing setup or answer fails it through `?`.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Open `path` in `session` and return the id the `Opened` event announced.
///
/// The document id is minted by the session, so a test that needs one has to
/// read it off the stream rather than guess.
fn open_document(session: &mut Session, events: &mut EventRx, path: PathBuf) -> Option<DocumentId> {
    session.handle(
        RequestId(1),
        DocumentCommand::Open {
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
async fn classifying_a_path_answers_the_submitting_request() -> TestResult {
    let (dir, backend, mut events) =
        workspace(&[("main.rs", b"fn main() {}\n")]).ok_or("a scratch workspace")?;

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ClassifyPath {
            path: dir.path().join("main.rs"),
            ignore_size: false,
        },
    )
    .await;

    let Some(Event::PathClassified { result, .. }) = event else {
        return Err(format!("expected PathClassified, got {event:?}").into());
    };
    assert_eq!(
        result.ok().map(|class| class.kind),
        Some(karet_filetype::FileKind::Text)
    );
    Ok(())
}

#[tokio::test]
async fn reading_bytes_answers_with_a_chunk() -> TestResult {
    let (dir, backend, mut events) =
        workspace(&[("data.bin", &[1, 2, 3, 4])]).ok_or("a scratch workspace")?;

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ReadFileBytes {
            path: dir.path().join("data.bin"),
            offset: 0,
            len: 4,
        },
    )
    .await;

    let Some(Event::FileBytes { result, .. }) = event else {
        return Err(format!("expected FileBytes, got {event:?}").into());
    };
    let chunk = result?;
    assert_eq!(chunk.bytes, vec![1, 2, 3, 4]);
    assert!(chunk.is_final());
    Ok(())
}

#[tokio::test]
async fn listing_files_walks_the_session_root_without_being_told_it() -> TestResult {
    let (_dir, backend, mut events) =
        workspace(&[("a.rs", b"x"), ("b.rs", b"x")]).ok_or("a scratch workspace")?;

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ListFiles { limit: 100 },
    )
    .await;

    let Some(Event::FilesListed { files, truncated }) = event else {
        return Err(format!("expected FilesListed, got {event:?}").into());
    };
    assert_eq!(files.len(), 2);
    assert!(!truncated);
    Ok(())
}

/// A session with no root cannot walk anything, but a client still has an
/// outstanding request. It must be answered — emptily — rather than dropped.
#[tokio::test]
async fn listing_files_without_a_root_still_answers() -> TestResult {
    let (session, mut events, _snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ListFiles { limit: 100 },
    )
    .await;

    let Some(Event::FilesListed { files, truncated }) = event else {
        return Err(format!("expected FilesListed, got {event:?}").into());
    };
    assert!(files.is_empty());
    assert!(!truncated);
    Ok(())
}

#[tokio::test]
async fn reading_a_directory_answers_with_its_children() -> TestResult {
    let (dir, backend, mut events) =
        workspace(&[("src/lib.rs", b"x"), ("top.rs", b"x")]).ok_or("a scratch workspace")?;

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ReadDirectory {
            path: dir.path().to_path_buf(),
            show_hidden: false,
            respect_gitignore: false,
        },
    )
    .await;

    let Some(Event::DirectoryListed { result, .. }) = event else {
        return Err(format!("expected DirectoryListed, got {event:?}").into());
    };
    let entries = result?;
    let labels: Vec<String> = entries
        .iter()
        .map(|entry| entry.label().to_owned())
        .collect();
    assert_eq!(labels, ["src", "top.rs"]);
    Ok(())
}

/// The mutation rides back on the answer so a client can refresh the right
/// directories without keeping its own request-to-path map.
#[tokio::test]
async fn a_mutation_is_echoed_back_with_its_result() -> TestResult {
    let (dir, backend, mut events) = workspace(&[]).ok_or("a scratch workspace")?;
    let path = dir.path().join("created.rs");

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::MutatePath {
            mutation: PathMutation::CreateFile { path: path.clone() },
        },
    )
    .await;

    let Some(Event::PathMutated { mutation, result }) = event else {
        return Err(format!("expected PathMutated, got {event:?}").into());
    };
    assert_eq!(result, Ok(()));
    assert_eq!(mutation.target(), &path);
    assert!(path.is_file());
    Ok(())
}

#[tokio::test]
async fn a_failed_mutation_reports_why_rather_than_going_silent() -> TestResult {
    let (dir, backend, mut events) = workspace(&[]).ok_or("a scratch workspace")?;

    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::MutatePath {
            mutation: PathMutation::Delete {
                path: dir.path().join("never-existed.rs"),
            },
        },
    )
    .await;

    let Some(Event::PathMutated { result, .. }) = event else {
        return Err(format!("expected PathMutated, got {event:?}").into());
    };
    assert!(result.is_err());
    Ok(())
}

/// `SetViewport` is fire-and-forget: it must not answer, and above all must not
/// wedge a session when it names a document that was never opened.
#[tokio::test]
async fn setting_a_viewport_for_an_unknown_document_is_harmless() -> TestResult {
    let (_dir, backend, mut events) = workspace(&[("a.rs", b"x")]).ok_or("a scratch workspace")?;
    let id = backend.next_id();
    assert!(
        backend
            .send(
                id,
                DocumentCommand::SetViewport {
                    doc: DocumentId(9999),
                    view: ViewId(1),
                    first_line: 0,
                    last_line: 40,
                }
                .into(),
            )
            .is_ok()
    );

    // The session must still serve the next command.
    let event = answer(
        &backend,
        &mut events,
        WorkspaceCommand::ListFiles { limit: 8 },
    )
    .await;

    assert!(
        matches!(event, Some(Event::FilesListed { .. })),
        "{event:?}"
    );
    Ok(())
}

/// View state is opaque durability, not interpretation: whatever bytes a client
/// checkpoints come back byte-for-byte, including none at all.
#[test]
fn checkpointed_view_state_is_stored_verbatim() {
    let (mut session, _events, _snaps) = Session::new(SessionConfig::default());

    assert!(session.view_state().is_empty());
    session.handle(
        RequestId(1),
        WorkspaceCommand::CheckpointViewState {
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
        WorkspaceCommand::CheckpointViewState {
            blob: b"first".to_vec(),
        },
    );
    session.handle(
        RequestId(2),
        WorkspaceCommand::CheckpointViewState {
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
fn two_views_of_one_document_widen_the_range_to_their_union() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path: PathBuf = dir.path().join("wide.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;

    session.handle(
        RequestId(2),
        DocumentCommand::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 1_000,
            last_line: 1_040,
        },
    );
    session.handle(
        RequestId(3),
        DocumentCommand::SetViewport {
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
    Ok(())
}

/// A viewport at the top of a file would underflow its margin. Saturating is the
/// difference between "start at line 0" and a panic.
#[test]
fn a_viewport_at_the_top_of_a_file_clamps_its_margin() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("top.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;

    session.handle(
        RequestId(2),
        DocumentCommand::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 0,
            last_line: 40,
        },
    );

    assert_eq!(session.viewport_lines(doc).map(|lines| lines.0), Some(0));
    Ok(())
}

/// Declaring a viewport must republish, scoped to it: the spans the client holds
/// were scoped to the *previous* window, so without a fresh snapshot the newly
/// revealed lines stay unhighlighted until something else touches the document,
/// and a snapshot that still carried the whole file would save nothing.
#[tokio::test]
async fn declaring_a_new_viewport_republishes_the_document_scoped_to_it() -> TestResult {
    const LINE: &str = "fn main() {}\n";
    let line_of = |span: &karet_core::HighlightSpan| span.span.start.0 / LINE.len();
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scroll.rs");
    std::fs::write(&path, LINE.repeat(2_000))?;
    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    // Whole-document highlights first, so the narrowing has something to cut.
    let doc = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((doc, snap)) = snaps.recv().await {
            if snap
                .highlights
                .all()
                .iter()
                .any(|span| line_of(span) > 1_900)
            {
                return Some(doc);
            }
        }
        None
    })
    .await?
    .ok_or("the document was highlighted")?;

    backend.send(
        backend.next_id(),
        DocumentCommand::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 1_000,
            last_line: 1_040,
        }
        .into(),
    )?;

    let margin = crate::session::VIEWPORT_MARGIN as usize;
    let (_, scoped) = tokio::time::timeout(Duration::from_secs(10), snaps.recv())
        .await?
        .ok_or("a snapshot for the new viewport")?;
    let lines: Vec<usize> = scoped.highlights.all().iter().map(line_of).collect();
    assert!(
        lines.contains(&1_000),
        "the viewport's own lines are highlighted"
    );
    assert!(
        lines
            .iter()
            .all(|line| (1_000 - margin..=1_040 + margin).contains(line)),
        "only the padded window travels"
    );
    Ok(())
}

/// A client sends its viewport on every scroll. One that stays inside the margin
/// already published must not republish: a fresh snapshot would carry a fresh
/// highlight slice, which a connection resends whole.
#[test]
fn a_scroll_inside_the_published_margin_does_not_republish() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scroll.rs");
    std::fs::write(&path, "fn main() {}\n".repeat(2_000))?;
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;
    let viewport = |first_line: u32| DocumentCommand::SetViewport {
        doc,
        view: ViewId(1),
        first_line,
        last_line: first_line + 40,
    };
    session.handle(RequestId(2), viewport(1_000));
    while snaps.try_recv().is_some() {}
    let window = session.highlight_window(doc);

    session.handle(RequestId(3), viewport(1_150));

    assert!(snaps.try_recv().is_none(), "a scroll inside the margin");
    assert_eq!(session.highlight_window(doc), window);

    session.handle(RequestId(4), viewport(1_300));

    assert!(snaps.try_recv().is_some(), "a scroll past the margin");
    let margin = crate::session::VIEWPORT_MARGIN;
    assert_eq!(
        session.highlight_window(doc),
        Some((1_300 - margin, 1_340 + margin))
    );
    Ok(())
}

/// A republish whose narrowed spans did not change keeps the slice's `Arc`, which
/// is what lets a connection comparing pointers send nothing for it.
#[test]
fn an_unchanged_highlight_slice_keeps_its_arc_across_republishes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scroll.rs");
    std::fs::write(&path, "fn main() {}\n".repeat(20))?;
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;
    while snaps.try_recv().is_some() {}
    for (id, view, first_line) in [(2, ViewId(1), 0), (3, ViewId(2), 5_000)] {
        session.handle(
            RequestId(id),
            DocumentCommand::SetViewport {
                doc,
                view,
                first_line,
                last_line: first_line + 40,
            },
        );
    }

    let (_, first) = snaps.try_recv().ok_or("the first viewport's snapshot")?;
    let (_, second) = snaps.try_recv().ok_or("the widened window's snapshot")?;

    assert!(std::sync::Arc::ptr_eq(
        &first.highlights,
        &second.highlights
    ));
    Ok(())
}

/// A client sends its viewport on every scroll, and most scrolls land inside the
/// margin already covered. Republishing each time would spend a frame — and, on a
/// connection, a payload — to say nothing new.
#[test]
fn re_declaring_an_unchanged_viewport_does_not_republish() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("scroll.rs");
    std::fs::write(&path, "fn main() {}\n".repeat(500))?;
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;
    let viewport = DocumentCommand::SetViewport {
        doc,
        view: ViewId(1),
        first_line: 10,
        last_line: 50,
    };
    session.handle(RequestId(2), viewport.clone());
    while snaps.try_recv().is_some() {}

    session.handle(RequestId(3), viewport);

    assert!(snaps.try_recv().is_none());
    Ok(())
}

/// A viewport reversed by a drag-select upward must still describe a range, not an
/// empty one — otherwise selecting backwards blanks the highlighting.
#[test]
fn a_reversed_viewport_is_normalized_rather_than_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("reversed.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;

    session.handle(
        RequestId(2),
        DocumentCommand::SetViewport {
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
    Ok(())
}

/// Closing a document must drop its viewports, or the next document to reuse that
/// id inherits a window nobody declared for it.
#[test]
fn closing_a_document_forgets_its_viewport() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("closed.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;
    session.handle(
        RequestId(2),
        DocumentCommand::SetViewport {
            doc,
            view: ViewId(1),
            first_line: 0,
            last_line: 40,
        },
    );
    assert!(session.viewport_lines(doc).is_some());

    session.handle(RequestId(3), DocumentCommand::Close { doc });

    assert_eq!(session.viewport_lines(doc), None);
    Ok(())
}

/// A closed view must stop widening the window: the document stays open while
/// another view shows it, so waiting for the document to close is too late.
#[test]
fn clearing_a_view_narrows_the_range_to_the_views_left() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("two_views.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
    let doc = open_document(&mut session, &mut events, path).ok_or("the document opened")?;
    for (view, first_line) in [(ViewId(1), 0), (ViewId(2), 5_000)] {
        session.handle(
            RequestId(2),
            DocumentCommand::SetViewport {
                doc,
                view,
                first_line,
                last_line: first_line + 40,
            },
        );
    }

    session.handle(
        RequestId(3),
        DocumentCommand::ClearViewport {
            doc,
            view: ViewId(2),
        },
    );

    let margin = crate::session::VIEWPORT_MARGIN;
    assert_eq!(session.viewport_lines(doc), Some((0, 40 + margin)));
    Ok(())
}

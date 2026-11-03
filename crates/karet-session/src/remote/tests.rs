//! End-to-end tests: a real [`Session`] served over a real stream, driven by a
//! real [`RemoteBackend`].
//!
//! `tokio::io::duplex` gives an in-process byte stream with the same semantics as
//! a socket — the same substitution `LspManager`'s connector already uses to fake
//! a language server. Nothing here mocks the protocol; what is tested is whether
//! the two halves converge.

use std::time::Duration;

use karet_core::Change;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;

use super::RemoteBackend;
use crate::api::Command;
use crate::api::DocumentCommand;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::GithubCommand;
use crate::api::WorkspaceCommand;
use crate::backend::Backend;
use crate::local::SnapshotRx;
use crate::session::EventRx;
use crate::session::SessionConfig;

/// How long a test waits for an answer before giving up.
///
/// Generous: the session starts real producers (a VCS scan, a highlight worker)
/// and a loaded machine should not turn that into a flake.
const PATIENCE: Duration = Duration::from_secs(10);

/// What a test returns: a setup step that fails fails the test, rather than
/// letting it pass without exercising anything.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A served session and a client connected to it over a duplex pair.
struct Pair {
    dir: tempfile::TempDir,
    backend: RemoteBackend,
    events: EventRx,
    snapshots: SnapshotRx,
}

/// Stand up a workspace containing `files`, serve it, and connect a client.
async fn pair(files: &[(&str, &str)]) -> Option<Pair> {
    let dir = tempfile::tempdir().ok()?;
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body).ok()?;
    }
    let config = SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    };

    // Two duplex pairs: one stream per direction, so neither side reads its own
    // writes — exactly how a socket behaves.
    let (client_reader, server_writer) = tokio::io::duplex(1 << 20);
    let (server_reader, client_writer) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let _ = super::serve(
            config,
            tokio::io::BufReader::new(server_reader),
            server_writer,
        )
        .await;
    });

    let (backend, snapshots) =
        super::connect(tokio::io::BufReader::new(client_reader), client_writer, 0)
            .await
            .ok()?;
    let events = backend.take_events()?;
    Some(Pair {
        dir,
        backend,
        events,
        snapshots,
    })
}

impl Pair {
    /// Submit `command` and wait for the event answering it.
    async fn ask(&mut self, command: impl Into<Command>) -> Option<Event> {
        let id = self.backend.next_id();
        self.backend.send(id, command.into()).ok()?;
        tokio::time::timeout(PATIENCE, async {
            while let Some((event_id, event)) = self.events.recv().await {
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

    /// Wait for a snapshot of `doc` whose text satisfies `wanted`.
    ///
    /// Polling for a *condition* rather than a fixed number of snapshots keeps
    /// the test independent of how many times a producer republishes.
    async fn await_text(
        &mut self,
        doc: DocumentId,
        wanted: impl Fn(&str) -> bool,
    ) -> Option<String> {
        tokio::time::timeout(PATIENCE, async {
            while let Some((id, snapshot)) = self.snapshots.recv().await {
                let text = snapshot.buffer.text();
                if id == doc && wanted(&text) {
                    return Some(text);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }

    /// Wait for a snapshot of `doc` whose text satisfies `wanted`, and keep it.
    ///
    /// The version matters as much as the text: an edit is built against the
    /// version a snapshot reports, so a test that only checks text cannot tell a
    /// replica that is numbered correctly from one that merely happens to hold
    /// the right characters.
    async fn await_snapshot(
        &mut self,
        doc: DocumentId,
        wanted: impl Fn(&str) -> bool,
    ) -> Option<std::sync::Arc<crate::local::DocSnapshot>> {
        tokio::time::timeout(PATIENCE, async {
            while let Some((id, snapshot)) = self.snapshots.recv().await {
                if id == doc && wanted(&snapshot.buffer.text()) {
                    return Some(snapshot);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }

    /// Submit `change` as an edit to `doc`, without waiting for the answer.
    fn edit(&mut self, doc: DocumentId, change: Change) {
        let id = self.backend.next_id();
        let _ = self.backend.send(
            id,
            DocumentCommand::ApplyChange {
                doc,
                change,
                cause: karet_core::EditCause::Type,
            }
            .into(),
        );
    }

    /// Open `path` and return the document id and version the backend assigned.
    ///
    /// The version matters: an edit is relative to one, and a rope refuses a
    /// change built against a version it is not at. Guessing it is how a test
    /// silently passes by never applying its edit at all.
    async fn open(&mut self, path: std::path::PathBuf) -> Option<(DocumentId, u64)> {
        match self
            .ask(DocumentCommand::Open {
                path,
                language: None,
            })
            .await
        {
            Some(Event::Opened { doc, version }) => Some((doc, version)),
            _ => None,
        }
    }

    fn root(&self) -> std::path::PathBuf {
        self.dir.path().to_path_buf()
    }
}

/// A change replacing the whole of line 0 with `text`.
fn replace_first_line(base_version: u64, columns: u32, text: &str) -> Change {
    Change::new(
        base_version,
        vec![TextEdit {
            range: Range {
                start: LineCol { line: 0, col: 0 },
                end: LineCol {
                    line: 0,
                    col: columns,
                },
            },
            new_text: text.to_owned(),
        }],
    )
}

#[tokio::test]
async fn a_client_opens_a_document_and_receives_its_text() -> TestResult {
    let mut pair = pair(&[("hello.txt", "hello\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("hello.txt");

    let (doc, _version) = pair.open(path).await.ok_or("the open")?;
    let text = pair.await_text(doc, |text| text.contains("hello")).await;

    assert_eq!(text.as_deref(), Some("hello\n"));
    Ok(())
}

/// The load-bearing property of the whole design: the client's replica must
/// converge on the backend's text after an edit, with the backend never sending
/// the document back.
#[tokio::test]
async fn an_edit_converges_between_the_client_replica_and_the_backend() -> TestResult {
    let mut pair = pair(&[("edit.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("edit.txt");
    let (doc, version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;

    let id = pair.backend.next_id();
    let _ = pair.backend.send(
        id,
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(version, 5, "omega"),
            cause: karet_core::EditCause::Type,
        }
        .into(),
    );

    let text = pair.await_text(doc, |text| text.starts_with("omega")).await;
    assert_eq!(text.as_deref(), Some("omega\n"));
    Ok(())
}

/// The echo path. The client must render its own edit without waiting for the
/// backend — that is the entire reason the replica exists.
#[tokio::test]
async fn a_local_edit_appears_before_the_backend_answers() -> TestResult {
    let mut pair = pair(&[("echo.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("echo.txt");
    let (doc, version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;

    let id = pair.backend.next_id();
    let _ = pair.backend.send(
        id,
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(version, 5, "omega"),
            cause: karet_core::EditCause::Type,
        }
        .into(),
    );

    // The very next snapshot is the local echo, minted client-side before any
    // frame could have made the round trip.
    let (echoed_doc, snapshot) = tokio::time::timeout(PATIENCE, pair.snapshots.recv())
        .await?
        .ok_or("the echo")?;
    assert_eq!(echoed_doc, doc);
    assert_eq!(snapshot.buffer.text(), "omega\n");
    Ok(())
}

/// A backend-originated edit — here an undo — must reach the client as an edit it
/// can apply, converging without the document being resent.
#[tokio::test]
async fn a_backend_originated_edit_converges_on_the_client() -> TestResult {
    let mut pair = pair(&[("undo.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("undo.txt");
    let (doc, version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;
    let id = pair.backend.next_id();
    let _ = pair.backend.send(
        id,
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(version, 5, "omega"),
            cause: karet_core::EditCause::Type,
        }
        .into(),
    );
    pair.await_text(doc, |text| text.starts_with("omega"))
        .await
        .ok_or("the edited text")?;

    let undo = pair.backend.next_id();
    let _ = pair
        .backend
        .send(undo, DocumentCommand::Undo { doc }.into());

    let text = pair.await_text(doc, |text| text.starts_with("alpha")).await;
    assert_eq!(text.as_deref(), Some("alpha\n"));
    Ok(())
}

/// Commands with no document behind them must work identically over a connection
/// — this is the whole non-editing surface (git, search, quick-open) in miniature.
#[tokio::test]
async fn a_workspace_command_is_answered_over_the_connection() -> TestResult {
    let mut pair = pair(&[("a.rs", "x\n"), ("b.rs", "y\n")])
        .await
        .ok_or("a pair")?;

    let event = pair.ask(WorkspaceCommand::ListFiles { limit: 100 }).await;

    let Some(Event::FilesListed { files, .. }) = event else {
        return Err(format!("expected a listing, got {event:?}").into());
    };
    assert_eq!(files.len(), 2);
    Ok(())
}

#[tokio::test]
async fn bytes_of_a_workspace_file_cross_the_connection() -> TestResult {
    let mut pair = pair(&[("media.bin", "0123456789")]).await.ok_or("a pair")?;
    let path = pair.root().join("media.bin");

    let event = pair
        .ask(WorkspaceCommand::ReadFileBytes {
            path,
            offset: 2,
            len: 4,
        })
        .await;

    let Some(Event::FileBytes { result, .. }) = event else {
        return Err(format!("expected bytes, got {event:?}").into());
    };
    let chunk = result.map_err(|error| format!("{error:?}"))?;
    assert_eq!(chunk.bytes, b"2345".to_vec());
    Ok(())
}

/// Saving clears the unsaved marker without touching the text or any derived
/// data, so it is the case a naive "send only what changed" would drop.
#[tokio::test]
async fn saving_clears_the_dirty_flag_on_the_client() -> TestResult {
    let mut pair = pair(&[("save.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("save.txt");
    let (doc, version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;
    let edit = pair.backend.next_id();
    let _ = pair.backend.send(
        edit,
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(version, 5, "omega"),
            cause: karet_core::EditCause::Type,
        }
        .into(),
    );
    pair.await_text(doc, |text| text.starts_with("omega"))
        .await
        .ok_or("the edited text")?;

    let _ = pair
        .ask(DocumentCommand::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        })
        .await;

    let clean = tokio::time::timeout(PATIENCE, async {
        while let Some((id, snapshot)) = pair.snapshots.recv().await {
            if id == doc && !snapshot.dirty {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(clean, "a save should reach the client as a clean document");
    Ok(())
}

/// A GitHub token is authenticated on the host that holds the repository. The
/// type refuses to serialize; the connection must drop the command rather than
/// ship the secret — answer the sign-in as failed on its own request id, so the
/// caller is not left waiting — and stay usable afterwards.
#[tokio::test]
async fn a_command_that_cannot_be_serialized_is_refused_without_killing_the_session() -> TestResult
{
    let mut pair = pair(&[("a.rs", "x\n")]).await.ok_or("a pair")?;

    let refused = pair
        .ask(GithubCommand::Login {
            token: crate::api::GithubToken::new("ghp_example".to_owned()),
        })
        .await;

    assert!(
        matches!(
            refused,
            Some(Event::GithubError {
                operation: crate::api::GithubOperation::SignIn,
                ..
            })
        ),
        "the sign-in must be answered on its request id: {refused:?}"
    );
    let event = pair.ask(WorkspaceCommand::ListFiles { limit: 8 }).await;
    assert!(
        matches!(event, Some(Event::FilesListed { .. })),
        "the session must survive an unsendable command: {event:?}"
    );
    Ok(())
}

/// A client that attaches knowing nothing is told so, and must be sent complete
/// state rather than a resumed stream.
#[tokio::test]
async fn a_fresh_client_is_told_it_did_not_resume() -> TestResult {
    let mut pair = pair(&[("a.rs", "x\n")]).await.ok_or("a pair")?;

    let restored = tokio::time::timeout(PATIENCE, async {
        while let Some((_, event)) = pair.events.recv().await {
            if let Event::ViewStateRestored { blob } = event {
                return Some(blob);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    assert_eq!(restored, Some(None), "a fresh session has no view state");
    Ok(())
}

/// The regression this guards: an event stream and a snapshot stream racing meant
/// a client was occasionally sent its own keystroke back, its replica rejected the
/// misplaced edit, and the document went blank for the rest of the session.
/// Several cycles, because the race only lost sometimes.
#[tokio::test]
async fn repeated_edit_and_undo_cycles_keep_converging() -> TestResult {
    let mut pair = pair(&[("cycle.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("cycle.txt");
    let (doc, mut version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;

    for round in 0..6_u32 {
        let typed = format!("omega{round}");
        let id = pair.backend.next_id();
        let _ = pair.backend.send(
            id,
            DocumentCommand::ApplyChange {
                doc,
                change: replace_first_line(version, 5, &typed),
                cause: karet_core::EditCause::Type,
            }
            .into(),
        );
        let typed_arrived = pair
            .await_text(doc, |text| text.starts_with(&typed))
            .await
            .is_some();
        assert!(
            typed_arrived,
            "round {round}: the typed text never reached the client"
        );

        let undo = pair.backend.next_id();
        let _ = pair
            .backend
            .send(undo, DocumentCommand::Undo { doc }.into());
        let undo_arrived = pair
            .await_text(doc, |text| text.starts_with("alpha"))
            .await
            .is_some();
        assert!(
            undo_arrived,
            "round {round}: the undo never reached the client"
        );
        // Each cycle leaves the document two versions further along.
        version += 2;
    }
    Ok(())
}

/// A client cannot assume its own working directory is the workspace — the files
/// may be on another machine — so the backend names it on attach.
#[tokio::test]
async fn a_client_is_told_which_workspace_it_is_rendering() -> TestResult {
    let mut pair = pair(&[("a.rs", "x\n")]).await.ok_or("a pair")?;
    let expected = pair.root();

    let roots = tokio::time::timeout(PATIENCE, async {
        while let Some((_, event)) = pair.events.recv().await {
            if let Event::WorkspaceRoots { roots } = event {
                return Some(roots);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    assert_eq!(roots, Some(vec![expected]));
    Ok(())
}

/// The workspace's configuration describes the code and lives beside it, so the
/// backend resolves it and the client is told.
#[tokio::test]
async fn a_client_is_told_the_workspaces_configuration() -> TestResult {
    let mut pair = pair(&[("a.rs", "x\n")]).await.ok_or("a pair")?;

    let announced = tokio::time::timeout(PATIENCE, async {
        while let Some((_, event)) = pair.events.recv().await {
            if matches!(event, Event::ConfigChanged { .. }) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    assert!(announced);
    Ok(())
}

/// A replica is numbered by the *document*, not by a count of its own edits. A
/// snapshot's version is what the presentation layer builds its next edit
/// against, and a replica rebuilt from a full copy of the text used to start
/// counting from its own zero — so the two disagreed and every edit after that
/// was refused, silently, for the rest of the session.
#[tokio::test]
async fn a_re_described_document_is_still_editable() -> TestResult {
    let mut pair = pair(&[("resync.txt", "alpha\n")]).await.ok_or("a pair")?;
    let path = pair.root().join("resync.txt");
    let (doc, version) = pair.open(path).await.ok_or("the open")?;
    pair.await_text(doc, |text| text == "alpha\n")
        .await
        .ok_or("the opened text")?;

    // Move the document off version zero first: a replica counting from its own
    // zero agrees with the document by accident there, and would hide the bug.
    pair.edit(doc, replace_first_line(version, 5, "beta"));
    let edited = pair
        .await_snapshot(doc, |text| text == "beta\n")
        .await
        .ok_or("the edited snapshot")?;
    assert!(edited.version > 0, "the document must have moved");

    // A change against a version the document is not at. The session refuses it,
    // and the client applied it optimistically first — so both halves know the
    // replica no longer represents the document and it is described afresh.
    pair.edit(doc, replace_first_line(edited.version + 7, 4, "nope"));

    let described = pair
        .await_snapshot(doc, |text| text == "beta\n")
        .await
        .ok_or("the re-description")?;
    // The re-description carries the document's version, not the replica's own.
    assert_eq!(
        described.buffer.version(),
        described.version,
        "a snapshot and the buffer inside it must name the same version"
    );

    // The point of all of it: the editor still works afterwards.
    pair.edit(doc, replace_first_line(described.version, 4, "gamma"));

    let text = pair.await_text(doc, |text| text == "gamma\n").await;
    assert_eq!(text.as_deref(), Some("gamma\n"));
    Ok(())
}

mod connection;

//! The client half against a scripted backend: the paths a real session never
//! takes on its own — a peer that vanishes, one that never greets, and a resync
//! the client asks for because it has no replica.

use std::time::Duration;

use tokio::io::BufReader;
use tokio::io::DuplexStream;

use super::PATIENCE;
use super::TestResult;
use super::replace_first_line;
use crate::api::DocumentCommand;
use crate::api::DocumentId;
use crate::api::RenderUpdate;
use crate::api::TextUpdate;
use crate::backend::Backend;
use crate::remote::RemoteBackend;
use crate::remote::frame;
use crate::remote::frame::FrameReader;
use crate::remote::wire;
use crate::remote::wire::ClientFrame;
use crate::remote::wire::Hello;
use crate::remote::wire::ServerFrame;

/// The backend's ends of a connection a test scripts by hand.
struct Scripted {
    reader: FrameReader<BufReader<DuplexStream>>,
    writer: DuplexStream,
}

impl Scripted {
    /// The next frame the client sent.
    async fn next(&mut self) -> Result<ClientFrame, Box<dyn std::error::Error>> {
        let body = tokio::time::timeout(PATIENCE, self.reader.next())
            .await??
            .ok_or("the client closed")?;
        Ok(wire::decode(&body)?)
    }

    async fn send(&mut self, message: &ServerFrame) -> TestResult {
        frame::write(&mut self.writer, &wire::encode(message)?).await?;
        Ok(())
    }
}

/// The client's and the scripted backend's ends of one connection.
fn link() -> (BufReader<DuplexStream>, DuplexStream, Scripted) {
    let (client_reader, server_writer) = tokio::io::duplex(1 << 16);
    let (server_reader, client_writer) = tokio::io::duplex(1 << 16);
    let scripted = Scripted {
        reader: FrameReader::new(BufReader::new(server_reader)),
        writer: server_writer,
    };
    (BufReader::new(client_reader), client_writer, scripted)
}

/// A client connected to a scripted backend that has greeted it and read its
/// attach.
async fn connected()
-> Result<(RemoteBackend, crate::local::SnapshotRx, Scripted), Box<dyn std::error::Error>> {
    let (reader, writer, mut scripted) = link();
    let connecting = tokio::spawn(super::super::connect(reader, writer, 0));
    let ClientFrame::Hello(_) = scripted.next().await? else {
        return Err("the client must greet first".into());
    };
    scripted.send(&ServerFrame::Hello(Hello::current())).await?;
    let (backend, snapshots) = connecting.await??;
    let ClientFrame::Attach { last_seq } = scripted.next().await? else {
        return Err("the client must attach after the greeting".into());
    };
    assert_eq!(last_seq, 0, "a fresh client has seen nothing");
    Ok((backend, snapshots, scripted))
}

/// A backend that never greets leaves `connect` pending, never resolved: the
/// caller bounds the handshake (the app gives up after 30 seconds), because
/// nothing on the stream ever will.
#[tokio::test]
async fn a_backend_that_never_greets_leaves_the_handshake_pending() {
    let (reader, writer, _scripted) = link();

    let outcome = tokio::time::timeout(
        Duration::from_millis(200),
        super::super::connect(reader, writer, 0),
    )
    .await;

    assert!(outcome.is_err(), "the handshake must not complete");
}

/// A backend that closes mid-session ends both of the client's streams, so the
/// presentation layer can tell the user instead of waiting on them for ever,
/// and a later command is refused rather than silently dropped.
#[tokio::test]
async fn a_connection_closing_mid_session_ends_the_clients_streams() -> TestResult {
    let (backend, mut snapshots, scripted) = connected().await?;
    let mut events = backend.take_events().ok_or("the event stream")?;

    drop(scripted);

    let events_ended =
        tokio::time::timeout(PATIENCE, async { while events.recv().await.is_some() {} }).await;
    assert!(events_ended.is_ok(), "the event stream must end");
    let snapshot = tokio::time::timeout(PATIENCE, snapshots.recv()).await?;
    assert!(snapshot.is_none(), "the snapshot stream must end");
    let refused = backend.send(
        backend.next_id(),
        DocumentCommand::Undo { doc: DocumentId(1) }.into(),
    );
    assert!(
        refused.is_err(),
        "a command after the close must be refused"
    );
    Ok(())
}

/// A local edit to a document the client holds no replica of cannot be echoed,
/// so the client says so: the command goes out, then a resync for the document,
/// and the backend's re-description rebuilds the replica.
#[tokio::test]
async fn an_edit_without_a_replica_asks_for_the_document_again() -> TestResult {
    let (backend, mut snapshots, mut scripted) = connected().await?;
    let doc = DocumentId(9);

    backend.send(
        backend.next_id(),
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(3, 5, "omega"),
            cause: karet_core::EditCause::Type,
        }
        .into(),
    )?;

    let ClientFrame::Command { .. } = scripted.next().await? else {
        return Err("the edit must still reach the backend".into());
    };
    let ClientFrame::Resync { doc: asked } = scripted.next().await? else {
        return Err("the client must ask for the document again".into());
    };
    assert_eq!(asked, doc);

    scripted
        .send(&ServerFrame::Event {
            seq: 1,
            id: None,
            event: Box::new(crate::api::Event::Render {
                doc,
                update: Box::new(RenderUpdate {
                    text: TextUpdate::Full("omega\n".to_owned()),
                    ..RenderUpdate::at(4)
                }),
            }),
        })
        .await?;
    let (described, snapshot) = tokio::time::timeout(PATIENCE, snapshots.recv())
        .await?
        .ok_or("the re-description")?;
    assert_eq!(described, doc);
    assert_eq!(snapshot.buffer.text(), "omega\n");
    assert_eq!(snapshot.version, 4);
    Ok(())
}

/// A render frame for `doc` carrying `update`.
fn render(seq: u64, doc: DocumentId, update: RenderUpdate) -> ServerFrame {
    ServerFrame::Event {
        seq,
        id: None,
        event: Box::new(crate::api::Event::Render {
            doc,
            update: Box::new(update),
        }),
    }
}

/// A render establishing `doc` as `text` at `version`.
fn described(seq: u64, doc: DocumentId, version: u64, text: &str) -> ServerFrame {
    render(
        seq,
        doc,
        RenderUpdate {
            text: TextUpdate::Full(text.to_owned()),
            ..RenderUpdate::at(version)
        },
    )
}

/// `len` ASCII letters with no short period, so deflate cannot shrink a frame
/// carrying them below a duplex buffer.
pub(super) fn noise(len: usize) -> String {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            char::from(b'a' + state.to_le_bytes()[0] % 26)
        })
        .collect()
}

/// A render that carries no text, for a document the client holds no replica
/// of, is relative to text the client does not have — one in flight when the
/// replica was discarded. Applying it to an empty replica would blank the tab;
/// it is dropped, and the full description that follows is what renders.
#[tokio::test]
async fn a_textless_render_for_a_document_with_no_replica_is_dropped() -> TestResult {
    let (_backend, mut snapshots, mut scripted) = connected().await?;
    let doc = DocumentId(5);

    scripted.send(&render(1, doc, RenderUpdate::at(3))).await?;
    scripted.send(&described(2, doc, 4, "kept\n")).await?;

    let (shown, snapshot) = tokio::time::timeout(PATIENCE, snapshots.recv())
        .await?
        .ok_or("the full description")?;
    assert_eq!(shown, doc);
    assert_eq!(snapshot.buffer.text(), "kept\n");
    assert_eq!(snapshot.version, 4);
    Ok(())
}

/// A backend change the replica cannot place means the replica no longer
/// represents the document: the client discards it, asks for the document
/// again, and renders the re-description rather than anything in between.
#[tokio::test]
async fn a_render_the_replica_cannot_apply_asks_for_the_document_again() -> TestResult {
    let (_backend, mut snapshots, mut scripted) = connected().await?;
    let doc = DocumentId(6);
    scripted.send(&described(1, doc, 1, "abc\n")).await?;
    tokio::time::timeout(PATIENCE, snapshots.recv())
        .await?
        .ok_or("the first description")?;

    // Built against a version the replica never held.
    scripted
        .send(&render(
            2,
            doc,
            RenderUpdate {
                text: TextUpdate::Change(Box::new(replace_first_line(7, 3, "xyz"))),
                ..RenderUpdate::at(8)
            },
        ))
        .await?;

    let ClientFrame::Resync { doc: asked } = scripted.next().await? else {
        return Err("the client must ask for the document again".into());
    };
    assert_eq!(asked, doc);
    scripted.send(&described(3, doc, 8, "xyz\n")).await?;
    let (_, snapshot) = tokio::time::timeout(PATIENCE, snapshots.recv())
        .await?
        .ok_or("the re-description")?;
    assert_eq!(snapshot.buffer.text(), "xyz\n");
    assert_eq!(snapshot.version, 8);
    Ok(())
}

/// A frame this build cannot decode comes from a newer peer; the client skips
/// it and keeps reading rather than losing the session.
#[tokio::test]
async fn an_unreadable_backend_frame_is_skipped() -> TestResult {
    let (_backend, mut snapshots, mut scripted) = connected().await?;
    let doc = DocumentId(7);

    frame::write(&mut scripted.writer, &[0xff, 0xff, 0xff, 0xff]).await?;
    scripted.send(&described(1, doc, 1, "after\n")).await?;

    let (_, snapshot) = tokio::time::timeout(PATIENCE, snapshots.recv())
        .await?
        .ok_or("the frame after the unreadable one")?;
    assert_eq!(snapshot.buffer.text(), "after\n");
    Ok(())
}

/// A large command and a large event written at the same moment must both
/// arrive. A client that awaited its own write inside its read loop stopped
/// reading while the backend's equally large write waited for it to read, and
/// neither finished.
#[tokio::test]
async fn a_large_command_and_a_large_event_cross_at_once() -> TestResult {
    let (backend, _snapshots, scripted) = connected().await?;
    let mut events = backend.take_events().ok_or("the event stream")?;
    let Scripted { mut reader, writer } = scripted;
    let big = noise(512 * 1024);

    // The backend writes first, and reads nothing until it has finished.
    let body = wire::encode(&ServerFrame::Event {
        seq: 1,
        id: None,
        event: Box::new(crate::api::Event::Notification {
            severity: karet_core::Severity::Information,
            kind: karet_core::NotificationKind::System,
            message: big.clone(),
        }),
    })?;
    let writing = tokio::spawn(async move {
        let mut writer = writer;
        frame::write(&mut writer, &body).await.map(|()| writer)
    });
    backend.send(
        backend.next_id(),
        DocumentCommand::ApplyChange {
            doc: DocumentId(1),
            change: replace_first_line(1, 0, &big),
            cause: karet_core::EditCause::Paste,
        }
        .into(),
    )?;

    let _writer = tokio::time::timeout(PATIENCE, writing).await???;
    let delivered = tokio::time::timeout(PATIENCE, async {
        while let Some((_, event)) = events.recv().await {
            if let crate::api::Event::Notification { message, .. } = event {
                return Some(message.len());
            }
        }
        None
    })
    .await?;
    assert_eq!(delivered, Some(big.len()));
    let body = tokio::time::timeout(PATIENCE, reader.next())
        .await??
        .ok_or("the command")?;
    let ClientFrame::Command { .. } = wire::decode(&body)? else {
        return Err("the paste must reach the backend".into());
    };
    Ok(())
}

/// A command too large for a frame is answered on its request id and does not
/// end the connection. An edit refused that way was already applied to the
/// client's own text, so the client asks for the document again to put the tab
/// back on the text the backend holds.
#[tokio::test]
async fn a_command_too_large_for_a_frame_is_answered_and_the_connection_survives() -> TestResult {
    let (backend, _snapshots, mut scripted) = connected().await?;
    let mut events = backend.take_events().ok_or("the event stream")?;
    let doc = DocumentId(3);
    let id = backend.next_id();

    backend.send(
        id,
        DocumentCommand::ApplyChange {
            doc,
            change: replace_first_line(1, 0, &"x".repeat(frame::MAX_FRAME)),
            cause: karet_core::EditCause::Paste,
        }
        .into(),
    )?;

    let (answered, answer) = tokio::time::timeout(PATIENCE, events.recv())
        .await?
        .ok_or("the refusal")?;
    assert_eq!(answered, Some(id));
    assert!(
        matches!(
            &answer,
            crate::api::Event::Notification { message, .. } if message.contains("too large")
        ),
        "{answer:?}"
    );
    let ClientFrame::Resync { doc: asked } = scripted.next().await? else {
        return Err("the refused edit's document must be described again".into());
    };
    assert_eq!(asked, doc);
    backend.send(backend.next_id(), DocumentCommand::Undo { doc }.into())?;
    let ClientFrame::Command { .. } = scripted.next().await? else {
        return Err("a later command must still cross".into());
    };
    Ok(())
}

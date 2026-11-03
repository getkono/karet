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

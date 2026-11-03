//! The presentation half of a remote connection.
//!
//! [`RemoteBackend`] implements the same [`Backend`] trait `LocalBackend` does, so
//! the composition root swaps one for the other and nothing downstream changes.
//! Its job beyond moving bytes is to rebuild the [`DocSnapshot`] stream the
//! renderer expects, from the [`RenderUpdate`]s that arrive instead.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use karet_core::NotificationKind;
use karet_core::Severity;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

use super::RemoteError;
use super::frame;
use super::frame::FrameReader;
use super::replica::Replica;
use super::wire::ClientFrame;
use super::wire::Hello;
use super::wire::ServerFrame;
use crate::api::Command;
use crate::api::DocumentCommand;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::GithubCommand;
use crate::api::GithubOperation;
use crate::api::RequestId;
use crate::api::TextUpdate;
use crate::backend::Backend;
use crate::backend::BackendError;
use crate::local::DocSnapshot;
use crate::local::SnapshotRx;
use crate::session::EventRx;

/// A [`Backend`] whose session lives at the other end of a byte stream.
pub struct RemoteBackend {
    commands: mpsc::UnboundedSender<(RequestId, Command)>,
    next: AtomicU64,
    events: std::sync::Mutex<Option<EventRx>>,
}

impl Backend for RemoteBackend {
    fn send(&self, id: RequestId, command: Command) -> Result<(), BackendError> {
        self.commands
            .send((id, command))
            .map_err(|_| BackendError::Closed)
    }

    fn next_id(&self) -> RequestId {
        RequestId(self.next.fetch_add(1, Ordering::Relaxed))
    }

    fn take_events(&self) -> Option<EventRx> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

/// Connect to a backend over `reader`/`writer`, resuming from `last_seq`.
///
/// Returns the backend and the reconstructed snapshot stream — the same pair
/// [`local`](crate::backend::local) returns, which is what lets the composition
/// root treat the two modes as one.
///
/// Must be called within a Tokio runtime: the connection is driven by a spawned
/// task that outlives this call.
///
/// # Errors
/// Returns [`RemoteError`] when the handshake fails or the peer is unusable.
pub async fn connect<R, W>(
    reader: R,
    mut writer: W,
    last_seq: u64,
) -> Result<(RemoteBackend, SnapshotRx), RemoteError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let greeting = super::wire::encode(&ClientFrame::Hello(Hello::current()))?;
    frame::write(&mut writer, &greeting).await?;
    // The same reader is handed to `pump` afterwards: bytes it buffered past the
    // greeting belong to the frames that follow, and building a second reader
    // would strand them.
    let mut reader = FrameReader::new(reader);
    let body = reader
        .next()
        .await?
        .ok_or_else(|| RemoteError::Protocol("backend closed before greeting".to_owned()))?;
    let ServerFrame::Hello(hello) = super::wire::decode(&body)? else {
        return Err(RemoteError::Protocol("expected a greeting".to_owned()));
    };
    hello.accept()?;
    let attach = super::wire::encode(&ClientFrame::Attach { last_seq })?;
    frame::write(&mut writer, &attach).await?;

    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = crate::session::event_channel();
    let (snapshots_tx, snapshots_rx) = crate::local::snapshot_channel();
    tokio::spawn(async move {
        let outcome = pump(reader, writer, commands_rx, &events_tx, &snapshots_tx).await;
        if let Err(error) = outcome {
            tracing::warn!(%error, "the remote backend connection ended");
        }
    });

    Ok((
        RemoteBackend {
            commands: commands_tx,
            next: AtomicU64::new(1),
            events: std::sync::Mutex::new(Some(events_rx)),
        },
        snapshots_rx,
    ))
}

/// Drive the connection: commands out, events and snapshots in.
///
/// Frames go out through [`frame::drain`], beside the read loop rather than
/// inside it, so a large command being written never stops this end reading a
/// large event the backend is writing at the same moment.
async fn pump<R, W>(
    reader: FrameReader<R>,
    writer: W,
    commands: mpsc::UnboundedReceiver<(RequestId, Command)>,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: &mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
) -> Result<(), RemoteError>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (outgoing, queue) = mpsc::unbounded_channel();
    let reading = read_loop(reader, commands, outgoing, events, snapshots);
    let writing = frame::drain(writer, queue);
    tokio::pin!(reading);
    tokio::pin!(writing);
    tokio::select! {
        // The read loop owns the queue's sender, so its ending closes the queue
        // and the writer finishes once it has written what was already queued.
        read = &mut reading => read.and(writing.await),
        // A writer only ends early on a failed write: nothing more can be sent.
        written = &mut writing => written,
    }
}

/// Queue one frame body for the connection's writer.
fn queue(outgoing: &mpsc::UnboundedSender<Vec<u8>>, body: Vec<u8>) -> Result<(), RemoteError> {
    outgoing
        .send(body)
        .map_err(|_| RemoteError::Protocol("the connection's writer has stopped".to_owned()))
}

/// Read backend frames and forward client commands until either side closes.
async fn read_loop<R>(
    mut reader: FrameReader<R>,
    mut commands: mpsc::UnboundedReceiver<(RequestId, Command)>,
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: &mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
) -> Result<(), RemoteError>
where
    R: AsyncBufRead + Unpin + Send,
{
    let mut replicas: std::collections::HashMap<DocumentId, Replica> =
        std::collections::HashMap::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some((id, command)) => {
                    on_command(id, command, &mut replicas, &outgoing, events, snapshots)?;
                },
                None => return Ok(()), // the backend handle was dropped
            },
            incoming = reader.next() => match incoming? {
                Some(body) => {
                    let frame: ServerFrame = match super::wire::decode(&body) {
                        Ok(frame) => frame,
                        Err(error) => {
                            tracing::warn!(%error, "skipping an unreadable backend frame");
                            continue;
                        },
                    };
                    match frame {
                        ServerFrame::Hello(_) => {},
                        ServerFrame::Attached { resumed, view_state } => {
                            if !resumed {
                                replicas.clear();
                            }
                            let _ = events.send((
                                None,
                                Event::ViewStateRestored { blob: view_state },
                            ));
                        },
                        ServerFrame::Event { id, event, .. } => {
                            // A diverged replica is discarded here; asking for it
                            // again is what makes that recoverable rather than a
                            // document stuck blank for the rest of the session.
                            if let Some(doc) = deliver(*event, id, &mut replicas, events, snapshots)
                                && let Ok(body) = super::wire::encode(&ClientFrame::Resync { doc })
                            {
                                queue(&outgoing, body)?;
                            }
                        },
                    }
                },
                None => return Ok(()), // the backend closed
            },
        }
    }
}

/// Send one command, echoing an edit into its replica first.
///
/// A command that cannot cross — one that refuses to encode, or one larger than
/// a frame — is answered here on its request id: `send` already returned `Ok`,
/// so a caller waiting on it would otherwise never resolve. It never ends the
/// connection; one lost command is not worth the session (as the backend treats
/// an event that cannot cross).
fn on_command(
    id: RequestId,
    command: Command,
    replicas: &mut std::collections::HashMap<DocumentId, Replica>,
    outgoing: &mpsc::UnboundedSender<Vec<u8>>,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: &mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
) -> Result<(), RemoteError> {
    let sign_in = matches!(command, Command::Github(GithubCommand::Login { .. }));
    let frame = ClientFrame::Command {
        id,
        command: Box::new(command),
    };
    let body = super::wire::encode(&frame);
    let ClientFrame::Command { command, .. } = frame else {
        return Ok(()); // built as a command just above
    };
    let edited = match *command {
        Command::Document(DocumentCommand::ApplyChange { doc, change, cause }) => {
            Some((doc, change, cause))
        },
        _ => None,
    };
    let refused = match &body {
        // The deliberate case is a GitHub token, which must be authenticated on
        // the workspace host instead.
        Err(error) => {
            tracing::warn!(%error, "dropping a command that cannot cross a connection");
            Some(refusal(sign_in))
        },
        Ok(body) if !frame::fits(body.len()) => {
            tracing::warn!(
                bytes = body.len(),
                "dropping a command too large for one frame"
            );
            Some(too_large(body.len()))
        },
        Ok(_) => None,
    };
    if let Some(answer) = refused {
        let _ = events.send((Some(id), answer));
        // The presentation layer already applied a refused edit to its own text,
        // which the backend will now never see. Describing the document again
        // puts the tab back on the text the backend holds.
        if let Some((doc, ..)) = edited {
            replicas.remove(&doc);
            queue(outgoing, super::wire::encode(&ClientFrame::Resync { doc })?)?;
        }
        return Ok(());
    }
    let Ok(body) = body else {
        return Ok(()); // answered above
    };
    // Echo the edit into the replica first, so the snapshot the renderer draws
    // from advances at local speed rather than at the connection's.
    let mut diverged = None;
    if let Some((doc, change, cause)) = edited {
        match replicas
            .get_mut(&doc)
            .and_then(|replica| replica.apply_local(&change, cause))
        {
            Some(snapshot) => {
                let _ = snapshots.send((doc, snapshot));
            },
            // Either there is no replica yet or the change did not fit the one
            // there is; either way this client does not hold the text its own
            // edit produces. Saying so matters as much as the echo does: the
            // backend suppresses text for versions it believes the client made,
            // so a silent failure here left the document stuck at the last text
            // both sides agreed on while every later keystroke was acknowledged
            // and dropped.
            None => {
                tracing::warn!(?doc, "a local edit did not fit its replica");
                replicas.remove(&doc);
                diverged = Some(doc);
            },
        }
    }
    queue(outgoing, body)?;
    // After the command, so the backend has already applied it and describes the
    // document at the version the resync will carry rather than the one before
    // it.
    if let Some(doc) = diverged {
        queue(outgoing, super::wire::encode(&ClientFrame::Resync { doc })?)?;
    }
    Ok(())
}

/// The answer to a command that cannot cross a connection.
///
/// A sign-in answers as the GitHub failure its caller already handles; anything
/// else as a notification, so the user is told rather than left waiting.
fn refusal(sign_in: bool) -> Event {
    if sign_in {
        return Event::GithubError {
            operation: GithubOperation::SignIn,
            message: "a GitHub token is not sent over a remote connection; sign in on the \
                      workspace host"
                .to_owned(),
        };
    }
    Event::Notification {
        severity: Severity::Error,
        kind: NotificationKind::System,
        message: "a command could not be sent to the remote backend".to_owned(),
    }
}

/// The answer to a command whose encoded body of `bytes` exceeds a frame.
fn too_large(bytes: usize) -> Event {
    Event::Notification {
        severity: Severity::Error,
        kind: NotificationKind::System,
        message: format!(
            "a change of {} MiB is too large to send to the remote backend",
            bytes / (1024 * 1024)
        ),
    }
}

/// Route one backend event: render updates rebuild a replica, everything else
/// goes straight to the presentation layer.
///
/// Returns the document to resynchronize, when a replica diverged badly enough to
/// be discarded.
fn deliver(
    event: Event,
    id: Option<RequestId>,
    replicas: &mut std::collections::HashMap<DocumentId, Replica>,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: &mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
) -> Option<DocumentId> {
    match event {
        Event::Render { doc, update } => {
            let replica = match replicas.entry(doc) {
                std::collections::hash_map::Entry::Occupied(held) => held.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot)
                    if matches!(update.text, TextUpdate::Full(_)) =>
                {
                    slot.insert(Replica::default())
                },
                // No replica, and no text to build one from: the update is
                // relative to text this client no longer holds — one that was in
                // flight when the replica was discarded. Applied to an empty
                // replica it would blank the tab and base the next keystroke on
                // nothing. The resync asked for when the replica was discarded
                // describes the document whole; wait for that.
                std::collections::hash_map::Entry::Vacant(_) => {
                    tracing::debug!(
                        ?doc,
                        "dropping a render update for a document with no replica"
                    );
                    return None;
                },
            };
            match replica.apply(*update) {
                Some(snapshot) => {
                    let _ = snapshots.send((doc, snapshot));
                    None
                },
                // The replica could not apply what the backend said, so it no
                // longer represents the document. Discard it and ask for the
                // document again rather than render text the backend never sent.
                None => {
                    tracing::warn!(?doc, "resetting a diverged document replica");
                    replicas.remove(&doc);
                    Some(doc)
                },
            }
        },
        Event::Closed { doc } => {
            replicas.remove(&doc);
            let _ = events.send((id, Event::Closed { doc }));
            None
        },
        other => {
            let _ = events.send((id, other));
            None
        },
    }
}

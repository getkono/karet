//! The presentation half of a remote connection.
//!
//! [`RemoteBackend`] implements the same [`Backend`] trait `LocalBackend` does, so
//! the composition root swaps one for the other and nothing downstream changes.
//! Its job beyond moving bytes is to rebuild the [`DocSnapshot`] stream the
//! renderer expects, from the [`RenderUpdate`]s that arrive instead.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

use super::RemoteError;
use super::frame;
use super::replica::Replica;
use super::wire::ClientFrame;
use super::wire::Hello;
use super::wire::ServerFrame;
use crate::api::Command;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::RequestId;
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
    mut reader: R,
    mut writer: W,
    last_seq: u64,
) -> Result<(RemoteBackend, SnapshotRx), RemoteError>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let greeting = super::wire::encode(&ClientFrame::Hello(Hello::current()))?;
    frame::write(&mut writer, &greeting).await?;
    let body = frame::read(&mut reader)
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
async fn pump<R, W>(
    mut reader: R,
    mut writer: W,
    mut commands: mpsc::UnboundedReceiver<(RequestId, Command)>,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: &mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
) -> Result<(), RemoteError>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut replicas: std::collections::HashMap<DocumentId, Replica> =
        std::collections::HashMap::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some((id, command)) => {
                    // Echo the edit into the replica first, so the snapshot the
                    // renderer draws from advances at local speed rather than at
                    // the connection's.
                    if let Command::ApplyChange { doc, change, cause } = &command
                        && let Some(replica) = replicas.get_mut(doc)
                        && let Some(snapshot) = replica.apply_local(change, *cause)
                    {
                        let _ = snapshots.send((*doc, snapshot));
                    }
                    let body = match super::wire::encode(&ClientFrame::Command {
                        id,
                        command: Box::new(command),
                    }) {
                        Ok(body) => body,
                        // The deliberate case is a GitHub token, which must be
                        // authenticated on the workspace host instead.
                        Err(error) => {
                            tracing::warn!(%error, "dropping a command that cannot cross a connection");
                            continue;
                        },
                    };
                    frame::write(&mut writer, &body).await?;
                },
                None => return Ok(()), // the backend handle was dropped
            },
            incoming = frame::read(&mut reader) => match incoming? {
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
                                frame::write(&mut writer, &body).await?;
                            }
                        },
                    }
                },
                None => return Ok(()), // the backend closed
            },
        }
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
            let replica = replicas.entry(doc).or_default();
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

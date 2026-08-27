//! The backend half of a remote connection.
//!
//! Runs a real [`Session`] — the same one local mode drives, through the same
//! [`local_session`](crate::backend::local_session) actor — and speaks the wire to
//! exactly one client. Nothing about the session knows it is being served
//! remotely; this loop is the whole difference.

use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;

use super::RemoteError;
use super::frame;
use super::frame::FrameReader;
use super::project::Projection;
use super::wire::ClientFrame;
use super::wire::Hello;
use super::wire::ServerFrame;
use crate::api::Command;
use crate::api::Event;
use crate::api::RequestId;
use crate::backend::Backend;
use crate::session::Session;
use crate::session::SessionConfig;

/// How many outgoing events are retained for a reattaching client to replay.
///
/// Sized for a disconnect measured in minutes of ordinary editing. Beyond it the
/// client is told to resynchronize, which costs one document's text rather than a
/// wrong screen — the trade the ring exists to make cheap, not to eliminate.
const REPLAY_CAPACITY: usize = 4096;

/// Serve one client over `reader`/`writer` until either side closes.
///
/// The session is created here and dropped when the connection ends, so a served
/// session lives exactly as long as the process hosting it — which, in the
/// intended deployment, is a terminal pane the multiplexer keeps alive across
/// client disconnects.
///
/// # Errors
/// Returns [`RemoteError`] on a transport failure or an unusable handshake. A
/// clean disconnect is `Ok(())`.
pub async fn serve<R, W>(config: SessionConfig, reader: R, writer: W) -> Result<(), RemoteError>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let roots = config.roots.clone();
    let loaded = Box::new(config.loaded_config.clone());
    let (session, events, snapshots) = Session::new(config);
    let backend = crate::backend::local_session(session, Some(events));
    Connection::new(reader, writer)
        .run(&backend, snapshots, roots, loaded)
        .await
}

/// One served connection's state.
struct Connection<R, W> {
    /// Framed, and cancel-safe: this reader is polled from a `select!` arm that
    /// loses the race whenever an event or a snapshot is ready first.
    reader: FrameReader<R>,
    writer: W,
    /// Monotonic sequence for outgoing events, so a reattaching client can name
    /// where it got to.
    seq: u64,
    /// Recently sent frames, oldest first, for replay on reattach.
    replay: std::collections::VecDeque<(u64, Vec<u8>)>,
    projection: Projection,
    /// Commands the client submitted that will move a document, so their
    /// answering versions can be attributed to the client rather than the backend.
    pending_edits: std::collections::HashMap<RequestId, crate::api::DocumentId>,
    /// The most recent snapshot of each open document, so a resync can be
    /// answered immediately rather than when the document next happens to change.
    /// An `Arc` sharing the session's own rope, so retaining it costs a pointer.
    latest: std::collections::HashMap<
        crate::api::DocumentId,
        std::sync::Arc<crate::local::DocSnapshot>,
    >,
}

impl<R, W> Connection<R, W>
where
    R: AsyncBufRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader: FrameReader::new(reader),
            writer,
            seq: 0,
            replay: std::collections::VecDeque::new(),
            projection: Projection::default(),
            pending_edits: std::collections::HashMap::new(),
            latest: std::collections::HashMap::new(),
        }
    }

    async fn run(
        mut self,
        backend: &crate::backend::LocalBackend,
        mut snapshots: crate::local::SnapshotRx,
        roots: Vec<std::path::PathBuf>,
        config: Box<crate::config::LoadedConfig>,
    ) -> Result<(), RemoteError> {
        let mut events = backend
            .take_events()
            .ok_or_else(|| RemoteError::Protocol("session event stream taken".to_owned()))?;
        if !self.handshake().await? {
            return Ok(()); // nobody connected
        }
        // Before anything else: a client has no way to know what workspace it is
        // rendering, and every relative path it resolves depends on the answer.
        self.send_event(None, Event::WorkspaceRoots { roots })
            .await?;
        // The workspace's own configuration — `.editorconfig`, the project
        // settings layer, linter and formatter config — is resolved here, because
        // it describes the code and lives beside it. A client keeps only the keys
        // that describe the terminal in front of the user.
        self.send_event(None, Event::ConfigChanged { report: config })
            .await?;

        loop {
            // Biased, and events before snapshots: a session emits `Applied`
            // before it publishes the snapshot for the same edit, so preferring
            // the event stream is what makes "the client already knows this
            // version" true by the time the snapshot is projected. Without it the
            // two channels race and a client is occasionally sent its own
            // keystroke back.
            tokio::select! {
                biased;
                event = events.recv() => match event {
                    Some((id, event)) => self.on_event(id, event).await?,
                    None => return Ok(()), // the session ended
                },
                incoming = self.reader.next() => match incoming? {
                    Some(body) => {
                        if !self.on_client_frame(&body, backend).await? {
                            return Ok(()); // the client said goodbye
                        }
                    },
                    None => return Ok(()),
                },
                snapshot = snapshots.recv() => {
                    if let Some((doc, snapshot)) = snapshot {
                        self.on_snapshot(doc, snapshot).await?;
                    }
                },
            }
        }
    }

    /// Project one snapshot onto the wire, retaining it for a later resync.
    async fn on_snapshot(
        &mut self,
        doc: crate::api::DocumentId,
        snapshot: std::sync::Arc<crate::local::DocSnapshot>,
    ) -> Result<(), RemoteError> {
        let update = self.projection.project(doc, &snapshot);
        self.latest.insert(doc, snapshot);
        let Some(update) = update else {
            return Ok(());
        };
        self.send_event(
            None,
            Event::Render {
                doc,
                update: Box::new(update),
            },
        )
        .await
    }

    /// Describe `doc` from scratch, because the client discarded its replica.
    async fn resync(&mut self, doc: crate::api::DocumentId) -> Result<(), RemoteError> {
        // Any edit of this document still in flight was submitted by a client
        // that has since said it cannot place the document. Crediting the client
        // with the version that edit produces would suppress the very text the
        // resync exists to deliver, so those edits stop counting as the client's.
        self.pending_edits.retain(|_, pending| *pending != doc);
        self.projection.forget(doc);
        let Some(snapshot) = self.latest.get(&doc).cloned() else {
            return Ok(()); // nothing open under that id; nothing to describe
        };
        self.on_snapshot(doc, snapshot).await
    }

    /// Exchange greetings and refuse a peer this build cannot talk to.
    ///
    /// Reports whether a client actually arrived: a stream that ends before the
    /// greeting is nobody connecting, which is an ordinary way for a backend to
    /// finish, not a failure to report.
    async fn handshake(&mut self) -> Result<bool, RemoteError> {
        let Some(body) = self.reader.next().await? else {
            return Ok(false);
        };
        // A handshake that will not decode is fatal, unlike a later frame: there
        // is no session yet to keep alive by skipping it.
        let ClientFrame::Hello(hello) = super::wire::decode(&body)? else {
            return Err(RemoteError::Protocol("expected a greeting".to_owned()));
        };
        hello.accept()?;
        let greeting = super::wire::encode(&ServerFrame::Hello(Hello::current()))?;
        frame::write(&mut self.writer, &greeting).await?;
        Ok(true)
    }

    /// Handle one client frame, reporting whether the connection continues.
    async fn on_client_frame(
        &mut self,
        body: &[u8],
        backend: &crate::backend::LocalBackend,
    ) -> Result<bool, RemoteError> {
        // A frame this build cannot name comes from a newer peer. Skipping it
        // loses that one feature; failing would lose the session.
        let frame: ClientFrame = match super::wire::decode(body) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "skipping an unreadable client frame");
                return Ok(true);
            },
        };
        match frame {
            ClientFrame::Hello(_) => Ok(true), // a repeated greeting is harmless
            ClientFrame::Attach { last_seq } => {
                self.attach(last_seq).await?;
                Ok(true)
            },
            ClientFrame::Resync { doc } => {
                self.resync(doc).await?;
                Ok(true)
            },
            ClientFrame::Command { id, command } => {
                if let Command::ApplyChange { doc, .. } = command.as_ref() {
                    self.pending_edits.insert(id, *doc);
                }
                backend
                    .send(id, *command)
                    .map_err(|error| RemoteError::Protocol(error.to_string()))?;
                Ok(true)
            },
        }
    }

    /// Answer an attach, replaying from `last_seq` when it is still in the ring.
    async fn attach(&mut self, last_seq: u64) -> Result<(), RemoteError> {
        let resumable = last_seq > 0
            && self
                .replay
                .front()
                .is_some_and(|(oldest, _)| *oldest <= last_seq.saturating_add(1));
        if !resumable {
            // The client holds no usable replicas; everything it is told next
            // must be complete.
            self.projection.reset();
        }
        let attached = super::wire::encode(&ServerFrame::Attached {
            resumed: resumable,
            view_state: None,
        })?;
        frame::write(&mut self.writer, &attached).await?;
        if !resumable {
            return Ok(());
        }
        let pending: Vec<Vec<u8>> = self
            .replay
            .iter()
            .filter(|(seq, _)| *seq > last_seq)
            .map(|(_, body)| body.clone())
            .collect();
        for body in pending {
            frame::write(&mut self.writer, &body).await?;
        }
        Ok(())
    }

    /// Forward one session event, attributing edit versions on the way past.
    async fn on_event(&mut self, id: Option<RequestId>, event: Event) -> Result<(), RemoteError> {
        match &event {
            // The client produced this version itself, so its text must not be
            // echoed back at it.
            Event::Applied { doc, version } => {
                if id.is_some_and(|id| self.pending_edits.remove(&id).is_some()) {
                    self.projection.client_reached(*doc, *version);
                }
            },
            Event::Closed { doc } => {
                self.projection.forget(*doc);
                self.latest.remove(doc);
            },
            // Any other answer to a pending edit is the session refusing it — a
            // stale or overlapping change, or a document that has gone. The
            // client applied that edit to its replica optimistically and is now a
            // version ahead of the document, while this connection still believes
            // the two agree. Forgetting what was sent makes the snapshot that
            // follows describe the document from scratch rather than as a delta
            // the client can no longer place. Clearing the entry also stops one
            // leaking per refused edit for the life of the connection.
            _ => {
                if let Some(id) = id
                    && let Some(doc) = self.pending_edits.remove(&id)
                {
                    tracing::warn!(?doc, "the session refused a client edit; re-describing it");
                    self.projection.forget(doc);
                }
            },
        }
        self.send_event(id, event).await
    }

    /// Number, record and write one event.
    async fn send_event(&mut self, id: Option<RequestId>, event: Event) -> Result<(), RemoteError> {
        self.seq = self.seq.saturating_add(1);
        let frame = ServerFrame::Event {
            seq: self.seq,
            id,
            event: Box::new(event),
        };
        // A payload that refuses to encode is one that must not travel — a GitHub
        // token is the deliberate case. Drop the event, keep the session.
        let body = match super::wire::encode(&frame) {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(%error, "dropping an event that cannot cross a connection");
                return Ok(());
            },
        };
        if self.replay.len() >= REPLAY_CAPACITY {
            self.replay.pop_front();
        }
        self.replay.push_back((self.seq, body.clone()));
        frame::write(&mut self.writer, &body).await
    }
}

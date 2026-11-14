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
use crate::api::DocumentCommand;
use crate::api::Event;
use crate::api::RequestId;
use crate::api::TextUpdate;
use crate::backend::Backend;
use crate::session::Session;
use crate::session::SessionConfig;

/// Serve one client over `reader`/`writer` until either side closes.
///
/// The session is created here and dropped when the connection ends: a served
/// session lives exactly as long as its one connection. Nothing survives a
/// disconnect to resume from, so every attach is answered as a fresh one (see
/// `docs/remote.md`).
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
    // Frames go out through `frame::drain`, beside the connection loop rather
    // than inside it, so a large event being written never stops this end
    // reading a large command the client is writing at the same moment.
    let (outgoing, queue) = tokio::sync::mpsc::unbounded_channel();
    let serving = Connection::new(reader, outgoing).run(&backend, snapshots, roots, loaded);
    let writing = frame::drain(writer, queue);
    tokio::pin!(serving);
    tokio::pin!(writing);
    tokio::select! {
        // The connection owns the queue's sender, so its ending closes the queue
        // and the writer finishes once it has written what was already queued.
        served = &mut serving => served.and(writing.await),
        // A writer only ends early on a failed write: nothing more can be sent.
        written = &mut writing => written,
    }
}

/// One served connection's state.
struct Connection<R> {
    /// Framed, and cancel-safe: this reader is polled from a `select!` arm that
    /// loses the race whenever an event or a snapshot is ready first.
    reader: FrameReader<R>,
    /// Frame bodies for the connection's writer, which runs beside this loop.
    outgoing: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    /// Monotonic sequence for outgoing events. The wire carries it so a future
    /// resume can name where a client got to; no frame is retained for one.
    seq: u64,
    projection: Projection,
    /// Commands the client submitted that will move a document, so their
    /// answering versions can be attributed to the client rather than the backend.
    pending_edits: std::collections::HashMap<RequestId, crate::api::DocumentId>,
    /// A client edit forwarded to the session and not yet answered. While one is
    /// outstanding no further client frame is read: an edit stacked on it is only
    /// placeable if it was applied, and the session processes commands in order,
    /// so forwarding the next one before this answer lands would let the session
    /// apply it after refusing the edit it was composed on.
    awaiting: Option<RequestId>,
    /// Per document, the sequence number of the re-description a client must have
    /// received before its edits to that document can be placed. Set when the
    /// session refuses one of the client's edits — `u64::MAX` until the complete
    /// render that follows is sent, then that render's number. The client numbers
    /// optimistic edits with the backend's own counter, so an edit composed on a
    /// refused one can carry a base version that matches the document's while
    /// describing text the document never held; only what the client had seen
    /// tells the two apart.
    fences: std::collections::HashMap<crate::api::DocumentId, u64>,
    /// The most recent snapshot of each open document, so a resync can be
    /// answered immediately rather than when the document next happens to change.
    /// An `Arc` sharing the session's own rope, so retaining it costs a pointer.
    latest: std::collections::HashMap<
        crate::api::DocumentId,
        std::sync::Arc<crate::local::DocSnapshot>,
    >,
}

impl<R> Connection<R>
where
    R: AsyncBufRead + Unpin + Send,
{
    fn new(reader: R, outgoing: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) -> Self {
        Self {
            reader: FrameReader::new(reader),
            outgoing,
            seq: 0,
            projection: Projection::default(),
            pending_edits: std::collections::HashMap::new(),
            awaiting: None,
            fences: std::collections::HashMap::new(),
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
                incoming = self.reader.next(), if self.awaiting.is_none() => match incoming? {
                    Some(body) => {
                        if !self.on_client_frame(&body, backend).await? {
                            return Ok(()); // the client said goodbye
                        }
                    },
                    None => return Ok(()),
                },
                snapshot = snapshots.recv() => {
                    if let Some((doc, snapshot)) = snapshot {
                        // `biased` orders branches only within one poll. The
                        // session actor runs on another task, so it can send an
                        // edit's `Applied` and then its snapshot after this poll
                        // found the event stream empty, and the snapshot would
                        // still be projected first — sending the client its own
                        // keystroke back. Every event already sent is forwarded
                        // before the snapshot, which restores the order the
                        // session emitted them in.
                        while let Some((id, event)) = events.try_recv() {
                            self.on_event(id, event).await?;
                        }
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
        // The re-description a refusal is waiting for: the client's edits to this
        // document are placeable again once it has received this frame. Only the
        // first one counts — a resync answered after it describes the same text
        // again, and an edit composed on the first is already placeable.
        if matches!(update.text, TextUpdate::Full(_))
            && let Some(fence) = self.fences.get_mut(&doc)
            && *fence == u64::MAX
        {
            *fence = self.seq.saturating_add(1);
        }
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
        self.queue(greeting)?;
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
            // The position a client names is not consulted: this session began
            // with the connection, so it has nothing earlier to resume from.
            ClientFrame::Attach { last_seq: _ } => {
                self.attach().await?;
                Ok(true)
            },
            ClientFrame::Resync { doc } => {
                self.resync(doc).await?;
                Ok(true)
            },
            ClientFrame::Command { id, command, seen } => {
                if let Command::Document(DocumentCommand::ApplyChange { doc, .. }) =
                    command.as_ref()
                {
                    // Composed before the client heard that an earlier edit was
                    // refused, so its positions describe that edit's text, not the
                    // document's. The re-description already on its way puts the
                    // client back on the document; nothing is answered, because
                    // the refusal that set the fence already told the user.
                    if let (Some(seen), Some(fence)) = (seen, self.fences.get(doc))
                        && seen < *fence
                    {
                        tracing::debug!(?doc, "dropping a client edit composed on refused text");
                        return Ok(true);
                    }
                    self.pending_edits.insert(id, *doc);
                    self.awaiting = Some(id);
                }
                backend
                    .send(id, *command)
                    .map_err(|error| RemoteError::Protocol(error.to_string()))?;
                Ok(true)
            },
        }
    }

    /// Answer an attach as a fresh one: never resumed, and with no view state.
    async fn attach(&mut self) -> Result<(), RemoteError> {
        // The client holds no usable replicas; everything it is told next must
        // be complete.
        self.projection.reset();
        let attached = super::wire::encode(&ServerFrame::Attached {
            resumed: false,
            view_state: None,
        })?;
        self.queue(attached)
    }

    /// Forward one session event, attributing edit versions on the way past.
    async fn on_event(&mut self, id: Option<RequestId>, event: Event) -> Result<(), RemoteError> {
        if id.is_some() && id == self.awaiting {
            self.awaiting = None;
        }
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
                self.fences.remove(doc);
            },
            // Any other answer to a pending edit is the session refusing it — a
            // stale or overlapping change, or a document that has gone. The
            // client applied that edit to its replica optimistically and is now a
            // version ahead of the document, while this connection still believes
            // the two agree. Forgetting what was sent makes the snapshot that
            // follows describe the document from scratch rather than as a delta
            // the client can no longer place. Clearing the entry also stops one
            // leaking per refused edit for the life of the connection.
            //
            // Every other edit of the document still pending stops counting as the
            // client's too, as a resync does: each was composed on the refused one,
            // and crediting the version one produces would project the document as
            // unchanged after the re-description and leave the client a version
            // ahead of text it never received. The fence then refuses whatever the
            // client composes before that re-description reaches it.
            _ => {
                if let Some(id) = id
                    && let Some(doc) = self.pending_edits.remove(&id)
                {
                    tracing::warn!(?doc, "the session refused a client edit; re-describing it");
                    self.pending_edits.retain(|_, pending| *pending != doc);
                    self.projection.forget(doc);
                    self.fences.insert(doc, u64::MAX);
                }
            },
        }
        self.send_event(id, event).await
    }

    /// Number and write one event.
    ///
    /// An event that cannot travel is skipped, never fatal: one lost answer is
    /// better than a lost session. When it answered a request, a notification
    /// takes its place on that request id, so the caller is not left waiting.
    async fn send_event(&mut self, id: Option<RequestId>, event: Event) -> Result<(), RemoteError> {
        // A payload that refuses to encode is one that must not travel — a GitHub
        // token is the deliberate case.
        let body = match self.encode_event(id, event) {
            Ok(body) => body,
            Err(error) => {
                tracing::warn!(%error, "dropping an event that cannot cross a connection");
                return Ok(());
            },
        };
        if frame::fits(body.len()) {
            return self.queue(body);
        }
        tracing::warn!(
            bytes = body.len(),
            "dropping an event too large for one frame"
        );
        if id.is_none() {
            return Ok(());
        }
        let refusal = Event::Notification {
            severity: karet_core::Severity::Error,
            kind: karet_core::NotificationKind::System,
            message: format!(
                "an answer of {} MiB is too large to send over the remote connection",
                body.len() / (1024 * 1024)
            ),
        };
        match self.encode_event(id, refusal) {
            Ok(body) => self.queue(body),
            Err(error) => {
                tracing::warn!(%error, "dropping the refusal of an oversized event");
                Ok(())
            },
        }
    }

    /// Number one event and encode its frame.
    fn encode_event(
        &mut self,
        id: Option<RequestId>,
        event: Event,
    ) -> Result<Vec<u8>, RemoteError> {
        self.seq = self.seq.saturating_add(1);
        super::wire::encode(&ServerFrame::Event {
            seq: self.seq,
            id,
            event: Box::new(event),
        })
    }

    /// Queue one frame body for the connection's writer.
    fn queue(&self, body: Vec<u8>) -> Result<(), RemoteError> {
        self.outgoing
            .send(body)
            .map_err(|_| RemoteError::Protocol("the connection's writer has stopped".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::io::BufReader;
    use tokio::io::Empty;
    use tokio::sync::mpsc;

    use super::*;
    use crate::api::DocumentId;
    use crate::local::DocSnapshot;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A connection with no client behind it, and the frames it queues.
    fn connection() -> (
        Connection<BufReader<Empty>>,
        mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let (outgoing, queued) = mpsc::unbounded_channel();
        (
            Connection::new(BufReader::new(tokio::io::empty()), outgoing),
            queued,
        )
    }

    /// The next event the connection queued, with the request id it answers.
    fn next_event(
        queued: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> Result<(Option<RequestId>, Event), Box<dyn std::error::Error>> {
        let body = queued.try_recv()?;
        let ServerFrame::Event { id, event, .. } = super::super::wire::decode(&body)? else {
            return Err("expected an event frame".into());
        };
        Ok((id, *event))
    }

    fn snapshot(version: u64, text: &str) -> Arc<DocSnapshot> {
        Arc::new(DocSnapshot {
            version,
            buffer: karet_text::TextBuffer::from_text(text),
            highlights: Arc::default(),
            folds: Arc::default(),
            semantic_blocks: Arc::default(),
            syntax_error_lines: Arc::default(),
            table_lines: Arc::default(),
            language: None,
            dirty: false,
            cursor: None,
        })
    }

    /// An answer too large for one frame is skipped, and an error takes its
    /// place on the same request id: the caller waiting on it is told, and the
    /// connection stays up for the next event.
    #[tokio::test]
    async fn an_oversized_answer_is_replaced_by_an_error_on_its_request_id() -> TestResult {
        let (mut connection, mut queued) = connection();
        let id = RequestId(11);
        let oversized = Event::Notification {
            severity: karet_core::Severity::Information,
            kind: karet_core::NotificationKind::System,
            message: "x".repeat(super::frame::MAX_FRAME),
        };

        connection.send_event(Some(id), oversized).await?;
        connection
            .send_event(None, Event::Closed { doc: DocumentId(2) })
            .await?;

        let (answered, answer) = next_event(&mut queued)?;
        assert_eq!(answered, Some(id));
        assert!(
            matches!(
                &answer,
                Event::Notification { severity: karet_core::Severity::Error, message, .. }
                    if message.contains("too large")
            ),
            "{answer:?}"
        );
        let (_, after) = next_event(&mut queued)?;
        assert!(matches!(after, Event::Closed { doc: DocumentId(2) }));
        Ok(())
    }

    /// An oversized event that answered nothing is dropped without a stand-in:
    /// nobody waits on it.
    #[tokio::test]
    async fn an_oversized_unsolicited_event_is_dropped() -> TestResult {
        let (mut connection, mut queued) = connection();

        connection
            .send_event(
                None,
                Event::Notification {
                    severity: karet_core::Severity::Information,
                    kind: karet_core::NotificationKind::System,
                    message: "x".repeat(super::frame::MAX_FRAME),
                },
            )
            .await?;

        assert!(queued.try_recv().is_err());
        Ok(())
    }

    /// A client edit the session refuses leaves the client's replica a version
    /// ahead of the document. The connection forgets what it sent, so the next
    /// snapshot describes the document whole rather than as a delta the client
    /// can no longer place.
    #[tokio::test]
    async fn a_refused_client_edit_makes_the_next_render_complete() -> TestResult {
        let (mut connection, mut queued) = connection();
        let doc = DocumentId(1);
        connection.on_snapshot(doc, snapshot(1, "abc\n")).await?;
        next_event(&mut queued)?;
        connection.pending_edits.insert(RequestId(9), doc);

        connection
            .on_event(
                Some(RequestId(9)),
                Event::Notification {
                    severity: karet_core::Severity::Error,
                    kind: karet_core::NotificationKind::System,
                    message: "stale edit".to_owned(),
                },
            )
            .await?;
        next_event(&mut queued)?;
        connection.on_snapshot(doc, snapshot(2, "abd\n")).await?;

        assert!(connection.pending_edits.is_empty());
        let (_, render) = next_event(&mut queued)?;
        let Event::Render { update, .. } = render else {
            return Err("expected a render".into());
        };
        assert!(matches!(update.text, TextUpdate::Full(ref text) if text == "abd\n"));
        Ok(())
    }

    /// The session's answer refusing one edit.
    fn refusal() -> Event {
        Event::Notification {
            severity: karet_core::Severity::Warning,
            kind: karet_core::NotificationKind::Io,
            message: "refused".to_owned(),
        }
    }

    /// An edit the client stacked on a refused one was composed on text the
    /// document never held. Were it still credited when the session applies it,
    /// the re-description would be projected as unchanged and the client left on
    /// its own text.
    #[tokio::test]
    async fn a_refusal_stops_crediting_every_pending_edit_of_the_document() -> TestResult {
        let (mut connection, mut queued) = connection();
        let doc = DocumentId(1);
        connection.on_snapshot(doc, snapshot(1, "abc\n")).await?;
        next_event(&mut queued)?;
        connection.pending_edits.insert(RequestId(9), doc);
        connection.pending_edits.insert(RequestId(10), doc);
        connection
            .pending_edits
            .insert(RequestId(11), DocumentId(2));

        connection.on_event(Some(RequestId(9)), refusal()).await?;
        next_event(&mut queued)?;
        connection
            .on_event(Some(RequestId(10)), Event::Applied { doc, version: 2 })
            .await?;
        next_event(&mut queued)?;
        connection.on_snapshot(doc, snapshot(2, "xbc\n")).await?;

        assert_eq!(
            connection.pending_edits.len(),
            1,
            "another document's edit stays"
        );
        let (_, render) = next_event(&mut queued)?;
        let Event::Render { update, .. } = render else {
            return Err("expected a render".into());
        };
        assert!(matches!(update.text, TextUpdate::Full(ref text) if text == "xbc\n"));
        Ok(())
    }

    /// An edit forwarded to the session holds back the client's next frame until
    /// it is answered, so a later edit cannot reach the session before the
    /// connection knows whether the one beneath it was applied.
    #[tokio::test]
    async fn a_forwarded_edit_is_awaited_until_answered() -> TestResult {
        let (mut connection, _queued) = connection();
        let (session, _events, _snapshots) = Session::new(SessionConfig::default());
        let backend = crate::backend::local_session(session, None);
        let doc = DocumentId(1);

        connection
            .on_client_frame(&edit_frame(RequestId(4), doc, Some(0))?, &backend)
            .await?;
        assert_eq!(connection.awaiting, Some(RequestId(4)));
        connection
            .on_event(Some(RequestId(4)), Event::Applied { doc, version: 2 })
            .await?;

        assert_eq!(connection.awaiting, None);
        Ok(())
    }

    /// After a refusal the client's optimistic version and the document's can
    /// coincide, so the base version cannot reject an edit composed on the
    /// refused text. What the client had received can: an edit sent before the
    /// re-description arrived is dropped, and one sent after it goes through.
    #[tokio::test]
    async fn an_edit_composed_before_the_re_description_is_dropped() -> TestResult {
        let (mut connection, mut queued) = connection();
        let (session, _events, _snapshots) = Session::new(SessionConfig::default());
        let backend = crate::backend::local_session(session, None);
        let doc = DocumentId(1);
        connection.on_snapshot(doc, snapshot(1, "abc\n")).await?;
        next_event(&mut queued)?;
        connection.pending_edits.insert(RequestId(9), doc);
        connection.on_event(Some(RequestId(9)), refusal()).await?;
        next_event(&mut queued)?;

        // Before the re-description is even sent, nothing the client sends fits.
        connection
            .on_client_frame(&edit_frame(RequestId(10), doc, Some(2))?, &backend)
            .await?;
        assert_eq!(connection.awaiting, None);

        connection.on_snapshot(doc, snapshot(2, "xbc\n")).await?;
        let full_seq = connection.seq;
        connection
            .on_client_frame(
                &edit_frame(RequestId(11), doc, Some(full_seq - 1))?,
                &backend,
            )
            .await?;
        assert_eq!(connection.awaiting, None);
        assert!(connection.pending_edits.is_empty());

        connection
            .on_client_frame(&edit_frame(RequestId(12), doc, Some(full_seq))?, &backend)
            .await?;
        assert_eq!(connection.awaiting, Some(RequestId(12)));
        Ok(())
    }

    /// A peer that does not report what it has seen keeps the unfenced
    /// behaviour rather than having every edit dropped.
    #[tokio::test]
    async fn an_edit_without_a_seen_position_is_never_fenced() -> TestResult {
        let (mut connection, _queued) = connection();
        let (session, _events, _snapshots) = Session::new(SessionConfig::default());
        let backend = crate::backend::local_session(session, None);
        let doc = DocumentId(1);
        connection.fences.insert(doc, u64::MAX);

        connection
            .on_client_frame(&edit_frame(RequestId(3), doc, None)?, &backend)
            .await?;

        assert_eq!(connection.awaiting, Some(RequestId(3)));
        Ok(())
    }

    /// One encoded client edit of `doc`, reporting `seen`.
    fn edit_frame(
        id: RequestId,
        doc: DocumentId,
        seen: Option<u64>,
    ) -> Result<Vec<u8>, RemoteError> {
        super::super::wire::encode(&ClientFrame::Command {
            id,
            command: Box::new(
                DocumentCommand::ApplyChange {
                    doc,
                    change: karet_core::Change::new(2, Vec::new()),
                    cause: karet_core::EditCause::Type,
                }
                .into(),
            ),
            seen,
        })
    }

    /// A frame this build cannot decode comes from a newer client; the backend
    /// skips it and keeps the connection.
    #[tokio::test]
    async fn an_unreadable_client_frame_is_skipped() -> TestResult {
        let (mut connection, mut queued) = connection();
        let (session, _events, _snapshots) = Session::new(SessionConfig::default());
        let backend = crate::backend::local_session(session, None);

        let continues = connection
            .on_client_frame(&[0xff, 0xff, 0xff, 0xff], &backend)
            .await?;

        assert!(continues);
        assert!(queued.try_recv().is_err());
        Ok(())
    }
}

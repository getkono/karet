//! The [`Backend`] seam: the single interface the presentation layer talks to,
//! identical in local mode today and (additively) in a future remote mode.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;

/// How often the actor sweeps for buffers due to be backed up. The per-document
/// dirty threshold is `files.backupInterval`; this only bounds detection latency.
const BACKUP_TICK: Duration = Duration::from_secs(2);

use crate::api::Command;
use crate::api::RequestId;
use crate::local::SnapshotRx;
use crate::session::EventRx;
use crate::session::Session;
use crate::session::SessionConfig;

/// Errors produced when submitting to a [`Backend`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// The backend connection has been closed.
    #[error("the backend connection is closed")]
    Closed,
    /// A transport-level failure (remote mode).
    #[error("transport error: {0}")]
    Transport(String),
}

/// The mode-agnostic seam the UI is written against.
///
/// It is deliberately *not* `async fn`-in-trait (so it stays `dyn`-compatible):
/// submission is synchronous and fallible, while results arrive asynchronously on
/// the session's [`EventRx`](crate::session::EventRx). The same UI code drives an
/// in-process [`LocalBackend`] today and a remote client later.
pub trait Backend: Send + Sync {
    /// Submit `command`, tagged with `id` so its answering event can be correlated.
    ///
    /// # Errors
    /// Returns [`BackendError::Closed`] if the backend is no longer accepting input.
    fn send(&self, id: RequestId, command: Command) -> Result<(), BackendError>;

    /// The next monotonic [`RequestId`] for this connection.
    #[must_use]
    fn next_id(&self) -> RequestId;

    /// Take this connection's [`Event`] stream.
    ///
    /// The stream is single-consumer: the first call yields `Some`, every later
    /// call `None`. Having the stream on the trait — not on a concrete
    /// constructor — is what lets a remote implementation slot in without the
    /// composition root changing.
    #[must_use]
    fn take_events(&self) -> Option<EventRx>;
}

/// An in-process backend that drives a [`Session`] on a background task.
///
/// `send` pushes onto an unbounded command channel (an unbounded send is the only
/// non-`async` send, which the synchronous [`Backend::send`] requires); the actor
/// task drains it in order and the session emits results on its event/snapshot
/// streams.
pub struct LocalBackend {
    commands: mpsc::UnboundedSender<(RequestId, Command)>,
    next: AtomicU64,
    events: std::sync::Mutex<Option<EventRx>>,
}

impl Backend for LocalBackend {
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

/// Build a [`Session`] from `config` and drive it in-process on a spawned task,
/// returning the [`LocalBackend`] to submit commands to plus the local-only
/// [`SnapshotRx`] render stream.
///
/// Must be called within a Tokio runtime context (the app enters one before
/// constructing the backend). The [`Event`](crate::api::Event) stream is obtained
/// from the backend itself ([`Backend::take_events`]); the snapshot stream is
/// returned directly because it is deliberately in-process-only. The actor ends
/// when the returned backend is dropped.
#[must_use]
pub fn local(config: SessionConfig) -> (LocalBackend, SnapshotRx) {
    let (session, events, snaps) = Session::new(config);
    (local_session(session, Some(events)), snaps)
}

/// Drive an already-constructed `session` (whose event/snapshot streams the
/// caller holds) — the lower-level seam used by in-crate tests.
pub(crate) fn local_session(mut session: Session, events: Option<EventRx>) -> LocalBackend {
    let (commands, mut rx) = mpsc::unbounded_channel::<(RequestId, Command)>();
    let (watcher, mut fs_rx) = session.take_watch();
    let mut highlights = session.take_highlights();
    let mut spell_results = session.take_spell_results();
    let mut lsp_updates = session.take_lsp_updates();
    let mut registry_updates = session.take_lsp_registry_updates();
    tokio::spawn(async move {
        // Hold the watcher alive for exactly as long as the actor consumes events.
        let _watcher = watcher;
        // Compute the initial VCS status here, on the actor task, rather than on the
        // construction thread — a large repository's `git status` then runs
        // concurrently with the first frame instead of blocking it.
        session.start();
        // A steady tick drives the crash-recovery backup sweep; the session decides
        // per-document whether the configured dirty interval has elapsed.
        let mut backup = tokio::time::interval(BACKUP_TICK);
        backup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = rx.recv() => match command {
                    Some((id, command)) => session.handle(id, command),
                    None => break, // the backend was dropped
                },
                fs_event = recv_opt(&mut fs_rx) => match fs_event {
                    Some(event) => session.handle_fs_event(event),
                    None => fs_rx = None, // the watcher stopped; stop selecting it
                },
                // Layered highlights computed off-actor; applied (and published) here.
                result = recv_opt(&mut highlights) => match result {
                    Some(result) => session.apply_highlights(result),
                    None => highlights = None, // the worker stopped; stop selecting it
                },
                result = recv_opt(&mut spell_results) => match result {
                    Some(result) => session.apply_spell_result(result),
                    None => spell_results = None,
                },
                // LSP answers computed on the server tasks; converted and emitted here.
                update = recv_opt(&mut lsp_updates) => match update {
                    Some(update) => session.apply_lsp_update(update),
                    None => lsp_updates = None, // no LSP; stop selecting it
                },
                update = recv_opt(&mut registry_updates) => match update {
                    Some(update) => session.apply_lsp_registry_update(update),
                    None => registry_updates = None,
                },
                _ = backup.tick() => session.backup_tick(),
            }
        }
    });
    LocalBackend {
        commands,
        next: AtomicU64::new(1),
        events: std::sync::Mutex::new(events),
    }
}

/// Await the next message on an optional worker stream, or never resolve when
/// the worker is absent/stopped — so the actor's `select!` simply ignores that
/// arm. The one receiver shape shared by every worker the actor drains.
async fn recv_opt<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod merge_conflict_tests;

#[cfg(test)]
mod tests;

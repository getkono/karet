//! The [`Backend`] seam: the single interface the presentation layer talks to,
//! identical in local mode today and (additively) in a future remote mode.

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use karet_watch::FsEvent;
use tokio::sync::mpsc;

use crate::api::Command;
use crate::api::RequestId;
use crate::session::Session;

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
}

/// Drive `session` in-process on a spawned task, returning a [`LocalBackend`] to
/// submit commands to.
///
/// Must be called within a Tokio runtime context (the app enters one before
/// constructing the backend). The session's [`EventRx`](crate::session::EventRx)
/// and [`SnapshotRx`](crate::local::SnapshotRx) (from [`Session::new`]) are the
/// matching output streams; the actor ends when the returned backend is dropped.
///
/// [`Session::new`]: crate::session::Session::new
#[must_use]
pub fn local(mut session: Session) -> LocalBackend {
    let (commands, mut rx) = mpsc::unbounded_channel::<(RequestId, Command)>();
    let (watcher, mut fs_rx) = session.take_watch();
    tokio::spawn(async move {
        // Hold the watcher alive for exactly as long as the actor consumes events.
        let _watcher = watcher;
        loop {
            tokio::select! {
                command = rx.recv() => match command {
                    Some((id, command)) => session.handle(id, command),
                    None => break, // the backend was dropped
                },
                fs_event = recv_fs(&mut fs_rx) => match fs_event {
                    Some(event) => session.handle_fs_event(event),
                    None => fs_rx = None, // the watcher stopped; stop selecting it
                },
            }
        }
    });
    LocalBackend {
        commands,
        next: AtomicU64::new(1),
    }
}

/// Await the next filesystem event, or never resolve when there is no watcher (so
/// the actor's `select!` simply ignores that arm).
async fn recv_fs(rx: &mut Option<mpsc::UnboundedReceiver<FsEvent>>) -> Option<FsEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_displays() {
        assert_eq!(
            BackendError::Closed.to_string(),
            "the backend connection is closed"
        );
    }

    #[tokio::test]
    async fn local_backend_drives_open() {
        use crate::api::Event;
        use crate::session::Session;
        use crate::session::SessionConfig;

        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("hello.txt");
        if std::fs::write(&path, "hello\n").is_err() {
            return;
        }

        let (session, mut events, _snaps) = Session::new(SessionConfig::default());
        let backend = local(session);
        let id = backend.next_id();
        assert!(
            backend
                .send(
                    id,
                    Command::OpenDocument {
                        path,
                        language: None
                    }
                )
                .is_ok()
        );

        // The actor processes the command and emits Opened on the event stream.
        let received = events.recv().await;
        assert!(
            matches!(received, Some((_, Event::Opened { .. }))),
            "local backend should drive the session to open the file, got {received:?}"
        );
    }
}

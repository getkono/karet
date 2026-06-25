//! The [`Backend`] seam: the single interface the presentation layer talks to,
//! identical in local mode today and (additively) in a future remote mode.

use crate::api::{Command, RequestId};
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
pub struct LocalBackend {}

impl Backend for LocalBackend {
    fn send(&self, id: RequestId, command: Command) -> Result<(), BackendError> {
        let _ = (id, command);
        todo!()
    }

    fn next_id(&self) -> RequestId {
        todo!()
    }
}

/// Drive `session` in-process, returning a [`LocalBackend`] to submit commands to.
///
/// The session's [`EventRx`](crate::session::EventRx) (from [`Session::new`]) is the
/// matching event stream.
///
/// [`Session::new`]: crate::session::Session::new
#[must_use]
pub fn local(session: Session) -> LocalBackend {
    let _ = session;
    todo!()
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
}

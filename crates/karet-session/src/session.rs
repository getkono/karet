//! The owned, headless editor model: [`Session`] and its read/event surface.

use crate::api::{Command, DocumentId, Event, RequestId};
use karet_core::Decoration;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Errors produced by the backend session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// A command referenced a document that is not open.
    #[error("unknown document")]
    UnknownDocument,
    /// An underlying engine reported an error.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Configuration for a [`Session`].
#[derive(Clone, Debug, Default)]
pub struct SessionConfig {
    /// Workspace root directories.
    pub roots: Vec<PathBuf>,
    /// Run format-on-save.
    pub format_on_save: bool,
    /// Enable spell-checking of comments/strings.
    pub spellcheck: bool,
}

/// The headless editor backend: owns documents and the workspace, orchestrates
/// the producer engines, applies [`Command`]s and emits [`Event`]s.
///
/// Construct with [`Session::new`], which also returns the [`EventRx`] half of the
/// event stream; drive it in-process with [`crate::backend::local`].
pub struct Session {
    config: SessionConfig,
    events: mpsc::UnboundedSender<(Option<RequestId>, Event)>,
}

impl Session {
    /// Create a session and its paired event receiver.
    #[must_use]
    pub fn new(config: SessionConfig) -> (Self, EventRx) {
        let (events, rx) = mpsc::unbounded_channel();
        (Self { config, events }, EventRx(rx))
    }

    /// Handle one request. Fast paths (open/apply/save) resolve inline; async
    /// producer requests are spawned and their answering [`Event`] is delivered
    /// later on the event stream, tagged with `id`.
    pub fn handle(&mut self, id: RequestId, command: Command) {
        let _ = (id, command, &self.config, &self.events);
        todo!()
    }

    /// Borrow a read-only view of a document for local-mode rendering.
    #[must_use]
    pub fn document(&self, doc: DocumentId) -> Option<DocumentView<'_>> {
        let _ = (doc, &self.config);
        todo!()
    }
}

/// A read-only borrow of a document's renderable state (local mode).
///
/// In a future remote split this is replaced by a client-side snapshot replicated
/// from [`Event`]s; the renderer (`karet-editor`) consumes the same data either way.
pub struct DocumentView<'a> {
    buffer: &'a TextBuffer,
    highlights: &'a Highlights,
    decorations: &'a [Decoration],
    version: u64,
}

impl DocumentView<'_> {
    /// The document's text buffer.
    #[must_use]
    pub fn buffer(&self) -> &TextBuffer {
        self.buffer
    }

    /// The document's syntax highlights.
    #[must_use]
    pub fn highlights(&self) -> &Highlights {
        self.highlights
    }

    /// The document's decorations (merged across producers).
    #[must_use]
    pub fn decorations(&self) -> &[Decoration] {
        self.decorations
    }

    /// The document's current version.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }
}

/// The receiving half of a session's server→client event stream.
pub struct EventRx(mpsc::UnboundedReceiver<(Option<RequestId>, Event)>);

impl EventRx {
    /// Receive the next event, with the [`RequestId`] it answers (if any).
    ///
    /// Returns `None` once the session has shut down.
    pub async fn recv(&mut self) -> Option<(Option<RequestId>, Event)> {
        self.0.recv().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_constructs_with_event_stream() {
        let (_session, _events) = Session::new(SessionConfig::default());
    }

    #[test]
    fn config_and_error() {
        let cfg = SessionConfig::default();
        assert!(!cfg.format_on_save);
        assert!(cfg.roots.is_empty());
        assert_eq!(
            SessionError::UnknownDocument.to_string(),
            "unknown document"
        );
    }
}

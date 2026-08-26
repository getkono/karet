//! Message framing seam.
//!
//! Deliberately local to `karet-supervisor`: the skeleton must not inherit a
//! protocol crate's framing types. If a shared framing trait later lands in a
//! JSON-RPC crate, only [`crate::broker::lsp::ContentLength`] changes — it
//! becomes a delegating adapter — and this trait stays the broker's contract.

use std::io;

use serde_json::Value;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;

/// Frames JSON-RPC messages onto a byte stream.
///
/// Implementors are stateless strategies: the broker skeleton never holds a
/// framing value, it only names the type through
/// [`BrokerProtocol::Framing`](crate::broker::BrokerProtocol::Framing).
///
/// The futures are spelled as `impl Future<Output = _> + Send` rather than
/// `async fn` because the skeleton `tokio::spawn`s them; implementors may still
/// write a plain `async fn`.
pub trait Framing: Send + Sync + 'static {
    /// Read one framed message, or `None` at a clean end of stream.
    ///
    /// # Errors
    /// Returns the transport error, or [`io::ErrorKind::InvalidData`] when the
    /// frame or its payload is malformed.
    fn read_message<R>(reader: &mut R) -> impl Future<Output = io::Result<Option<Value>>> + Send
    where
        R: AsyncBufRead + Unpin + Send;

    /// Write one framed message and flush it.
    ///
    /// # Errors
    /// Returns the transport error, or [`io::ErrorKind::InvalidData`] when the
    /// message cannot be serialised.
    fn write_message<W>(
        writer: &mut W,
        message: &Value,
    ) -> impl Future<Output = io::Result<()>> + Send
    where
        W: AsyncWrite + Unpin + Send;
}

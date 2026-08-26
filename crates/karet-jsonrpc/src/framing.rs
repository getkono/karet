//! Wire framing: how one JSON-RPC message body is delimited on a byte stream.
//!
//! JSON-RPC 2.0 says nothing about framing, so every protocol built on it picks
//! one. The two in practice both live here: [`content_length`] (the LSP/DAP base
//! protocol, an HTTP-style header block) and [`line_delimited`] (one compact JSON
//! document per line, as ACP and JSON-RPC-over-stdio generally use).
//!
//! The [`Framing`] trait is the seam the connection actor is generic over. It
//! moves **bytes**, not `serde_json::Value`s: the actor parses once, and a framing
//! implementation never needs serde.

use std::future::Future;

use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;

pub mod content_length;
pub mod line_delimited;

/// The largest message body any framing in this crate will read, guarding
/// against a corrupt or hostile length allocating unbounded memory.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// A wire framing for JSON-RPC message bodies.
///
/// Implementations are stateless marker types; both methods are associated
/// functions over a caller-owned stream, so one framing can serve many
/// connections. The returned futures are explicitly `Send` because the
/// connection actor drives them inside `tokio::spawn`ed tasks.
pub trait Framing: Send + Sync + 'static {
    /// Framing failures while reading a message.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Read one framed body, or `None` on a clean EOF between messages.
    ///
    /// # Errors
    ///
    /// Returns [`Framing::Error`] when the stream fails or the bytes on it do
    /// not form a valid frame — including EOF part-way through a message.
    fn read_frame<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, Self::Error>> + Send
    where
        R: AsyncBufRead + Send + Unpin;

    /// Write `body` as one frame and flush.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`std::io::Error`] if the write or flush fails.
    fn write_frame<W>(
        writer: &mut W,
        body: &[u8],
    ) -> impl Future<Output = std::io::Result<()>> + Send
    where
        W: AsyncWrite + Send + Unpin;
}

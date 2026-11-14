//! Remote mode: the same [`Backend`](crate::backend::Backend) seam, over a byte
//! stream.
//!
//! The presentation layer and the session it drives need not share a machine. The
//! session keeps the documents, git, language servers and the workspace itself;
//! the client renders. Only edits and derived data cross the gap, so typing is
//! answered by the machine the user is sitting at rather than by the network.
//!
//! # What this deliberately is not
//!
//! There is no transport here. [`serve`] and [`connect`] take any
//! `AsyncRead`/`AsyncWrite` pair and never learn where it came from — a pipe, a
//! socket, the stdio of `ssh host karet --serve`, or a channel a terminal
//! multiplexer forwarded. karet gains no TLS stack, no connection UX and no
//! `known_hosts`, because supplying a stream is a solved problem owned by
//! something else.
//!
//! # The split
//!
//! ```text
//!   client host                          workspace host
//!   ───────────                          ──────────────
//!   RemoteBackend  ──ClientFrame──▶      serve()
//!     replicas     ◀──ServerFrame──        Session (documents, vcs, lsp, …)
//!     snapshots ──▶ the renderer            snapshots ──▶ RenderUpdate
//! ```
//!
//! The client holds a *replica* of each open document — derived and discardable,
//! never authoritative — so it can echo a keystroke without waiting for a round
//! trip, and rebuilds the [`DocSnapshot`](crate::local::DocSnapshot) stream the
//! renderer already draws from. That is what makes remote mode additive: no
//! rendering code can tell which mode it is in.

mod client;
mod delta;
mod frame;
mod project;
mod replica;
mod serve;
mod wire;

#[cfg(test)]
mod tests;

pub use client::RemoteBackend;
pub use client::connect;
pub use serve::serve;

/// Something went wrong on a remote connection.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RemoteError {
    /// The underlying stream failed.
    #[error("remote transport: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes on the stream did not form a message this build can act on.
    #[error("remote protocol: {0}")]
    Protocol(String),
}

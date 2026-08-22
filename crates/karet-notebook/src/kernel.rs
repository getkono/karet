//! Kernel lifecycle (feature `kernel`): discover installed kernelspecs,
//! mint a connection file, and drive one kernel over the Jupyter wire
//! protocol — pure-Rust ZMTP sockets, hmac-sha256 signing, and
//! `jupyter-protocol`'s message vocabulary.
//!
//! The transport is a seam ([`KernelTransport`]): production speaks ZMQ
//! ([`ZmqTransport`]), unit tests script an in-process fake — no sockets, no
//! kernels. Spawning the kernel *process* is deliberately not here: the
//! consumer owns processes (karet routes them through its supervisor) and
//! hands this module a connection the process was told about.

mod client;
mod connection;
mod spec;
mod transport;

pub use client::CellOutcome;
pub use client::KernelClient;
pub use connection::local_connection;
pub use connection::substitute_argv;
pub use connection::write_connection_file;
pub use jupyter_protocol::ConnectionInfo;
pub use spec::KernelSpec;
pub use spec::default_dirs;
pub use spec::discover;
pub use spec::discover_in;
pub use spec::find;
pub use transport::KernelChannel;
pub use transport::KernelControl;
pub use transport::KernelTransport;
pub use transport::ZmqTransport;

/// Errors produced by the kernel client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KernelError {
    /// The transport failed (socket error, signature mismatch, bad frame).
    #[error("kernel transport error: {0}")]
    Transport(String),
    /// The kernel did not answer within the deadline.
    #[error("the kernel did not respond in time")]
    Timeout,
    /// A message could not be encoded or decoded.
    #[error("kernel protocol error: {0}")]
    Protocol(String),
}

//! Cross-process language-server broker.
//!
//! One hidden broker owns each `(server launch, repository root, karet protocol
//! version)` tuple. Editor processes connect over an authenticated loopback socket
//! and speak ordinary LSP; the broker rewrites JSON-RPC request identifiers,
//! broadcasts server notifications, and reference-counts document opens. This
//! prevents several karet windows from multiplying expensive server processes.
//! Brokers retire after an idle grace period, and stale endpoint files are
//! replaced atomically by the next connector.
//!
//! # Layout
//!
//! The process-management skeleton and the message semantics are separate, so a
//! future agent daemon can reuse the former without inheriting LSP:
//!
//! - [`key`], [`endpoint`], [`lease`] and [`serve`] are the skeleton — broker
//!   identity, the published endpoint file, `O_EXCL` election plus the hidden
//!   entry point, and the accept/serve loop with its pending-request map.
//! - [`framing`] and [`protocol`] are the seam: [`Framing`] says how messages sit
//!   on the wire, [`BrokerProtocol`] says what they mean.
//! - [`lsp`] is the one implementation, and the **only** module here permitted to
//!   name the `karet-lsp` crate. Keep it that way — `grep -rn` for that crate
//!   across `src/` should find nothing outside `src/broker/lsp.rs`: nothing in
//!   the skeleton may learn what `initialize` or `textDocument/didOpen` is.

mod endpoint;
mod failure;
mod framing;
mod key;
mod lease;
mod lsp;
mod protocol;
mod serve;

pub use failure::BrokeredLaunchFailure;
pub use framing::Framing;
pub use key::Launch;
pub use lsp::MODE_ENV;
pub use lsp::connect;
pub use lsp::managed_payload_in_use;
pub use lsp::requested;
pub use lsp::run_from_env;
pub use protocol::BrokerProtocol;
pub use protocol::ClientFlow;
pub use protocol::ClientId;
pub use protocol::ClientLink;
pub use protocol::ServerLink;
pub use protocol::ServerRoute;

/// Connect to the shared LSP broker for `launch`, and say which broker
/// answered.
///
/// [`connect`] is the same call with the identity dropped. A caller that has to
/// decide, after a handshake fails, whether its *own* broker is the reason
/// needs the process id: `{key}.error` is one path shared by every broker the
/// key ever had, and the verdict it carries is permanent, so a report credited
/// to the wrong attempt retires a server that never failed. Pair it with
/// [`reported_failure`].
///
/// # Errors
/// As [`connect`].
pub async fn connect_observed(
    executable: &std::path::Path,
    state_root: &std::path::Path,
    launch: &Launch,
) -> Result<(tokio::net::TcpStream, u32), BrokerError> {
    lease::connect_observed::<lsp::LspBroker>(executable, state_root, launch).await
}

/// What the broker `pid` reported about `launch`, if it reported anything.
///
/// The only positive evidence that a brokered server is the thing that failed.
/// A closed socket is not: on this path a close is a fact about a TCP
/// connection, which may not even have been our broker's.
#[must_use]
pub fn reported_failure(
    state_root: &std::path::Path,
    launch: &Launch,
    pid: u32,
) -> Option<BrokeredLaunchFailure> {
    lease::reported::<lsp::LspBroker>(state_root, launch, pid)
}

/// Errors returned while locating or starting a broker.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrokerError {
    /// The brokered process itself failed to run.
    ///
    /// Distinct from [`BrokerError::Io`] because the two need opposite
    /// responses: a broker that could not be reached may answer on the next
    /// attempt, and a server that exits on sight will not.
    #[error("{0}")]
    Launch(Box<BrokeredLaunchFailure>),
    /// Broker state or transport I/O failed.
    #[error("language-server broker I/O failed: {0}")]
    Io(String),
    /// Hidden broker launch state was invalid.
    #[error("invalid language-server broker state: {0}")]
    Spec(String),
}

pub(crate) fn io_error(error: impl std::fmt::Display) -> BrokerError {
    BrokerError::Io(error.to_string())
}

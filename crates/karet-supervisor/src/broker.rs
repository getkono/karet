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

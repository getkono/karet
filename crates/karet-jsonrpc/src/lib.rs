//! `karet-jsonrpc` — a protocol-agnostic async JSON-RPC 2.0 client core.
//!
//! JSON-RPC 2.0 is the transport under LSP, DAP, and ACP alike, and the parts
//! that are *not* protocol-specific are substantial: allocating request ids,
//! correlating responses through a pending map, bounding every request with a
//! timeout, draining a bounded outbound queue, failing all in-flight requests on
//! EOF, and closing politely. That machinery lives here once, with **no karet
//! dependencies at all**, so a consumer speaking one protocol never inherits
//! another protocol's model crates.
//!
//! Two seams keep it protocol-agnostic:
//!
//! - [`Framing`] — how a message body is delimited on the wire.
//!   [`framing::content_length`] is the LSP/DAP base protocol;
//!   [`framing::line_delimited`] is newline-delimited JSON. Framing moves
//!   **bytes**, so an implementation never touches serde.
//! - [`Handler`] — the broadcast payload built from peer notifications, the
//!   answers to peer→client requests, notification side effects, and the tuning
//!   constants (timeouts, channel capacities, the log-facing peer name).
//!
//! ```no_run
//! use karet_jsonrpc::Connection;
//! use karet_jsonrpc::Handler;
//! use karet_jsonrpc::framing::content_length::ContentLength;
//! use serde_json::Value;
//!
//! struct Echo;
//! impl Handler for Echo {
//!     type Framing = ContentLength;
//!     type Push = (String, Value);
//!     fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
//!         Some((method.to_owned(), params.clone()))
//!     }
//! }
//!
//! # async fn run(read: tokio::io::DuplexStream, write: tokio::io::DuplexStream) {
//! let connection = Connection::start(Echo, read, write);
//! let _: Result<Value, _> = connection.request("initialize", Value::Null).await;
//! # }
//! ```

mod connection;
pub mod framing;
mod message;

pub use connection::Connection;
pub use connection::Handler;
pub use connection::RpcError;
pub use framing::Framing;
pub use message::Incoming;
pub use message::JSONRPC_VERSION;
pub use message::METHOD_NOT_FOUND;
pub use message::OutgoingNotification;
pub use message::OutgoingRequest;
pub use message::OutgoingResponse;
pub use message::RequestId;
pub use message::ResponseError;
pub use message::classify;

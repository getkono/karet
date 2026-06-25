//! `karet-session` — the headless editor backend for the karet toolkit.
//!
//! This is the business-logic (server) half of karet. A [`Session`] owns the open
//! documents and workspace, orchestrates the headless producer engines
//! (`karet-lsp`, `karet-dap`, `karet-vcs`, `karet-search`, `karet-terminal`),
//! applies editing [`Command`]s and emits [`Event`]s. It pulls in **no** ratatui:
//! the presentation/client half (the `karet` app, `karet-editor`, `karet-widgets`)
//! talks to it only through the [`Command`]/[`Event`] vocabulary in [`api`] and the
//! [`Backend`] seam in [`backend`].
//!
//! Because the seam is message-passing over neutral models, a future client-server
//! split is *additive*: lift [`api`] into a `karet-protocol` crate, add a remote
//! `Backend` implementation, and the UI code is unchanged.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! document/producer orchestration (plus the migrated format-on-save, spell-check
//! and settings/session-restore logic) is filled in separately.

pub mod api;
pub mod backend;
pub mod session;

pub use api::{Command, DecorationLayer, DocumentId, Event, RequestId, ViewId};
pub use backend::{Backend, BackendError, LocalBackend, local};
pub use session::{DocumentView, EventRx, Session, SessionConfig, SessionError};

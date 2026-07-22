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
//! The document store, the editing fast paths (open / apply / save / undo / redo),
//! incremental tree-sitter highlighting, file-watching, and LSP completions (lazy
//! per-language servers) are live; the remaining producers (format-on-save,
//! spell-check, …) attach in later milestones.
//! In local mode the UI renders from the [`DocSnapshot`]s pushed on the snapshot
//! stream (`local`), not by borrowing a [`DocumentView`] across the actor boundary.

#[cfg(feature = "aicommit")]
mod aicommit;
pub mod api;
pub mod backend;
pub mod backup;
pub mod config;
mod highlight;
pub mod local;
mod lsp;
pub mod session;
mod vcs_worker;
pub mod viz;

pub use api::Command;
pub use api::DecorationLayer;
pub use api::DocumentId;
pub use api::Event;
pub use api::GithubAuth;
pub use api::GithubAuthSource;
pub use api::GithubCheckRun;
pub use api::GithubComment;
pub use api::GithubIssue;
pub use api::GithubLabel;
pub use api::GithubNewIssue;
pub use api::GithubNewPullRequest;
pub use api::GithubPage;
pub use api::GithubPullRequest;
pub use api::GithubPullRequestActivity;
pub use api::GithubPullRequestCommit;
pub use api::GithubRepository;
pub use api::GithubToken;
pub use api::GithubVerification;
pub use api::GithubWorkflow;
pub use api::GithubWorkflowRun;
pub use api::GraphKind;
pub use api::PullRequestSummary;
pub use api::RangeSpec;
pub use api::RepositorySnapshot;
pub use api::RequestId;
pub use api::SwapInfo;
pub use api::VcsAction;
pub use api::VcsOutcome;
pub use api::ViewId;
pub use backend::Backend;
pub use backend::BackendError;
pub use backend::LocalBackend;
pub use backend::local;
pub use config::ConfigDiagnostic;
pub use config::ConfigLayer;
pub use config::ConfigLayerReport;
pub use config::ConfigLayerStatus;
pub use config::LoadedConfig;
pub use config::Settings;
pub use local::DocSnapshot;
pub use local::SnapshotRx;
pub use session::DocumentView;
pub use session::EventRx;
pub use session::Session;
pub use session::SessionConfig;
pub use session::SessionError;

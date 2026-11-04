//! `karet-session` — the headless editor backend for the karet toolkit.
//!
//! This is the business-logic (server) half of karet. A session, started from a
//! [`SessionConfig`] by [`local`], owns the open documents and workspace, orchestrates the headless producer engines (see the
//! worker map below), applies editing [`Command`]s and emits [`Event`]s. It pulls
//! in **no** ratatui:
//! the presentation/client half (the `karet` app, `karet-editor`, `karet-widgets`)
//! talks to it only through the [`Command`]/[`Event`] vocabulary in [`api`] and the
//! [`Backend`] seam in [`backend`].
//!
//! Because the seam is message-passing over neutral models, a future client-server
//! split is *additive*: lift [`api`] into a `karet-protocol` crate, add a remote
//! `Backend` implementation, and the UI code is unchanged.
//!
//! Besides the document store and the editing fast paths (open / apply / save /
//! undo / redo), the session runs one module per producer or worker:
//!
//! - `highlight` — layered (injection-aware) tree-sitter highlighting on a
//!   background worker.
//! - `lsp` / `lsp_registry` — per-language servers over `karet-lsp`, started
//!   lazily (completions, hover, format-on-save edits, …), and the machine-local
//!   registry of managed server installations.
//! - `dap` — debugger orchestration over `karet-dap`.
//! - `vcs_worker` — ordered repository (`karet-vcs`, `blameline`) and forge
//!   (`karet-github`, feature `github`) operations.
//! - `seam_worker` — the `karet-seam` index, cached by `seam_cache`.
//! - `search_worker` — workspace search over `karet-search`.
//! - `spell` / `spell_scan` — per-document spell-check and the workspace-wide scan.
//! - `todo_scan` — the workspace codetag (`TODO:`-style) scan.
//! - `notebook_kernel` — Jupyter kernels per notebook (feature `notebook-kernel`).
//! - `aicommit` — AI commit-message generation (feature `aicommit`).
//! - `manifest_hints` — `Cargo.toml` dependency hints (feature `deps`).
//! - `toml_lsp` — the built-in TOML language server, taplo run in-process
//!   (feature `toml-lsp`).
//! - `toml_format` — the built-in TOML formatter, used when no language server
//!   formats a TOML document (feature `toml-format`).
//! - `viz` — the `Cargo.lock` dependency-graph producer (feature `viz`).
//! - `latex` — external LaTeX compilation and diagnostics.
//! - `wakatime` — WakaTime heartbeats.
//!
//! Every per-user path (config, swaps, caches, state) resolves through [`dirs`], on
//! one platform root.
//!
//! File-watching runs over `karet-watch`, and external process trees are owned
//! through `karet-supervisor`. In local mode the UI renders from the [`DocSnapshot`]s pushed on the snapshot
//! stream (`local`), not by borrowing document state across the actor boundary.
//!
//! # Public surface
//!
//! One rule decides what a client names, and from where:
//!
//! - every [`api`] item is re-exported at the crate root (`pub use api::*`);
//! - the root also carries what starting and wiring a backend needs:
//!   [`SessionConfig`], [`local`] / [`Backend`] / [`LocalBackend`], the
//!   [`EventRx`] and [`SnapshotRx`] streams, the default on-disk locations
//!   ([`default_swap_dir`], [`default_seam_cache_dir`]), and the settings types
//!   the seam's events carry ([`Settings`], [`LoadedConfig`], …);
//! - [`config`] is the settings loader, and [`config::schema`] the full typed
//!   settings tree; [`layout`] persists the client's workspace layout; [`dirs`]
//!   resolves the per-user directories.
//!
//! Everything else -- the session actor, swap files, the seam cache, the
//! settings-file writers -- is internal and reached only through the
//! [`Command`]/[`Event`] seam.

#[cfg(feature = "aicommit")]
mod aicommit;
pub mod api;
pub mod backend;
mod backup;
mod cancellation;
pub mod config;
mod dap;
mod diff_prepare;
pub mod dirs;
mod editorconfig;
mod event_tx;
mod highlight;
mod latex;
mod local;
mod lsp;
mod lsp_registry;
#[cfg(feature = "deps")]
mod manifest_hints;
#[cfg(feature = "notebook-kernel")]
mod notebook_kernel;
mod scan_walk;
mod seam_cache;
mod seam_worker;
mod search_worker;
mod session;
mod spell;
mod spell_scan;
mod todo_scan;
#[cfg(feature = "toml-format")]
mod toml_format;
#[cfg(feature = "toml-lsp")]
mod toml_lsp;
mod vcs_worker;
#[cfg(feature = "viz")]
mod viz;
mod wakatime;

/// Persisting the client's workspace layout (panes, tabs, cursors) beside the
/// crash-recovery swaps, keyed by workspace root.
///
/// The layout is the client's own state rather than a session model, so it is
/// read and written directly instead of through the [`Command`] seam.
pub mod layout {
    pub use crate::backup::LayoutAxis;
    pub use crate::backup::LayoutNode;
    pub use crate::backup::LayoutPane;
    pub use crate::backup::LayoutTab;
    pub use crate::backup::WorkspaceLayout;
    pub use crate::backup::read_layout;
    pub use crate::backup::remove_layout;
    pub use crate::backup::write_layout;
}

pub use api::*;
pub use backend::Backend;
pub use backend::BackendError;
pub use backend::LocalBackend;
pub use backend::local;
pub use backup::default_swap_dir;
pub use config::ConfigDiagnostic;
pub use config::ConfigLayer;
pub use config::ConfigLayerReport;
pub use config::ConfigLayerStatus;
pub use config::LoadedConfig;
pub use config::schema::AUTO_MODEL as AI_COMMIT_AUTO_MODEL;
pub use config::schema::AiCommit;
pub use config::schema::AiCommitAgent;
pub use config::schema::AiCommitEffort;
pub use config::schema::Settings;
pub use local::DocSnapshot;
pub use local::SnapshotRx;
pub use lsp::CHANGE_DEBOUNCE as LSP_CHANGE_DEBOUNCE;
pub use seam_cache::default_cache_dir as default_seam_cache_dir;
pub use session::EventRx;
pub use session::SessionConfig;

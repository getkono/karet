//! `karet-lsp` — an async Language Server Protocol client for karet.
//!
//! Headless by default: connects to language servers (stdio/TCP) and turns their
//! responses into `karet-core` models (`Diagnostic`, `Symbol`, decorations,
//! inlay hints, code lenses), implementing `SymbolProvider`. Usable from a CLI
//! or a non-ratatui UI. Enable `view` for ratatui popups.
//!
//! # Responsibilities (to implement)
//! - `transport` — stdio/TCP JSON-RPC framing via async-lsp.
//! - `session` — server lifecycle, capabilities, text-document sync.
//! - `features` — completion/hover/signature/diagnostics/code-actions/rename/
//!   goto/references/workspace-symbols/inlay/codelens → core models.
//! - `view` — completion list, hover/signature/code-action/rename popups (feature `view`).
//!
//! # Internal dependencies
//! - `karet-core` — emitted models + `SymbolProvider`.
//! - `karet-fuzzy`, `karet-markdown` — completion ranking & hover rendering (`view` only).

// TODO: transport — async-lsp JSON-RPC over stdio/TCP.
// TODO: session   — lifecycle, capabilities, text-document sync.
// TODO: features  — the LSP request/notification surface → core models.
// TODO: view      — ratatui popups (feature = "view").

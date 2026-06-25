//! `karet-widgets` — a reusable ratatui widget toolkit for building editors.
//!
//! A single, lightweight (ratatui-only) crate of the UI widgets an editor needs.
//! Widgets render data fed in by the application — they consume `karet-core`
//! models and a `SymbolProvider`, and so do **not** depend on the producers
//! (`karet-lsp`/`karet-vcs`/`karet-dap`).
//!
//! # Responsibilities (to implement)
//! - `filetree` — gitignore-aware file tree with git-status overlay (decorations) + icons.
//! - `picker` — fuzzy quick-open + command palette.
//! - `outline` — symbol outline + breadcrumbs over `SymbolProvider`.
//! - `statusbar`, `toast`, `progress`, `dialog`, `dock`, `whichkey` — UI chrome.
//! - `problems` — diagnostics list (renders core `Diagnostic`s).
//! - `layout` — pane split tree, resize, maximize, named layouts, focus ring.
//! - `hex` — binary hex-dump view.
//!
//! # Internal dependencies
//! - `karet-core` — models + `SymbolProvider`.
//! - `karet-fuzzy` — picker/palette ranking.

// TODO: filetree  — file tree widget (ignore walk + status overlay + icons).
// TODO: picker    — fuzzy quick-open + command palette.
// TODO: outline   — symbol outline + breadcrumbs.
// TODO: statusbar — status bar with pluggable sections.
// TODO: toast/progress/dialog/dock/whichkey — UI chrome widgets.
// TODO: problems  — diagnostics panel.
// TODO: layout    — pane split tree + focus ring.
// TODO: hex       — hex-dump viewer.

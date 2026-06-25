//! `karet-vcs` — editor-oriented git integration for karet.
//!
//! A `gix`-backed engine for status, blame, branches and staging, emitting
//! per-line change markers and blame annotations as `karet-core` decorations.
//! Headless by default; enable `view` for ratatui source-control panels (which
//! render `karet-diff` hunk data directly, since diff has no widget of its own).
//!
//! # Responsibilities (to implement)
//! - `status` — working-tree status (staged/unstaged/untracked) + gutter markers.
//! - `blame` — per-line blame with age shading and hover detail.
//! - `branch` — branch list, create/switch/delete.
//! - `stage` — stage/unstage/discard per file or hunk.
//! - `view` — SCM panel, branch picker, staging UI, blame overlay (feature `view`).
//!
//! # Internal dependencies
//! - `karet-core` — emitted decorations.
//! - `karet-diff` — hunk data for the staging/diff views (optional, `view` only).

// TODO: status — working-tree status + gutter decorations.
// TODO: blame  — per-line blame + age shading.
// TODO: branch — branch listing & actions.
// TODO: stage  — staging/unstaging/discard (gix; optional git2 backend).
// TODO: view   — ratatui source-control panels (feature = "view").

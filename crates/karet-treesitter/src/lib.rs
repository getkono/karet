//! `karet-treesitter` — the shared tree-sitter parse host for the karet toolkit.
//!
//! Owns parser pooling, incremental edit application, tree caching and query
//! execution so that `karet-syntax`, `karet-diff`, and (via syntax) the editor
//! all reuse a single parse of each buffer. Reusable for any structural-analysis
//! tool (linters, formatters, code navigation).
//!
//! Tree-sitter is karet's *sole* syntax backend — there is deliberately no
//! second backend to abstract over.
//!
//! # Responsibilities (to implement)
//! - `parser` — per-language parser pool and configuration.
//! - `tree` — incremental edit application + tree cache keyed by buffer.
//! - `query` — compiled query running (highlights, folds, locals, …).
//! - `lang` — grammar registry, gated behind `lang-*` features.
//!
//! # Internal dependencies
//! - `karet-core` — shared coordinates.

// TODO: parser — parser pool + language configuration.
// TODO: tree   — incremental edits, tree cache, change-range tracking.
// TODO: query  — query compilation & execution.
// TODO: lang   — grammar registration behind lang-* features.

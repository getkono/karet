//! `karet-diff` — a pure, syntax-aware diffing engine.
//!
//! Focused entirely on the (non-trivial) problem of diffing many kinds of files
//! *well*: it parses both sides with tree-sitter and diffs the structure
//! (difftastic-style), falling back to line/word diffing for formats without a
//! grammar. It produces hunks and a staging model and does **no** presentation —
//! how a diff is displayed is left to whichever consumer integrates it.
//!
//! # Responsibilities (to implement)
//! - `line` — line/word (Myers/histogram) diffing via imara-diff (the fallback).
//! - `structural` — tree-sitter tree diffing for syntax-aware results.
//! - `hunk` — hunk grouping and a per-hunk staging/unstaging model.
//! - `detect` — choose structural vs. line strategy per file type.
//!
//! # Internal dependencies
//! - `karet-core` — ranges, hunk coordinates.
//! - `karet-treesitter` — parse trees for structural diffing.

// TODO: line       — imara-diff line/word fallback.
// TODO: structural — tree-sitter syntax-aware diff.
// TODO: hunk       — hunk model + staging.
// TODO: detect     — strategy selection per file format.

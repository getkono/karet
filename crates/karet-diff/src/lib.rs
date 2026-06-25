//! `karet-diff` — a pure, syntax-aware diffing engine.
//!
//! Parses both sides with tree-sitter and diffs the structure (difftastic-style),
//! falling back to line/word diffing for formats without a grammar. It produces
//! [`Hunk`]s and a [`Staging`] model and does **no** presentation — how a diff is
//! displayed is left to whichever consumer integrates it.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! diffing logic is filled in separately.

use karet_core::Range;
use std::path::Path;

/// Errors produced while diffing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiffError {
    /// The diff could not be computed.
    #[error("diff failed")]
    Failed,
}

/// The strategy used to diff two texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffStrategy {
    /// Tree-sitter structural diff (syntax-aware).
    Structural,
    /// Line/word diff (the format-agnostic fallback).
    Line,
}

/// The kind of change a [`Hunk`] represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkKind {
    /// Lines added.
    Added,
    /// Lines removed.
    Removed,
    /// Lines modified.
    Modified,
}

/// A contiguous change between the old and new texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The affected range in the old text.
    pub old: Range,
    /// The affected range in the new text.
    pub new: Range,
    /// The kind of change.
    pub kind: HunkKind,
}

/// The result of diffing two texts.
#[derive(Clone, Debug, Default)]
pub struct Diff {
    /// The change hunks, in order.
    pub hunks: Vec<Hunk>,
}

/// Diff `old` against `new` using `strategy`.
#[must_use]
pub fn diff(old: &str, new: &str, strategy: DiffStrategy) -> Diff {
    let _ = (old, new, strategy);
    todo!()
}

/// Choose a [`DiffStrategy`] based on the file at `path` (extension/grammar).
#[must_use]
pub fn detect_strategy(path: &Path) -> DiffStrategy {
    let _ = path;
    todo!()
}

/// Per-hunk staging state for building partial commits.
#[derive(Clone, Debug, Default)]
pub struct Staging {}

impl Staging {
    /// Mark `hunk` as staged.
    pub fn stage(&mut self, hunk: &Hunk) {
        let _ = hunk;
        todo!()
    }

    /// The unified patch text for the currently-staged hunks.
    #[must_use]
    pub fn staged_patch(&self) -> String {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_kinds_compare() {
        assert_eq!(HunkKind::Added, HunkKind::Added);
        assert_ne!(HunkKind::Added, HunkKind::Removed);
    }

    #[test]
    fn error_displays() {
        assert_eq!(DiffError::Failed.to_string(), "diff failed");
    }
}

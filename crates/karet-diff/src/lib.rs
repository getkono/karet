//! `karet-diff` — a pure, headless text diffing engine.
//!
//! Produces a neutral diff model ([`FileDiff`] / [`Hunk`] / [`DiffLine`]) from
//! either two in-memory texts ([`diff_text`]) or an existing unified diff
//! ([`parse`]), plus the building blocks a viewer needs: side-by-side alignment
//! ([`align_hunk`]), intra-line change highlighting ([`compute_highlights`]), and
//! patch reconstruction / per-hunk staging ([`format_hunk_patch`], [`Staging`]).
//!
//! It does **no** presentation — colors, layout and syntax highlighting are the
//! consumer's job. Diffing is line- and word-level today; tree-sitter structural
//! diffing is reserved behind [`DiffStrategy::Structural`].

mod align;
mod engine;
mod intraline;
mod model;
mod parse;
mod patch;

use std::path::Path;

pub use align::Cell;
pub use align::SideBySideRow;
pub use align::align_hunk;
pub use engine::DiffOptions;
pub use engine::diff_files;
pub use engine::diff_text;
pub use intraline::HighlightedPair;
pub use intraline::Segment;
pub use intraline::compute_highlights;
pub use model::Diff;
pub use model::DiffLine;
pub use model::FileDiff;
pub use model::FileStatus;
pub use model::Hunk;
pub use model::LineKind;
pub use parse::parse;
pub use patch::Staging;
pub use patch::format_hunk_patch;

/// Errors produced while diffing or parsing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiffError {
    /// Reading an input file failed.
    #[error("i/o error: {0}")]
    Io(String),
    /// Parsing a unified diff failed.
    #[error("failed to parse diff: {0}")]
    Parse(String),
}

/// The strategy used to diff two texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffStrategy {
    /// Tree-sitter structural diff (syntax-aware). Reserved; currently behaves like
    /// [`DiffStrategy::Line`] until the structural engine lands.
    Structural,
    /// Line + intra-word diff (the format-agnostic default).
    Line,
}

/// Diff `old` against `new`, returning a single-file diff.
///
/// `strategy` selects the framing only: both variants produce the line + intra-word
/// diff today. Tree-sitter structural diffing is reserved for
/// [`DiffStrategy::Structural`].
#[must_use]
pub fn diff(old: &str, new: &str, strategy: DiffStrategy) -> FileDiff {
    let _ = strategy;
    diff_text(old, new, &DiffOptions::default())
}

/// Choose a [`DiffStrategy`] for the file at `path`.
///
/// Structural diffing is not yet implemented, so this currently always returns
/// [`DiffStrategy::Line`]; the signature is kept for forward compatibility.
#[must_use]
pub fn detect_strategy(path: &Path) -> DiffStrategy {
    let _ = path;
    DiffStrategy::Line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_displays() {
        assert_eq!(DiffError::Io("boom".into()).to_string(), "i/o error: boom");
        assert_eq!(
            DiffError::Parse("bad".into()).to_string(),
            "failed to parse diff: bad"
        );
    }

    #[test]
    fn strategy_variants_differ() {
        assert_ne!(DiffStrategy::Structural, DiffStrategy::Line);
    }

    #[test]
    fn diff_wrapper_produces_single_file_diff() {
        let f = diff("a\n", "b\n", DiffStrategy::Line);
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(detect_strategy(Path::new("x.rs")), DiffStrategy::Line);
    }
}

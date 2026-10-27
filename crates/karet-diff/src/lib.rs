//! `karet-diff` — a pure, headless text diffing engine.
//!
//! Produces a neutral diff model ([`FileDiff`] / [`Hunk`] / [`DiffLine`]) from
//! either two in-memory texts ([`diff_text`]) or an existing unified diff
//! ([`parse`]), plus the building blocks a viewer needs: side-by-side alignment
//! ([`align_hunk`]), intra-line change highlighting ([`compute_highlights`]), and
//! patch reconstruction / per-hunk staging ([`format_hunk_patch`], [`Staging`]).
//!
//! It does **no** presentation — colors, layout and syntax highlighting are the
//! consumer's job. Diffing is line- and word-level.

mod align;
mod engine;
mod intraline;
mod layout;
mod model;
mod parse;
mod patch;
mod prepared;
#[cfg(feature = "view")]
mod view;

pub use align::Cell;
pub use align::SideBySideRow;
pub use align::align_hunk;
pub use engine::DiffOptions;
pub use engine::diff_files;
pub use engine::diff_text;
pub use intraline::HighlightedPair;
pub use intraline::Segment;
pub use intraline::compute_highlights;
pub use layout::RowContent;
pub use layout::side_by_side_hunk_at_row;
pub use layout::side_by_side_hunk_row;
pub use layout::side_by_side_row;
pub use layout::unified_hunk_at_row;
pub use layout::unified_hunk_row;
pub use layout::unified_row;
pub use model::Diff;
pub use model::DiffLine;
pub use model::FileDiff;
pub use model::FileStatus;
pub use model::Hunk;
pub use model::LineKind;
pub use parse::parse;
pub use patch::Staging;
pub use patch::format_hunk_patch;
pub use prepared::PreparedDiff;
pub use prepared::TokenSpan;
pub use prepared::line_stats;
#[cfg(feature = "view")]
pub use view::DiffPalette;
#[cfg(feature = "view")]
pub use view::pad_diff_lines;
#[cfg(feature = "view")]
pub use view::side_by_side_lines;
#[cfg(feature = "view")]
pub use view::unified_lines;

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
}

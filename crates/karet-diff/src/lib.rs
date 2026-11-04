//! `karet-diff` — a pure, headless text diffing engine.
//!
//! Produces a neutral diff model ([`FileDiff`] / [`Hunk`] / [`DiffLine`]) from
//! either two in-memory texts ([`diff_text`]) or an existing unified diff
//! ([`parse`]), plus the building blocks a viewer needs: side-by-side alignment
//! ([`align_hunk`]), intra-line change highlighting ([`compute_highlights`]), and
//! patch reconstruction / per-hunk staging ([`format_hunk_patch`], [`Staging`]).
//!
//! The engine does **no** presentation — colors and syntax highlighting are the
//! consumer's job. Diffing is line- and word-level. With the **`view`**
//! feature, the `view` module paints a [`PreparedDiff`] as ratatui lines
//! (`view::unified_lines`, `view::side_by_side_lines`) from a caller-supplied
//! `view::DiffPalette`.

mod align;
mod engine;
mod intraline;
mod layout;
mod model;
mod parse;
mod patch;
mod prepared;
#[cfg(feature = "view")]
pub mod view;

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
pub use prepared::line_stats;

/// Deprecated root alias of [`view::DiffPalette`].
#[cfg(feature = "view")]
#[deprecated(note = "use `karet_diff::view::DiffPalette`")]
pub type DiffPalette<'a> = view::DiffPalette<'a>;

/// Deprecated root path of [`view::unified_lines`].
#[cfg(feature = "view")]
#[deprecated(note = "use `karet_diff::view::unified_lines`")]
#[must_use]
pub fn unified_lines(
    prepared: &PreparedDiff,
    palette: &view::DiffPalette<'_>,
) -> Vec<ratatui::text::Line<'static>> {
    view::unified_lines(prepared, palette)
}

/// Deprecated root path of [`view::side_by_side_lines`].
#[cfg(feature = "view")]
#[deprecated(note = "use `karet_diff::view::side_by_side_lines`")]
#[must_use]
pub fn side_by_side_lines(
    prepared: &PreparedDiff,
    palette: &view::DiffPalette<'_>,
) -> (
    Vec<ratatui::text::Line<'static>>,
    Vec<ratatui::text::Line<'static>>,
) {
    view::side_by_side_lines(prepared, palette)
}

/// Deprecated root path of [`view::pad_diff_lines`].
#[cfg(feature = "view")]
#[deprecated(note = "use `karet_diff::view::pad_diff_lines`")]
pub fn pad_diff_lines(lines: &mut [ratatui::text::Line<'static>], width: u16) {
    view::pad_diff_lines(lines, width);
}

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

    /// The deprecated root paths still paint exactly what `view::` paints, so
    /// a consumer gets a warning, not a different picture. Falsified by a root
    /// wrapper that drifts from its `view` counterpart.
    #[cfg(feature = "view")]
    #[test]
    #[allow(deprecated)] // Exercising the deprecated root paths is the point.
    fn deprecated_root_painters_delegate_to_view() {
        use ratatui::style::Color;

        let token_fg = |_: karet_core::TokenId| Color::White;
        let palette: DiffPalette<'_> = DiffPalette {
            foreground: Color::White,
            added_bg: Color::Green,
            added_emphasis_bg: Color::LightGreen,
            removed_bg: Color::Red,
            removed_emphasis_bg: Color::LightRed,
            plain_emphasis_bg: Color::Yellow,
            add_marker: Color::Green,
            remove_marker: Color::Red,
            gutter: Color::DarkGray,
            header: Color::Cyan,
            dim: Color::Gray,
            token_fg: &token_fg,
        };
        let prepared = PreparedDiff::new(
            diff_text("a\nold\n", "a\nnew\n", &DiffOptions::default()),
            Vec::new(),
            Vec::new(),
        );
        let mut root = unified_lines(&prepared, &palette);
        assert_eq!(root, view::unified_lines(&prepared, &palette));
        assert_eq!(
            side_by_side_lines(&prepared, &palette),
            view::side_by_side_lines(&prepared, &palette)
        );
        let mut via_view = root.clone();
        pad_diff_lines(&mut root, 30);
        view::pad_diff_lines(&mut via_view, 30);
        assert_eq!(root, via_view);
    }
}

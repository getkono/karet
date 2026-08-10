//! Neutral highlight and fold data.
//!
//! The *shapes* a highlighter produces and a renderer consumes, with no
//! tree-sitter (or any engine) dependency: `karet-syntax` computes them,
//! `karet-editor` (and any other view) renders them, and the backend seam can
//! carry them.

use crate::coord::BytePos;
use crate::coord::Span;
use crate::token::TokenId;

/// A highlighted region: a byte [`Span`] tagged with a semantic [`TokenId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HighlightSpan {
    /// The byte range covered.
    pub span: Span,
    /// The semantic token class.
    pub token: TokenId,
}

/// An ordered, non-overlapping set of [`HighlightSpan`]s for a buffer.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Highlights {
    spans: Vec<HighlightSpan>,
}

impl Highlights {
    /// Wrap an already-sorted, non-overlapping span list (a highlighter's output).
    ///
    /// The invariants ([`all`](Self::all) is byte-ordered and non-overlapping) are
    /// the producer's responsibility; [`spans_in`](Self::spans_in) relies on them.
    #[must_use]
    pub fn from_sorted_spans(spans: Vec<HighlightSpan>) -> Self {
        Self { spans }
    }

    /// All highlight spans, in document (byte) order.
    #[must_use]
    pub fn all(&self) -> &[HighlightSpan] {
        &self.spans
    }

    /// The highlight spans overlapping `range`, in order.
    #[must_use]
    pub fn spans_in(&self, range: Span) -> &[HighlightSpan] {
        // `spans` is sorted by start and non-overlapping, so both predicates are
        // monotonic and `partition_point` gives the overlapping window.
        let start = self
            .spans
            .partition_point(|s| s.span.end.0 <= range.start.0);
        let end = self.spans.partition_point(|s| s.span.start.0 < range.end.0);
        &self.spans[start..end.max(start)]
    }

    /// Shift these spans to stay aligned with a buffer edited in `[start, old_end)` →
    /// `[start, new_end)`.
    ///
    /// When re-highlighting is asynchronous the buffer changes before fresh spans
    /// arrive. Rendering the old spans verbatim would smear color across the shifted
    /// text; translating them keeps everything after the edit correctly aligned for the
    /// frame or two before the highlighter answers.
    ///
    /// Spans wholly before the edit are untouched, spans wholly after are shifted, and
    /// a span the edit actually cut through is dropped — its extent is no longer known,
    /// so the affected text renders unhighlighted rather than wrong.
    #[must_use]
    pub fn translate(&self, start: BytePos, old_end: BytePos, new_end: BytePos) -> Self {
        let spans = self
            .spans
            .iter()
            .filter_map(|s| {
                if s.span.end.0 <= start.0 {
                    return Some(*s);
                }
                if s.span.start.0 >= old_end.0 {
                    return Some(HighlightSpan {
                        span: Span {
                            start: BytePos(shift_pos(s.span.start.0, old_end.0, new_end.0)),
                            end: BytePos(shift_pos(s.span.end.0, old_end.0, new_end.0)),
                        },
                        token: s.token,
                    });
                }
                // The edit cut through this span.
                None
            })
            .collect();
        Self { spans }
    }
}

/// Move `pos` (which lies at or after `old_end`) by the edit's signed length delta.
fn shift_pos(pos: usize, old_end: usize, new_end: usize) -> usize {
    if new_end >= old_end {
        pos + (new_end - old_end)
    } else {
        pos.saturating_sub(old_end - new_end)
    }
}

/// A foldable region as an inclusive line range `[start, end]` (0-based lines). The
/// `start` line is the header that stays visible when collapsed; lines
/// `start + 1 ..= end` are the ones that hide. Line ranges (not byte spans) because
/// folding is inherently a line operation for every consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FoldRegion {
    /// The 0-based header line (stays visible when collapsed).
    pub start: u32,
    /// The 0-based last line of the region, inclusive.
    pub end: u32,
}

/// The foldable regions of a buffer, in document order (outermost first), with at
/// most one region per start line.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FoldRegions {
    regions: Vec<FoldRegion>,
}

impl FoldRegions {
    /// Wrap an already-ordered region list (a fold computation's output).
    #[must_use]
    pub fn from_sorted_regions(regions: Vec<FoldRegion>) -> Self {
        Self { regions }
    }

    /// The fold regions, outermost first.
    #[must_use]
    pub fn regions(&self) -> &[FoldRegion] {
        &self.regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    fn hs(start: usize, end: usize, token: u16) -> HighlightSpan {
        HighlightSpan {
            span: span(start, end),
            token: TokenId(token),
        }
    }

    #[test]
    fn spans_in_returns_the_overlapping_window() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(4, 8, 2), hs(10, 12, 3)]);
        assert_eq!(h.spans_in(span(4, 10)), &[hs(4, 8, 2)]);
        assert_eq!(h.spans_in(span(2, 11)).len(), 3);
        assert!(h.spans_in(span(8, 10)).is_empty());
    }

    #[test]
    fn translate_shifts_after_and_drops_cut_spans() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(4, 8, 2), hs(10, 12, 3)]);
        // Replace bytes [4,8) with 2 bytes: the middle span is cut, the last shifts.
        let t = h.translate(BytePos(4), BytePos(8), BytePos(6));
        assert_eq!(t.all(), &[hs(0, 4, 1), hs(8, 10, 3)]);
    }

    #[test]
    fn fold_regions_round_trip() {
        let f = FoldRegions::from_sorted_regions(vec![FoldRegion { start: 0, end: 4 }]);
        assert_eq!(f.regions(), &[FoldRegion { start: 0, end: 4 }]);
    }
}

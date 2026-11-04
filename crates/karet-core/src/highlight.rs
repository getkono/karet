//! Neutral highlight, fold, and semantic-block data.
//!
//! The *shapes* a highlighter produces and a renderer consumes, with no
//! tree-sitter (or any engine) dependency: `karet-syntax` computes them,
//! `karet-editor` (and any other view) renders them, and the backend seam can
//! carry them.

use crate::coord::BytePos;
use crate::coord::Span;
use crate::edit::AppliedEdit;
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

/// A highlighted run within a single line: byte offsets relative to the line's
/// start, tagged with a semantic [`TokenId`].
///
/// The line-relative counterpart of [`HighlightSpan`], for consumers that carry
/// per-line token tables (a prepared diff, a source preview) rather than a whole
/// buffer's spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TokenSpan {
    /// The run's starting byte offset within its line.
    pub start: usize,
    /// The run's ending byte offset (exclusive) within its line.
    pub end: usize,
    /// The run's semantic token class.
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
        // Unlike `shift_span`, which clamps a cut span to the edit and grows a
        // span starting at a pure insertion over the inserted text, this drops
        // the cut span and moves the starting one with the text it covers: a
        // stale colour on new text is worse than none.
        let spans = self
            .spans
            .iter()
            .filter_map(|s| {
                if s.span.end.0 <= start.0 {
                    return Some(*s);
                }
                if s.span.start.0 >= old_end.0 {
                    let span = Span {
                        start: BytePos(shifted(s.span.start.0, old_end.0, new_end.0)),
                        end: BytePos(shifted(s.span.end.0, old_end.0, new_end.0)),
                    };
                    return Some(HighlightSpan { span, ..*s });
                }
                // The edit cut through this span.
                None
            })
            .collect();
        Self { spans }
    }
}

/// Map `span` from the text before `edit` into the text after it.
///
/// A span wholly before the edit is unchanged and one wholly after it moves by
/// the edit's length change. An end that falls inside the replaced region moves
/// to the end of the inserted text, and a start inside it to the edit's start, so
/// a span overlapping the edit still covers what replaced the part it lost.
#[must_use]
pub fn shift_span(span: Span, edit: &AppliedEdit) -> Span {
    let start = if span.start.0 <= edit.start_byte {
        span.start.0
    } else if span.start.0 >= edit.old_end_byte {
        shifted(span.start.0, edit.old_end_byte, edit.new_end_byte)
    } else {
        edit.start_byte
    };
    let end = if span.end.0 <= edit.start_byte {
        span.end.0
    } else if span.end.0 >= edit.old_end_byte {
        shifted(span.end.0, edit.old_end_byte, edit.new_end_byte)
    } else {
        edit.new_end_byte
    };
    Span {
        start: BytePos(start),
        end: BytePos(end.max(start)),
    }
}

/// `byte`, at or after an edit's `old_end`, moved by the edit's length change
/// (`old_end` → `new_end`).
fn shifted(byte: usize, old_end: usize, new_end: usize) -> usize {
    (byte + new_end).saturating_sub(old_end)
}

/// Sort `spans` and merge every pair that overlaps or touches.
#[must_use]
pub fn merge_spans(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.start.0, s.end.0));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match out.last_mut() {
            Some(last) if span.start.0 <= last.end.0 => {
                last.end = BytePos(last.end.0.max(span.end.0));
            },
            _ => out.push(span),
        }
    }
    out
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
    /// What the region holds, so a consumer can fold every comment or import
    /// block at once. [`FoldKind::Code`] for anything not classified.
    #[cfg_attr(feature = "serde", serde(default))]
    pub kind: FoldKind,
}

impl FoldRegion {
    /// A [`FoldKind::Code`] region over lines `start..=end`.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end,
            kind: FoldKind::Code,
        }
    }

    /// This region with its kind set to `kind`.
    #[must_use]
    pub const fn with_kind(self, kind: FoldKind) -> Self {
        Self { kind, ..self }
    }
}

/// What a [`FoldRegion`] holds. Mirrors the LSP `FoldingRangeKind` classes, so a
/// server's ranges and a grammar's fold query land in one vocabulary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FoldKind {
    /// A code construct (a block, an item, a literal) or anything unclassified.
    #[default]
    Code,
    /// A comment block: one multi-line comment, or a run of line comments.
    Comment,
    /// A run of import (`use`, `import`, `#include`) declarations.
    Imports,
    /// A region the source marks out itself (`#region` … `#endregion`, or an
    /// editor fold marker), as a language server reports it.
    Region,
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

    /// Normalize regions from a source that promises no order (a language
    /// server's answer): a region that hides no line (`end <= start`) is
    /// dropped, the rest are put in document order, and where several start on
    /// one line only the outermost is kept.
    #[must_use]
    pub fn from_unordered(mut regions: Vec<FoldRegion>) -> Self {
        regions.retain(|region| region.end > region.start);
        regions.sort_by_key(|region| (region.start, std::cmp::Reverse(region.end)));
        regions.dedup_by_key(|region| region.start);
        Self { regions }
    }

    /// The fold regions, outermost first.
    #[must_use]
    pub fn regions(&self) -> &[FoldRegion] {
        &self.regions
    }

    /// These regions carried across `edit`, by line.
    ///
    /// A region wholly before the edit's first line stays; one wholly after
    /// its last replaced line moves by the change in line count; one that
    /// holds the edit keeps its header and moves its end. A region with its
    /// header or its end strictly inside the replaced lines is dropped, since
    /// no line maps it, as is one the edit leaves hiding nothing.
    #[must_use]
    pub fn translate(&self, edit: &AppliedEdit) -> Self {
        let start_row = row(edit.start_point.row);
        let old_end = row(edit.old_end_point.row);
        let new_end = row(edit.new_end_point.row);
        let moved = |line: u32| line.saturating_add(new_end).saturating_sub(old_end);
        let regions = self
            .regions
            .iter()
            .filter_map(|region| {
                if region.end <= start_row {
                    Some(*region)
                } else if region.start > start_row && region.start >= old_end {
                    Some(FoldRegion {
                        start: moved(region.start),
                        end: moved(region.end),
                        kind: region.kind,
                    })
                } else if region.start <= start_row && region.end >= old_end {
                    Some(FoldRegion {
                        end: moved(region.end),
                        ..*region
                    })
                } else {
                    None
                }
            })
            .collect();
        Self::from_unordered(regions)
    }
}

/// An edit's row as a fold line, saturating on a buffer past `u32::MAX` lines.
fn row(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX)
}

/// One semantic source block with a header and an enclosing lifetime, the unit
/// sticky scroll pins and structural navigation steps between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SemanticBlock {
    /// First 0-based source line of the header.
    pub header_start: u32,
    /// Last 0-based source line occupied by the header or signature.
    pub header_end: u32,
    /// Last 0-based source line for which this header remains the active context.
    pub scope_end: u32,
}

impl SemanticBlock {
    /// Whether `line` is inside the block after its header has scrolled away.
    #[must_use]
    pub fn active_at(self, line: u32) -> bool {
        self.header_start < line && line <= self.scope_end
    }

    /// Whether the source signature occupies more than one physical line.
    #[must_use]
    pub fn has_multiline_header(self) -> bool {
        self.header_end > self.header_start
    }
}

/// Semantic blocks in document order, outer scopes before inner scopes on ties.
///
/// With the `serde` feature the collection travels as a bare array of blocks,
/// and deserializing goes through [`SemanticBlocks::new`], so input in any
/// order still comes out normalized.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(from = "Vec<SemanticBlock>", into = "Vec<SemanticBlock>")
)]
pub struct SemanticBlocks {
    blocks: Vec<SemanticBlock>,
}

impl From<Vec<SemanticBlock>> for SemanticBlocks {
    fn from(blocks: Vec<SemanticBlock>) -> Self {
        Self::new(blocks)
    }
}

impl From<SemanticBlocks> for Vec<SemanticBlock> {
    fn from(blocks: SemanticBlocks) -> Self {
        blocks.blocks
    }
}

impl SemanticBlocks {
    /// Build a normalized semantic-block collection from external block data.
    #[must_use]
    pub fn new(mut blocks: Vec<SemanticBlock>) -> Self {
        blocks.sort_unstable_by_key(|block| (block.header_start, u32::MAX - block.scope_end));
        blocks.dedup();
        Self { blocks }
    }

    /// Borrow the extracted blocks.
    #[must_use]
    pub fn blocks(&self) -> &[SemanticBlock] {
        &self.blocks
    }

    /// Return the active outer-to-inner block chain at `line`.
    #[must_use]
    pub fn active_at(&self, line: u32) -> Vec<SemanticBlock> {
        self.blocks
            .iter()
            .copied()
            .filter(|block| block.active_at(line))
            .collect()
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
        // A window strictly inside one span returns just that span.
        assert_eq!(h.spans_in(span(5, 7)), &[hs(4, 8, 2)]);
    }

    #[test]
    fn translate_on_a_pure_deletion_shifts_later_spans_left() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 3, 1), hs(10, 14, 2)]);
        // Delete [5,8): the span before is untouched, the one after slides left by 3.
        let t = h.translate(BytePos(5), BytePos(8), BytePos(5));
        assert_eq!(t.all(), &[hs(0, 3, 1), hs(7, 11, 2)]);
        // Deleting up to a span's start moves it to the deletion point.
        let t = h.translate(BytePos(4), BytePos(10), BytePos(4));
        assert_eq!(t.all(), &[hs(0, 3, 1), hs(4, 8, 2)]);
    }

    #[test]
    fn translate_shifts_after_and_drops_cut_spans() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(4, 8, 2), hs(10, 12, 3)]);
        // Replace bytes [4,8) with 2 bytes: the middle span is cut, the last shifts.
        let t = h.translate(BytePos(4), BytePos(8), BytePos(6));
        assert_eq!(t.all(), &[hs(0, 4, 1), hs(8, 10, 3)]);
    }

    #[test]
    fn translate_on_a_pure_insertion_shifts_spans_at_or_after_it() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(4, 8, 2), hs(10, 12, 3)]);
        // Insert 3 bytes at byte 4: the span ending there is kept, the span
        // starting there moves with the text it covers.
        let t = h.translate(BytePos(4), BytePos(4), BytePos(7));
        assert_eq!(t.all(), &[hs(0, 4, 1), hs(7, 11, 2), hs(13, 15, 3)]);
        // Inserting strictly inside a span cuts it.
        let t = h.translate(BytePos(6), BytePos(6), BytePos(9));
        assert_eq!(t.all(), &[hs(0, 4, 1), hs(13, 15, 3)]);
    }

    #[test]
    fn translate_on_a_growing_replacement_shifts_by_the_net_growth() {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(10, 12, 3)]);
        // Replace [5,8) with 7 bytes: everything after grows by 4.
        let t = h.translate(BytePos(5), BytePos(8), BytePos(12));
        assert_eq!(t.all(), &[hs(0, 4, 1), hs(14, 16, 3)]);
        // A no-op edit translates to the same spans.
        assert_eq!(
            h.translate(BytePos(8), BytePos(8), BytePos(8)).all(),
            h.all()
        );
    }

    fn edit(start: usize, old_end: usize, new_end: usize) -> AppliedEdit {
        AppliedEdit {
            start_byte: start,
            old_end_byte: old_end,
            new_end_byte: new_end,
            ..AppliedEdit::default()
        }
    }

    #[test]
    fn a_span_before_an_edit_stays_and_one_after_moves() {
        let insert = edit(10, 10, 13);
        assert_eq!(shift_span(span(2, 10), &insert), span(2, 10));
        assert_eq!(shift_span(span(10, 12), &insert), span(10, 15));
        assert_eq!(shift_span(span(11, 12), &insert), span(14, 15));
        let delete = edit(4, 8, 4);
        assert_eq!(shift_span(span(9, 12), &delete), span(5, 8));
    }

    #[test]
    fn a_span_overlapping_an_edit_covers_its_replacement() {
        // Replace 4..8 with two bytes: 4..6.
        let replace = edit(4, 8, 6);
        assert_eq!(shift_span(span(2, 6), &replace), span(2, 6));
        assert_eq!(shift_span(span(6, 10), &replace), span(4, 8));
        assert_eq!(shift_span(span(5, 7), &replace), span(4, 6));
        assert_eq!(shift_span(span(2, 10), &replace), span(2, 8));
    }

    #[test]
    fn translate_parts_from_shift_span_where_an_edit_meets_a_span() {
        // A span the edit cuts through: translate drops it, shift_span clamps it.
        let h = Highlights::from_sorted_spans(vec![hs(2, 6, 1)]);
        let t = h.translate(BytePos(4), BytePos(8), BytePos(6));
        assert!(t.all().is_empty());
        assert_eq!(shift_span(span(2, 6), &edit(4, 8, 6)), span(2, 6));
        // An insertion at a span's start: translate moves the span with its
        // text, shift_span grows it over the insertion.
        let h = Highlights::from_sorted_spans(vec![hs(4, 8, 1)]);
        let t = h.translate(BytePos(4), BytePos(4), BytePos(7));
        assert_eq!(t.all(), &[hs(7, 11, 1)]);
        assert_eq!(shift_span(span(4, 8), &edit(4, 4, 7)), span(4, 11));
    }

    #[test]
    fn merge_spans_joins_overlapping_and_touching_spans() {
        let merged = merge_spans(vec![span(8, 9), span(0, 2), span(2, 4), span(3, 6)]);
        assert_eq!(merged, vec![span(0, 6), span(8, 9)]);
        assert!(merge_spans(Vec::new()).is_empty());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn highlights_and_folds_round_trip_through_serde() -> Result<(), serde_json::Error> {
        let h = Highlights::from_sorted_spans(vec![hs(0, 4, 1), hs(10, 12, 31)]);
        let back = serde_json::from_str::<Highlights>(&serde_json::to_string(&h)?)?;
        assert_eq!(back.all(), h.all());

        let f = FoldRegions::from_sorted_regions(vec![
            FoldRegion::new(0, 9),
            FoldRegion::new(1, 3).with_kind(FoldKind::Imports),
        ]);
        let back = serde_json::from_str::<FoldRegions>(&serde_json::to_string(&f)?)?;
        assert_eq!(back.regions(), f.regions());
        // A region from before `kind` existed decodes as code.
        let old = serde_json::from_str::<FoldRegion>(r#"{"start":2,"end":5}"#)?;
        assert_eq!(old, FoldRegion::new(2, 5));
        Ok(())
    }

    #[test]
    fn public_model_reports_active_and_multiline_headers() {
        let block = SemanticBlock {
            header_start: 1,
            header_end: 2,
            scope_end: 8,
        };
        assert!(!block.active_at(1));
        assert!(block.active_at(2));
        assert!(block.has_multiline_header());
        let blocks = SemanticBlocks::new(vec![block]);
        assert_eq!(blocks.blocks(), [block]);
        assert_eq!(blocks.active_at(4), [block]);
    }

    fn block(header_start: u32, header_end: u32, scope_end: u32) -> SemanticBlock {
        SemanticBlock {
            header_start,
            header_end,
            scope_end,
        }
    }

    #[test]
    fn semantic_blocks_sort_outer_first_and_drop_duplicates() {
        let outer = block(0, 0, 9);
        let inner = block(0, 1, 4);
        let later = block(5, 5, 8);
        let blocks = SemanticBlocks::new(vec![later, inner, outer, inner]);
        // Document order, and on a shared header line the wider scope first.
        assert_eq!(blocks.blocks(), [outer, inner, later]);
        // The block ends at `scope_end` inclusive; its header line is not inside it.
        assert_eq!(blocks.active_at(4), [outer, inner]);
        assert_eq!(blocks.active_at(5), [outer]);
        assert_eq!(blocks.active_at(9), [outer]);
        assert!(blocks.active_at(0).is_empty());
        assert!(blocks.active_at(10).is_empty());
        assert!(!later.has_multiline_header());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn semantic_blocks_round_trip_through_serde() -> Result<(), serde_json::Error> {
        let blocks = SemanticBlocks::new(vec![block(0, 0, 9), block(2, 3, 5)]);
        let back = serde_json::from_str::<SemanticBlocks>(&serde_json::to_string(&blocks)?)?;
        assert_eq!(back, blocks);
        Ok(())
    }

    #[cfg(feature = "serde")]
    #[test]
    fn semantic_blocks_deserialize_normalizes_unsorted_input() -> Result<(), serde_json::Error> {
        // Inner before outer on a shared header line, a later block first, and a
        // duplicate: the wire form never went through `new`.
        let json = r#"[
            {"header_start":5,"header_end":5,"scope_end":8},
            {"header_start":0,"header_end":1,"scope_end":4},
            {"header_start":0,"header_end":0,"scope_end":9},
            {"header_start":0,"header_end":1,"scope_end":4}
        ]"#;
        let back = serde_json::from_str::<SemanticBlocks>(json)?;
        let (outer, inner, later) = (block(0, 0, 9), block(0, 1, 4), block(5, 5, 8));
        assert_eq!(back.blocks(), [outer, inner, later]);
        assert_eq!(back.active_at(4), [outer, inner]);
        // And it round-trips to the normalized array.
        let again = serde_json::from_str::<SemanticBlocks>(&serde_json::to_string(&back)?)?;
        assert_eq!(again, back);
        Ok(())
    }

    #[test]
    fn fold_regions_round_trip() {
        let f = FoldRegions::from_sorted_regions(vec![FoldRegion::new(0, 4)]);
        assert_eq!(f.regions(), &[FoldRegion::new(0, 4)]);
        assert_eq!(f.regions()[0].kind, FoldKind::Code);
        let comment = FoldRegion::new(1, 2).with_kind(FoldKind::Comment);
        assert_eq!((comment.start, comment.end), (1, 2));
        assert_eq!(comment.kind, FoldKind::Comment);
    }

    /// An unordered answer comes out in document order, an empty or inverted
    /// region dropped and only the outermost kept per header line. Falsified
    /// by keeping the first region seen on a shared line (the inner one here).
    #[test]
    fn unordered_regions_normalize() {
        let f = FoldRegions::from_unordered(vec![
            FoldRegion::new(5, 9),
            FoldRegion::new(0, 2).with_kind(FoldKind::Comment),
            FoldRegion::new(5, 6).with_kind(FoldKind::Region),
            FoldRegion::new(5, 12).with_kind(FoldKind::Region),
            FoldRegion::new(3, 3),
            FoldRegion::new(8, 4),
        ]);
        assert_eq!(
            f.regions(),
            [
                FoldRegion::new(0, 2).with_kind(FoldKind::Comment),
                FoldRegion::new(5, 12).with_kind(FoldKind::Region),
            ]
        );
    }

    /// An edit replacing rows `start..=old_end` with rows `start..=new_end`.
    fn rows(start: usize, old_end: usize, new_end: usize) -> AppliedEdit {
        let point = |row| crate::coord::BytePoint { row, col: 0 };
        AppliedEdit {
            start_byte: 0,
            old_end_byte: 0,
            new_end_byte: 0,
            start_point: point(start),
            old_end_point: point(old_end),
            new_end_point: point(new_end),
            replaced: String::new(),
        }
    }

    /// Lines inserted inside a region stretch it and move the regions after
    /// it, leaving the ones before alone; an edit within one line moves
    /// nothing. Falsified by shifting a holding region's header, or by not
    /// moving the regions below.
    #[test]
    fn translating_moves_regions_by_lines() {
        let f = FoldRegions::from_sorted_regions(vec![
            FoldRegion::new(0, 1),
            FoldRegion::new(2, 6),
            FoldRegion::new(8, 9).with_kind(FoldKind::Imports),
        ]);
        let grown = f.translate(&rows(4, 4, 6));
        assert_eq!(
            grown.regions(),
            [
                FoldRegion::new(0, 1),
                FoldRegion::new(2, 8),
                FoldRegion::new(10, 11).with_kind(FoldKind::Imports),
            ]
        );
        assert_eq!(f.translate(&rows(3, 3, 3)).regions(), f.regions());
    }

    /// Deleting lines pulls later regions up, drops a region whose header the
    /// deletion swallowed, and drops one it leaves hiding nothing. Falsified
    /// by keeping a region whose header line no longer exists.
    #[test]
    fn translating_a_deletion_drops_what_it_swallowed() {
        let f = FoldRegions::from_sorted_regions(vec![
            FoldRegion::new(0, 3),
            FoldRegion::new(4, 7),
            FoldRegion::new(10, 12),
        ]);
        // Rows 2..=5 collapse onto row 2: (0, 3) ends inside, (4, 7) starts
        // inside, (10, 12) is below and moves up by three.
        assert_eq!(
            f.translate(&rows(2, 5, 2)).regions(),
            [FoldRegion::new(7, 9)]
        );
        // A region holding the deletion shrinks to nothing and goes.
        let single = FoldRegions::from_sorted_regions(vec![FoldRegion::new(1, 2)]);
        assert!(single.translate(&rows(1, 2, 1)).regions().is_empty());
    }

    /// `TokenSpan` moved here from `karet-diff`, whose prepared diffs and the
    /// seam previews already cross the backend seam with it: the wire shape
    /// must stay `{start, end, token}` with the token as a bare number.
    /// Falsified by renaming a field or wrapping the token.
    #[cfg(feature = "serde")]
    #[test]
    fn token_span_keeps_its_wire_shape() -> Result<(), serde_json::Error> {
        let span = TokenSpan {
            start: 2,
            end: 5,
            token: TokenId(7),
        };
        let json = serde_json::to_string(&span)?;
        assert_eq!(json, r#"{"start":2,"end":5,"token":7}"#);
        assert_eq!(serde_json::from_str::<TokenSpan>(&json)?, span);
        Ok(())
    }
}

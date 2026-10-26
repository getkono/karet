//! A document's syntax-derived state, held and replaced as one value.

use std::ops::RangeInclusive;

use super::*;

/// What the syntax worker last derived from one document, translated across
/// any edits applied since. The parsed trees live on the worker, not here.
///
/// The parts describe one parse together, so they change together, and only
/// through the three transitions below: a worker answer replaces them all
/// ([`adopt`](Self::adopt)), a document no grammar parses empties them all
/// ([`clear`](Self::clear)), and an edit shifts what can be shifted and hides
/// the rest ([`translate`](Self::translate)). Resetting them field by field at
/// each call site is how ranges from a previous grammar once lingered on a
/// snapshot.
#[derive(Default)]
pub(super) struct SyntaxState {
    /// Syntax highlights, shifted across edits until the worker answers.
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    /// Semantic block scopes for this version.
    semantic_blocks: Arc<SemanticBlocks>,
    /// Grammar-backed outline used when LSP supplies no symbols.
    symbols: Arc<Vec<Symbol>>,
    /// Syntax-error line ranges from the last parse (see
    /// [`DocSnapshot::syntax_error_lines`]).
    error_lines: Arc<Vec<(u32, u32)>>,
    /// GFM table line ranges from the last parse (see
    /// [`DocSnapshot::table_lines`]).
    table_lines: Arc<Vec<RangeInclusive<u32>>>,
}

impl SyntaxState {
    /// Take the worker's answer for the buffer's current version, replacing
    /// every part.
    pub(super) fn adopt(&mut self, result: HighlightResult) {
        let HighlightResult {
            highlights,
            folds,
            semantic_blocks,
            symbols,
            error_lines,
            table_lines,
            ..
        } = result;
        *self = Self {
            highlights,
            folds,
            semantic_blocks,
            symbols,
            error_lines,
            table_lines,
        };
    }

    /// Empty every part for a document no grammar parses, keeping only `folds`
    /// (its indentation folds). No worker answer will replace this, so nothing
    /// a previous grammar produced may survive it.
    pub(super) fn clear(&mut self, folds: Arc<FoldRegions>) {
        *self = Self {
            folds,
            ..Self::default()
        };
    }

    /// Carry the state across `edits` until the worker answers: highlights
    /// shift with the text so color does not smear across what moved, while
    /// block scopes and the outline, which are line-based and cannot be
    /// translated safely, are hidden rather than shown stale.
    pub(super) fn translate(&mut self, edits: &[AppliedEdit]) {
        self.semantic_blocks = Arc::default();
        self.symbols = Arc::default();
        for edit in edits {
            self.highlights = Arc::new(self.highlights.translate(
                BytePos(edit.start_byte),
                BytePos(edit.old_end_byte),
                BytePos(edit.new_end_byte),
            ));
        }
    }

    /// The syntax highlights.
    pub(super) fn highlights(&self) -> &Arc<Highlights> {
        &self.highlights
    }

    /// The fold regions.
    pub(super) fn folds(&self) -> &Arc<FoldRegions> {
        &self.folds
    }

    /// The semantic block scopes.
    pub(super) fn semantic_blocks(&self) -> &Arc<SemanticBlocks> {
        &self.semantic_blocks
    }

    /// The grammar-backed outline.
    pub(super) fn symbols(&self) -> &Arc<Vec<Symbol>> {
        &self.symbols
    }

    /// The syntax-error line ranges.
    pub(super) fn error_lines(&self) -> &Arc<Vec<(u32, u32)>> {
        &self.error_lines
    }

    /// The GFM table line ranges.
    pub(super) fn table_lines(&self) -> &Arc<Vec<RangeInclusive<u32>>> {
        &self.table_lines
    }
}

#[cfg(test)]
mod tests {
    use karet_core::SymbolKind;

    use super::*;

    /// A state as a parse of a Markdown file with an outline, a syntax error
    /// and a table leaves it.
    fn parsed() -> SyntaxState {
        let mut state = SyntaxState::default();
        state.adopt(HighlightResult {
            doc: DocumentId(0),
            version: 1,
            highlights: Arc::default(),
            folds: Arc::default(),
            semantic_blocks: Arc::default(),
            symbols: Arc::new(vec![Symbol {
                name: "Heading".to_owned(),
                kind: SymbolKind::Module,
                detail: None,
                range: Range::default(),
                selection_range: Range::default(),
                container_name: None,
                children: Vec::new(),
            }]),
            error_lines: Arc::new(vec![(3, 4)]),
            table_lines: Arc::new(vec![6..=8]),
        });
        state
    }

    #[test]
    fn adopting_an_answer_replaces_every_part() {
        let state = parsed();
        assert_eq!(state.symbols().len(), 1);
        assert_eq!(**state.error_lines(), [(3, 4)]);
        assert_eq!(**state.table_lines(), [6..=8]);
    }

    #[test]
    fn clearing_leaves_nothing_a_previous_grammar_produced() {
        let mut state = parsed();
        state.clear(Arc::default());
        assert!(state.symbols().is_empty());
        assert!(state.error_lines().is_empty());
        assert!(state.table_lines().is_empty());
        assert!(state.highlights().all().is_empty());
    }

    #[test]
    fn translating_hides_the_outline_and_keeps_line_ranges() {
        let mut state = parsed();
        state.translate(&[]);
        assert!(state.symbols().is_empty());
        assert_eq!(**state.error_lines(), [(3, 4)]);
        assert_eq!(**state.table_lines(), [6..=8]);
    }
}

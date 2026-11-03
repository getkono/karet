//! Fold regions, from three sources in falling order of precision:
//!
//! - [`Folder`] runs a grammar's fold query (see
//!   [`karet_treesitter::folds_query`]): per-construct folds, plus classified
//!   comment and import blocks.
//! - [`fold`] is the grammar-agnostic fallback for a grammar with no fold
//!   query: every multi-line named node folds.
//! - [`fold_by_indent`] needs no grammar at all: a line folds over the lines
//!   indented deeper than it that follow.

use std::collections::BTreeMap;
use std::collections::HashMap;

use karet_core::BytePos;
use karet_core::FoldKind;
use karet_core::FoldRegion;
use karet_core::FoldRegions;
use karet_core::LineIndex;
use karet_treesitter::LanguageId;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;

/// Compute fold regions from a parsed tree, with no per-language configuration.
///
/// The algorithm is deliberately **language-agnostic**: it folds every named node
/// below the root that spans more than one line, keeping the outermost node per start line so a
/// construct and its wrapper (or two constructs sharing a line, like `} else {`)
/// don't stack duplicate folds. This works on any tree-sitter grammar.
///
/// The trade-off is granularity: because it folds *any* multi-line node, it also
/// creates folds for coarse nodes a human might not, and it cannot tell a comment
/// or an import block from code, so every region is [`FoldKind::Code`]. A grammar
/// with a fold query gets the per-construct result from [`Folder`] instead.
#[must_use]
pub fn fold(tree: &SyntaxTree) -> FoldRegions {
    let mut regions = Regions::default();
    let root = tree.span();
    // The root node spans the whole document: folding it would collapse the
    // file into its first line, which "fold all" would then always do.
    for node in tree
        .multiline_named_spans()
        .into_iter()
        .filter(|n| n.span != root)
    {
        regions.insert(node.start_row, node.end_row, FoldKind::Code);
    }
    regions.finish()
}

/// Fold regions from indentation alone, for text no grammar parses.
///
/// A line heads a region when the next non-blank line is indented deeper; the
/// region runs through the last non-blank line before indentation returns to
/// the header's depth or shallower, so trailing blank lines stay visible. A tab
/// advances to the next multiple of `tab_width` columns (a `tab_width` of 0
/// counts as 1).
#[must_use]
pub fn fold_by_indent(text: &str, tab_width: u32) -> FoldRegions {
    let tab_width = tab_width.max(1);
    let mut out = Vec::new();
    // Open headers as `(indent, line)`, each deeper than the one below it.
    let mut open: Vec<(u32, u32)> = Vec::new();
    let mut last_content = 0;
    let mut close = |open: &mut Vec<(u32, u32)>, depth: Option<u32>, end: u32| {
        while let Some(&(indent, start)) = open.last() {
            if depth.is_some_and(|depth| indent < depth) {
                break;
            }
            open.pop();
            if end > start {
                out.push(FoldRegion::new(start, end));
            }
        }
    };
    for (row, line) in (0u32..).zip(text.split('\n')) {
        let Some(indent) = indent_width(line, tab_width) else {
            continue;
        };
        close(&mut open, Some(indent), last_content);
        open.push((indent, row));
        last_content = row;
    }
    close(&mut open, None, last_content);
    out.sort_unstable_by_key(|r| r.start);
    FoldRegions::from_sorted_regions(out)
}

/// The display width of `line`'s leading whitespace, or `None` for a line that
/// is only whitespace (which neither opens nor closes a region).
fn indent_width(line: &str, tab_width: u32) -> Option<u32> {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += tab_width - width % tab_width,
            '\r' => {},
            _ => return Some(width),
        }
    }
    None
}

/// Computes fold regions from the grammars' fold queries (see
/// [`karet_treesitter::folds_query`]), falling back to [`fold`] for a grammar
/// with none.
///
/// Keeps each language's compiled query, so build one and reuse it across the
/// parses it serves.
#[derive(Default)]
pub struct Folder {
    /// The compiled query per language; `None` for a grammar with no fold
    /// query, remembered so it is not looked up again for every version.
    queries: HashMap<LanguageId, Option<Query>>,
}

impl Folder {
    /// A folder with no query compiled yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The fold regions of `tree`, whose source is `text`.
    ///
    /// Each `@fold` capture spanning more than one line becomes a
    /// [`FoldKind::Code`] region. Comments and imports are joined into blocks:
    /// a run of `@fold.comment` captures on adjacent lines is one
    /// [`FoldKind::Comment`] region, and a run of `@fold.imports` captures,
    /// blank lines between them allowed, is one [`FoldKind::Imports`] region.
    /// A comment that trails code on its line takes no part. Where regions
    /// share a header line the longest is kept.
    #[must_use]
    pub fn analyze(&mut self, tree: &SyntaxTree, text: &str) -> FoldRegions {
        let lang = tree.language();
        let query = self.queries.entry(lang).or_insert_with(|| {
            karet_treesitter::folds_query(lang).and_then(|source| Query::compile(lang, source).ok())
        });
        match query {
            Some(query) => from_captures(tree, query, text),
            None => fold(tree),
        }
    }
}

/// One captured comment or import, by its first and last line.
#[derive(Clone, Copy)]
struct Piece {
    first: u32,
    last: u32,
}

fn from_captures(tree: &SyntaxTree, query: &Query, text: &str) -> FoldRegions {
    let names = query.capture_names();
    let lines = LineIndex::new(text);
    let mut regions = Regions::default();
    let mut comments = Vec::new();
    let mut imports = Vec::new();
    for capture in tree.captures(query, text) {
        let Some(name) = names.get(capture.capture as usize) else {
            continue;
        };
        let (start, end) = (capture.span.start, capture.span.end);
        let first = lines.row_at(start);
        // The line holding the last byte: a node that swallows its line break
        // (a line comment, an `#include`) does not claim the next line.
        let last = lines.row_at(BytePos(end.0.saturating_sub(1).max(start.0)));
        let piece = Piece { first, last };
        match *name {
            "fold" => regions.insert(first, last, FoldKind::Code),
            "fold.comment" if leads_line(text, &lines, start) => comments.push(piece),
            "fold.imports" => imports.push(piece),
            _ => {},
        }
    }
    for run in runs(comments, |_| false) {
        regions.insert(run.first, run.last, FoldKind::Comment);
    }
    for run in runs(imports, |row| line_is_blank(text, &lines, row)) {
        regions.insert(run.first, run.last, FoldKind::Imports);
    }
    regions.finish()
}

/// Join `pieces` into runs: a piece starting on or right after the line the
/// run ends on extends it, and so does one separated from it only by lines
/// `bridges` accepts.
fn runs(mut pieces: Vec<Piece>, bridges: impl Fn(u32) -> bool) -> Vec<Piece> {
    pieces.sort_unstable_by_key(|p| (p.first, p.last));
    let mut out: Vec<Piece> = Vec::new();
    for piece in pieces {
        match out.last_mut() {
            Some(run)
                if piece.first <= run.last.saturating_add(1)
                    || (run.last + 1..piece.first).all(&bridges) =>
            {
                run.last = run.last.max(piece.last);
            },
            _ => out.push(piece),
        }
    }
    out
}

/// Whether only whitespace precedes `at` on its line.
fn leads_line(text: &str, lines: &LineIndex, at: BytePos) -> bool {
    let start = lines.start_of(lines.row_at(at)).map_or(0, |s| s.0);
    text.get(start..at.0)
        .is_some_and(|before| before.chars().all(char::is_whitespace))
}

/// Whether line `row` of `text` holds only whitespace.
fn line_is_blank(text: &str, lines: &LineIndex, row: u32) -> bool {
    let start = lines.start_of(row).map_or(text.len(), |s| s.0);
    let end = lines.start_of(row + 1).map_or(text.len(), |s| s.0);
    text.get(start..end)
        .is_some_and(|line| line.chars().all(char::is_whitespace))
}

/// Fold regions under construction: one per header line, the longest kept.
#[derive(Default)]
struct Regions {
    by_start: BTreeMap<u32, (u32, FoldKind)>,
}

impl Regions {
    /// Offer the region `start..=end`. A single-line region folds nothing and
    /// is dropped; on a tie in length a classified kind beats
    /// [`FoldKind::Code`], so "fold all comments" still finds a block a code
    /// fold shares its lines with.
    fn insert(&mut self, start: u32, end: u32, kind: FoldKind) {
        if end <= start {
            return;
        }
        let slot = self.by_start.entry(start).or_insert((end, kind));
        if end > slot.0 || (end == slot.0 && slot.1 == FoldKind::Code) {
            *slot = (end, kind);
        }
    }

    /// The regions in document order, which is [`FoldRegions`]' contract.
    fn finish(self) -> FoldRegions {
        FoldRegions::from_sorted_regions(
            self.by_start
                .into_iter()
                .map(|(start, (end, kind))| FoldRegion::new(start, end).with_kind(kind))
                .collect(),
        )
    }
}

#[cfg(test)]
#[path = "folding/tests.rs"]
mod tests;

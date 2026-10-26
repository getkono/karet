//! `karet-treesitter` — the shared tree-sitter parse host for the karet toolkit.
//!
//! Owns parser pooling, tree caching and query execution so that `karet-syntax`
//! and (via syntax) the editor reuse a single parse of each buffer. (`karet-diff`
//! is purely textual today and does not depend on this crate.) Tree-sitter is karet's *sole* syntax backend — there is deliberately no
//! second backend to abstract over. Grammars are compiled in behind `lang-*`
//! features; [`language_id_from_path`] / [`language_name_from_path`] map a file to
//! one (or to a plaintext fallback).

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use karet_core::BytePos;
use karet_core::Span;

mod changes;
mod detect;
mod folds;
mod indent;
mod injection;
mod registry;
mod walk;

#[cfg(test)]
mod tests;

pub use changes::merge_spans;
pub use changes::shift_span;
pub use detect::language_id_from_injection_name;
pub use detect::language_id_from_path;
pub use detect::language_name_from_path;
pub use indent::IndentAnchor;
pub use injection::InjectionRegion;
pub use walk::WalkControl;
pub use walk::WalkNode;

/// Errors produced by the parse host.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TsError {
    /// No grammar is registered (compiled in) for the requested language.
    #[error("unknown language")]
    UnknownLanguage,
    /// The parser failed to produce a tree.
    #[error("parse failed")]
    ParseFailed,
    /// A query failed to compile.
    #[error("invalid query: {0}")]
    InvalidQuery(String),
}

/// An identifier for a registered tree-sitter grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LanguageId(pub u16);

/// The highlights query source for `lang`, if its grammar is compiled in.
#[must_use]
pub fn highlights_query(lang: LanguageId) -> Option<&'static str> {
    registry::grammar(lang).map(|g| g.highlights)
}

/// The injections query source for `lang`, if its grammar is compiled in *and* has
/// one. `None` means the language embeds nothing.
///
/// Returns a [`Cow`] because karet appends its own patterns to some grammars (see
/// `injections_extra`) — the common case still borrows the grammar's `&'static str`
/// with no allocation.
#[must_use]
pub fn injections_query(lang: LanguageId) -> Option<Cow<'static, str>> {
    let g = registry::grammar(lang)?;
    match (g.injections, g.injections_extra) {
        (Some(base), Some(extra)) => Some(Cow::Owned(format!("{base}\n{extra}"))),
        (Some(q), None) | (None, Some(q)) => Some(Cow::Borrowed(q)),
        (None, None) => None,
    }
}

/// The semantic-block query source for `lang`, if that grammar has meaningful
/// nested text scopes.
///
/// Queries use `semantic.scope` for the full lifetime of a block, optionally
/// `semantic.header` for the source header and `semantic.body` for the body that
/// follows a signature. Markdown headings instead use
/// `semantic.heading.{1..6}` so consumers can derive CommonMark section ranges.
#[must_use]
pub fn semantic_query(lang: LanguageId) -> Option<&'static str> {
    registry::semantic_query(lang)
}

/// The declaration query used to build a document outline for `lang`.
///
/// Captures named `definition.*` cover the complete declaration and their
/// accompanying `name` capture covers the navigation label. Languages whose
/// upstream grammar ships a tags query reuse it; data and markup grammars use
/// small karet-authored equivalents.
#[must_use]
pub fn outline_query(lang: LanguageId) -> Option<Cow<'static, str>> {
    registry::outline_query(lang)
}

/// The indent query for `lang`, if karet ships one for its grammar (Rust,
/// Python, JavaScript, TypeScript/TSX, JSON, Go, C, C++, YAML and Lua).
///
/// `@indent` captures a node whose inner lines sit one level deeper than the line
/// it starts on; `@outdent` captures a closing token. [`SyntaxTree::newline_indent`]
/// reads the query; `None` means the caller keeps copying the previous line's
/// indentation.
#[must_use]
pub fn indents_query(lang: LanguageId) -> Option<&'static str> {
    indent::query_source(lang)
}

/// The fold query for `lang`, if karet ships one for its grammar (Rust,
/// Python, JavaScript, TypeScript/TSX, JSON, Go, C, C++, YAML and Lua).
///
/// `@fold` captures a construct that folds from the line it starts on through
/// the line it ends on; `@fold.comment` and `@fold.imports` capture one comment
/// or import declaration each, which a consumer joins into a block when they sit
/// on adjacent lines. `None` means the caller keeps a grammar-agnostic fallback.
#[must_use]
pub fn folds_query(lang: LanguageId) -> Option<&'static str> {
    folds::query_source(lang)
}

/// A pool of reusable tree-sitter parsers, keyed by [`LanguageId`].
#[derive(Default)]
pub struct ParserPool {
    parsers: HashMap<LanguageId, tree_sitter::Parser>,
}

impl ParserPool {
    /// Create an empty parser pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get (or lazily create) the parser for `lang`.
    fn parser_for(&mut self, lang: LanguageId) -> Result<&mut tree_sitter::Parser, TsError> {
        match self.parsers.entry(lang) {
            Entry::Occupied(e) => Ok(e.into_mut()),
            Entry::Vacant(e) => {
                let info = registry::grammar(lang).ok_or(TsError::UnknownLanguage)?;
                let mut parser = tree_sitter::Parser::new();
                parser
                    .set_language(&(info.language)())
                    .map_err(|_| TsError::UnknownLanguage)?;
                Ok(e.insert(parser))
            },
        }
    }
}

/// The neutral applied-edit descriptor consumed by [`SyntaxTree::edit`], shared
/// with `karet-text` (which produces one per applied edit) through `karet-core`.
///
/// Points are `(row, column-in-bytes)` — tree-sitter columns are byte offsets from
/// the line start, **not** `char` columns.
pub type Edit = karet_core::AppliedEdit;

/// A parsed syntax tree for one buffer.
pub struct SyntaxTree {
    tree: tree_sitter::Tree,
    lang: LanguageId,
}

impl SyntaxTree {
    /// Parse `text` as `lang`, drawing a parser from `pool`.
    ///
    /// # Errors
    /// Returns [`TsError::UnknownLanguage`] if `lang` has no grammar compiled in, or
    /// [`TsError::ParseFailed`] if parsing fails.
    pub fn parse(pool: &mut ParserPool, lang: LanguageId, text: &str) -> Result<Self, TsError> {
        let parser = pool.parser_for(lang)?;
        let tree = parser
            .parse(text.as_bytes(), None)
            .ok_or(TsError::ParseFailed)?;
        Ok(Self { tree, lang })
    }

    /// The language this tree was parsed with.
    #[must_use]
    pub fn language(&self) -> LanguageId {
        self.lang
    }

    /// The byte span this tree covers. For an injected layer this is the extent of its
    /// included ranges, in document coordinates — not `0..len`.
    #[must_use]
    pub fn span(&self) -> Span {
        let root = self.tree.root_node();
        Span {
            start: BytePos(root.start_byte()),
            end: BytePos(root.end_byte()),
        }
    }

    /// The named node at `byte` followed by its named ancestors, innermost first.
    ///
    /// The returned descriptors contain only stable, neutral data; no tree-sitter
    /// node handles escape this crate. A point at end-of-file resolves against the
    /// root/last node so editor features can inspect an insertion caret there.
    #[must_use]
    pub fn named_ancestors_at(&self, byte: BytePos) -> Vec<SyntaxNode> {
        let root = self.tree.root_node();
        let point = byte.0.min(root.end_byte());
        let mut node = root.descendant_for_byte_range(point, point).unwrap_or(root);
        let mut nodes = Vec::new();
        loop {
            if node.is_named() {
                nodes.push(SyntaxNode {
                    kind: node.kind().to_owned(),
                    span: Span {
                        start: BytePos(node.start_byte()),
                        end: BytePos(node.end_byte()),
                    },
                });
            }
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent;
        }
        nodes
    }

    /// The byte spans of every node that covers `span`, innermost first, each span
    /// listed once — the steps Expand Selection grows through.
    ///
    /// Anonymous nodes count as well as named ones, so a selection inside a keyword
    /// grows to the keyword. A node covers `span` when it contains both ends; an
    /// empty `span` is covered by the node that starts at or before it and ends
    /// after it, so a caret between two tokens resolves to the one that follows.
    /// For an injected layer, nodes outside its included ranges never cover
    /// anything, and a `span` outside the tree yields nothing.
    #[must_use]
    pub fn enclosing_spans(&self, span: Span) -> Vec<Span> {
        let root = self.tree.root_node();
        let (start, end) = (span.start.0, span.end.0.max(span.start.0));
        let mut node = root.descendant_for_byte_range(start, end);
        let mut spans: Vec<Span> = Vec::new();
        while let Some(current) = node {
            let covering = Span {
                start: BytePos(current.start_byte()),
                end: BytePos(current.end_byte()),
            };
            let covers = covering.start.0 <= start && end <= covering.end.0;
            if covers && spans.last() != Some(&covering) {
                spans.push(covering);
            }
            node = current.parent();
        }
        spans
    }

    /// The inclusive line ranges (0-based rows) covered by syntax errors: the
    /// outermost `ERROR` nodes plus zero-width *missing* nodes the parser
    /// inserted to recover (a missing `;` is as much an error as a stray one).
    /// Empty when the tree parsed cleanly.
    ///
    /// This is the "no outright errors" gate for features that should hold off
    /// while the user is mid-edit — e.g. auto-triggered completion suppresses
    /// itself when an error range intersects the caret's line. Ranges are
    /// sorted by start line; a multi-line error covers every line it spans.
    #[must_use]
    pub fn error_lines(&self) -> Vec<(u32, u32)> {
        let root = self.tree.root_node();
        if !root.has_error() {
            return Vec::new();
        }
        let mut out = Vec::new();
        // Iterative walk (deep sources nest arbitrarily): descend only into
        // subtrees that contain an error, and stop at the outermost error node.
        let mut cursor = root.walk();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.is_error() || node.is_missing() {
                let start = node.start_position();
                let end = node.end_position();
                let start_row = u32::try_from(start.row).unwrap_or(u32::MAX);
                let mut end_row = u32::try_from(end.row).unwrap_or(u32::MAX);
                // A node ending at column 0 of a later line doesn't occupy it.
                if end.column == 0 && end_row > start_row {
                    end_row -= 1;
                }
                out.push((start_row, end_row.max(start_row)));
                continue;
            }
            if !node.has_error() {
                continue;
            }
            stack.extend(node.children(&mut cursor));
        }
        out.sort_unstable();
        out
    }

    /// Parse only `ranges` of `text` as `lang`, leaving the rest of the document
    /// invisible to the grammar — the mechanism behind language injection.
    ///
    /// `ranges` must be ascending and non-overlapping (see `injection::normalize`).
    /// Because the parser reads the *whole* `text` and merely restricts itself to
    /// `ranges`, every node's byte offset is already in document coordinates, so the
    /// resulting tree's captures need no translation to merge with the root's.
    ///
    /// # Errors
    /// Returns [`TsError::UnknownLanguage`] if `lang` has no grammar compiled in, or
    /// [`TsError::ParseFailed`] if the ranges are rejected or parsing fails.
    pub fn parse_ranges(
        pool: &mut ParserPool,
        lang: LanguageId,
        text: &str,
        ranges: &[Span],
    ) -> Result<Self, TsError> {
        Self::parse_ranges_indexed(pool, lang, text, ranges, &karet_core::LineIndex::new(text))
    }

    /// [`parse_ranges`](Self::parse_ranges) against a precomputed line index.
    ///
    /// Building that index is `O(text)`, and a document can have hundreds of injected
    /// layers, so the layered parser computes it once and lends it to every layer rather
    /// than paying for it per parse.
    fn parse_ranges_indexed(
        pool: &mut ParserPool,
        lang: LanguageId,
        text: &str,
        ranges: &[Span],
        starts: &karet_core::LineIndex,
    ) -> Result<Self, TsError> {
        if ranges.is_empty() {
            return Err(TsError::ParseFailed);
        }
        let ts_ranges = to_ts_ranges(ranges, starts);
        let parser = pool.parser_for(lang)?;
        let result = parser
            .set_included_ranges(&ts_ranges)
            .map_err(|_| TsError::ParseFailed)
            .and_then(|()| {
                parser
                    .parse(text.as_bytes(), None)
                    .ok_or(TsError::ParseFailed)
            });
        // The parser is pooled: clear the range restriction before anyone else draws it,
        // whether or not the parse succeeded.
        parser.set_included_ranges(&[]).ok();
        Ok(Self {
            tree: result?,
            lang,
        })
    }

    /// The embedded-language regions `query` (an injections query) finds in this tree.
    ///
    /// `text` must be the source this tree was parsed from. Regions naming a language
    /// with no grammar compiled in are omitted.
    #[must_use]
    pub fn injections(&self, query: &Query, text: &str) -> Vec<InjectionRegion> {
        injection::extract(&self.tree, query, text)
    }

    /// Re-parse after the buffer changed to `text`.
    ///
    /// Without a prior edit applied to the old tree this is a full reparse; that is
    /// acceptable for the snippet-sized inputs the MVP highlights. Prefer
    /// [`edit`](Self::edit) + [`reparse_with`](Self::reparse_with) for genuine
    /// incremental reparsing.
    ///
    /// # Errors
    /// Returns [`TsError::ParseFailed`] if re-parsing fails.
    pub fn reparse(&mut self, pool: &mut ParserPool, text: &str) -> Result<(), TsError> {
        let parser = pool.parser_for(self.lang)?;
        let tree = parser
            .parse(text.as_bytes(), Some(&self.tree))
            .ok_or(TsError::ParseFailed)?;
        self.tree = tree;
        Ok(())
    }

    /// Mark an [`Edit`] on the tree so the next reparse can reuse unaffected
    /// subtrees.
    ///
    /// Apply edits in **descending start-byte order** with original-frame
    /// coordinates (matching `karet-text`'s applied edits), so each edit's
    /// coordinates remain valid against the tree's evolving state.
    pub fn edit(&mut self, edit: &Edit) {
        self.tree.edit(&tree_sitter::InputEdit {
            start_byte: edit.start_byte,
            old_end_byte: edit.old_end_byte,
            new_end_byte: edit.new_end_byte,
            start_position: to_point(edit.start_point),
            old_end_position: to_point(edit.old_end_point),
            new_end_position: to_point(edit.new_end_point),
        });
    }

    /// Incrementally re-parse the edited tree, reading the new text through `read`
    /// (a byte-offset → byte-slice callback), reusing subtrees the prior
    /// [`edit`](Self::edit) calls left untouched.
    ///
    /// `read(byte)` must return the buffer bytes starting at `byte` (e.g. a rope
    /// chunk), or an empty slice at/after the end — so the parser is fed without
    /// ever materializing the whole file as one `String`.
    ///
    /// # Errors
    /// Returns [`TsError::ParseFailed`] if re-parsing fails.
    pub fn reparse_with<T, F>(&mut self, pool: &mut ParserPool, mut read: F) -> Result<(), TsError>
    where
        T: AsRef<[u8]>,
        F: FnMut(usize) -> T,
    {
        let parser = pool.parser_for(self.lang)?;
        let mut callback = |byte: usize, _: tree_sitter::Point| read(byte);
        let tree = parser
            .parse_with_options(&mut callback, Some(&self.tree), None)
            .ok_or(TsError::ParseFailed)?;
        self.tree = tree;
        Ok(())
    }

    /// The byte ranges that differ between `old` and this tree.
    #[must_use]
    pub fn changed_ranges(&self, old: &SyntaxTree) -> Vec<Span> {
        old.tree
            .changed_ranges(&self.tree)
            .map(|r| Span {
                start: BytePos(r.start_byte),
                end: BytePos(r.end_byte),
            })
            .collect()
    }

    /// Run `query` over this tree and collect every capture.
    ///
    /// `text` must be the same source this tree was parsed from. This is the seam
    /// that keeps the streaming-iterator query API inside this crate.
    #[must_use]
    pub fn captures(&self, query: &Query, text: &str) -> Vec<RawCapture> {
        self.captures_with(tree_sitter::QueryCursor::new(), query, text)
    }

    /// [`captures`](Self::captures) restricted to the matches that intersect
    /// `range`.
    ///
    /// A match is kept whole, so a capture may reach outside `range`: a string
    /// node holding the range is reported with its full extent.
    #[must_use]
    pub fn captures_in(&self, query: &Query, text: &str, range: Span) -> Vec<RawCapture> {
        let mut cursor = tree_sitter::QueryCursor::new();
        cursor.set_byte_range(range.start.0..range.end.0);
        self.captures_with(cursor, query, text)
    }

    fn captures_with(
        &self,
        mut cursor: tree_sitter::QueryCursor,
        query: &Query,
        text: &str,
    ) -> Vec<RawCapture> {
        use tree_sitter::StreamingIterator;

        let mut it = cursor.captures(&query.inner, self.tree.root_node(), text.as_bytes());
        let mut out = Vec::new();
        while let Some((m, idx)) = it.next() {
            if let Some(cap) = m.captures.get(*idx) {
                out.push(RawCapture {
                    capture: cap.index,
                    span: Span {
                        start: BytePos(cap.node.start_byte()),
                        end: BytePos(cap.node.end_byte()),
                    },
                });
            }
        }
        out
    }

    /// Run `query` and retain captures grouped by their original query match.
    ///
    /// Grouping is required when several captures describe one construct, such
    /// as a method's full scope, signature and body.
    #[must_use]
    pub fn matches(&self, query: &Query, text: &str) -> Vec<QueryMatch> {
        use tree_sitter::StreamingIterator;

        let mut cursor = tree_sitter::QueryCursor::new();
        let mut it = cursor.matches(&query.inner, self.tree.root_node(), text.as_bytes());
        let mut out = Vec::new();
        while let Some(m) = it.next() {
            out.push(QueryMatch {
                pattern: m.pattern_index,
                captures: m
                    .captures
                    .iter()
                    .map(|cap| RawCapture {
                        capture: cap.index,
                        span: Span {
                            start: BytePos(cap.node.start_byte()),
                            end: BytePos(cap.node.end_byte()),
                        },
                    })
                    .collect(),
            });
        }
        out
    }

    /// Every *named* node that begins and ends on different lines, in a pre-order
    /// walk (outermost before inner). This is the neutral raw material for deriving
    /// fold regions — the grammar-agnostic tree geometry, with no tree-sitter types
    /// leaking into the public API.
    #[must_use]
    pub fn multiline_named_spans(&self) -> Vec<MultilineSpan> {
        let mut out = Vec::new();
        let mut cursor = self.tree.walk();
        'walk: loop {
            let node = cursor.node();
            if node.is_named() {
                let start = node.start_position().row;
                let end = node.end_position().row;
                if end > start {
                    out.push(MultilineSpan {
                        span: Span {
                            start: BytePos(node.start_byte()),
                            end: BytePos(node.end_byte()),
                        },
                        start_row: start as u32,
                        end_row: end as u32,
                    });
                }
            }
            // Descend to the first child, else advance to the next sibling, else climb
            // until a sibling exists — standard iterative pre-order DFS.
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        out
    }
}

/// A neutral descriptor for one named syntax node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxNode {
    /// Grammar-defined node kind (for example `function_item` or `paragraph`).
    pub kind: String,
    /// Byte range occupied by the node in the source document.
    pub span: Span,
}

/// A named syntax node that spans more than one line — the raw input to fold-region
/// computation. Rows are 0-based line numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultilineSpan {
    /// The node's byte range.
    pub span: Span,
    /// The 0-based start line.
    pub start_row: u32,
    /// The 0-based end line.
    pub end_row: u32,
}

/// One capture from [`SyntaxTree::captures`]: a query capture index plus the byte
/// [`Span`] it covers. The index resolves to a name via [`Query::capture_names`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawCapture {
    /// The query capture index (into [`Query::capture_names`]).
    pub capture: u32,
    /// The byte span the capture covers.
    pub span: Span,
}

/// One tree-sitter query match with all of its captures kept together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryMatch {
    /// The index of the matching pattern in the query source.
    pub pattern: usize,
    /// The captures belonging to this match.
    pub captures: Vec<RawCapture>,
}

/// A compiled tree-sitter query (highlights, folds, locals, …).
pub struct Query {
    pub(crate) inner: tree_sitter::Query,
}

impl Query {
    /// Compile `source` against the grammar for `lang`.
    ///
    /// # Errors
    /// Returns [`TsError::UnknownLanguage`] if `lang` has no grammar compiled in, or
    /// [`TsError::InvalidQuery`] if the query text is malformed.
    pub fn compile(lang: LanguageId, source: &str) -> Result<Self, TsError> {
        let info = registry::grammar(lang).ok_or(TsError::UnknownLanguage)?;
        let language = (info.language)();
        let inner = tree_sitter::Query::new(&language, source)
            .map_err(|e| TsError::InvalidQuery(e.to_string()))?;
        Ok(Self { inner })
    }

    /// The capture names, indexed by [`RawCapture::capture`].
    #[must_use]
    pub fn capture_names(&self) -> &[&str] {
        self.inner.capture_names()
    }
}

/// Convert a neutral `(row, byte-column)` point to a tree-sitter `Point`.
fn to_point(point: karet_core::BytePoint) -> tree_sitter::Point {
    let (row, column) = (point.row, point.col);
    tree_sitter::Point { row, column }
}

/// The `(row, byte-column)` of `byte`, resolved against the shared
/// [`karet_core::LineIndex`] (the one line-index implementation in the toolkit).
fn point_at(starts: &karet_core::LineIndex, byte: usize) -> tree_sitter::Point {
    let (row, column) = starts.point_at(karet_core::BytePos(byte));
    tree_sitter::Point {
        row: row as usize,
        column,
    }
}

/// Convert byte [`Span`]s into the `tree_sitter::Range`s `set_included_ranges` wants,
/// resolving points against a precomputed line index.
fn to_ts_ranges(ranges: &[Span], starts: &karet_core::LineIndex) -> Vec<tree_sitter::Range> {
    ranges
        .iter()
        .map(|s| tree_sitter::Range {
            start_byte: s.start.0,
            end_byte: s.end.0,
            start_point: point_at(starts, s.start.0),
            end_point: point_at(starts, s.end.0),
        })
        .collect()
}

/// How deep injections may nest before the parser stops descending.
///
/// Rust → markdown (doc comment) → rust (a ` ```rust ` doctest) → markdown is already
/// three levels, and a doctest's own doc comments could recurse forever. Four bounds
/// the useful cases and terminates the pathological ones.
const MAX_INJECTION_DEPTH: usize = 4;

/// An upper bound on injected layers per document, so a pathological file cannot make
/// the parse host allocate without limit. Generous enough that no real document trips
/// it — a large markdown file has one inline layer per paragraph.
const MAX_INJECTION_LAYERS: usize = 4096;

/// A parse host for documents that embed other languages.
///
/// Owns the parser pool *and* the compiled injections queries, because deriving the
/// layers of a tree needs both and the borrow checker would otherwise force the caller
/// to interleave them.
#[derive(Default)]
pub struct LayeredParser {
    pool: ParserPool,
    /// Compiled injections query per language; `None` records "this grammar ships
    /// none", so we never try to compile it twice.
    injections: HashMap<LanguageId, Option<Query>>,
}

impl LayeredParser {
    /// Create an empty layered parse host.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The underlying parser pool, for callers that need a plain single-language parse.
    pub fn pool(&mut self) -> &mut ParserPool {
        &mut self.pool
    }

    /// Parse `text` as `lang`, then recursively parse every injected region.
    ///
    /// # Errors
    /// Returns [`TsError::UnknownLanguage`] if `lang` has no grammar compiled in, or
    /// [`TsError::ParseFailed`] if the root parse fails. A child layer that fails to
    /// parse is skipped rather than failing the document.
    pub fn parse(&mut self, lang: LanguageId, text: &str) -> Result<LayeredTree, TsError> {
        let root = SyntaxTree::parse(&mut self.pool, lang, text)?;
        let children = self.build_layers(&root, text);
        Ok(LayeredTree { root, children })
    }

    /// Re-parse `tree` after `edits` changed the buffer to `text`.
    ///
    /// The root tree is reparsed incrementally (reusing untouched subtrees); the
    /// injected layers are re-derived, since an edit can create, destroy or retarget a
    /// region — typing ` ```rust ` turns a paragraph into a code fence.
    ///
    /// # Errors
    /// Returns [`TsError::ParseFailed`] if the root cannot be reparsed.
    pub fn reparse(
        &mut self,
        tree: &mut LayeredTree,
        edits: &[Edit],
        text: &str,
    ) -> Result<(), TsError> {
        self.reparse_changed(tree, edits, text).map(drop)
    }

    /// [`reparse`](Self::reparse), returning the byte ranges of `text` whose
    /// syntax may differ from `tree` before the edits: sorted, disjoint, and
    /// covering every range the edits wrote, every range where the root tree's
    /// structure changed, and the span of every injected layer that is new, gone,
    /// or reparsed over edited text. Outside them, a query over the new tree
    /// finds what it found over the old one, moved by the edits.
    ///
    /// Too many edits at once (a large multi-cursor replace) report the whole
    /// document rather than track each one.
    ///
    /// # Errors
    /// Returns [`TsError::ParseFailed`] if the root cannot be reparsed.
    pub fn reparse_changed(
        &mut self,
        tree: &mut LayeredTree,
        edits: &[Edit],
        text: &str,
    ) -> Result<Vec<Span>, TsError> {
        let edited = changes::edited_ranges(edits);
        for edit in edits {
            tree.root.edit(edit);
            // The old layers are only compared when the edits are tracked.
            if edited.is_some() {
                for child in &mut tree.children {
                    child.edit(edit);
                }
            }
        }
        // Cloning a tree shares its nodes; it costs a reference count.
        let old_root = SyntaxTree {
            tree: tree.root.tree.clone(),
            lang: tree.root.lang,
        };
        tree.root.reparse(&mut self.pool, text)?;
        let old_children =
            std::mem::replace(&mut tree.children, self.build_layers(&tree.root, text));

        let Some(mut changed) = edited else {
            return Ok(vec![Span {
                start: BytePos(0),
                end: BytePos(text.len()),
            }]);
        };
        let layers = changes::layer_changes(&old_children, &tree.children, &changed);
        changed.extend(tree.root.changed_ranges(&old_root));
        changed.extend(layers);
        Ok(changes::merge_spans(changed))
    }

    /// Expand every injected layer beneath `root`, breadth-first.
    ///
    /// Breadth-first rather than depth-first so the result is ordered by depth. A
    /// consumer merging captures across layers can then let the later (deeper) layer
    /// win a tie against the shallower layer that injected it.
    fn build_layers(&mut self, root: &SyntaxTree, text: &str) -> Vec<SyntaxTree> {
        // One line index for the whole descent: every injected layer needs `(row, col)`
        // points for its ranges, and rebuilding the index per layer would make a
        // many-layer document quadratic in its own length.
        let starts = karet_core::LineIndex::new(text);
        let mut out: Vec<SyntaxTree> = Vec::new();
        let mut frontier = self.expand(root, text, &starts);

        for _ in 1..=MAX_INJECTION_DEPTH {
            if frontier.is_empty() {
                break;
            }
            // Admit what we can of this level, then stop rather than silently
            // half-highlighting a pathological document.
            let room = MAX_INJECTION_LAYERS.saturating_sub(out.len());
            if frontier.len() >= room {
                frontier.truncate(room);
                out.append(&mut frontier);
                break;
            }
            let mut next = Vec::new();
            for layer in &frontier {
                next.extend(self.expand(layer, text, &starts));
            }
            out.append(&mut frontier);
            frontier = next;
        }
        out
    }

    /// Parse the regions `parent` directly injects — one layer per region.
    fn expand(
        &mut self,
        parent: &SyntaxTree,
        text: &str,
        starts: &karet_core::LineIndex,
    ) -> Vec<SyntaxTree> {
        let Some(regions) = self.regions_of(parent, text) else {
            return Vec::new();
        };

        // `injection.combined` means every region of that pattern forms ONE tree (a Rust
        // doc comment's `///` lines are a single markdown document); otherwise each
        // region is its own tree (two code fences are two independent programs).
        let mut combined: Vec<(LanguageId, Vec<Span>)> = Vec::new();
        let mut separate: Vec<(LanguageId, Vec<Span>)> = Vec::new();
        for region in regions {
            if region.combined {
                match combined.iter_mut().find(|(l, _)| *l == region.lang) {
                    Some((_, ranges)) => ranges.extend(region.ranges),
                    None => combined.push((region.lang, region.ranges)),
                }
            } else {
                separate.push((region.lang, region.ranges));
            }
        }

        let mut children = Vec::new();
        for (lang, ranges) in combined.into_iter().chain(separate) {
            let ranges = injection::normalize(ranges);
            // A layer that reproduces its parent verbatim would recurse forever; the
            // depth cap catches it eventually, but refusing it here keeps the tree tidy.
            if ranges.is_empty() || covers_same_text(parent, &ranges, lang) {
                continue;
            }
            // A failed layer degrades to the parent's highlighting rather than failing
            // the whole document.
            if let Ok(child) =
                SyntaxTree::parse_ranges_indexed(&mut self.pool, lang, text, &ranges, starts)
            {
                children.push(child);
            }
        }
        children
    }

    /// The injected regions `tree` declares, or `None` if its grammar injects nothing.
    fn regions_of(&mut self, tree: &SyntaxTree, text: &str) -> Option<Vec<InjectionRegion>> {
        let lang = tree.language();
        // Compile once per language and remember the "ships no query" answer too.
        self.injections.entry(lang).or_insert_with(|| {
            let source = injections_query(lang)?;
            Query::compile(lang, &source).ok()
        });
        let query = self.injections.get(&lang)?.as_ref()?;
        Some(tree.injections(query, text))
    }
}

/// Whether an injected layer would re-parse exactly its parent's own text in its
/// parent's own language — an identity injection that must not be followed.
fn covers_same_text(parent: &SyntaxTree, ranges: &[Span], lang: LanguageId) -> bool {
    lang == parent.language()
        && ranges.len() == 1
        && ranges[0].start.0 == parent.tree.root_node().start_byte()
        && ranges[0].end.0 == parent.tree.root_node().end_byte()
}

/// A document parsed as a root tree plus one tree per injected region.
///
/// Every layer's nodes carry document byte offsets (they are parsed with included
/// ranges over the shared source), so a consumer can run each layer's own queries and
/// merge the results without translating coordinates.
pub struct LayeredTree {
    root: SyntaxTree,
    /// Injected layers in breadth-first order: every layer the root injects
    /// directly, then every layer those inject, and so on (shallowest first).
    children: Vec<SyntaxTree>,
}

impl LayeredTree {
    /// The root tree — the document's own language.
    #[must_use]
    pub fn root(&self) -> &SyntaxTree {
        &self.root
    }

    /// The injected layers, excluding the root, in breadth-first order
    /// (shallowest first).
    #[must_use]
    pub fn children(&self) -> &[SyntaxTree] {
        &self.children
    }

    /// Every layer, root first, then each injected layer.
    pub fn layers(&self) -> impl Iterator<Item = &SyntaxTree> {
        std::iter::once(&self.root).chain(&self.children)
    }

    /// The union of every layer's [`SyntaxTree::error_lines`], sorted by start
    /// line — the whole document's "has a syntax error on this line" index.
    #[must_use]
    pub fn error_lines(&self) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = self.layers().flat_map(SyntaxTree::error_lines).collect();
        out.sort_unstable();
        out
    }

    /// [`SyntaxTree::enclosing_spans`] across every layer that contains
    /// `span.start`: an injected language's nodes first, then its host's, ordered
    /// smallest first and each span listed once.
    ///
    /// Sorting by length rather than concatenating keeps the order right where an
    /// injected layer's own root is wider than the host node holding the caret (a
    /// combined injection spanning several comments).
    #[must_use]
    pub fn enclosing_spans(&self, span: Span) -> Vec<Span> {
        let mut out: Vec<Span> = self
            .layers_at(span.start)
            .into_iter()
            .flat_map(|layer| layer.enclosing_spans(span))
            .collect();
        out.sort_by_key(|s| (s.end.0 - s.start.0, s.start.0));
        out.dedup();
        out
    }
}

/// The error variants and the injection-layer cap, pinned exactly. Kept here rather
/// than in `tests.rs`, which sits near the file-line ceiling.
#[cfg(test)]
mod contract_tests {
    use super::*;

    /// A language id no grammar is registered under.
    const UNREGISTERED: LanguageId = LanguageId(60000);

    #[test]
    fn parsing_an_unregistered_language_is_unknown_language() {
        let mut pool = ParserPool::new();
        let plain = SyntaxTree::parse(&mut pool, UNREGISTERED, "x");
        assert!(matches!(plain, Err(TsError::UnknownLanguage)));
        let layered = LayeredParser::new().parse(UNREGISTERED, "x");
        assert!(matches!(layered, Err(TsError::UnknownLanguage)));
    }

    #[test]
    fn compiling_for_an_unregistered_language_is_unknown_language() {
        let query = Query::compile(UNREGISTERED, "(identifier) @name");
        assert!(matches!(query, Err(TsError::UnknownLanguage)));
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_malformed_query_is_invalid_query() -> Result<(), TsError> {
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        // A node type the grammar does not have, then an unbalanced pattern.
        for source in ["(no_such_node) @x", "((identifier) @x"] {
            let err = Query::compile(rust, source);
            assert!(
                matches!(&err, Err(TsError::InvalidQuery(message)) if !message.is_empty()),
                "{source:?} must be rejected as an invalid query"
            );
        }
        Ok(())
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn layers_past_the_cap_are_truncated_to_exactly_the_cap() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        // One more fence than the cap admits: every fence is its own rust layer at
        // depth 1, so that level alone overflows and is cut to the cap.
        let src = "```rust\nfn f() {}\n```\n".repeat(MAX_INJECTION_LAYERS + 1);
        let tree = LayeredParser::new().parse(md, &src)?;
        assert_eq!(tree.children().len(), MAX_INJECTION_LAYERS);
        // Every admitted layer is one of the fences.
        assert!(tree.children().iter().all(|layer| layer.language() == rust));
        Ok(())
    }
}

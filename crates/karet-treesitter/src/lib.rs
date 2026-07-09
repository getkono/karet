//! `karet-treesitter` — the shared tree-sitter parse host for the karet toolkit.
//!
//! Owns parser pooling, tree caching and query execution so that `karet-syntax`,
//! `karet-diff`, and (via syntax) the editor all reuse a single parse of each
//! buffer. Tree-sitter is karet's *sole* syntax backend — there is deliberately no
//! second backend to abstract over. Grammars are compiled in behind `lang-*`
//! features; [`language_id_from_path`] / [`language_name_from_path`] map a file to
//! one (or to a plaintext fallback).

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use karet_core::BytePos;
use karet_core::Span;

mod detect;
mod injection;
mod registry;

pub use detect::language_id_from_injection_name;
pub use detect::language_id_from_path;
pub use detect::language_name_from_path;
pub use injection::InjectionRegion;

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

/// A neutral edit descriptor mirroring tree-sitter's `InputEdit`.
///
/// Points are `(row, column-in-bytes)` — tree-sitter columns are byte offsets from
/// the line start, **not** `char` columns. `karet-text` produces these from each
/// applied edit; feed them to [`SyntaxTree::edit`] before reparsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Byte offset where the edit starts.
    pub start_byte: usize,
    /// Byte offset of the end of the replaced region in the old text.
    pub old_end_byte: usize,
    /// Byte offset of the end of the inserted text in the new text.
    pub new_end_byte: usize,
    /// `(row, byte-column)` of `start_byte`.
    pub start_point: (usize, usize),
    /// `(row, byte-column)` of `old_end_byte`.
    pub old_end_point: (usize, usize),
    /// `(row, byte-column)` of `new_end_byte`.
    pub new_end_point: (usize, usize),
}

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
        if ranges.is_empty() {
            return Err(TsError::ParseFailed);
        }
        let ts_ranges = to_ts_ranges(text, ranges);
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
        use tree_sitter::StreamingIterator;

        let mut cursor = tree_sitter::QueryCursor::new();
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
fn to_point((row, column): (usize, usize)) -> tree_sitter::Point {
    tree_sitter::Point { row, column }
}

/// Byte offsets at which each line of `text` begins (line 0 starts at 0).
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    starts
}

/// The `(row, byte-column)` of `byte`, resolved against a precomputed line index.
fn point_at(starts: &[usize], byte: usize) -> tree_sitter::Point {
    let row = starts.partition_point(|&s| s <= byte).saturating_sub(1);
    tree_sitter::Point {
        row,
        column: byte - starts.get(row).copied().unwrap_or(0),
    }
}

/// Convert byte [`Span`]s into the `tree_sitter::Range`s `set_included_ranges` wants.
/// The line index is built once, so this is linear in `text` rather than in
/// `text × ranges`.
fn to_ts_ranges(text: &str, ranges: &[Span]) -> Vec<tree_sitter::Range> {
    let starts = line_starts(text);
    ranges
        .iter()
        .map(|s| tree_sitter::Range {
            start_byte: s.start.0,
            end_byte: s.end.0,
            start_point: point_at(&starts, s.start.0),
            end_point: point_at(&starts, s.end.0),
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
        for edit in edits {
            tree.root.edit(edit);
        }
        tree.root.reparse(&mut self.pool, text)?;
        tree.children = self.build_layers(&tree.root, text);
        Ok(())
    }

    /// Expand every injected layer beneath `root`, breadth-first.
    ///
    /// Breadth-first rather than depth-first so the result is ordered by depth. A
    /// consumer merging captures across layers can then let the later (deeper) layer
    /// win a tie against the shallower layer that injected it.
    fn build_layers(&mut self, root: &SyntaxTree, text: &str) -> Vec<SyntaxTree> {
        let mut out: Vec<SyntaxTree> = Vec::new();
        let mut frontier = self.expand(root, text);

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
                next.extend(self.expand(layer, text));
            }
            out.append(&mut frontier);
            frontier = next;
        }
        out
    }

    /// Parse the regions `parent` directly injects — one layer per region.
    fn expand(&mut self, parent: &SyntaxTree, text: &str) -> Vec<SyntaxTree> {
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
            if let Ok(child) = SyntaxTree::parse_ranges(&mut self.pool, lang, text, &ranges) {
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
    /// Injected layers, innermost first within each branch of the descent.
    children: Vec<SyntaxTree>,
}

impl LayeredTree {
    /// The root tree — the document's own language.
    #[must_use]
    pub fn root(&self) -> &SyntaxTree {
        &self.root
    }

    /// The injected layers, excluding the root.
    #[must_use]
    pub fn children(&self) -> &[SyntaxTree] {
        &self.children
    }

    /// Every layer, root first, then each injected layer.
    pub fn layers(&self) -> impl Iterator<Item = &SyntaxTree> {
        std::iter::once(&self.root).chain(&self.children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_ids_compare() {
        assert_eq!(LanguageId(1), LanguageId(1));
        assert_ne!(LanguageId(1), LanguageId(2));
    }

    #[test]
    fn error_displays() {
        assert_eq!(TsError::UnknownLanguage.to_string(), "unknown language");
    }

    #[test]
    fn unknown_language_has_no_highlights() {
        assert!(highlights_query(LanguageId(60000)).is_none());
        assert!(injections_query(LanguageId(60000)).is_none());
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn injections_query_compiles_for_grammars_that_ship_one() -> Result<(), TsError> {
        let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let src = injections_query(lang).ok_or(TsError::UnknownLanguage)?;
        let query = Query::compile(lang, &src)?;
        assert!(query.capture_names().contains(&"injection.content"));
        Ok(())
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn grammar_without_injections_reports_none() -> Result<(), TsError> {
        // Python's grammar ships no injections query; that is not an error.
        let lang = language_id_from_injection_name("python").ok_or(TsError::UnknownLanguage)?;
        assert!(highlights_query(lang).is_some());
        assert!(injections_query(lang).is_none());
        Ok(())
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn parses_rust_and_runs_highlights() -> Result<(), TsError> {
        let lang = language_id_from_path(std::path::Path::new("main.rs"))
            .ok_or(TsError::UnknownLanguage)?;
        let src = "fn main() { let x = 1; }";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src)?;
        let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
        let query = Query::compile(lang, query_src)?;
        let caps = tree.captures(&query, src);
        assert!(!caps.is_empty(), "rust highlights should match something");
        assert!(query.capture_names().contains(&"keyword"));
        // Every capture index is within range.
        assert!(
            caps.iter()
                .all(|c| (c.capture as usize) < query.capture_names().len())
        );
        Ok(())
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn incremental_reparse_matches_full() -> Result<(), TsError> {
        // Insert "let z=1;" before the closing brace and reparse incrementally;
        // the captures must be identical to a fresh full parse of the new text.
        let Some(lang) = language_id_from_path(std::path::Path::new("x.rs")) else {
            return Ok(());
        };
        let old = "fn main() {}";
        let new = "fn main() {let z=1;}";
        let mut pool = ParserPool::new();
        let mut tree = SyntaxTree::parse(&mut pool, lang, old)?;
        // The insertion happens at byte 11 (before '}'), 8 bytes long, same line.
        tree.edit(&Edit {
            start_byte: 11,
            old_end_byte: 11,
            new_end_byte: 19,
            start_point: (0, 11),
            old_end_point: (0, 11),
            new_end_point: (0, 19),
        });
        tree.reparse_with(&mut pool, |byte| new.as_bytes().get(byte..).unwrap_or(&[]))?;

        let full = SyntaxTree::parse(&mut pool, lang, new)?;
        let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
        let query = Query::compile(lang, query_src)?;
        assert_eq!(
            tree.captures(&query, new),
            full.captures(&query, new),
            "incremental reparse must match a full parse"
        );
        Ok(())
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn parses_markdown_and_compiles_block_query() -> Result<(), TsError> {
        let lang = language_id_from_path(std::path::Path::new("README.md"))
            .ok_or(TsError::UnknownLanguage)?;
        let src = "# Title\n\nSome `code` and a [link](http://x).\n";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src)?;
        let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
        let query = Query::compile(lang, query_src)?;
        // The block grammar should at least capture the heading text.
        let caps = tree.captures(&query, src);
        assert!(query.capture_names().contains(&"text.title"));
        assert!(
            caps.iter()
                .all(|c| (c.capture as usize) < query.capture_names().len())
        );
        Ok(())
    }

    /// The `(row, byte-column)` of `byte` in `text` — for building test edits.
    #[cfg(feature = "lang-markdown")]
    fn point_of(text: &str, byte: usize) -> (usize, usize) {
        let before = text.get(..byte).unwrap_or("");
        let row = before.matches('\n').count();
        let col = before.rfind('\n').map_or(byte, |i| byte - i - 1);
        (row, col)
    }

    #[cfg(feature = "lang-markdown")]
    fn layer_langs(tree: &LayeredTree) -> Vec<LanguageId> {
        tree.children().iter().map(SyntaxTree::language).collect()
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn markdown_injects_fenced_code_into_its_language() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let src = "# Title\n\n```rust\nfn main() {}\n```\n";

        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, src)?;

        assert_eq!(tree.root().language(), md);
        // The fence's info string names rust, so its content becomes a rust layer.
        assert!(
            layer_langs(&tree).contains(&rust),
            "expected an embedded rust layer, got {:?}",
            layer_langs(&tree)
        );
        // Root is always the first layer.
        assert_eq!(tree.layers().count(), tree.children().len() + 1);
        Ok(())
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn markdown_injects_its_own_inline_grammar() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let inline =
            language_id_from_injection_name("markdown_inline").ok_or(TsError::UnknownLanguage)?;
        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, "Some *emphasis* and a [link](http://x).\n")?;
        assert!(layer_langs(&tree).contains(&inline));
        Ok(())
    }

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn unknown_fence_language_injects_nothing() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let mut parser = LayeredParser::new();
        // No grammar for `brainfuck`; the fence stays plain text rather than erroring.
        let tree = parser.parse(md, "```brainfuck\n+++.\n```\n")?;
        assert!(
            tree.children().is_empty(),
            "an unresolvable fence language must yield no layer"
        );
        // A resolvable fence over the same shape does produce one.
        let rust = parser.parse(md, "```rust\nfn f() {}\n```\n")?;
        assert!(!rust.children().is_empty());
        Ok(())
    }

    #[cfg(all(
        feature = "lang-html",
        feature = "lang-javascript",
        feature = "lang-css"
    ))]
    #[test]
    fn html_injects_script_and_style() -> Result<(), TsError> {
        let html = language_id_from_injection_name("html").ok_or(TsError::UnknownLanguage)?;
        let js = language_id_from_injection_name("javascript").ok_or(TsError::UnknownLanguage)?;
        let css = language_id_from_injection_name("css").ok_or(TsError::UnknownLanguage)?;

        let mut parser = LayeredParser::new();
        let tree = parser.parse(
            html,
            "<script>let x = 1;</script><style>a { color: red }</style>",
        )?;
        let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
        assert!(
            langs.contains(&js),
            "expected a javascript layer: {langs:?}"
        );
        assert!(langs.contains(&css), "expected a css layer: {langs:?}");
        Ok(())
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn reparse_discovers_a_newly_typed_code_fence() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;

        let old = "text\n";
        let new = "text\n\n```rust\nfn f() {}\n```\n";
        let mut parser = LayeredParser::new();
        let mut tree = parser.parse(md, old)?;
        assert!(!layer_langs(&tree).contains(&rust), "no fence yet");

        // Append the fence — the edit turns a paragraph into an injected rust region.
        let edit = Edit {
            start_byte: old.len(),
            old_end_byte: old.len(),
            new_end_byte: new.len(),
            start_point: point_of(old, old.len()),
            old_end_point: point_of(old, old.len()),
            new_end_point: point_of(new, new.len()),
        };
        parser.reparse(&mut tree, &[edit], new)?;

        assert!(
            layer_langs(&tree).contains(&rust),
            "reparse must discover the new fence, got {:?}",
            layer_langs(&tree)
        );
        // And it agrees with a cold parse of the same text.
        let fresh = parser.parse(md, new)?;
        let (mut a, mut b) = (layer_langs(&tree), layer_langs(&fresh));
        a.sort_unstable_by_key(|l| l.0);
        b.sort_unstable_by_key(|l| l.0);
        assert_eq!(a, b, "incremental layers must match a full layered parse");
        Ok(())
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn deleting_a_fence_drops_its_layer() -> Result<(), TsError> {
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let old = "```rust\nfn f() {}\n```\n";
        let new = "";

        let mut parser = LayeredParser::new();
        let mut tree = parser.parse(md, old)?;
        assert!(layer_langs(&tree).contains(&rust));

        parser.reparse(
            &mut tree,
            &[Edit {
                start_byte: 0,
                old_end_byte: old.len(),
                new_end_byte: 0,
                start_point: (0, 0),
                old_end_point: point_of(old, old.len()),
                new_end_point: (0, 0),
            }],
            new,
        )?;
        assert!(
            !layer_langs(&tree).contains(&rust),
            "the rust layer must vanish with its fence"
        );
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rust_doc_comment_is_markdown() -> Result<(), TsError> {
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, "/// Adds *one*.\npub fn f() {}\n")?;
        let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
        assert!(
            langs.contains(&md),
            "doc comment must inject markdown: {langs:?}"
        );
        // A plain `//` comment is not markdown.
        let plain = parser.parse(rust, "// not *markdown*\npub fn f() {}\n")?;
        assert!(!plain.children().iter().any(|c| c.language() == md));
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rust_doctest_fence_in_a_doc_comment_is_rust() -> Result<(), TsError> {
        // The headline case: a doctest fence spans several `///` lines, each its own
        // `line_comment` node. Only a *combined* markdown injection can see the fence,
        // and markdown must then recursively inject rust back into it.
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let src = "\
/// Adds one.
///
/// ```rust
/// let y = 1 + 1;
/// assert_eq!(y, 2);
/// ```
pub fn add_one() {}
";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, src)?;
        let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
        assert!(langs.contains(&md), "expected a markdown layer: {langs:?}");

        // The doctest body must come back as a *nested* rust layer covering the fence
        // body — not merely some rust layer (a macro injection would also be rust).
        let fence_body = src.find("let y").ok_or(TsError::ParseFailed)?;
        let doctest = tree
            .children()
            .iter()
            .find(|c| c.language() == rust && c.span().start.0 >= fence_body);
        let doctest = doctest.ok_or(TsError::ParseFailed)?;
        assert!(
            doctest.span().end.0 <= src.find("pub fn").unwrap_or(src.len()),
            "the doctest layer must stay inside the doc comment"
        );
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn layers_are_ordered_shallowest_first() -> Result<(), TsError> {
        // rust (root) → markdown (doc comment, depth 1) → rust (doctest fence, depth 2).
        // The nested rust layer must come *after* the markdown layer that produced it,
        // so a capture merge can let the deeper layer win an exact-range tie.
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
        let src = "/// ```rust\n/// let y = 1;\n/// ```\npub fn f() {}\n";

        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, src)?;
        let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
        let md_at = langs.iter().position(|l| *l == md);
        let nested_rust_at = tree
            .children()
            .iter()
            .position(|c| c.language() == rust && c.span().start.0 < src.len());
        let (Some(md_at), Some(rust_at)) = (md_at, nested_rust_at) else {
            return Err(TsError::ParseFailed);
        };
        assert!(
            md_at < rust_at,
            "markdown (depth 1) must precede the doctest rust layer (depth 2): {langs:?}"
        );
        Ok(())
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn self_injecting_grammar_terminates() -> Result<(), TsError> {
        // Rust's own injections query re-parses macro token trees as rust. Without the
        // depth cap and identity guard this descends forever.
        let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, "macro_rules! m { () => { m!(); } }\nfn f() { m!(); }")?;
        assert!(tree.children().len() < MAX_INJECTION_LAYERS);
        Ok(())
    }

    #[test]
    fn parse_ranges_rejects_an_empty_range_list() {
        let mut pool = ParserPool::new();
        let err = SyntaxTree::parse_ranges(&mut pool, LanguageId(60000), "x", &[]);
        assert!(matches!(err, Err(TsError::ParseFailed)));
    }

    #[test]
    fn point_at_resolves_rows_and_columns() {
        let text = "ab\ncd\n";
        let starts = line_starts(text);
        assert_eq!(starts, vec![0, 3, 6]);
        assert_eq!(
            point_at(&starts, 0),
            tree_sitter::Point { row: 0, column: 0 }
        );
        assert_eq!(
            point_at(&starts, 4),
            tree_sitter::Point { row: 1, column: 1 }
        );
        // End of buffer sits at the start of the (empty) trailing line.
        assert_eq!(
            point_at(&starts, 6),
            tree_sitter::Point { row: 2, column: 0 }
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn detects_rust_by_extension_and_name() {
        let p = std::path::Path::new("src/lib.rs");
        assert!(language_id_from_path(p).is_some());
        assert_eq!(language_name_from_path(p), Some("Rust"));
    }
}

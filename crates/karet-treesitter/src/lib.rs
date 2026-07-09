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
mod registry;

pub use detect::language_id_from_injection_name;
pub use detect::language_id_from_path;
pub use detect::language_name_from_path;

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
    inner: tree_sitter::Query,
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn detects_rust_by_extension_and_name() {
        let p = std::path::Path::new("src/lib.rs");
        assert!(language_id_from_path(p).is_some());
        assert_eq!(language_name_from_path(p), Some("Rust"));
    }
}

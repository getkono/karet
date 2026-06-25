//! `karet-treesitter` — the shared tree-sitter parse host for the karet toolkit.
//!
//! Owns parser pooling, incremental edit application, tree caching and query
//! execution so that `karet-syntax`, `karet-diff`, and (via syntax) the editor all
//! reuse a single parse of each buffer. Tree-sitter is karet's *sole* syntax
//! backend — there is deliberately no second backend to abstract over.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! incremental-parsing and query logic is filled in separately.

use karet_core::Span;

/// Errors produced by the parse host.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TsError {
    /// No grammar is registered for the requested language.
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

/// A pool of reusable tree-sitter parsers, keyed by [`LanguageId`].
#[derive(Default)]
pub struct ParserPool {}

impl ParserPool {
    /// Create an empty parser pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// A parsed, incrementally-maintainable syntax tree for one buffer.
pub struct SyntaxTree {}

impl SyntaxTree {
    /// Parse `text` as `lang`, drawing a parser from `pool`.
    ///
    /// # Errors
    /// Returns [`TsError::UnknownLanguage`] or [`TsError::ParseFailed`].
    pub fn parse(pool: &mut ParserPool, lang: LanguageId, text: &str) -> Result<Self, TsError> {
        let _ = (pool, lang, text);
        todo!()
    }

    /// Re-parse incrementally after the buffer changed to `text`.
    ///
    /// # Errors
    /// Returns [`TsError::ParseFailed`] if re-parsing fails.
    pub fn reparse(&mut self, pool: &mut ParserPool, text: &str) -> Result<(), TsError> {
        let _ = (pool, text);
        todo!()
    }

    /// The byte ranges that differ between `old` and this tree.
    #[must_use]
    pub fn changed_ranges(&self, old: &SyntaxTree) -> Vec<Span> {
        let _ = old;
        todo!()
    }
}

/// A compiled tree-sitter query (highlights, folds, locals, …).
pub struct Query {}

impl Query {
    /// Compile `source` against the grammar for `lang`.
    ///
    /// # Errors
    /// Returns [`TsError::InvalidQuery`] if the query text is malformed.
    pub fn compile(lang: LanguageId, source: &str) -> Result<Self, TsError> {
        let _ = (lang, source);
        todo!()
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
}

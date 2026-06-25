//! `karet-fuzzy` — fuzzy matching and ranking for the karet toolkit.
//!
//! Standalone (depends on no other karet crate). Wraps `nucleo` with frecency
//! scoring and quick-open query parsing, shared by the widgets toolkit and
//! completion ranking so neither has to depend on the other.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! matching logic is filled in separately.

/// Errors produced by the matcher.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FuzzyError {
    /// The pattern could not be parsed.
    #[error("invalid pattern")]
    InvalidPattern,
}

/// A ranked item: a reference to the source item, its score, and the indices of
/// the matched characters (for highlighting).
#[derive(Clone, Debug)]
pub struct Scored<'a, T> {
    /// The matched item.
    pub item: &'a T,
    /// Higher is a better match.
    pub score: u32,
    /// Character indices within the item's string that matched.
    pub matched: Vec<u32>,
}

/// A fuzzy matcher over arbitrary string-like items.
#[derive(Default)]
pub struct Matcher {}

impl Matcher {
    /// Create a matcher.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rank `items` against `pattern`, best match first.
    pub fn rank<'a, T: AsRef<str>>(&mut self, pattern: &str, items: &'a [T]) -> Vec<Scored<'a, T>> {
        let _ = (pattern, items);
        todo!()
    }
}

/// A frequency + recency ("frecency") ranking store keyed by string.
#[derive(Default)]
pub struct FrecencyStore {}

impl FrecencyStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a use of `key`, increasing its future boost.
    pub fn record(&mut self, key: &str) {
        let _ = key;
        todo!()
    }

    /// The current frecency boost for `key` (0 if never seen).
    #[must_use]
    pub fn boost(&self, key: &str) -> u32 {
        let _ = key;
        todo!()
    }
}

/// The kind of a parsed quick-open query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickOpenKind {
    /// A file-path query (the default).
    Path,
    /// An `@`-prefixed symbol query.
    Symbol,
    /// A `:`-prefixed line-number query.
    Line,
    /// A `>`-prefixed command query.
    Command,
}

/// Parse a quick-open query into its [`QuickOpenKind`] and the remaining term.
///
/// `@` selects symbols, `:` a line, `>` a command; anything else is a path.
#[must_use]
pub fn parse_query(input: &str) -> (QuickOpenKind, &str) {
    match input.as_bytes().first() {
        Some(b'@') => (QuickOpenKind::Symbol, &input[1..]),
        Some(b':') => (QuickOpenKind::Line, &input[1..]),
        Some(b'>') => (QuickOpenKind::Command, &input[1..]),
        _ => (QuickOpenKind::Path, input),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quick_open_prefixes() {
        assert_eq!(parse_query("@sym"), (QuickOpenKind::Symbol, "sym"));
        assert_eq!(parse_query(":42"), (QuickOpenKind::Line, "42"));
        assert_eq!(parse_query(">cmd"), (QuickOpenKind::Command, "cmd"));
        assert_eq!(
            parse_query("src/main.rs"),
            (QuickOpenKind::Path, "src/main.rs")
        );
    }

    #[test]
    fn error_displays() {
        assert_eq!(FuzzyError::InvalidPattern.to_string(), "invalid pattern");
    }
}

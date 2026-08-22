//! Seam identity: the semantic path that names a node, and the interned handle the
//! index uses in its place.
//!
//! Identity is derived from *where a node sits in the containment hierarchy*, never
//! from where it sits in a file. That is what lets view state survive editing: inserting
//! a hundred lines above a function moves every byte offset in it and changes nothing
//! about `karet-core::provider::SymbolProvider`. Renaming or reparenting the node is
//! precisely the case where the old identity *should* stop resolving.
//!
//! Two conventions make the path total. Anonymous constructs — a Rust `impl` block has no
//! name — are written as a braced segment describing them, `{impl SymbolProvider for
//! Vec<Symbol>}`. Siblings that still collide after that get a `#n` ordinal, assigned by
//! source order within a file, files sorted by resolved module path, so the same tree
//! always yields the same ordinals across sessions and machines.
//!
//! Generic parameters and `where` clauses are deliberately *excluded*. Adding a bound to
//! a function is not a rename, and it must not invalidate the user's place in the view.
//!
//! ```
//! # use karet_seam::SeamPath;
//! let path: SeamPath = "karet-core::provider::{impl SymbolProvider for Vec<Symbol>}::symbols"
//!     .parse()
//!     .expect("well-formed path");
//! assert_eq!(path.package(), Some("karet-core"));
//! assert_eq!(path.leaf(), Some("symbols"));
//! assert_eq!(path.len(), 4);
//! ```

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

/// The separator between path segments.
const SEPARATOR: &str = "::";

/// Errors parsing a [`SeamPath`] from its textual form.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SeamPathError {
    /// The whole path was empty or blank.
    #[error("a seam path needs at least one segment")]
    Empty,
    /// A segment between two separators was empty, as in `a::::b`.
    #[error("empty path segment at position {0}")]
    EmptySegment(usize),
    /// A braced segment was opened but never closed.
    #[error("unbalanced `{{` in segment at position {0}")]
    UnbalancedBrace(usize),
    /// The text after `#` was not a positive integer.
    #[error("invalid disambiguating ordinal `{ordinal}` in segment at position {position}")]
    InvalidOrdinal {
        /// The offending text.
        ordinal: String,
        /// Which segment carried it.
        position: usize,
    },
}

/// One segment of a [`SeamPath`]: a name plus its disambiguating ordinal.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SeamSegment {
    /// The segment text, without any `#n` suffix. Braced for anonymous constructs.
    pub name: String,
    /// `0` when the segment is unique among its siblings; otherwise the 1-based
    /// occurrence, rendered as a `#n` suffix.
    pub ordinal: u32,
}

impl SeamSegment {
    /// A segment that needs no disambiguation.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ordinal: 0,
        }
    }

    /// A segment carrying the given 1-based occurrence ordinal.
    #[must_use]
    pub fn numbered(name: impl Into<String>, ordinal: u32) -> Self {
        Self {
            name: name.into(),
            ordinal,
        }
    }

    /// Whether this segment names an anonymous construct, written `{…}`.
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        self.name.starts_with('{') && self.name.ends_with('}')
    }
}

impl fmt::Display for SeamSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)?;
        if self.ordinal > 0 {
            write!(f, "#{}", self.ordinal)?;
        }
        Ok(())
    }
}

/// The semantic path naming one node, and the citation unit an agent hands back to a
/// human.
///
/// See the [module docs](self) for the identity rules this encodes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SeamPath {
    segments: Vec<SeamSegment>,
}

impl SeamPath {
    /// Build a path from its segments, outermost first.
    #[must_use]
    pub fn new(segments: Vec<SeamSegment>) -> Self {
        Self { segments }
    }

    /// The path's segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[SeamSegment] {
        &self.segments
    }

    /// How many segments the path has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Whether the path has no segments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The outermost segment's name — the package, by construction.
    #[must_use]
    pub fn package(&self) -> Option<&str> {
        self.segments.first().map(|s| s.name.as_str())
    }

    /// The innermost segment's name: what this path actually identifies.
    #[must_use]
    pub fn leaf(&self) -> Option<&str> {
        self.segments.last().map(|s| s.name.as_str())
    }

    /// The path of this node's parent, or `None` at the root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            return None;
        }
        let mut segments = self.segments.clone();
        segments.pop();
        Some(Self { segments })
    }

    /// This path with `segment` appended.
    #[must_use]
    pub fn child(&self, segment: SeamSegment) -> Self {
        let mut segments = self.segments.clone();
        segments.push(segment);
        Self { segments }
    }

    /// Whether `self` is `other` or lies underneath it.
    ///
    /// This is the containment test behind the query language's `in:` term.
    #[must_use]
    pub fn is_under(&self, other: &Self) -> bool {
        self.segments.len() >= other.segments.len()
            && self.segments[..other.segments.len()] == other.segments[..]
    }
}

impl fmt::Display for SeamPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                f.write_str(SEPARATOR)?;
            }
            write!(f, "{segment}")?;
        }
        Ok(())
    }
}

impl FromStr for SeamPath {
    type Err = SeamPathError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.trim().is_empty() {
            return Err(SeamPathError::Empty);
        }
        let mut segments = Vec::new();
        for (position, raw) in split_segments(text)?.into_iter().enumerate() {
            if raw.is_empty() {
                return Err(SeamPathError::EmptySegment(position));
            }
            segments.push(parse_segment(raw, position)?);
        }
        if segments.is_empty() {
            return Err(SeamPathError::Empty);
        }
        Ok(Self { segments })
    }
}

/// Split on `::`, but only at brace depth zero — a braced segment such as
/// `{impl core::fmt::Display for Widget}` carries separators of its own.
fn split_segments(text: &str) -> Result<Vec<&str>, SeamPathError> {
    let bytes = text.as_bytes();
    let mut segments = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && bytes.get(i + 1) == Some(&b':') => {
                segments.push(&text[start..i]);
                i += SEPARATOR.len();
                start = i;
                continue;
            },
            _ => {},
        }
        i += 1;
    }
    if depth != 0 {
        return Err(SeamPathError::UnbalancedBrace(segments.len()));
    }
    segments.push(&text[start..]);
    Ok(segments)
}

/// Peel an optional `#n` ordinal off a segment. The `#` must sit outside any braces so
/// a type named in an impl segment cannot be mistaken for an ordinal marker.
fn parse_segment(raw: &str, position: usize) -> Result<SeamSegment, SeamPathError> {
    let closing = raw.rfind('}').map_or(0, |i| i + 1);
    let Some(hash) = raw[closing..].rfind('#').map(|i| i + closing) else {
        return Ok(SeamSegment::new(raw));
    };
    let ordinal = &raw[hash + 1..];
    let parsed = ordinal
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| SeamPathError::InvalidOrdinal {
            ordinal: ordinal.to_owned(),
            position,
        })?;
    let name = &raw[..hash];
    if name.is_empty() {
        return Err(SeamPathError::EmptySegment(position));
    }
    Ok(SeamSegment::numbered(name, parsed))
}

/// An interned handle for a [`SeamPath`].
///
/// The index stores and compares these instead of paths: an id is `Copy`, hashes in
/// constant time, and keeps the node arena dense. Resolve one back to its path through
/// [`SeamInterner::path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SeamId(pub u32);

/// The two-way map between [`SeamPath`]s and [`SeamId`]s.
///
/// Ids are handed out in first-seen order and are never reused, so an id stays valid for
/// the interner's lifetime even as the tree around it is re-indexed.
#[derive(Debug, Default, Clone)]
pub struct SeamInterner {
    paths: Vec<SeamPath>,
    ids: HashMap<SeamPath, SeamId>,
}

impl SeamInterner {
    /// An interner holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The id for `path`, assigning a fresh one the first time it is seen.
    pub fn intern(&mut self, path: SeamPath) -> SeamId {
        if let Some(id) = self.ids.get(&path) {
            return *id;
        }
        // `u32::MAX` ids is far beyond any real package; saturating keeps this
        // total rather than panicking on an impossible input.
        let id = SeamId(u32::try_from(self.paths.len()).unwrap_or(u32::MAX));
        self.paths.push(path.clone());
        self.ids.insert(path, id);
        id
    }

    /// The id already assigned to `path`, without interning a new one.
    #[must_use]
    pub fn lookup(&self, path: &SeamPath) -> Option<SeamId> {
        self.ids.get(path).copied()
    }

    /// The path behind `id`, or `None` if it came from a different interner.
    #[must_use]
    pub fn path(&self, id: SeamId) -> Option<&SeamPath> {
        self.paths.get(id.0 as usize)
    }

    /// How many distinct paths have been interned.
    #[must_use]
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// Whether nothing has been interned yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> SeamPath {
        text.parse().unwrap_or_default()
    }

    #[test]
    fn round_trips_a_plain_path() {
        let text = "karet-core::provider::SymbolProvider";
        assert_eq!(path(text).to_string(), text);
        assert_eq!(path(text).len(), 3);
    }

    #[test]
    fn separators_inside_a_braced_segment_do_not_split_it() {
        let text = "karet-core::model::{impl core::fmt::Display for Symbol}";
        let parsed = path(text);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed.leaf(), Some("{impl core::fmt::Display for Symbol}"));
        assert_eq!(parsed.to_string(), text);
    }

    #[test]
    fn round_trips_an_ordinal_suffix() {
        let text = "karet-core::model::{impl Widget}#2::render";
        let parsed = path(text);
        assert_eq!(parsed.segments()[2].ordinal, 2);
        assert_eq!(parsed.segments()[2].name, "{impl Widget}");
        assert_eq!(parsed.to_string(), text);
    }

    #[test]
    fn a_hash_inside_braces_is_not_an_ordinal() {
        let parsed = path("pkg::{impl Trait for Raw#Type}");
        assert_eq!(parsed.segments()[1].ordinal, 0);
        assert_eq!(parsed.segments()[1].name, "{impl Trait for Raw#Type}");
    }

    #[test]
    fn package_and_leaf_read_from_the_ends() {
        let parsed = path("pkg::a::b::c");
        assert_eq!(parsed.package(), Some("pkg"));
        assert_eq!(parsed.leaf(), Some("c"));
    }

    #[test]
    fn parent_climbs_one_level_and_stops_at_the_root() {
        let parsed = path("pkg::a::b");
        let up = parsed.parent().unwrap_or_default();
        assert_eq!(up.to_string(), "pkg::a");
        let root = path("pkg");
        assert_eq!(root.parent(), None);
    }

    #[test]
    fn child_appends_a_segment() {
        let parsed = path("pkg::a").child(SeamSegment::new("b"));
        assert_eq!(parsed.to_string(), "pkg::a::b");
    }

    #[test]
    fn containment_is_prefix_matching_on_whole_segments() {
        let root = path("pkg::model");
        assert!(path("pkg::model").is_under(&root));
        assert!(path("pkg::model::Symbol").is_under(&root));
        assert!(
            !path("pkg::modeling").is_under(&root),
            "a prefix of a name is not containment"
        );
        assert!(!path("pkg").is_under(&root));
    }

    #[test]
    fn anonymous_segments_are_recognized() {
        assert!(SeamSegment::new("{impl A for B}").is_anonymous());
        assert!(!SeamSegment::new("Symbol").is_anonymous());
    }

    #[test]
    fn rejects_malformed_paths() {
        assert_eq!("".parse::<SeamPath>(), Err(SeamPathError::Empty));
        assert_eq!("   ".parse::<SeamPath>(), Err(SeamPathError::Empty));
        assert_eq!(
            "pkg::::a".parse::<SeamPath>(),
            Err(SeamPathError::EmptySegment(1))
        );
        assert_eq!(
            "pkg::{impl A".parse::<SeamPath>(),
            Err(SeamPathError::UnbalancedBrace(1))
        );
        assert!(matches!(
            "pkg::a#0".parse::<SeamPath>(),
            Err(SeamPathError::InvalidOrdinal { .. })
        ));
        assert!(matches!(
            "pkg::a#x".parse::<SeamPath>(),
            Err(SeamPathError::InvalidOrdinal { .. })
        ));
    }

    #[test]
    fn interning_is_stable_and_reversible() {
        let mut interner = SeamInterner::new();
        assert!(interner.is_empty());
        let a = interner.intern(path("pkg::a"));
        let b = interner.intern(path("pkg::b"));
        // Re-interning the same path yields the same id.
        assert_eq!(interner.intern(path("pkg::a")), a);
        assert_ne!(a, b);
        assert_eq!(interner.len(), 2);
        assert_eq!(
            interner.path(a).map(ToString::to_string).as_deref(),
            Some("pkg::a")
        );
        assert_eq!(interner.lookup(&path("pkg::b")), Some(b));
        assert_eq!(interner.lookup(&path("pkg::missing")), None);
        assert_eq!(interner.path(SeamId(99)), None);
    }
}

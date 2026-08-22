//! A predicate language over the index, shared verbatim by the filter box and the
//! programmatic surface.
//!
//! One language, not two. Whatever an agent can ask for, a person can type — and whatever
//! a person narrows to by pressing keys, an agent can be handed as a string and reproduce
//! exactly. That is what makes an agent's narrowing inspectable: it serializes to the same
//! breadcrumb state the user could have reached themselves, so they can adopt it or
//! discard it rather than take it on trust.
//!
//! # Shape
//!
//! Whitespace-separated terms, implicitly conjoined. `!` negates a term. A bare word
//! fuzzy-matches the node name; a `"quoted phrase"` matches a literal substring, which a
//! fuzzy matcher cannot express. There is deliberately no `or` and no grouping: the cost
//! of a query language nobody can read exceeds the value of the queries it would allow.
//!
//! | Form | Matches |
//! |---|---|
//! | `lens:<name>` | nodes carrying any facet of that lens |
//! | `<lens>:<subtype>` | nodes carrying that specific facet |
//! | `vis:<level>` | effective visibility at least that reachable |
//! | `kind:<kind>` | a universal node kind |
//! | `in:<path>` | subtree containment |
//! | `cfg:<text>` | gated by a matching variation predicate |
//! | `config:<name>` | evaluate under a named configuration |
//! | `pivot:<edge>:<node>` | the result set of following an edge |
//!
//! # Errors are positioned, never silent
//!
//! An unknown term is a parse error carrying the byte range that produced it and the
//! closest valid names. Silently ignoring it would be worse than useless: the reader would
//! get a *different* result set than they asked for and no indication that anything was
//! wrong, which is precisely how a filter becomes untrustworthy.

use std::fmt;
use std::ops::Range;

use crate::edge::EdgeKind;
use crate::id::SeamPath;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

mod eval;
mod parse;

#[cfg(test)]
mod tests;

pub use eval::QueryResult;
pub use eval::evaluate;
pub use parse::parse;

/// What a single term matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermKind {
    /// Any facet of a lens.
    Lens(Lens),
    /// One specific facet subtype within a lens.
    Facet {
        /// The lens it belongs to.
        lens: Lens,
        /// The subtype name.
        subtype: String,
    },
    /// Effective visibility at least this reachable.
    Visibility(Visibility),
    /// A universal node kind.
    Kind(NodeKind),
    /// Containment beneath a path.
    In(SeamPath),
    /// A variation predicate whose text contains this.
    Cfg(String),
    /// The result set of following an edge from a node.
    Pivot {
        /// Which relation to follow.
        edge: EdgeKind,
        /// The node to follow it from, as a path.
        target: SeamPath,
    },
    /// A fuzzy match on the node name.
    Name(String),
    /// A literal substring of the node name.
    Phrase(String),
}

impl fmt::Display for TermKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lens(lens) => write!(f, "lens:{}", lens.name()),
            Self::Facet { lens, subtype } => write!(f, "{}:{subtype}", lens.name()),
            Self::Visibility(vis) => write!(f, "vis:{}", vis.name()),
            Self::Kind(kind) => write!(f, "kind:{}", kind.name()),
            Self::In(path) => write!(f, "in:{path}"),
            Self::Cfg(text) => write!(f, "cfg:{text}"),
            Self::Pivot { edge, target } => write!(f, "pivot:{}:{target}", edge.name()),
            Self::Name(name) => f.write_str(name),
            Self::Phrase(phrase) => write!(f, "\"{phrase}\""),
        }
    }
}

/// One term of a query, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Whether the term is negated.
    pub negated: bool,
    /// What it matches.
    pub kind: TermKind,
    /// The byte range in the source text that produced it.
    pub span: Range<usize>,
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negated {
            f.write_str("!")?;
        }
        write!(f, "{}", self.kind)
    }
}

/// A parsed query: conjoined terms plus the configuration to evaluate them under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// The terms, in the order written.
    pub terms: Vec<Term>,
    /// The configuration named by a `config:` directive, when one was given.
    pub configuration: Option<String>,
}

impl Query {
    /// Whether the query constrains anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.configuration.is_none()
    }

    /// The subtree this query is rooted at, if it names exactly one.
    ///
    /// The UI's "reroot at selection" is an `in:` term, so reading it back out is how a
    /// query string restores a breadcrumb.
    #[must_use]
    pub fn root(&self) -> Option<&SeamPath> {
        self.terms.iter().find_map(|term| match &term.kind {
            TermKind::In(path) if !term.negated => Some(path),
            _ => None,
        })
    }

    /// The lenses this query filters on.
    #[must_use]
    pub fn lenses(&self) -> Vec<Lens> {
        self.terms
            .iter()
            .filter(|term| !term.negated)
            .filter_map(|term| match &term.kind {
                TermKind::Lens(lens) => Some(*lens),
                TermKind::Facet { lens, .. } => Some(*lens),
                _ => None,
            })
            .collect()
    }
}

impl fmt::Display for Query {
    /// Render back to the text form, so UI state round-trips through a string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        if let Some(configuration) = &self.configuration {
            write!(f, "config:{configuration}")?;
            first = false;
        }
        for term in &self.terms {
            if !first {
                f.write_str(" ")?;
            }
            write!(f, "{term}")?;
            first = false;
        }
        Ok(())
    }
}

/// A positioned parse failure, with the closest valid alternatives.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct QueryError {
    /// What went wrong, phrased for the reader.
    pub message: String,
    /// The byte range of the offending text, for a caret under the filter box.
    pub span: Range<usize>,
    /// The nearest valid names, when the failure was an unrecognized one.
    pub suggestions: Vec<String>,
}

impl QueryError {
    /// A failure at `span` with no suggestions.
    #[must_use]
    pub fn new(message: impl Into<String>, span: Range<usize>) -> Self {
        Self {
            message: message.into(),
            span,
            suggestions: Vec::new(),
        }
    }

    /// This failure with alternatives attached.
    #[must_use]
    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }

    /// The message plus its suggestions, as one line for a terminal.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.suggestions.is_empty() {
            return self.message.clone();
        }
        format!(
            "{} (did you mean {}?)",
            self.message,
            self.suggestions.join(", ")
        )
    }
}

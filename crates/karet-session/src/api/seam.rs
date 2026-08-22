//! The wire model for the Seam view: what crosses the backend seam, and why in this shape.
//!
//! The index itself never crosses. It owns a parser pool, an edge store, and a fuzzy
//! matcher, none of which a presentation layer should hold — and it is the kind of
//! structure a future client-server split could not send at all. What crosses is a
//! flattened, serde-ready projection of it.
//!
//! The whole node list is sent at once rather than fetched level by level. That looks
//! profligate until you consider what the alternative costs: a round trip per column
//! keystroke, which is exactly the latency a navigator like this cannot afford. A large
//! crate flattens to a few hundred kilobytes, and in exchange every navigation, lens
//! toggle, and reroot is answered locally and instantly.
//!
//! Node identity travels as its path string. It is the citation unit an agent hands back
//! to a human, the key view state is restored against, and stable across every edit that
//! does not rename or reparent — so it is the right thing to key on across a boundary
//! that a numeric handle could not survive.

use std::path::PathBuf;

use karet_core::Range;

/// One node, flattened for the presentation layer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamNodeView {
    /// The node's identity, as its semantic path.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The universal kind, by name.
    pub kind: String,
    /// A short signature or descriptor.
    pub detail: Option<String>,
    /// The file the node lives in.
    pub file: PathBuf,
    /// The node's full extent.
    pub range: Range,
    /// The range to reveal when navigating here.
    pub selection: Range,
    /// The parent's identity, absent at a root.
    pub parent: Option<String>,
    /// The children's identities, in source order.
    pub children: Vec<String>,
    /// The seam properties this node carries.
    pub facets: Vec<SeamFacetView>,
    /// Per-lens subtree counts, in lens order.
    pub rollups: [u32; 5],
    /// Effective visibility, by name.
    pub visibility: Option<String>,
    /// Whether the active configuration includes it: active, inactive, or indeterminate.
    pub membership: String,
    /// Whether the node came from a subtree that failed to parse cleanly.
    pub provisional: bool,
}

/// One facet, flattened.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamFacetView {
    /// Which lens it belongs to, by name.
    pub lens: String,
    /// The subtype within that lens.
    pub subtype: String,
    /// Supporting text.
    pub detail: Option<String>,
    /// Sub-item occurrences inside the node.
    pub sites: Vec<Range>,
    /// Reach through re-exports, on `api` facets that have it.
    pub effective: Option<String>,
}

/// One edge leaving or arriving at a node.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamEdgeView {
    /// The relation, by name.
    pub kind: String,
    /// Whether the edge points away from the node or at it.
    pub outgoing: bool,
    /// The other end's identity, when it resolves inside the package.
    pub target: Option<String>,
    /// What to show for the other end when it does not.
    pub display: Option<String>,
    /// `resolved`, `external`, or `unresolved`.
    pub state: String,
    /// Whether an unresolved endpoint could still become resolved.
    pub resolvable: bool,
    /// Where the relation is written.
    pub site: Option<Range>,
}

/// What an indexed package amounts to, for the header and the empty states.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamSummary {
    /// The package name, and the tree's root identity.
    pub package: String,
    /// How many nodes the index holds.
    pub nodes: usize,
    /// How many files it spans.
    pub files: usize,
    /// The active configuration's name, always shown in the header.
    pub configuration: String,
    /// Every configuration the package can be read under.
    pub available_configurations: Vec<String>,
    /// Whether the `variation` lens can claim completeness under the active configuration.
    ///
    /// False when no manifest was readable, in which case the feature set is a guess and
    /// the header has to say so rather than implying a complete answer.
    pub variation_complete: bool,
    /// How many files were scanned before indexing was cut short, if it was.
    ///
    /// A truncated index that says nothing reads as a complete one.
    pub truncated_after: Option<usize>,
    /// Modules whose text could not be found, with the paths that were tried.
    pub unresolved_modules: Vec<(String, Vec<PathBuf>)>,
}

impl SeamSummary {
    /// Whether the index is complete: nothing truncated, nothing unresolved.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.truncated_after.is_none() && self.unresolved_modules.is_empty()
    }
}

/// A parse failure in a query, positioned so a caret can sit under it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SeamQueryError {
    /// What went wrong.
    pub message: String,
    /// The byte offset the offending text starts at.
    pub start: usize,
    /// One past its last byte.
    pub end: usize,
    /// The nearest valid names.
    pub suggestions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_reports_completeness_only_when_nothing_was_lost() {
        let complete = SeamSummary {
            package: "demo".to_owned(),
            nodes: 10,
            ..SeamSummary::default()
        };
        assert!(complete.is_complete());

        let truncated = SeamSummary {
            truncated_after: Some(20_000),
            ..complete.clone()
        };
        assert!(!truncated.is_complete());

        let unresolved = SeamSummary {
            unresolved_modules: vec![("demo::absent".to_owned(), Vec::new())],
            ..complete
        };
        assert!(!unresolved.is_complete());
    }
}

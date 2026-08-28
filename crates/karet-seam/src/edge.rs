//! Non-containment relations, stored and queried apart from the tree.
//!
//! Containment is a tree; everything else is an edge. Merging the two is the mistake
//! that turns a navigable hierarchy into a hairball, so edges live here and the view
//! never draws them as a persistent graph — it follows one at a time, on request.
//!
//! An endpoint has three states and all of them are normal. Pointing outside the package
//! is not a failure, and *not yet knowing* is not a failure either: a package indexed
//! with no language server running will have mostly [`Endpoint::Unresolved`] edges and
//! must still be worth reading. That is why unresolved is modelled as a first-class
//! state carrying whatever hint the structural tier could recover, rather than as a
//! missing edge.

use std::collections::HashMap;

use karet_core::Range;

use crate::id::SeamId;

/// The kind of relation an edge expresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
#[non_exhaustive]
pub enum EdgeKind {
    /// An implementation binds a contract to a type.
    Implements,
    /// A member replaces a default supplied by its contract.
    OverridesDefault,
    /// A site where the concrete callee is chosen at run time.
    DynDispatchSite,
    /// A declaration republished under another path.
    ReExports,
    /// A node whose presence depends on a variation predicate.
    GatedBy,
    /// A macro invocation and what it expands into.
    ExpandsTo,
}

impl EdgeKind {
    /// The stable name, as written in a query (`pivot:implements:…`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Implements => "implements",
            Self::OverridesDefault => "overrides-default",
            Self::DynDispatchSite => "dyn-dispatch-site",
            Self::ReExports => "re-exports",
            Self::GatedBy => "gated-by",
            Self::ExpandsTo => "expands-to",
        }
    }

    /// Every kind, for query-term suggestions and the facet pane's grouping.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Implements,
            Self::OverridesDefault,
            Self::DynDispatchSite,
            Self::ReExports,
            Self::GatedBy,
            Self::ExpandsTo,
        ]
    }

    /// Resolve a kind from its query name, case-sensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::all().iter().copied().find(|kind| kind.name() == name)
    }
}

/// What an edge points at.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Endpoint {
    /// A node in this package's tree.
    Resolved(SeamId),
    /// Something outside this package, known only by how it is written.
    External {
        /// The path as written at the use site.
        display: String,
    },
    /// Not resolved — either not yet, or not ever.
    ///
    /// A normal state, not an error. The view distinguishes it from an absent edge.
    Unresolved {
        /// Whatever the structural tier could recover, usually the written name.
        hint: Option<String>,
        /// Whether resolution is still possible with better information.
        ///
        /// `false` means no tier will ever resolve this, so the view can stop showing
        /// it as pending and say so plainly.
        resolvable: bool,
    },
}

impl Endpoint {
    /// The node this points at, when it points inside the package.
    #[must_use]
    pub fn resolved(&self) -> Option<SeamId> {
        match self {
            Self::Resolved(id) => Some(*id),
            _ => None,
        }
    }

    /// The text to show for this endpoint.
    #[must_use]
    pub fn display_hint(&self) -> Option<&str> {
        match self {
            Self::Resolved(_) => None,
            Self::External { display } => Some(display),
            Self::Unresolved { hint, .. } => hint.as_deref(),
        }
    }

    /// The state's stable name, for serialized output and the facet pane.
    #[must_use]
    pub fn state(&self) -> &'static str {
        match self {
            Self::Resolved(_) => "resolved",
            Self::External { .. } => "external",
            Self::Unresolved { .. } => "unresolved",
        }
    }
}

/// One typed relation between a node and something else.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Edge {
    /// Which relation this is.
    pub kind: EdgeKind,
    /// The node the relation starts at. Always resolved — it is in this tree.
    pub from: SeamId,
    /// What it points at.
    pub to: Endpoint,
    /// Where in `from`'s source the relation is written, when it is a specific site.
    pub site: Option<Range>,
}

/// Edges indexed for lookup from either end.
///
/// Kept separate from the node arena so the tree stays a tree, and so an edge whose
/// target is unresolved still has somewhere to live.
#[derive(Debug, Default, Clone)]
pub struct EdgeStore {
    edges: Vec<Edge>,
    by_from: HashMap<SeamId, Vec<usize>>,
    by_to: HashMap<SeamId, Vec<usize>>,
}

impl EdgeStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an edge, indexing it from both ends.
    pub fn insert(&mut self, edge: Edge) {
        let position = self.edges.len();
        self.by_from.entry(edge.from).or_default().push(position);
        if let Some(target) = edge.to.resolved() {
            self.by_to.entry(target).or_default().push(position);
        }
        self.edges.push(edge);
    }

    /// Every edge, in insertion order.
    #[must_use]
    pub fn all(&self) -> &[Edge] {
        &self.edges
    }

    /// Take every edge, consuming the store.
    ///
    /// Merging one index into another re-assigns node ids, so the edges are rebuilt
    /// rather than kept — this hands them over without cloning what is about to be
    /// dropped.
    #[must_use]
    pub fn into_all(self) -> Vec<Edge> {
        self.edges
    }

    /// How many edges are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Whether the store holds no edges.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Edges leaving `node`.
    pub fn from(&self, node: SeamId) -> impl Iterator<Item = &Edge> {
        self.lookup(&self.by_from, node)
    }

    /// Edges arriving at `node` from elsewhere in this package.
    ///
    /// This is how a trait finds its implementors: they point at it, not the reverse.
    pub fn to(&self, node: SeamId) -> impl Iterator<Item = &Edge> {
        self.lookup(&self.by_to, node)
    }

    /// Edges leaving `node` that are of one kind.
    pub fn from_of_kind(&self, node: SeamId, kind: EdgeKind) -> impl Iterator<Item = &Edge> {
        self.from(node).filter(move |edge| edge.kind == kind)
    }

    /// Edges arriving at `node` that are of one kind.
    pub fn to_of_kind(&self, node: SeamId, kind: EdgeKind) -> impl Iterator<Item = &Edge> {
        self.to(node).filter(move |edge| edge.kind == kind)
    }

    /// Drop every edge leaving `node`, for an incremental re-index of its file.
    pub fn remove_from(&mut self, node: SeamId) {
        if !self.by_from.contains_key(&node) {
            return;
        }
        self.edges.retain(|edge| edge.from != node);
        self.reindex();
    }

    /// Point every endpoint at wherever its node moved to.
    ///
    /// Regrouping changes ids, and an edge left naming the old one would point at a node
    /// that no longer exists — an unresolvable relation invented by bookkeeping, which is
    /// exactly the kind of false answer this crate refuses to give.
    pub fn remap(&mut self, remap: &std::collections::HashMap<SeamId, SeamId>) {
        if remap.is_empty() {
            return;
        }
        for edge in &mut self.edges {
            if let Some(moved) = remap.get(&edge.from) {
                edge.from = *moved;
            }
            if let Endpoint::Resolved(to) = &mut edge.to
                && let Some(moved) = remap.get(to)
            {
                *to = *moved;
            }
        }
        self.reindex();
    }

    /// Resolve a side index into edge references.
    fn lookup<'a>(
        &'a self,
        side: &'a HashMap<SeamId, Vec<usize>>,
        node: SeamId,
    ) -> impl Iterator<Item = &'a Edge> {
        side.get(&node)
            .into_iter()
            .flatten()
            .filter_map(|position| self.edges.get(*position))
    }

    /// Rebuild both side indexes after a removal shifted every position.
    fn reindex(&mut self) {
        self.by_from.clear();
        self.by_to.clear();
        for (position, edge) in self.edges.iter().enumerate() {
            self.by_from.entry(edge.from).or_default().push(position);
            if let Some(target) = edge.to.resolved() {
                self.by_to.entry(target).or_default().push(position);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(kind: EdgeKind, from: u32, to: Endpoint) -> Edge {
        Edge {
            kind,
            from: SeamId(from),
            to,
            site: None,
        }
    }

    #[test]
    fn edge_kind_names_round_trip() {
        for kind in EdgeKind::all() {
            assert_eq!(EdgeKind::from_name(kind.name()), Some(*kind));
        }
        assert_eq!(EdgeKind::from_name("calls"), None);
    }

    #[test]
    fn endpoints_report_their_three_states() {
        let resolved = Endpoint::Resolved(SeamId(3));
        assert_eq!(resolved.state(), "resolved");
        assert_eq!(resolved.resolved(), Some(SeamId(3)));
        assert_eq!(resolved.display_hint(), None);

        let external = Endpoint::External {
            display: "std::fmt::Display".to_owned(),
        };
        assert_eq!(external.state(), "external");
        assert_eq!(external.resolved(), None);
        assert_eq!(external.display_hint(), Some("std::fmt::Display"));

        let unresolved = Endpoint::Unresolved {
            hint: Some("Widget".to_owned()),
            resolvable: true,
        };
        assert_eq!(unresolved.state(), "unresolved");
        assert_eq!(unresolved.resolved(), None);
        assert_eq!(unresolved.display_hint(), Some("Widget"));
    }

    #[test]
    fn an_empty_store_answers_without_panicking() {
        let store = EdgeStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        assert_eq!(store.from(SeamId(0)).count(), 0);
        assert_eq!(store.to(SeamId(0)).count(), 0);
    }

    #[test]
    fn edges_are_findable_from_both_ends() {
        let mut store = EdgeStore::new();
        store.insert(edge(EdgeKind::Implements, 1, Endpoint::Resolved(SeamId(9))));
        store.insert(edge(EdgeKind::Implements, 2, Endpoint::Resolved(SeamId(9))));
        store.insert(edge(
            EdgeKind::ReExports,
            1,
            Endpoint::External {
                display: "other::Thing".to_owned(),
            },
        ));

        assert_eq!(store.len(), 3);
        assert_eq!(store.from(SeamId(1)).count(), 2);
        // A trait finds its implementors through the incoming side.
        assert_eq!(store.to(SeamId(9)).count(), 2);
        assert_eq!(
            store.from_of_kind(SeamId(1), EdgeKind::ReExports).count(),
            1
        );
        assert_eq!(store.to_of_kind(SeamId(9), EdgeKind::Implements).count(), 2);
    }

    #[test]
    fn an_unresolved_target_is_stored_but_not_reverse_indexed() {
        let mut store = EdgeStore::new();
        store.insert(edge(
            EdgeKind::Implements,
            1,
            Endpoint::Unresolved {
                hint: Some("Unknown".to_owned()),
                resolvable: true,
            },
        ));
        // The edge exists and is reachable from its source — losing it would erase the
        // difference between "no implementors" and "not resolved yet".
        assert_eq!(store.len(), 1);
        assert_eq!(store.from(SeamId(1)).count(), 1);
        assert_eq!(store.to(SeamId(1)).count(), 0);
    }

    #[test]
    fn removing_a_source_drops_only_its_edges_and_keeps_the_rest_findable() {
        let mut store = EdgeStore::new();
        store.insert(edge(EdgeKind::Implements, 1, Endpoint::Resolved(SeamId(9))));
        store.insert(edge(EdgeKind::Implements, 2, Endpoint::Resolved(SeamId(9))));
        store.insert(edge(EdgeKind::ReExports, 2, Endpoint::Resolved(SeamId(8))));

        store.remove_from(SeamId(1));

        assert_eq!(store.len(), 2);
        assert_eq!(store.from(SeamId(1)).count(), 0);
        // The surviving edges must still be reachable — positions shifted, so a stale
        // index would silently return the wrong edge or none at all.
        assert_eq!(store.from(SeamId(2)).count(), 2);
        assert_eq!(store.to(SeamId(9)).count(), 1);
        assert_eq!(store.to(SeamId(8)).count(), 1);
    }

    #[test]
    fn removing_an_unknown_source_is_a_no_op() {
        let mut store = EdgeStore::new();
        store.insert(edge(EdgeKind::Implements, 1, Endpoint::Resolved(SeamId(9))));
        store.remove_from(SeamId(42));
        assert_eq!(store.len(), 1);
        assert_eq!(store.from(SeamId(1)).count(), 1);
    }
}

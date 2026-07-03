//! A neutral directed-graph model for TUI code visualizations.
//!
//! Producers (`karet-vcs` commit history, a `dependable`-backed dependency graph, a
//! tree-sitter tags call/usage graph) emit a [`GraphView`]; the renderer
//! (`karet-graph`) lays it out and draws it. Keeping the model here — alongside the
//! other neutral vocabulary — lets it cross the session `Command`/`Event` seam without
//! any widget dependency, exactly like [`Symbol`](crate::model::Symbol) and
//! [`Decoration`](crate::model::Decoration).
//!
//! The model is deliberately small: nodes carry a stable `id`, a display `label`, a
//! [`GraphNodeKind`], and an optional `badge` (a short annotation such as a version or
//! status); edges are directed `from → to` with a [`GraphEdgeKind`]. `roots` names the
//! entry nodes a layout should start from (branch tips, a focused symbol, …).

/// What a [`GraphNode`] represents. Drives the glyph/colour a renderer picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum GraphNodeKind {
    /// A version-control commit.
    Commit,
    /// A package / crate / module in a dependency graph.
    Package,
    /// A source file or module.
    Module,
    /// A function / method / symbol.
    Symbol,
    /// An entity outside the workspace (a registry dependency, an external ref).
    External,
    /// Anything else.
    Other,
}

/// What a [`GraphEdge`] represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum GraphEdgeKind {
    /// A commit's parent (child → parent).
    Parent,
    /// A depends-on relationship (dependent → dependency).
    Dependency,
    /// A call / use relationship (caller → callee).
    Call,
    /// A containment relationship (module → symbol).
    Contains,
    /// Anything else.
    Other,
}

/// A node in a [`GraphView`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GraphNode {
    /// Stable identifier, unique within the view (referenced by [`GraphEdge`]s and
    /// [`GraphView::roots`]).
    pub id: String,
    /// Human-readable label shown to the user.
    pub label: String,
    /// What the node represents.
    pub kind: GraphNodeKind,
    /// An optional short annotation (version, status, count, …).
    pub badge: Option<String>,
}

impl GraphNode {
    /// A node with no badge.
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>, kind: GraphNodeKind) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            badge: None,
        }
    }

    /// Builder: attach a badge.
    #[must_use]
    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }
}

/// A directed edge `from → to` in a [`GraphView`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GraphEdge {
    /// The source node id.
    pub from: String,
    /// The target node id.
    pub to: String,
    /// What the edge represents.
    pub kind: GraphEdgeKind,
}

impl GraphEdge {
    /// A directed edge from `from` to `to`.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: GraphEdgeKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }
}

/// A neutral directed graph for visualization: nodes, directed edges, and the entry
/// nodes a layout should start from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GraphView {
    /// The nodes, in the producer's preferred order (e.g. commits newest-first).
    pub nodes: Vec<GraphNode>,
    /// The directed edges between nodes.
    pub edges: Vec<GraphEdge>,
    /// Entry-point node ids (branch tips, a focused symbol) a layout starts from.
    pub roots: Vec<String>,
}

impl GraphView {
    /// An empty view.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the view has no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The direct successors of `id` following edges of `kind` (`from == id → to`).
    #[must_use]
    pub fn successors(&self, id: &str, kind: GraphEdgeKind) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.kind == kind && e.from == id)
            .map(|e| e.to.as_str())
            .collect()
    }

    /// The direct predecessors of `id` following edges of `kind` (`to == id ← from`).
    #[must_use]
    pub fn predecessors(&self, id: &str, kind: GraphEdgeKind) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.kind == kind && e.to == id)
            .map(|e| e.from.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successors_and_predecessors_follow_edge_direction() {
        let mut view = GraphView::new();
        view.nodes
            .push(GraphNode::new("a", "A", GraphNodeKind::Symbol));
        view.nodes
            .push(GraphNode::new("b", "B", GraphNodeKind::Symbol));
        view.nodes
            .push(GraphNode::new("c", "C", GraphNodeKind::Symbol));
        // a calls b and c; b calls c.
        view.edges
            .push(GraphEdge::new("a", "b", GraphEdgeKind::Call));
        view.edges
            .push(GraphEdge::new("a", "c", GraphEdgeKind::Call));
        view.edges
            .push(GraphEdge::new("b", "c", GraphEdgeKind::Call));

        let mut callees = view.successors("a", GraphEdgeKind::Call);
        callees.sort_unstable();
        assert_eq!(callees, vec!["b", "c"]);

        let mut callers = view.predecessors("c", GraphEdgeKind::Call);
        callers.sort_unstable();
        assert_eq!(callers, vec!["a", "b"]);
    }

    #[test]
    fn badge_builder_sets_the_annotation() {
        let node = GraphNode::new("p", "serde", GraphNodeKind::Package).with_badge("1.0.228");
        assert_eq!(node.badge.as_deref(), Some("1.0.228"));
    }
}

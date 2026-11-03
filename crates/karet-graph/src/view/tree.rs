//! The ratatui tree renderer: a [`GraphView`] flattened into indented, styled rows.
//!
//! Like the rail renderer it carries no theme dependency — the caller supplies plain
//! ratatui styles via [`TreeStyles`], the heading text, and the edge kind to follow, so
//! one flattener serves every lens (dependency, usage, …).

use std::collections::HashMap;
use std::collections::HashSet;

use karet_core::GraphEdgeKind;
use karet_core::GraphNode;
use karet_core::GraphView;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

/// The styles a [`graph_tree_lines`] caller supplies — plain ratatui styles so
/// the renderer carries no theme dependency (the caller maps its palette).
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeStyles {
    /// The title/header line.
    pub header: Style,
    /// Indentation depth guides.
    pub guide: Style,
    /// Node labels.
    pub name: Style,
    /// Trailing node badges.
    pub badge: Style,
    /// The "already expanded / cycle" marker.
    pub revisit: Style,
}

/// Flatten `view` into indented, styled rows: a DFS from the graph's roots along `edge`
/// edges with box-drawing depth guides. Cycles and already-expanded nodes are shown once
/// and marked `⟲` rather than re-expanded. `heading` is the header line's text (the
/// caller names the lens); the renderer owns only the leading glyph. The caller owns
/// scrolling and painting.
#[must_use]
pub fn graph_tree_lines(
    heading: &str,
    view: &GraphView,
    edge: GraphEdgeKind,
    styles: &TreeStyles,
) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> =
        vec![Line::styled(format!(" \u{2689} {heading}"), styles.header)];
    // One pass to index the nodes: the DFS below looks up every id it visits, and a
    // linear scan per visit is quadratic on a wide graph.
    let by_id: HashMap<&str, &GraphNode> = view.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut expanded: HashSet<&str> = HashSet::new();
    let mut stack: Vec<(&str, usize)> = view
        .roots
        .iter()
        .rev()
        .map(|r| (r.as_str(), 0usize))
        .collect();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = by_id.get(id) else {
            continue;
        };
        let first_visit = expanded.insert(id);
        let children = view.successors(id, edge);
        let mut spans = vec![Span::raw(" ")];
        for _ in 0..depth {
            spans.push(Span::styled("\u{2502} ", styles.guide));
        }
        spans.push(Span::styled("\u{25CF} ", styles.guide));
        spans.push(Span::styled(node.label.clone(), styles.name));
        if let Some(badge) = &node.badge {
            spans.push(Span::styled(format!("  {badge}"), styles.badge));
        }
        if !first_visit && !children.is_empty() {
            // Already expanded elsewhere (or a cycle): show but don't recurse again.
            spans.push(Span::styled("  \u{27F2}", styles.revisit));
        }
        rows.push(Line::from(spans));
        if first_visit {
            for child in children.iter().rev() {
                stack.push((child, depth + 1));
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> GraphView {
        use karet_core::GraphEdge;
        GraphView {
            roots: vec!["a".into()],
            nodes: vec![
                GraphNode {
                    id: "a".into(),
                    label: "alpha".into(),
                    kind: karet_core::GraphNodeKind::Package,
                    badge: Some("v1".into()),
                },
                GraphNode {
                    id: "b".into(),
                    label: "beta".into(),
                    kind: karet_core::GraphNodeKind::Package,
                    badge: None,
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "a".into(),
                    to: "b".into(),
                    kind: GraphEdgeKind::Dependency,
                },
                // A cycle back to the root.
                GraphEdge {
                    from: "b".into(),
                    to: "a".into(),
                    kind: GraphEdgeKind::Dependency,
                },
            ],
        }
    }

    fn flat(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn flattens_roots_depth_first_with_badges_and_cycle_marks() {
        let rows = graph_tree_lines(
            "ws \u{2014} dependency graph",
            &view(),
            GraphEdgeKind::Dependency,
            &TreeStyles::default(),
        );
        assert!(flat(&rows[0]).contains("ws"));
        assert!(flat(&rows[0]).contains("dependency graph"));
        assert!(flat(&rows[1]).contains("alpha"));
        assert!(flat(&rows[1]).contains("v1"));
        assert!(flat(&rows[2]).contains("beta"));
        // The cycle back to alpha renders once more, marked, without recursing.
        assert!(flat(&rows[3]).contains("alpha"));
        assert!(flat(&rows[3]).contains("\u{27F2}"));
        assert_eq!(rows.len(), 4);
    }

    /// The edge kind is a parameter, not a constant: following a kind the graph has no
    /// edges of yields the roots alone.
    #[test]
    fn only_follows_the_requested_edge_kind() {
        let rows = graph_tree_lines("ws", &view(), GraphEdgeKind::Call, &TreeStyles::default());
        assert_eq!(rows.len(), 2, "header + the single root, no dependents");
        assert!(flat(&rows[1]).contains("alpha"));
    }
}

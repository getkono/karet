//! The ratatui rail renderer (behind the `view` feature).
//!
//! [`render_rail`] turns a [`RailRow`] into a [`Line`] whose glyphs are coloured by
//! lane. It takes a `lane_style` closure (index → [`Style`]) instead of depending on a
//! theme crate, so the caller maps lane colours onto whatever palette it uses.

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::RailRow;

/// Render `row`'s glyph gutter as a coloured [`Line`], mapping each glyph's lane colour
/// index through `lane_style`. Contiguous same-style glyphs are coalesced into one
/// [`Span`] so the line stays cheap.
#[must_use]
pub fn render_rail<'a>(row: &RailRow, lane_style: impl Fn(u8) -> Style) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut run = String::new();
    let mut run_color: Option<u8> = None;

    for (ch, &color) in row.gutter.chars().zip(row.colors.iter()) {
        if let Some(prev) = run_color
            && prev != color
        {
            spans.push(Span::styled(std::mem::take(&mut run), lane_style(prev)));
        }
        run_color = Some(color);
        run.push(ch);
    }
    if let Some(color) = run_color {
        spans.push(Span::styled(run, lane_style(color)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;
    use crate::LaneInput;
    use crate::assign_lanes;

    #[test]
    fn renders_a_line_with_the_gutter_glyphs() {
        let rows = assign_lanes(&[
            LaneInput {
                id: "b".into(),
                parents: vec!["a".into()],
                head: true,
            },
            LaneInput::new("a", vec![]),
        ]);
        let line = render_rail(&rows[0], |_| Style::default().fg(Color::Red));
        // The rendered line reproduces the gutter text.
        let text: String = line.spans.iter().flat_map(|s| s.content.chars()).collect();
        assert_eq!(text, rows[0].gutter);
    }

    #[test]
    fn different_lane_colors_split_into_spans() {
        // A merge row has at least two lane colours → at least two spans.
        let rows = assign_lanes(&[
            LaneInput::new("d", vec!["c".into(), "b".into()]),
            LaneInput::new("c", vec!["a".into()]),
            LaneInput::new("b", vec!["a".into()]),
            LaneInput::new("a", vec![]),
        ]);
        let line = render_rail(&rows[0], |c| {
            Style::default().fg(if c == 0 { Color::Red } else { Color::Blue })
        });
        assert!(
            line.spans.len() >= 2,
            "merge row spans multiple lane colours"
        );
    }
}

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

/// Flatten a [`GraphView`](karet_core::GraphView) into indented, styled rows:
/// a DFS from the graph's roots along dependency edges with box-drawing depth
/// guides. Cycles and already-expanded nodes are shown once and marked `⟲`
/// rather than re-expanded. The caller owns scrolling and painting.
#[must_use]
pub fn graph_tree_lines(
    title: &str,
    view: &karet_core::GraphView,
    styles: &TreeStyles,
) -> Vec<Line<'static>> {
    use karet_core::GraphEdgeKind;

    let mut rows: Vec<Line<'static>> = vec![Line::styled(
        format!(" \u{2689} {title} \u{2014} dependency graph"),
        styles.header,
    )];
    let mut expanded: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut stack: Vec<(&str, usize)> = view
        .roots
        .iter()
        .rev()
        .map(|r| (r.as_str(), 0usize))
        .collect();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = view.nodes.iter().find(|n| n.id == id) else {
            continue;
        };
        let first_visit = expanded.insert(id);
        let children = view.successors(id, GraphEdgeKind::Dependency);
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
mod tree_tests {
    use super::*;

    fn view() -> karet_core::GraphView {
        use karet_core::GraphEdge;
        use karet_core::GraphEdgeKind;
        use karet_core::GraphNode;
        karet_core::GraphView {
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
        let rows = graph_tree_lines("ws", &view(), &TreeStyles::default());
        assert!(flat(&rows[0]).contains("ws"));
        assert!(flat(&rows[1]).contains("alpha"));
        assert!(flat(&rows[1]).contains("v1"));
        assert!(flat(&rows[2]).contains("beta"));
        // The cycle back to alpha renders once more, marked, without recursing.
        assert!(flat(&rows[3]).contains("alpha"));
        assert!(flat(&rows[3]).contains("\u{27F2}"));
        assert_eq!(rows.len(), 4);
    }
}

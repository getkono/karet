//! A [`PaneLayout`]'s tree detached from the live layout, so a caller can
//! persist the shape and split ratios and rebuild them later.

use super::Node;
use super::PaneId;
use super::PaneLayout;
use super::SplitAxis;
use super::first_leaf;

/// A pane tree's shape and split ratios, detached from a live layout: what
/// [`PaneLayout::tree`] reports and [`PaneLayout::from_tree`] rebuilds.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneTree {
    /// A single pane.
    Pane(PaneId),
    /// Child trees laid out along `axis`, each sized by its share of `weights`.
    Split {
        /// The axis the children divide.
        axis: SplitAxis,
        /// The children, left-to-right or top-to-bottom.
        children: Vec<PaneTree>,
        /// One relative size per child.
        weights: Vec<f32>,
    },
}

impl PaneLayout {
    /// The layout's shape and split ratios, for persisting it.
    #[must_use]
    pub fn tree(&self) -> PaneTree {
        to_tree(&self.root)
    }

    /// Rebuild a layout from `tree`, focusing `focus` (the first pane when
    /// `focus` is not in it). `None` when the tree holds no pane.
    ///
    /// The tree may come from storage, so it is taken as untrusted: a split
    /// with no children is dropped and one with a single child is replaced by
    /// it, a repeated pane id is kept only where it first appears, and a split
    /// whose weights do not give every child a positive finite share sizes its
    /// children equally.
    #[must_use]
    pub fn from_tree(tree: PaneTree, focus: PaneId) -> Option<Self> {
        let mut seen = Vec::new();
        let root = from_tree_node(tree, &mut seen)?;
        let next_id = seen
            .iter()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let focus = if seen.contains(&focus) {
            focus
        } else {
            first_leaf(&root)
        };
        Some(Self {
            root,
            focus,
            next_id,
        })
    }
}

/// The detached [`PaneTree`] of a subtree.
fn to_tree(node: &Node) -> PaneTree {
    match node {
        Node::Leaf(id) => PaneTree::Pane(*id),
        Node::Split {
            axis,
            children,
            weights,
        } => PaneTree::Split {
            axis: *axis,
            children: children.iter().map(to_tree).collect(),
            weights: weights.clone(),
        },
    }
}

/// The live subtree for `tree`, sanitized as [`PaneLayout::from_tree`]
/// describes; `seen` collects the pane ids already placed.
fn from_tree_node(tree: PaneTree, seen: &mut Vec<PaneId>) -> Option<Node> {
    match tree {
        PaneTree::Pane(id) if seen.contains(&id) => None,
        PaneTree::Pane(id) => {
            seen.push(id);
            Some(Node::Leaf(id))
        },
        PaneTree::Split {
            axis,
            children,
            weights,
        } => {
            let sound = weights.len() == children.len()
                && weights.iter().all(|w| w.is_finite() && *w > 0.0);
            let mut kept = Vec::new();
            let mut kept_weights = Vec::new();
            for (index, child) in children.into_iter().enumerate() {
                if let Some(node) = from_tree_node(child, seen) {
                    kept.push(node);
                    let weight = weights.get(index).copied().filter(|_| sound);
                    kept_weights.push(weight.unwrap_or(1.0));
                }
            }
            match kept.len() {
                0 => None,
                1 => kept.pop(),
                _ => Some(Node::Split {
                    axis,
                    children: kept,
                    weights: kept_weights,
                }),
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::*;
    use crate::pane::SplitDir;

    fn area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    #[test]
    fn a_tree_round_trip_keeps_shape_ratios_and_focus() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right);
        let c = l.split(b, SplitDir::Down);
        // Uneven ratios, so a rebuild that dropped them would tile differently.
        let divider = l.dividers(area())[0];
        assert!(l.resize_divider(divider, 12, area()));
        l.set_focus(b);
        let rebuilt = PaneLayout::from_tree(l.tree(), l.focus());
        assert!(rebuilt.is_some(), "a live layout's tree always rebuilds");
        let Some(mut rebuilt) = rebuilt else {
            return;
        };
        assert_eq!(rebuilt.layout(area()), l.layout(area()));
        assert_eq!(rebuilt.focus(), b);
        // New panes never reuse a restored id.
        let d = rebuilt.split(c, SplitDir::Right);
        assert!(![a, b, c].contains(&d));
    }

    #[test]
    fn from_tree_sanitizes_an_untrusted_tree() {
        let tree = PaneTree::Split {
            axis: SplitAxis::Cols,
            children: vec![
                PaneTree::Pane(PaneId(4)),
                PaneTree::Split {
                    axis: SplitAxis::Rows,
                    children: vec![],
                    weights: vec![],
                },
                PaneTree::Split {
                    axis: SplitAxis::Rows,
                    children: vec![PaneTree::Pane(PaneId(4)), PaneTree::Pane(PaneId(7))],
                    weights: vec![1.0, 1.0],
                },
            ],
            weights: vec![f32::NAN, 1.0],
        };
        let l = PaneLayout::from_tree(tree, PaneId(99));
        assert!(l.is_some(), "two panes survive");
        let Some(l) = l else {
            return;
        };
        assert_eq!(
            l.panes(),
            vec![PaneId(4), PaneId(7)],
            "the repeat is dropped"
        );
        assert_eq!(
            l.focus(),
            PaneId(4),
            "an unknown focus falls to the first pane"
        );
        assert_eq!(
            l.tree(),
            PaneTree::Split {
                axis: SplitAxis::Cols,
                children: vec![PaneTree::Pane(PaneId(4)), PaneTree::Pane(PaneId(7))],
                weights: vec![1.0, 1.0],
            },
            "empty splits go, lone children are promoted, bad weights even out"
        );
        let empty = PaneTree::Split {
            axis: SplitAxis::Rows,
            children: vec![],
            weights: vec![],
        };
        assert!(PaneLayout::from_tree(empty, PaneId(0)).is_none());
    }
}

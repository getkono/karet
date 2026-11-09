//! One function's call tree, expanded for reading.
//!
//! The graph is the source of truth; this is a view of it rooted at one function, for a
//! person or a model to read top-down. Each function is expanded at its first
//! appearance only — later ones are marked as repeats — so a tree over a graph with
//! shared helpers and cycles stays as large as the graph rather than exponential.

use std::collections::HashSet;
use std::fmt::Write as _;

use super::graph::Bound;
use super::graph::CallGraph;
use crate::id::SeamId;
use crate::index::SeamIndex;

/// How far to expand a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeLimits {
    /// Levels below the root; `None` for the whole reach.
    pub depth: Option<usize>,
    /// Children shown per node, most inclusive first; `None` for all of them.
    pub width: Option<usize>,
}

impl TreeLimits {
    /// The whole tree.
    pub const FULL: Self = Self {
        depth: None,
        width: None,
    };
}

/// One function in a call tree.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TreeNode {
    /// The function's identity.
    pub path: String,
    /// Its own code lines.
    pub code_lines: u32,
    /// Its inclusive lines under the tree's bound.
    pub inclusive: u64,
    /// How many call sites in the parent reach it; 1 at the root.
    pub sites: u32,
    /// Whether it was already expanded higher up, so its subtree is not repeated.
    pub repeat: bool,
    /// Its callees, most inclusive first.
    pub children: Vec<TreeNode>,
    /// Callees left out by the width limit.
    pub elided: usize,
    /// Whether the depth limit stopped expansion here with callees remaining.
    pub truncated: bool,
}

/// A call tree rooted at one function.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CallTree {
    /// Which bound the tree follows.
    pub bound: Bound,
    /// The root function.
    pub root: TreeNode,
}

/// The call tree below `root`, or `None` when the graph does not hold it.
#[must_use]
pub fn call_tree(
    index: &SeamIndex,
    graph: &CallGraph,
    root: SeamId,
    bound: Bound,
    limits: TreeLimits,
) -> Option<CallTree> {
    graph.metrics(root)?;
    let mut seen = HashSet::new();
    let root = expand(index, graph, root, 1, bound, limits, 0, &mut seen);
    Some(CallTree { bound, root })
}

#[allow(
    clippy::too_many_arguments,
    reason = "a recursive walk threading its limits and visited set; bundling them would only rename the arguments"
)]
fn expand(
    index: &SeamIndex,
    graph: &CallGraph,
    id: SeamId,
    sites: u32,
    bound: Bound,
    limits: TreeLimits,
    level: usize,
    seen: &mut HashSet<SeamId>,
) -> TreeNode {
    let metrics = graph.metrics(id);
    let inclusive = metrics.map_or(0, |m| match bound {
        Bound::Lower => m.inclusive_lower,
        Bound::Upper => m.inclusive_upper,
    });
    let mut node = TreeNode {
        path: index.path(id).map(ToString::to_string).unwrap_or_default(),
        code_lines: metrics.map_or(0, |m| m.code_lines),
        inclusive,
        sites,
        repeat: seen.contains(&id),
        children: Vec::new(),
        elided: 0,
        truncated: false,
    };
    if node.repeat {
        return node;
    }
    let mut callees: Vec<(SeamId, u32)> = graph
        .callees(id, bound)
        .into_iter()
        .filter(|(callee, _)| *callee != id)
        .collect();
    if callees.is_empty() {
        seen.insert(id);
        return node;
    }
    // Cut off here, the subtree has not been shown — so it is not marked seen, and a later
    // appearance expands it rather than pointing "above" at nothing.
    if limits.depth.is_some_and(|depth| level >= depth) {
        node.truncated = true;
        return node;
    }
    seen.insert(id);
    let weight = |callee: &SeamId| {
        graph.metrics(*callee).map_or(0, |m| match bound {
            Bound::Lower => m.inclusive_lower,
            Bound::Upper => m.inclusive_upper,
        })
    };
    callees.sort_by(|a, b| {
        weight(&b.0).cmp(&weight(&a.0)).then_with(|| {
            index
                .path(a.0)
                .map(ToString::to_string)
                .cmp(&index.path(b.0).map(ToString::to_string))
        })
    });
    if let Some(width) = limits.width
        && callees.len() > width
    {
        node.elided = callees.len() - width;
        callees.truncate(width);
    }
    node.children = callees
        .into_iter()
        .map(|(callee, count)| expand(index, graph, callee, count, bound, limits, level + 1, seen))
        .collect();
    node
}

impl CallTree {
    /// The tree as indented text: own lines, inclusive lines, and the path per row.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        render_node(&self.root, 0, &mut out);
        out
    }

    /// The tree as Markdown: a heading and the rendered tree in a fenced block.
    #[must_use]
    pub fn markdown(&self) -> String {
        let bound = match self.bound {
            Bound::Lower => "established calls (lower bound)",
            Bound::Upper => "every plausible call (upper bound)",
        };
        format!(
            "# Call tree of {}\n\nFollowing {bound}. Each row: own lines / inclusive \
             lines, then the function; `×n` is the number of call sites, `↺` a function \
             already expanded above.\n\n```text\n{}```\n",
            super::report::code(&self.root.path),
            self.render()
        )
    }
}

fn render_node(node: &TreeNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let sites = if node.sites > 1 {
        format!(" ×{}", node.sites)
    } else {
        String::new()
    };
    let marker = if node.repeat {
        " ↺ above"
    } else if node.truncated {
        " …"
    } else {
        ""
    };
    let _ = writeln!(
        out,
        "{indent}{:>5} / {:<7} {}{sites}{marker}",
        node.code_lines, node.inclusive, node.path
    );
    for child in &node.children {
        render_node(child, depth + 1, out);
    }
    if node.elided > 0 {
        let _ = writeln!(out, "{indent}  … {} more", node.elided);
    }
}

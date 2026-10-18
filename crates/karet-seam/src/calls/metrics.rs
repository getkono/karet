//! Sizes over a call graph: what each function reaches, and what its call tree weighs.
//!
//! A memoized sum down the call tree is wrong twice over. A function reached along two
//! paths — a diamond — is counted twice, and a cycle never finishes. So the graph is
//! first condensed: every strongly connected component (a function calling itself, or a
//! set of functions calling one another) becomes one unit, and the result is acyclic.
//!
//! Over that acyclic graph, components are visited callees-first, and each computes:
//!
//! - **inclusive** — the lines of the *distinct* code it reaches, itself included. Kept
//!   as the set of reached components, the union of its callees' sets, so shared code
//!   counts once. Sets are dropped as soon as the last caller has read them.
//! - **tree-weighted** — its own lines plus each callee's tree weight once per call
//!   site. Shared code counts once per path, which is what makes it a measure of how
//!   much a function leans on reuse; it saturates rather than overflowing.
//! - **depth** — the longest chain of calls below it.
//!
//! A component is one unit throughout: its members share every value.

use std::collections::BTreeMap;
use std::collections::VecDeque;

/// Per-node results, indexed like the adjacency they came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Computed {
    pub(crate) inclusive: Vec<u64>,
    pub(crate) tree: Vec<u64>,
    pub(crate) saturated: Vec<bool>,
    pub(crate) depth: Vec<u32>,
    pub(crate) component: Vec<u32>,
    pub(crate) size: Vec<u32>,
    pub(crate) recursive: Vec<bool>,
}

/// Compute every metric for every node.
///
/// `adjacency[n]` lists `(callee, sites)` for node `n`; `weights[n]` is its own lines.
/// Above `max_dense` components the reached sets are not materialized and each
/// inclusive size is a walk of its own instead — slower, and bounded in memory.
pub(crate) fn compute(
    adjacency: &[Vec<(u32, u32)>],
    weights: &[u64],
    max_dense: usize,
) -> Computed {
    let nodes = adjacency.len();
    let (component, count) = components(adjacency);
    let mut size = vec![0u32; count];
    let mut weight = vec![0u64; count];
    for (node, c) in component.iter().enumerate() {
        if let Some(slot) = size.get_mut(*c as usize) {
            *slot = slot.saturating_add(1);
        }
        if let Some(slot) = weight.get_mut(*c as usize) {
            *slot = slot.saturating_add(weights.get(node).copied().unwrap_or(0));
        }
    }

    // Condensed edges, with call sites summed between components.
    let mut successors: Vec<BTreeMap<u32, u64>> = vec![BTreeMap::new(); count];
    let mut self_loop = vec![false; count];
    for (from, edges) in adjacency.iter().enumerate() {
        let Some(&cf) = component.get(from) else {
            continue;
        };
        for &(to, sites) in edges {
            let Some(&ct) = component.get(to as usize) else {
                continue;
            };
            if cf == ct {
                if let Some(flag) = self_loop.get_mut(cf as usize) {
                    *flag = true;
                }
            } else if let Some(map) = successors.get_mut(cf as usize) {
                let entry = map.entry(ct).or_insert(0);
                *entry = entry.saturating_add(u64::from(sites));
            }
        }
    }

    let mut tree = vec![0u64; count];
    let mut saturated = vec![false; count];
    let mut depth = vec![0u32; count];
    // Components are numbered callees-first, so every successor is already settled.
    for c in 0..count {
        let own = weight.get(c).copied().unwrap_or(0);
        let mut total = own;
        let mut overflowed = false;
        let mut deepest = 0u32;
        for (&d, &sites) in successors.get(c).into_iter().flatten() {
            let d = d as usize;
            let below = tree.get(d).copied().unwrap_or(0);
            overflowed |= saturated.get(d).copied().unwrap_or(false);
            let added = below
                .checked_mul(sites)
                .and_then(|add| total.checked_add(add));
            match added {
                Some(sum) => total = sum,
                None => {
                    total = u64::MAX;
                    overflowed = true;
                },
            }
            deepest = deepest.max(depth.get(d).copied().unwrap_or(0).saturating_add(1));
        }
        if let Some(slot) = tree.get_mut(c) {
            *slot = total;
        }
        if let Some(slot) = saturated.get_mut(c) {
            *slot = overflowed;
        }
        if let Some(slot) = depth.get_mut(c) {
            *slot = deepest;
        }
    }

    let inclusive = if count <= max_dense {
        dense_inclusive(&successors, &weight)
    } else {
        walked_inclusive(&successors, &weight)
    };

    let per = |values: &[u64]| -> Vec<u64> {
        component
            .iter()
            .map(|c| values.get(*c as usize).copied().unwrap_or(0))
            .collect()
    };
    Computed {
        inclusive: per(&inclusive),
        tree: per(&tree),
        saturated: component
            .iter()
            .map(|c| saturated.get(*c as usize).copied().unwrap_or(false))
            .collect(),
        depth: component
            .iter()
            .map(|c| depth.get(*c as usize).copied().unwrap_or(0))
            .collect(),
        size: component
            .iter()
            .map(|c| size.get(*c as usize).copied().unwrap_or(1))
            .collect(),
        recursive: component
            .iter()
            .map(|c| {
                size.get(*c as usize).is_some_and(|s| *s > 1)
                    || self_loop.get(*c as usize).copied().unwrap_or(false)
            })
            .collect(),
        component: {
            debug_assert_eq!(component.len(), nodes);
            component
        },
    }
}

/// Inclusive sizes from materialized reached-sets, released after their last reader.
fn dense_inclusive(successors: &[BTreeMap<u32, u64>], weight: &[u64]) -> Vec<u64> {
    let count = successors.len();
    let mut readers = vec![0u32; count];
    for map in successors {
        for d in map.keys() {
            if let Some(slot) = readers.get_mut(*d as usize) {
                *slot = slot.saturating_add(1);
            }
        }
    }
    let mut sets: Vec<Option<Bits>> = vec![None; count];
    let mut out = vec![0u64; count];
    for c in 0..count {
        let mut reached = Bits::new(count);
        reached.set(c);
        for d in successors
            .get(c)
            .into_iter()
            .flatten()
            .map(|(d, _)| *d as usize)
        {
            if let Some(Some(below)) = sets.get(d) {
                reached.union(below);
            }
        }
        if let Some(slot) = out.get_mut(c) {
            *slot = reached
                .ones()
                .map(|x| weight.get(x).copied().unwrap_or(0))
                .fold(0u64, u64::saturating_add);
        }
        for d in successors
            .get(c)
            .into_iter()
            .flatten()
            .map(|(d, _)| *d as usize)
        {
            if let Some(left) = readers.get_mut(d) {
                *left = left.saturating_sub(1);
                if *left == 0
                    && let Some(set) = sets.get_mut(d)
                {
                    *set = None;
                }
            }
        }
        if readers.get(c).is_some_and(|left| *left > 0)
            && let Some(set) = sets.get_mut(c)
        {
            *set = Some(reached);
        }
    }
    out
}

/// Inclusive sizes by walking from each component, holding one visited set at a time.
fn walked_inclusive(successors: &[BTreeMap<u32, u64>], weight: &[u64]) -> Vec<u64> {
    let count = successors.len();
    let mut out = vec![0u64; count];
    let mut seen = vec![usize::MAX; count];
    let mut queue = VecDeque::new();
    for c in 0..count {
        let mut total = 0u64;
        queue.clear();
        queue.push_back(c);
        if let Some(mark) = seen.get_mut(c) {
            *mark = c;
        }
        while let Some(at) = queue.pop_front() {
            total = total.saturating_add(weight.get(at).copied().unwrap_or(0));
            for d in successors
                .get(at)
                .into_iter()
                .flatten()
                .map(|(d, _)| *d as usize)
            {
                if let Some(mark) = seen.get_mut(d)
                    && *mark != c
                {
                    *mark = c;
                    queue.push_back(d);
                }
            }
        }
        if let Some(slot) = out.get_mut(c) {
            *slot = total;
        }
    }
    out
}

/// Strongly connected components, numbered so every successor's number is lower.
///
/// Tarjan's algorithm with an explicit stack: call chains in real code run deep enough
/// that recursion on the native stack is not safe.
pub(crate) fn components(adjacency: &[Vec<(u32, u32)>]) -> (Vec<u32>, usize) {
    const UNVISITED: u32 = u32::MAX;
    let nodes = adjacency.len();
    let mut index = vec![UNVISITED; nodes];
    let mut low = vec![0u32; nodes];
    let mut on_stack = vec![false; nodes];
    let mut stack: Vec<usize> = Vec::new();
    let mut component = vec![UNVISITED; nodes];
    let mut next_index = 0u32;
    let mut count = 0u32;
    // (node, position of the next edge to explore)
    let mut frames: Vec<(usize, usize)> = Vec::new();

    for root in 0..nodes {
        if index.get(root) != Some(&UNVISITED) {
            continue;
        }
        frames.push((root, 0));
        while let Some(&mut (node, ref mut edge)) = frames.last_mut() {
            if *edge == 0 && index.get(node) == Some(&UNVISITED) {
                if let (Some(i), Some(l)) = (index.get_mut(node), low.get_mut(node)) {
                    *i = next_index;
                    *l = next_index;
                }
                next_index = next_index.saturating_add(1);
                stack.push(node);
                if let Some(flag) = on_stack.get_mut(node) {
                    *flag = true;
                }
            }
            let edges = adjacency.get(node).map_or(&[][..], Vec::as_slice);
            if let Some(&(to, _)) = edges.get(*edge) {
                *edge += 1;
                let to = to as usize;
                match index.get(to) {
                    Some(&UNVISITED) => frames.push((to, 0)),
                    Some(&seen) if on_stack.get(to).copied().unwrap_or(false) => {
                        if let Some(l) = low.get_mut(node) {
                            *l = (*l).min(seen);
                        }
                    },
                    _ => {},
                }
                continue;
            }
            frames.pop();
            let node_low = low.get(node).copied().unwrap_or(0);
            if let Some(&(parent, _)) = frames.last()
                && let Some(l) = low.get_mut(parent)
            {
                *l = (*l).min(node_low);
            }
            if index.get(node) == Some(&node_low) {
                while let Some(member) = stack.pop() {
                    if let Some(flag) = on_stack.get_mut(member) {
                        *flag = false;
                    }
                    if let Some(slot) = component.get_mut(member) {
                        *slot = count;
                    }
                    if member == node {
                        break;
                    }
                }
                count = count.saturating_add(1);
            }
        }
    }
    (component, count as usize)
}

/// A fixed-size bit set.
#[derive(Clone, Debug)]
struct Bits {
    words: Vec<u64>,
}

impl Bits {
    fn new(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
        }
    }

    fn set(&mut self, at: usize) {
        if let Some(word) = self.words.get_mut(at / 64) {
            *word |= 1u64 << (at % 64);
        }
    }

    fn union(&mut self, other: &Self) {
        for (word, theirs) in self.words.iter_mut().zip(&other.words) {
            *word |= *theirs;
        }
    }

    fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(at, word)| {
            let mut rest = *word;
            std::iter::from_fn(move || {
                if rest == 0 {
                    return None;
                }
                let bit = rest.trailing_zeros() as usize;
                rest &= rest - 1;
                Some(at * 64 + bit)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an adjacency list from `(from, to)` pairs, one site each.
    fn graph(nodes: usize, edges: &[(u32, u32)]) -> Vec<Vec<(u32, u32)>> {
        let mut out = vec![Vec::new(); nodes];
        for &(from, to) in edges {
            if let Some(list) = out.get_mut(from as usize) {
                match list.iter_mut().find(|(t, _)| *t == to) {
                    Some((_, sites)) => *sites += 1,
                    None => list.push((to, 1)),
                }
            }
        }
        out
    }

    #[test]
    fn a_diamond_counts_shared_code_once_inclusively_and_twice_in_the_tree() {
        // A → B, A → C, B → D, C → D.
        let g = graph(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let weights = [1, 10, 100, 1000];
        for max_dense in [usize::MAX, 0] {
            let m = compute(&g, &weights, max_dense);
            assert_eq!(
                m.inclusive,
                [1111, 1010, 1100, 1000],
                "dense cap {max_dense}"
            );
            assert_eq!(m.tree[0], 1 + 10 + 100 + 2 * 1000);
            assert_eq!(m.depth, [2, 1, 1, 0]);
            assert!(m.recursive.iter().all(|r| !r));
        }
    }

    #[test]
    fn self_recursion_terminates_and_is_flagged() {
        let g = graph(2, &[(0, 0), (0, 1)]);
        let m = compute(&g, &[5, 7], usize::MAX);
        assert_eq!(m.inclusive, [12, 7]);
        assert_eq!(m.tree, [12, 7]);
        assert_eq!(m.recursive, [true, false]);
        assert_eq!(m.size, [1, 1]);
    }

    #[test]
    fn mutual_recursion_is_one_unit_sharing_every_value() {
        // A ↔ B → C, and D → A.
        let g = graph(4, &[(0, 1), (1, 0), (1, 2), (3, 0)]);
        let m = compute(&g, &[1, 2, 4, 8], usize::MAX);
        assert_eq!(m.component[0], m.component[1]);
        assert_eq!(m.size[0], 2);
        assert_eq!(m.inclusive[0], 7);
        assert_eq!(m.inclusive[1], 7);
        assert_eq!(m.inclusive[3], 15);
        assert_eq!(m.depth[3], 2);
        assert!(m.recursive[0] && m.recursive[1] && !m.recursive[2]);
        // Callees settle first, so a callee's component number is lower.
        assert!(m.component[2] < m.component[0] && m.component[0] < m.component[3]);
    }

    #[test]
    fn repeated_call_sites_multiply_only_the_tree_weight() {
        let mut g = graph(2, &[]);
        if let Some(list) = g.get_mut(0) {
            list.push((1, 3));
        }
        let m = compute(&g, &[1, 10], usize::MAX);
        assert_eq!(m.inclusive[0], 11);
        assert_eq!(m.tree[0], 31);
    }

    #[test]
    fn a_doubling_chain_saturates_instead_of_overflowing() {
        // Each link calls the next twice: 2^70 paths outgrow u64.
        let nodes = 72;
        let mut g = vec![Vec::new(); nodes];
        for (at, list) in g.iter_mut().enumerate().take(nodes - 1) {
            list.push((u32::try_from(at + 1).unwrap_or(0), 2));
        }
        let m = compute(&g, &vec![1; nodes], usize::MAX);
        assert_eq!(m.tree[0], u64::MAX);
        assert!(m.saturated[0]);
        assert!(!m.saturated[nodes - 1]);
        assert_eq!(m.inclusive[0], nodes as u64);
    }

    #[test]
    fn a_deep_chain_does_not_exhaust_the_stack() {
        let nodes = 200_000;
        let g: Vec<Vec<(u32, u32)>> = (0..nodes)
            .map(|at| match u32::try_from(at + 1) {
                Ok(next) if at + 1 < nodes => vec![(next, 1)],
                _ => Vec::new(),
            })
            .collect();
        let (component, count) = components(&g);
        assert_eq!(count, nodes);
        assert_eq!(
            component.first().copied(),
            Some(u32::try_from(nodes - 1).unwrap_or(0))
        );
    }

    #[test]
    fn an_empty_graph_has_nothing_to_say() {
        let m = compute(&[], &[], usize::MAX);
        assert!(m.inclusive.is_empty() && m.component.is_empty());
    }
}

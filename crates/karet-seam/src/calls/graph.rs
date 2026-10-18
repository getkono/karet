//! The call graph under one configuration, with every function's size worked out.
//!
//! Built from the index's [`EdgeKind::Calls`] edges, twice over: the **lower** bound
//! follows only edges whose [`Resolution`] is established, the **upper** bound every
//! plausible one. Each function's metrics are computed once, at build, so asking for one
//! is a lookup.
//!
//! The graph holds the callables the configuration includes. Under the default one that
//! leaves out test code — a test calls everything, and counting it would make every
//! function look reachable and every helper look shared.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;

use super::metrics;
use crate::config::Configuration;
use crate::edge::EdgeKind;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;

/// Which edges a traversal follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Bound {
    /// Only what is established: [`Resolution::in_lower`].
    Lower,
    /// Everything plausible: [`Resolution::in_upper`].
    Upper,
}

impl Bound {
    /// Whether an edge resolved this way is followed under this bound.
    #[must_use]
    pub fn follows(self, resolution: Resolution) -> bool {
        match self {
            Self::Lower => resolution.in_lower(),
            Self::Upper => resolution.in_upper(),
        }
    }
}

/// How to build a graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphOptions {
    /// Which callables exist: anything the configuration excludes is left out.
    pub configuration: Configuration,
    /// Above this many components, reached sets are walked rather than materialized,
    /// trading time for memory that would otherwise grow with the square of the graph:
    /// at the default, every set alive at once would be 200 MB, and in practice sets are
    /// released long before that.
    pub max_dense: usize,
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            configuration: Configuration::unconfigured(),
            max_dense: 40_000,
        }
    }
}

/// One function's sizes and shape.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FunctionMetrics {
    /// The function.
    pub id: SeamId,
    /// Code lines it owns itself.
    pub code_lines: u32,
    /// Physical lines it spans, nested functions and comments included.
    pub span_lines: u32,
    /// Code lines of every distinct function it reaches through established calls,
    /// itself included.
    pub inclusive_lower: u64,
    /// The same through every plausible call. Never less than the lower bound.
    pub inclusive_upper: u64,
    /// Its own lines plus each callee's tree weight per call site, established calls only.
    pub tree_weighted: u64,
    /// Whether the tree weight hit the numeric ceiling.
    pub saturated: bool,
    /// Distinct functions calling it through established calls, itself aside.
    pub fan_in: u32,
    /// Distinct functions it calls through established calls, itself aside.
    pub fan_out: u32,
    /// Distinct callees outside the indexed code.
    pub external_apis: u32,
    /// The longest chain of established calls below it.
    pub depth: u32,
    /// Its strongly connected component, numbered callees-first.
    pub scc: u32,
    /// How many functions share that component.
    pub scc_size: u32,
    /// Whether it can reach itself.
    pub recursive: bool,
}

/// The call graph, its two bounds, and every function's metrics.
#[derive(Clone, Debug, Default)]
pub struct CallGraph {
    nodes: Vec<SeamId>,
    slot: HashMap<SeamId, u32>,
    lower: Vec<Vec<(u32, u32)>>,
    upper: Vec<Vec<(u32, u32)>>,
    lower_in: Vec<Vec<u32>>,
    upper_in: Vec<Vec<u32>>,
    metrics: Vec<FunctionMetrics>,
}

impl CallGraph {
    /// Build the graph of every callable `options` includes, and size every function.
    #[must_use]
    pub fn build(index: &SeamIndex, options: &GraphOptions) -> Self {
        let membership = crate::config::evaluate(index, &options.configuration);
        let mut nodes: Vec<(String, SeamId)> = index
            .calls()
            .callables()
            .into_iter()
            .filter(|(id, _)| {
                index.node(*id).is_some() && membership.get(id) != Some(&ConfigMembership::Inactive)
            })
            .map(|(id, _)| {
                (
                    index.path(id).map(ToString::to_string).unwrap_or_default(),
                    id,
                )
            })
            .collect();
        // By path, so component numbers do not depend on the order ids were handed out.
        nodes.sort();
        let nodes: Vec<SeamId> = nodes.into_iter().map(|(_, id)| id).collect();
        let slot: HashMap<SeamId, u32> = nodes
            .iter()
            .enumerate()
            .filter_map(|(at, id)| Some((*id, u32::try_from(at).ok()?)))
            .collect();

        let mut lower = vec![BTreeMap::<u32, u32>::new(); nodes.len()];
        let mut upper = vec![BTreeMap::<u32, u32>::new(); nodes.len()];
        let mut externals = vec![BTreeSet::<&str>::new(); nodes.len()];
        for (from, id) in nodes.iter().enumerate() {
            for edge in index.edges().from_of_kind(*id, EdgeKind::Calls) {
                let resolution = edge.resolution.unwrap_or(Resolution::Unresolved);
                match &edge.to {
                    Endpoint::Resolved(target) => {
                        let Some(&to) = slot.get(target) else {
                            continue;
                        };
                        for (bound, map) in [(Bound::Lower, &mut lower), (Bound::Upper, &mut upper)]
                        {
                            if bound.follows(resolution)
                                && let Some(edges) = map.get_mut(from)
                            {
                                let sites = edges.entry(to).or_insert(0);
                                *sites = sites.saturating_add(1);
                            }
                        }
                    },
                    Endpoint::External { display } => {
                        if let Some(set) = externals.get_mut(from) {
                            set.insert(display.as_str());
                        }
                    },
                    Endpoint::Unresolved { .. } => {},
                }
            }
        }
        let flatten = |maps: Vec<BTreeMap<u32, u32>>| -> Vec<Vec<(u32, u32)>> {
            maps.into_iter()
                .map(|map| map.into_iter().collect())
                .collect()
        };
        let lower = flatten(lower);
        let upper = flatten(upper);
        let reverse = |adjacency: &[Vec<(u32, u32)>]| -> Vec<Vec<u32>> {
            let mut out = vec![Vec::new(); adjacency.len()];
            for (from, edges) in adjacency.iter().enumerate() {
                for (to, _) in edges {
                    if let (Some(list), Ok(from)) = (out.get_mut(*to as usize), u32::try_from(from))
                    {
                        list.push(from);
                    }
                }
            }
            out
        };
        let lower_in = reverse(&lower);
        let upper_in = reverse(&upper);

        let lines: Vec<(u32, u32)> = nodes
            .iter()
            .map(|id| {
                index
                    .calls()
                    .callable(*id)
                    .map_or((0, 0), |c| (c.code_lines, c.span_lines))
            })
            .collect();
        let weights: Vec<u64> = lines.iter().map(|(code, _)| u64::from(*code)).collect();
        let low = metrics::compute(&lower, &weights, options.max_dense);
        let high = metrics::compute(&upper, &weights, options.max_dense);

        let distinct = |list: Option<&Vec<u32>>, at: usize| -> u32 {
            let others = list
                .into_iter()
                .flatten()
                .filter(|other| **other as usize != at)
                .count();
            u32::try_from(others).unwrap_or(u32::MAX)
        };
        let metrics = nodes
            .iter()
            .enumerate()
            .map(|(at, id)| FunctionMetrics {
                id: *id,
                code_lines: lines.get(at).map_or(0, |(code, _)| *code),
                span_lines: lines.get(at).map_or(0, |(_, span)| *span),
                inclusive_lower: low.inclusive.get(at).copied().unwrap_or(0),
                inclusive_upper: high.inclusive.get(at).copied().unwrap_or(0),
                tree_weighted: low.tree.get(at).copied().unwrap_or(0),
                saturated: low.saturated.get(at).copied().unwrap_or(false),
                fan_in: distinct(lower_in.get(at), at),
                fan_out: u32::try_from(
                    lower
                        .get(at)
                        .into_iter()
                        .flatten()
                        .filter(|(to, _)| *to as usize != at)
                        .count(),
                )
                .unwrap_or(u32::MAX),
                external_apis: u32::try_from(externals.get(at).map_or(0, BTreeSet::len))
                    .unwrap_or(u32::MAX),
                depth: low.depth.get(at).copied().unwrap_or(0),
                scc: low.component.get(at).copied().unwrap_or(0),
                scc_size: low.size.get(at).copied().unwrap_or(1),
                recursive: low.recursive.get(at).copied().unwrap_or(false),
            })
            .collect();

        Self {
            nodes,
            slot,
            lower,
            upper,
            lower_in,
            upper_in,
            metrics,
        }
    }

    /// The metrics of one function, when the graph includes it.
    #[must_use]
    pub fn metrics(&self, id: SeamId) -> Option<&FunctionMetrics> {
        self.metrics.get(*self.slot.get(&id)? as usize)
    }

    /// Every function's metrics, ordered by path.
    #[must_use]
    pub fn all(&self) -> &[FunctionMetrics] {
        &self.metrics
    }

    /// How many functions the graph holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph holds no functions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The functions `id` calls directly, each with how many sites call it.
    #[must_use]
    pub fn callees(&self, id: SeamId, bound: Bound) -> Vec<(SeamId, u32)> {
        let Some(&at) = self.slot.get(&id) else {
            return Vec::new();
        };
        self.adjacency(bound)
            .get(at as usize)
            .into_iter()
            .flatten()
            .filter_map(|(to, sites)| Some((*self.nodes.get(*to as usize)?, *sites)))
            .collect()
    }

    /// The functions calling `id` directly.
    #[must_use]
    pub fn callers(&self, id: SeamId, bound: Bound) -> Vec<SeamId> {
        let Some(&at) = self.slot.get(&id) else {
            return Vec::new();
        };
        let incoming = match bound {
            Bound::Lower => &self.lower_in,
            Bound::Upper => &self.upper_in,
        };
        incoming
            .get(at as usize)
            .into_iter()
            .flatten()
            .filter_map(|from| self.nodes.get(*from as usize).copied())
            .collect()
    }

    /// Every function `id` reaches, in breadth-first order. `id` itself only when a cycle
    /// leads back to it.
    #[must_use]
    pub fn reach(&self, id: SeamId, bound: Bound) -> Vec<SeamId> {
        self.walk(id, |at| {
            self.adjacency(bound)
                .get(at)
                .into_iter()
                .flatten()
                .map(|(to, _)| *to as usize)
                .collect()
        })
    }

    /// Every function that reaches `id`, in breadth-first order.
    #[must_use]
    pub fn reached_by(&self, id: SeamId, bound: Bound) -> Vec<SeamId> {
        let incoming = match bound {
            Bound::Lower => &self.lower_in,
            Bound::Upper => &self.upper_in,
        };
        self.walk(id, |at| {
            incoming
                .get(at)
                .into_iter()
                .flatten()
                .map(|from| *from as usize)
                .collect()
        })
    }

    fn adjacency(&self, bound: Bound) -> &[Vec<(u32, u32)>] {
        match bound {
            Bound::Lower => &self.lower,
            Bound::Upper => &self.upper,
        }
    }

    fn walk(&self, id: SeamId, next: impl Fn(usize) -> Vec<usize>) -> Vec<SeamId> {
        let Some(&start) = self.slot.get(&id) else {
            return Vec::new();
        };
        let mut seen = vec![false; self.nodes.len()];
        let mut out = Vec::new();
        let mut queue = VecDeque::from([start as usize]);
        while let Some(at) = queue.pop_front() {
            for to in next(at) {
                if let Some(flag) = seen.get_mut(to)
                    && !*flag
                {
                    *flag = true;
                    out.extend(self.nodes.get(to).copied());
                    queue.push_back(to);
                }
            }
        }
        out
    }
}

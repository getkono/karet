//! Where structure most likely needs attention, each claim with its evidence.
//!
//! Five shapes, each a known smell with a mechanical test:
//!
//! - **Cycles** — functions that call one another. They cannot be understood, tested or
//!   moved apart, and the report shows one concrete path round each.
//! - **Hubs** — functions many others call. A change there reaches every caller.
//! - **Heavy entry points** — functions nobody here calls (or that are public) whose
//!   reach dwarfs their own size: thin facades over a lot of machinery.
//! - **Large leaves** — functions that call nothing here and are long anyway.
//! - **Unreachable** — non-public functions with no caller even in the upper bound. Those
//!   only tests call are listed apart, as are the ones a reflection or callback
//!   mechanism may reach, which the structural tier cannot see.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use super::Context;
use super::path_of;
use crate::calls::graph::Bound;
use crate::calls::graph::CallGraph;
use crate::calls::graph::FunctionMetrics;
use crate::calls::tree::TreeLimits;
use crate::calls::tree::call_tree;
use crate::id::SeamId;
use crate::model::Visibility;

/// One function singled out, with the number that singled it out and the evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Hotspot {
    /// The function's identity.
    pub path: String,
    /// The file it is written in.
    pub file: String,
    /// Its first line, 1-based.
    pub line: u32,
    /// The measure that put it here.
    pub value: u64,
    /// That measure in words, with the threshold it passed.
    pub why: String,
    /// The top of its call tree, rendered, as evidence.
    pub excerpt: String,
}

/// One group of functions calling one another.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Cycle {
    /// Every member, by identity.
    pub members: Vec<String>,
    /// One concrete path from the first member back to itself.
    pub path: Vec<String>,
    /// The members' own lines, summed.
    pub code_lines: u64,
}

/// Every hotspot list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Hotspots {
    /// Groups of functions calling one another, largest first.
    pub cycles: Vec<Cycle>,
    /// Functions with the most callers.
    pub hubs: Vec<Hotspot>,
    /// Entry points reaching far more code than they hold.
    pub heavy_entry_points: Vec<Hotspot>,
    /// Long functions that call nothing here.
    pub large_leaves: Vec<Hotspot>,
    /// Non-public functions nothing calls.
    pub unreachable: Vec<Hotspot>,
    /// Non-public functions only tests call.
    pub test_only: Vec<Hotspot>,
}

/// Names that are entry points by convention, called by a runtime rather than by code.
const ENTRY_NAMES: &[&str] = &["main", "__main__", "__init__", "new", "default", "drop"];

pub(super) fn find(
    context: &Context<'_>,
    measured: &[&FunctionMetrics],
    testing: &CallGraph,
    test_gated: &HashSet<SeamId>,
) -> Hotspots {
    let top = context.options.top_n;
    let thresholds = context.thresholds;
    let mut hubs: Vec<&FunctionMetrics> = measured
        .iter()
        .copied()
        .filter(|m| m.fan_in >= thresholds.hub_fan_in)
        .collect();
    hubs.sort_by_key(|m| std::cmp::Reverse(m.fan_in));

    let mut heavy: Vec<(&FunctionMetrics, f64)> = measured
        .iter()
        .copied()
        .filter(|m| m.code_lines > 0 && m.inclusive_lower >= thresholds.heavy_inclusive)
        .filter(|m| m.fan_in == 0 || visibility(context, m.id) == Some(Visibility::Public))
        .map(|m| (m, m.inclusive_lower as f64 / f64::from(m.code_lines)))
        .filter(|(_, ratio)| *ratio >= thresholds.heavy_ratio)
        .collect();
    heavy.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut leaves: Vec<&FunctionMetrics> = measured
        .iter()
        .copied()
        .filter(|m| m.fan_out == 0 && m.code_lines >= thresholds.large_leaf_lines)
        .collect();
    leaves.sort_by_key(|m| std::cmp::Reverse(m.code_lines));

    let mut unreachable = Vec::new();
    let mut test_only = Vec::new();
    // A function calling only itself is still called by nobody.
    let called = |graph: &CallGraph, id: SeamId| {
        graph
            .callers(id, Bound::Upper)
            .into_iter()
            .any(|caller| caller != id)
    };
    let contracts = contract_names(context);
    for m in measured {
        let named_entry = context
            .index
            .node(m.id)
            .is_some_and(|node| ENTRY_NAMES.contains(&node.name.as_str()));
        let hidden = visibility(context, m.id).is_some_and(|v| v < Visibility::Public);
        // A test is called by its runner, not by code, so silence proves nothing there.
        if named_entry
            || !hidden
            || test_gated.contains(&m.id)
            || through_contract(context, &contracts, m.id)
            || called(context.graph, m.id)
        {
            continue;
        }
        if called(testing, m.id) {
            test_only.push(*m);
        } else {
            unreachable.push(*m);
        }
    }
    for list in [&mut unreachable, &mut test_only] {
        list.sort_by(|a, b| {
            b.code_lines
                .cmp(&a.code_lines)
                .then_with(|| path_of(context.index, a.id).cmp(&path_of(context.index, b.id)))
        });
    }

    Hotspots {
        cycles: cycles(context, measured),
        hubs: hubs
            .into_iter()
            .take(top)
            .map(|m| {
                spot(context, m, u64::from(m.fan_in), format!(
                    "called from {} functions (hub threshold {})",
                    m.fan_in, thresholds.hub_fan_in
                ))
            })
            .collect(),
        heavy_entry_points: heavy
            .into_iter()
            .take(top)
            .map(|(m, ratio)| {
                spot(context, m, m.inclusive_lower, format!(
                    "reaches {} lines from {} of its own ({ratio:.1}×; threshold {:.1}× and {} lines)",
                    m.inclusive_lower, m.code_lines, thresholds.heavy_ratio, thresholds.heavy_inclusive
                ))
            })
            .collect(),
        large_leaves: leaves
            .into_iter()
            .take(top)
            .map(|m| {
                spot(context, m, u64::from(m.code_lines), format!(
                    "{} lines calling nothing here (large-leaf threshold {})",
                    m.code_lines, thresholds.large_leaf_lines
                ))
            })
            .collect(),
        unreachable: unreachable
            .into_iter()
            .map(|m| {
                spot(context, m, u64::from(m.code_lines), format!(
                    "{} lines with no caller, even a plausible one; likely dead, unless it \
                     is passed as a value (`.map(f)`), called inside a macro, or reached by \
                     reflection",
                    m.code_lines
                ))
            })
            .collect(),
        test_only: test_only
            .into_iter()
            .map(|m| {
                spot(context, m, u64::from(m.code_lines), format!(
                    "{} lines called only from tests — or from inside a macro, whose \
                     arguments are not read as calls",
                    m.code_lines
                ))
            })
            .collect(),
    }
}

/// Whether a function may be called through a contract rather than by name.
///
/// A trait's implementation, an override, and a method sharing its name with some
/// contract's declaration are reached by dispatch the structural tier may not see —
/// `Display::fmt` is called by `format!`, not by anything here — so their having no
/// caller says nothing about whether they are dead.
fn through_contract(context: &Context<'_>, contracts: &HashSet<&str>, id: SeamId) -> bool {
    let index = context.index;
    let Some(node) = index.node(id) else {
        return false;
    };
    let in_implementation = node
        .parent
        .and_then(|parent| index.node(parent))
        .is_some_and(|parent| parent.kind == crate::model::NodeKind::Implementation);
    let overrides = ["override", "default-method"]
        .iter()
        .any(|subtype| node.has_subtype(crate::model::Lens::Substitution, subtype));
    let in_contract = node
        .parent
        .and_then(|parent| index.node(parent))
        .is_some_and(|parent| parent.kind == crate::model::NodeKind::Interface);
    in_implementation || overrides || (!in_contract && contracts.contains(node.name.as_str()))
}

/// Every name some contract declares a method under.
fn contract_names<'a>(context: &Context<'a>) -> HashSet<&'a str> {
    let index = context.index;
    index
        .nodes()
        .filter(|node| {
            node.parent
                .and_then(|parent| index.node(parent))
                .is_some_and(|parent| parent.kind == crate::model::NodeKind::Interface)
        })
        .map(|node| node.name.as_str())
        .collect()
}

fn visibility(context: &Context<'_>, id: SeamId) -> Option<Visibility> {
    context
        .index
        .node(id)
        .and_then(crate::model::Node::effective_visibility)
}

fn spot(context: &Context<'_>, m: &FunctionMetrics, value: u64, why: String) -> Hotspot {
    let node = context.index.node(m.id);
    let excerpt = call_tree(
        context.index,
        context.graph,
        m.id,
        Bound::Lower,
        TreeLimits {
            depth: Some(context.options.excerpt_depth),
            width: Some(context.options.excerpt_width),
        },
    )
    .map(|tree| tree.render())
    .unwrap_or_default();
    Hotspot {
        path: path_of(context.index, m.id),
        file: node
            .and_then(|node| context.index.file_path(node.location.file))
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        line: node.map_or(0, |node| node.location.range.start.line.saturating_add(1)),
        value,
        why,
        excerpt,
    }
}

/// Every group of two or more functions calling one another that touches the scope.
///
/// Grouped over the whole graph, not the scope: a cycle half inside it is still a whole
/// cycle, and showing only its in-scope half would show no cycle at all.
fn cycles(context: &Context<'_>, measured: &[&FunctionMetrics]) -> Vec<Cycle> {
    let touched: HashSet<u32> = measured
        .iter()
        .filter(|m| m.scc_size > 1)
        .map(|m| m.scc)
        .collect();
    let mut groups: HashMap<u32, Vec<&FunctionMetrics>> = HashMap::new();
    for m in context
        .graph
        .all()
        .iter()
        .filter(|m| touched.contains(&m.scc))
    {
        groups.entry(m.scc).or_default().push(m);
    }
    let mut out: Vec<Cycle> = groups
        .into_values()
        .filter_map(|mut members| {
            members.sort_by_key(|m| path_of(context.index, m.id));
            let ids: HashSet<SeamId> = members.iter().map(|m| m.id).collect();
            let start = members.first()?.id;
            Some(Cycle {
                path: cycle_path(context.graph, start, &ids)
                    .into_iter()
                    .map(|id| path_of(context.index, id))
                    .collect(),
                code_lines: members.iter().map(|m| u64::from(m.code_lines)).sum(),
                members: members
                    .iter()
                    .map(|m| path_of(context.index, m.id))
                    .collect(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.members.cmp(&b.members))
    });
    out.truncate(context.options.top_n);
    out
}

/// The shortest path of established calls from `start` back to itself within `members`.
fn cycle_path(graph: &CallGraph, start: SeamId, members: &HashSet<SeamId>) -> Vec<SeamId> {
    let mut previous: HashMap<SeamId, SeamId> = HashMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(at) = queue.pop_front() {
        for (next, _) in graph.callees(at, Bound::Lower) {
            if !members.contains(&next) {
                continue;
            }
            if next == start {
                // Walk back from the last step to the start, then read it forwards.
                let mut back = Vec::new();
                let mut cursor = at;
                while cursor != start {
                    back.push(cursor);
                    let Some(earlier) = previous.get(&cursor) else {
                        break;
                    };
                    cursor = *earlier;
                }
                back.reverse();
                let mut path = vec![start];
                path.extend(back);
                path.push(start);
                return path;
            }
            if next != start && !previous.contains_key(&next) {
                previous.insert(next, at);
                queue.push_back(next);
            }
        }
    }
    vec![start]
}

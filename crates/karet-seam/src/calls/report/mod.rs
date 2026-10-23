//! A structural report over the call graph, written for a reader that will act on it.
//!
//! The intended reader is a person or a language model deciding where code structure and
//! encapsulation could improve, so every claim carries its evidence: a function's
//! identity and location to cite, the numbers behind a ranking, a call-tree excerpt
//! showing *why* something is large, and a ledger of how much of it is guesswork.
//!
//! Unlike the seam view, which deliberately never ranks, a report ranks: that is what
//! makes it short enough to act on. The cut-offs are recorded in the report itself
//! ([`Thresholds`]) so a reader can see exactly what "large" meant here.
//!
//! Everything is computed under one configuration — by default the one that leaves test
//! code out — and over a scope, which is any node set a query selected.

mod coupling;
mod hotspots;
mod ledger;
mod markdown;

pub(crate) use markdown::code;

#[cfg(test)]
mod tests;

use std::collections::HashSet;

pub use coupling::Envy;
pub use coupling::InternalReach;
pub use coupling::ModuleCoupling;
pub use coupling::ModuleEdge;
pub use coupling::ModuleRow;
pub use hotspots::Cycle;
pub use hotspots::Hotspot;
pub use hotspots::Hotspots;
pub use ledger::LanguageLedger;
pub use ledger::PrecisionLedger;
pub use ledger::Uncertain;

use super::graph::CallGraph;
use super::graph::FunctionMetrics;
use super::graph::GraphOptions;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::NodeKind;

/// How much a report shows, and where its cut-offs fall.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportOptions {
    /// At most this many entries per hotspot list.
    pub top_n: usize,
    /// The percentile a value must reach to count as a hotspot, as a fraction (0.95).
    pub percentile: f64,
    /// Levels of call tree shown as evidence under each hotspot.
    pub excerpt_depth: usize,
    /// Callees shown per level of an excerpt.
    pub excerpt_width: usize,
    /// At most this many entries in the precision ledger's least-certain list.
    pub least_certain_n: usize,
}

impl Default for ReportOptions {
    fn default() -> Self {
        Self {
            top_n: 20,
            percentile: 0.95,
            excerpt_depth: 3,
            excerpt_width: 8,
            least_certain_n: 50,
        }
    }
}

/// The cut-offs a report actually used, so a reader can see what "large" meant.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Thresholds {
    /// At most this many entries per hotspot list.
    pub top_n: usize,
    /// At most this many entries in the precision ledger's least-certain list.
    pub least_certain_n: usize,
    /// The percentile used, as a fraction.
    pub percentile: f64,
    /// Fan-in at or above which a function is a hub.
    pub hub_fan_in: u32,
    /// Inclusive-to-own ratio at or above which an entry point is heavy.
    pub heavy_ratio: f64,
    /// Inclusive lines an entry point needs before its ratio matters.
    pub heavy_inclusive: u64,
    /// Own lines at or above which a function with no callees is a large leaf.
    pub large_leaf_lines: u32,
}

/// Counts over the whole scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Summary {
    /// Functions measured.
    pub functions: usize,
    /// Their own code lines, summed.
    pub code_lines: u64,
    /// Modules holding them.
    pub modules: usize,
    /// Packages holding them.
    pub packages: usize,
    /// Functions that reach themselves.
    pub recursive: usize,
    /// Groups of two or more functions calling one another.
    pub cycles: usize,
    /// The function reaching the most code, and how much.
    pub largest: Option<(String, u64)>,
}

/// One function's row in the report.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FunctionRow {
    /// Its identity, the citation unit.
    pub path: String,
    /// The file it is written in.
    pub file: String,
    /// Its first line, 1-based.
    pub line: u32,
    /// The language it is written in.
    pub language: Option<String>,
    /// Its effective visibility, when known.
    pub visibility: Option<String>,
    /// Its measures.
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub metrics: RowMetrics,
}

/// The measures on a [`FunctionRow`], without the index-local id.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct RowMetrics {
    /// Own code lines.
    pub code_lines: u32,
    /// Physical lines spanned.
    pub span_lines: u32,
    /// Distinct lines reached through established calls.
    pub inclusive_lower: u64,
    /// Distinct lines reached through every plausible call.
    pub inclusive_upper: u64,
    /// How much of the upper bound is guesswork.
    pub gap: u64,
    /// Lines per call site, established calls only.
    pub tree_weighted: u64,
    /// Whether the tree weight hit the numeric ceiling.
    pub saturated: bool,
    /// Distinct established callers.
    pub fan_in: u32,
    /// Distinct established callees.
    pub fan_out: u32,
    /// Distinct callees outside the indexed code.
    pub external_apis: u32,
    /// Longest established call chain below it.
    pub depth: u32,
    /// Its strongly connected component.
    pub scc: u32,
    /// Functions sharing that component.
    pub scc_size: u32,
}

impl From<&FunctionMetrics> for RowMetrics {
    fn from(m: &FunctionMetrics) -> Self {
        Self {
            code_lines: m.code_lines,
            span_lines: m.span_lines,
            inclusive_lower: m.inclusive_lower,
            inclusive_upper: m.inclusive_upper,
            gap: m.inclusive_upper.saturating_sub(m.inclusive_lower),
            tree_weighted: m.tree_weighted,
            saturated: m.saturated,
            fan_in: m.fan_in,
            fan_out: m.fan_out,
            external_apis: m.external_apis,
            depth: m.depth,
            scc: m.scc,
            scc_size: m.scc_size,
        }
    }
}

/// The whole report.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CallReport {
    /// The configuration everything was measured under.
    pub configuration: String,
    /// The cut-offs the rankings used.
    pub thresholds: Thresholds,
    /// Counts over the scope.
    pub summary: Summary,
    /// Where structure is most likely to need attention, with evidence.
    pub hotspots: Hotspots,
    /// How modules depend on one another through calls.
    pub coupling: ModuleCoupling,
    /// How much of every number is established, and how much is a bound.
    pub ledger: PrecisionLedger,
    /// Every function in scope, most inclusive first.
    pub functions: Vec<FunctionRow>,
}

/// Build the report over the functions in `scope`.
///
/// `scope` is any node set — a query result, a subtree — and the functions among it are
/// what is reported on. An empty scope means the whole index. Callers and callees outside
/// the scope still count towards every measure; the scope decides only what is listed.
#[must_use]
pub fn report(
    index: &SeamIndex,
    graph_options: &GraphOptions,
    scope: &[SeamId],
    options: &ReportOptions,
) -> CallReport {
    let graph = CallGraph::build(index, graph_options);
    let in_scope: HashSet<SeamId> = scope.iter().copied().collect();
    let mut measured: Vec<&FunctionMetrics> = graph
        .all()
        .iter()
        .filter(|m| in_scope.is_empty() || in_scope.contains(&m.id))
        .collect();
    measured.sort_by(|a, b| {
        b.inclusive_lower
            .cmp(&a.inclusive_lower)
            .then_with(|| path_of(index, a.id).cmp(&path_of(index, b.id)))
    });

    // Over the whole graph, not the scope: a narrowed report ranks against the same
    // yardstick as the whole one, so "a hub" means the same thing in both.
    let everything: Vec<&FunctionMetrics> = graph.all().iter().collect();
    let thresholds = thresholds(&everything, options);
    let context = Context {
        index,
        graph: &graph,
        options,
        thresholds: &thresholds,
    };
    let configuration = &graph_options.configuration;
    let tests_included = configuration.env.flags.contains("test");
    let built_with_tests;
    let testing = if tests_included {
        &graph
    } else {
        built_with_tests = CallGraph::build(
            index,
            &GraphOptions {
                configuration: configuration.clone().for_target("test"),
                ..graph_options.clone()
            },
        );
        &built_with_tests
    };
    // When tests are built, test code is in the graph, and a test function is called by
    // the test runner rather than by code: a function gated out without tests is an entry
    // point, never "unreachable". Without tests no such function is measured at all.
    let test_gated: HashSet<SeamId> = if tests_included {
        let mut production = configuration.clone();
        production.env.flags.remove("test");
        let membership = crate::config::evaluate(index, &production);
        graph
            .all()
            .iter()
            .map(|m| m.id)
            .filter(|id| membership.get(id) == Some(&ConfigMembership::Inactive))
            .collect()
    } else {
        HashSet::new()
    };

    CallReport {
        // A test build already says so in its name.
        configuration: configuration.name.clone(),
        summary: summary(index, &measured),
        hotspots: hotspots::find(&context, &measured, testing, &test_gated),
        coupling: coupling::measure(index, &graph, &measured),
        ledger: ledger::account(index, &measured, options.least_certain_n),
        functions: measured.iter().map(|m| row(index, m)).collect(),
        thresholds,
    }
}

/// What every part of the report reads.
pub(crate) struct Context<'a> {
    pub(crate) index: &'a SeamIndex,
    pub(crate) graph: &'a CallGraph,
    pub(crate) options: &'a ReportOptions,
    pub(crate) thresholds: &'a Thresholds,
}

/// A function's identity as text.
pub(crate) fn path_of(index: &SeamIndex, id: SeamId) -> String {
    index.path(id).map(ToString::to_string).unwrap_or_default()
}

/// The nearest module or package around a node.
pub(crate) fn module_of(index: &SeamIndex, id: SeamId) -> Option<SeamId> {
    index.ancestors(id).into_iter().find(|up| {
        index
            .node(*up)
            .is_some_and(|node| matches!(node.kind, NodeKind::Module | NodeKind::Package))
    })
}

/// One function's row.
fn row(index: &SeamIndex, metrics: &FunctionMetrics) -> FunctionRow {
    let node = index.node(metrics.id);
    let file = node
        .and_then(|node| index.file_path(node.location.file))
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    FunctionRow {
        path: path_of(index, metrics.id),
        language: node
            .and_then(|node| index.file_path(node.location.file))
            .and_then(karet_treesitter::language_name_from_path)
            .map(str::to_owned),
        file,
        line: node.map_or(0, |node| node.location.range.start.line.saturating_add(1)),
        visibility: node
            .and_then(crate::model::Node::effective_visibility)
            .map(|v| v.name().to_owned()),
        metrics: RowMetrics::from(metrics),
    }
}

/// Counts over the scope.
fn summary(index: &SeamIndex, measured: &[&FunctionMetrics]) -> Summary {
    let modules: HashSet<SeamId> = measured
        .iter()
        .filter_map(|m| module_of(index, m.id))
        .collect();
    let packages: HashSet<SeamId> = measured
        .iter()
        .filter_map(|m| index.ancestors(m.id).last().copied())
        .collect();
    let cycles: HashSet<u32> = measured
        .iter()
        .filter(|m| m.scc_size > 1)
        .map(|m| m.scc)
        .collect();
    Summary {
        functions: measured.len(),
        code_lines: measured.iter().map(|m| u64::from(m.code_lines)).sum(),
        modules: modules.len(),
        packages: packages.len(),
        recursive: measured.iter().filter(|m| m.recursive).count(),
        cycles: cycles.len(),
        largest: measured
            .first()
            .map(|m| (path_of(index, m.id), m.inclusive_lower)),
    }
}

/// The value at `percentile` of `values`, nearest-rank.
pub(crate) fn percentile(mut values: Vec<f64>, percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let rank = (percentile.clamp(0.0, 1.0) * values.len() as f64).ceil() as usize;
    values
        .get(rank.saturating_sub(1).min(values.len() - 1))
        .copied()
        .unwrap_or(0.0)
}

/// The cut-offs: a percentile of every function, never below a floor that keeps a small
/// codebase from calling its median function a hotspot.
fn thresholds(measured: &[&FunctionMetrics], options: &ReportOptions) -> Thresholds {
    let p = options.percentile;
    let fan_in = percentile(measured.iter().map(|m| f64::from(m.fan_in)).collect(), p);
    let leaf = percentile(
        measured
            .iter()
            .filter(|m| m.fan_out == 0)
            .map(|m| f64::from(m.code_lines))
            .collect(),
        p,
    );
    let ratio = percentile(
        measured
            .iter()
            .filter(|m| m.code_lines > 0)
            .map(|m| m.inclusive_lower as f64 / f64::from(m.code_lines))
            .collect(),
        p,
    );
    Thresholds {
        top_n: options.top_n,
        least_certain_n: options.least_certain_n,
        percentile: p,
        hub_fan_in: (fan_in.ceil() as u32).max(5),
        heavy_ratio: ratio.max(2.0),
        heavy_inclusive: 200,
        large_leaf_lines: (leaf.ceil() as u32).max(50),
    }
}

//! How modules depend on one another through the calls between them.
//!
//! A module is the nearest module or package around a function. Only established calls
//! count: coupling built from guesses would report dependencies that may not exist.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::module_of;
use super::path_of;
use super::visibility_of;
use crate::calls::graph::Bound;
use crate::calls::graph::CallGraph;
use crate::calls::graph::FunctionMetrics;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::Visibility;

/// One module's coupling.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ModuleRow {
    /// The module's identity.
    pub path: String,
    /// Functions in scope inside it.
    pub functions: usize,
    /// Their own code lines, summed.
    pub code_lines: u64,
    /// Afferent coupling: distinct other modules calling into it.
    pub afferent: usize,
    /// Efferent coupling: distinct other modules it calls into.
    pub efferent: usize,
    /// Efferent over total coupling: 0 is depended on and depends on nothing, 1 the
    /// reverse. `None` for a module coupled to nothing.
    pub instability: Option<f64>,
}

/// Calls from one module into another.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ModuleEdge {
    /// The calling module.
    pub from: String,
    /// The called module.
    pub to: String,
    /// Call sites crossing.
    pub calls: u32,
    /// Distinct calling functions.
    pub callers: usize,
}

/// A call crossing into another module to reach something that module does not export.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct InternalReach {
    /// The calling function.
    pub caller: String,
    /// The function reached.
    pub callee: String,
    /// How visible the callee is: anything short of public.
    pub visibility: String,
}

/// A function that calls into one other module more than into its own.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Envy {
    /// The function.
    pub function: String,
    /// Its call sites into its own module.
    pub own_calls: u32,
    /// The module it leans on.
    pub other: String,
    /// Its call sites into that module.
    pub other_calls: u32,
}

/// Module-level coupling over the scope.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ModuleCoupling {
    /// Every module holding a function in scope, most coupled first.
    pub modules: Vec<ModuleRow>,
    /// Every module-to-module dependency, most calls first.
    pub edges: Vec<ModuleEdge>,
    /// Cross-module calls into non-public items.
    pub internal_reach: Vec<InternalReach>,
    /// Functions leaning on another module more than their own.
    pub envious: Vec<Envy>,
}

/// A function calling into another module this many times is worth naming when it
/// also calls there more than at home.
const ENVY_MINIMUM: u32 = 3;

/// Measure module coupling, listing what concerns the functions in scope.
///
/// Every function's calls count towards the coupling numbers — a module's afferent
/// coupling is who calls into it, wherever they are — and the scope decides only which
/// modules, edges and findings are listed.
pub(super) fn measure(
    index: &SeamIndex,
    graph: &CallGraph,
    measured: &[&FunctionMetrics],
) -> ModuleCoupling {
    let listed: BTreeSet<SeamId> = measured.iter().map(|m| m.id).collect();
    let mut rows: BTreeMap<SeamId, (usize, u64)> = BTreeMap::new();
    let mut edges: BTreeMap<(SeamId, SeamId), (u32, BTreeSet<SeamId>)> = BTreeMap::new();
    let mut internal_reach = Vec::new();
    let mut envious = Vec::new();

    for function in graph.all() {
        let Some(home) = module_of(index, function.id) else {
            continue;
        };
        let in_scope = listed.contains(&function.id);
        if in_scope {
            let row = rows.entry(home).or_default();
            row.0 += 1;
            row.1 = row.1.saturating_add(u64::from(function.code_lines));
        }

        let mut per_module: BTreeMap<SeamId, u32> = BTreeMap::new();
        for (callee, sites) in graph.callees(function.id, Bound::Lower) {
            if callee == function.id {
                continue;
            }
            let Some(there) = module_of(index, callee) else {
                continue;
            };
            let count = per_module.entry(there).or_default();
            *count = count.saturating_add(sites);
            if there == home {
                continue;
            }
            let edge = edges.entry((home, there)).or_default();
            edge.0 = edge.0.saturating_add(sites);
            edge.1.insert(function.id);
            if !in_scope {
                continue;
            }
            if let Some(visibility) =
                visibility_of(index, callee).filter(|visibility| *visibility < Visibility::Public)
            {
                internal_reach.push(InternalReach {
                    caller: path_of(index, function.id),
                    callee: path_of(index, callee),
                    visibility: visibility.name().to_owned(),
                });
            }
        }
        let own = per_module.get(&home).copied().unwrap_or(0);
        // The most-called other module; on a tie, the first by path.
        if let Some((other, calls)) = per_module
            .iter()
            .filter(|(module, _)| in_scope && **module != home)
            .max_by(|a, b| {
                a.1.cmp(b.1)
                    .then_with(|| path_of(index, *b.0).cmp(&path_of(index, *a.0)))
            })
            && *calls >= ENVY_MINIMUM
            && *calls > own
        {
            envious.push(Envy {
                function: path_of(index, function.id),
                own_calls: own,
                other: path_of(index, *other),
                other_calls: *calls,
            });
        }
    }

    let afferent = |module: SeamId| {
        edges
            .keys()
            .filter(|(from, to)| *to == module && *from != module)
            .count()
    };
    let efferent = |module: SeamId| {
        edges
            .keys()
            .filter(|(from, to)| *from == module && *to != module)
            .count()
    };
    let scoped_modules: BTreeSet<SeamId> = rows.keys().copied().collect();
    let mut modules: Vec<ModuleRow> = rows
        .into_iter()
        .map(|(module, (functions, code_lines))| {
            let (ca, ce) = (afferent(module), efferent(module));
            ModuleRow {
                path: path_of(index, module),
                functions,
                code_lines,
                afferent: ca,
                efferent: ce,
                instability: (ca + ce > 0).then(|| ce as f64 / (ca + ce) as f64),
            }
        })
        .collect();
    modules.sort_by(|a, b| {
        (b.afferent + b.efferent)
            .cmp(&(a.afferent + a.efferent))
            .then_with(|| a.path.cmp(&b.path))
    });

    let mut edges: Vec<ModuleEdge> = edges
        .into_iter()
        .filter(|((from, to), _)| scoped_modules.contains(from) || scoped_modules.contains(to))
        .map(|((from, to), (calls, callers))| ModuleEdge {
            from: path_of(index, from),
            to: path_of(index, to),
            calls,
            callers: callers.len(),
        })
        .collect();
    edges.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to)))
    });
    internal_reach.sort_by(|a, b| (&a.caller, &a.callee).cmp(&(&b.caller, &b.callee)));
    internal_reach.dedup();
    envious.sort_by(|a, b| {
        b.other_calls
            .cmp(&a.other_calls)
            .then_with(|| a.function.cmp(&b.function))
    });

    ModuleCoupling {
        modules,
        edges,
        internal_reach,
        envious,
    }
}

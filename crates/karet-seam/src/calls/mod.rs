//! The call tier: who calls what, and how much code each function pulls in.
//!
//! Three steps, each with its own owner:
//!
//! 1. **Extraction** (per file, cached). A language contributes one tree-sitter query —
//!    its [call query](crate::lang::SeamLanguage::call_query) — and nothing else. The
//!    query marks calls, bodies, comments and imports with the captures listed in
//!    [`site::CAPTURES`]; the neutral pass in [`extract`] attributes each call to the
//!    innermost callable around it and counts each callable's code lines.
//! 2. **Linking** (whole index, derived). [`link`] resolves every recorded call by name
//!    and scope into [`EdgeKind::Calls`](crate::EdgeKind::Calls) edges, each tagged with
//!    how it was [resolved](crate::Resolution). A callee nobody indexed is
//!    [`External`](crate::Endpoint::External), not missing.
//! 3. **Measuring** (per configuration). [`CallGraph`] condenses recursion, then gives
//!    every function its own lines, the distinct lines it reaches, and its tree weight —
//!    each as a lower bound over established calls and an upper bound over plausible ones.
//!
//! # The capture convention
//!
//! | Capture | Marks |
//! |---|---|
//! | `@call` | the whole call — required; matches sharing its span merge |
//! | `@call.name` | the callee's name; absent when the callee is computed |
//! | `@call.qualifier` | the scope written before the name (`a::b` in `a::b::f()`) |
//! | `@call.receiver` | the receiver of a method call (`x` in `x.f()`) |
//! | `@call.self`, `@call.super` | the receiver is the current instance, or its parent |
//! | `@call.new` | the call constructs a value |
//! | `@call.external` | a builtin or prelude callee, usually via `#any-of?` |
//! | `@call.weak` | only probably a call — counted in the upper bound only |
//! | `@body` | a callable's body, which tells a definition from a declaration |
//! | `@comment` | a comment or documentation string; its lines are not code |
//! | `@import.path`, `@import.prefix`, `@import.alias`, `@import.glob` | imports |
//! | `@import.head` | the import binds its path's first segment (Python's `import a.b`) |
//!
//! # What the numbers do not claim
//!
//! Calls are resolved by name, so a function passed as a value, a call through a
//! function pointer, and anything reflection reaches are invisible. Closures are not
//! nodes: their lines and calls are their enclosing function's. A site with more than
//! [`resolve::MAX_CANDIDATES`] candidates is counted in neither bound. The precision of
//! every number is reported beside it rather than assumed.

pub mod extract;
pub mod graph;
pub mod lines;
mod metrics;
pub mod resolve;
mod scope;
pub mod site;
pub mod table;

pub use graph::Bound;
pub use graph::CallGraph;
pub use graph::FunctionMetrics;
pub use graph::GraphOptions;
use rayon::prelude::*;
pub use resolve::CallResolver;
pub use resolve::NameResolver;
pub use resolve::Resolved;
pub use table::CallTable;

use crate::edge::Edge;
use crate::edge::EdgeKind;
use crate::index::SeamIndex;

/// Resolve every recorded call into edges, replacing whatever call edges were there.
///
/// Derived over the whole index because a call in one file names a function in
/// another: the edges are recomputed rather than patched whenever the index changes.
pub fn link(index: &mut SeamIndex) {
    let edges = {
        let resolver = NameResolver::new(index);
        resolve(index, &[&resolver])
    };
    index.edges_mut().remove_kind(EdgeKind::Calls);
    index.edges_mut().insert_all(edges);
}

/// Resolve every recorded call through a chain of resolvers, first answer wins.
///
/// A site no resolver answers for is [`Unresolved`](crate::Endpoint::Unresolved). The
/// edges come back in callable order, then site order, so two runs over the same index
/// produce the same edge list.
#[must_use]
pub fn resolve(index: &SeamIndex, resolvers: &[&dyn CallResolver]) -> Vec<Edge> {
    index
        .calls()
        .callables()
        .par_iter()
        .flat_map_iter(|(caller, callable)| {
            callable.sites.iter().flat_map(move |site| {
                let resolved = resolvers
                    .iter()
                    .find_map(|resolver| resolver.resolve(index, *caller, site))
                    .unwrap_or_else(|| Resolved::unresolved(site.name.clone()));
                resolved
                    .targets
                    .into_iter()
                    .map(move |(to, resolution)| Edge {
                        kind: EdgeKind::Calls,
                        from: *caller,
                        to,
                        site: Some(site.range),
                        resolution: Some(resolution),
                    })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;

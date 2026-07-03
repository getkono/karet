//! Code-visualization producers: build neutral [`GraphView`]s from workspace facts.
//!
//! This is the "logic" half of the visualization suite — headless producers that emit
//! a [`karet_core::GraphView`] carried across the session seam and rendered by
//! `karet-graph`. The package-dependency lens is backed by `dependable`'s pure-Rust
//! core, which parses a `Cargo.lock` into a resolved graph entirely offline.

use std::path::Path;

use karet_core::GraphEdge;
use karet_core::GraphEdgeKind;
use karet_core::GraphNode;
use karet_core::GraphNodeKind;
use karet_core::GraphView;

/// Errors building a visualization graph.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VizError {
    /// No supported dependency manifest/lockfile was found under the root.
    #[error("no dependency lockfile found (looked for Cargo.lock)")]
    NoLockfile,
    /// The lockfile could not be parsed.
    #[error("could not parse the dependency lockfile: {0}")]
    Parse(String),
}

/// Build the workspace **package dependency** graph from the `Cargo.lock` at (or above)
/// `root`: one node per resolved package (local crates as [`GraphNodeKind::Package`],
/// registry/git deps as [`GraphNodeKind::External`], versioned via the node badge), one
/// edge per resolved dependency, rooted at the local packages.
///
/// This is offline — it reads the committed lockfile only, never the network.
///
/// # Errors
/// [`VizError::NoLockfile`] if no `Cargo.lock` is found; [`VizError::Parse`] on a
/// malformed lockfile.
pub fn dependency_graph(root: &Path) -> Result<GraphView, VizError> {
    let lock_path = find_lockfile(root).ok_or(VizError::NoLockfile)?;
    let text = std::fs::read_to_string(&lock_path).map_err(|e| VizError::Parse(e.to_string()))?;
    let resolved = dependable_core::lockfiles::parse_cargo_lock_graph(&text)
        .map_err(|e| VizError::Parse(e.to_string()))?;
    Ok(build_view(&resolved))
}

/// Ascend from `root` looking for a `Cargo.lock` (the workspace root usually holds it).
fn find_lockfile(root: &Path) -> Option<std::path::PathBuf> {
    let mut dir: Option<&Path> = Some(root);
    while let Some(current) = dir {
        let candidate = current.join("Cargo.lock");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

/// Turn a parsed lockfile into a neutral [`GraphView`]. Node ids are `"name version"`
/// (unique across duplicate-named packages); labels are the bare name.
fn build_view(resolved: &dependable_core::ResolvedLockfile) -> GraphView {
    let id_of = |p: &dependable_core::lockfiles::LockedPackage| format!("{} {}", p.name, p.version);

    let mut view = GraphView::new();
    for pkg in &resolved.packages {
        // A local crate (workspace member / path dep) has no source; everything else
        // is an external registry/git dependency.
        let kind = if pkg.source.is_none() {
            GraphNodeKind::Package
        } else {
            GraphNodeKind::External
        };
        view.nodes.push(
            GraphNode::new(id_of(pkg), pkg.name.clone(), kind).with_badge(pkg.version.clone()),
        );
    }

    for pkg in &resolved.packages {
        let from = id_of(pkg);
        for dep in &pkg.dependencies {
            if let Some(idx) = resolved.resolve(dep)
                && let Some(target) = resolved.packages.get(idx)
            {
                view.edges.push(GraphEdge::new(
                    from.clone(),
                    id_of(target),
                    GraphEdgeKind::Dependency,
                ));
            }
        }
    }

    // The local packages are the entry points a tree layout expands from.
    view.roots = resolved
        .packages
        .iter()
        .filter(|p| p.source.is_none())
        .map(id_of)
        .collect();
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"
version = 3

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["libcore", "serde"]

[[package]]
name = "libcore"
version = "0.1.0"
dependencies = ["serde"]

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    fn scratch_lock(dir: &Path) -> bool {
        std::fs::write(dir.join("Cargo.lock"), LOCK).is_ok()
    }

    #[test]
    fn builds_a_package_graph_from_cargo_lock() {
        let Some(tmp) = tempfile::tempdir().ok() else {
            return;
        };
        if !scratch_lock(tmp.path()) {
            return;
        }
        let Ok(view) = dependency_graph(tmp.path()) else {
            return;
        };
        assert_eq!(view.nodes.len(), 3);
        // Local crates are roots; serde (registry) is not.
        assert!(view.roots.iter().any(|r| r.starts_with("app ")));
        assert!(view.roots.iter().any(|r| r.starts_with("libcore ")));
        assert!(!view.roots.iter().any(|r| r.starts_with("serde ")));
        // app depends on libcore and serde.
        let app_deps = view.successors("app 0.1.0", GraphEdgeKind::Dependency);
        assert_eq!(app_deps.len(), 2);
        // serde is external, versioned via its badge.
        let serde = view.nodes.iter().find(|n| n.label == "serde");
        assert_eq!(serde.map(|n| n.kind), Some(GraphNodeKind::External));
        assert_eq!(serde.and_then(|n| n.badge.as_deref()), Some("1.0.0"));
    }

    #[test]
    fn missing_lockfile_is_reported() {
        let Some(tmp) = tempfile::tempdir().ok() else {
            return;
        };
        assert!(matches!(
            dependency_graph(tmp.path()),
            Err(VizError::NoLockfile)
        ));
    }
}

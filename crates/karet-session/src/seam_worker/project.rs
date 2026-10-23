//! Projecting the index onto the wire model.
//!
//! One direction only. Nothing here reads a projection back into an index, because the
//! index is always built by walking source — so this is a lossy, presentation-shaped view
//! by design rather than a serialization format.

use std::collections::BTreeMap;
use std::path::Path;

use karet_seam::CallGraph;
use karet_seam::Configuration;
use karet_seam::SeamId;
use karet_seam::SeamIndex;
use karet_seam::model::Node;

use crate::api::SeamCallMetricsView;
use crate::api::SeamEdgeView;
use crate::api::SeamFacetView;
use crate::api::SeamNodeView;
use crate::api::SeamSummary;

/// Project one node, or `None` when its identity cannot be resolved.
#[must_use]
pub(crate) fn node_view(index: &SeamIndex, node: &Node) -> Option<SeamNodeView> {
    let id = index.path(node.id)?.to_string();
    Some(SeamNodeView {
        id,
        name: node.name.clone(),
        kind: node.kind.name().to_owned(),
        detail: node.detail.clone(),
        file: index
            .file_path(node.location.file)
            .unwrap_or(Path::new(""))
            .to_path_buf(),
        range: node.location.range,
        selection: node.location.selection,
        parent: node
            .parent
            .and_then(|parent| index.path(parent).map(ToString::to_string)),
        children: node
            .children
            .iter()
            .filter_map(|child| index.path(*child).map(ToString::to_string))
            .collect(),
        facets: node.facets.iter().map(facet_view).collect(),
        rollups: node.rollups.counts(),
        visibility: node.effective_visibility().map(|v| v.name().to_owned()),
        membership: node.membership.name().to_owned(),
        provisional: node.provisional,
        unread_lenses: index
            .unread_lenses(node.id)
            .iter()
            .map(|lens| lens.name().to_owned())
            .collect(),
    })
}

/// Project one facet.
fn facet_view(facet: &karet_seam::Facet) -> SeamFacetView {
    SeamFacetView {
        lens: facet.lens.name().to_owned(),
        subtype: facet.subtype.name().to_owned(),
        detail: facet.detail.clone(),
        sites: facet.sites.clone(),
        effective: facet
            .effective
            .as_ref()
            .map(|effective| effective.reach.name().to_owned()),
    }
}

/// Every edge touching `node`, in both directions.
///
/// Both directions, because the reader's question is "what is on the other end" and the
/// relation's written direction is an implementation detail of the grammar: a trait's
/// implementors point at it, while its re-export points away.
///
/// Structural relations come first, as they always have. Calls follow, one row per
/// target and resolution rather than one per call site — a busy function would
/// otherwise bury everything else under hundreds of rows — with the callees reaching the
/// most code first, so the row worth pivoting to is near the top, and then the callers.
#[must_use]
pub(crate) fn edges_of(
    index: &SeamIndex,
    graph: Option<&CallGraph>,
    node: SeamId,
) -> Vec<SeamEdgeView> {
    let resolve = |id: SeamId| index.path(id).map(ToString::to_string);
    let is_call = |edge: &&karet_seam::Edge| edge.kind == karet_seam::EdgeKind::Calls;
    let outgoing = index
        .edges()
        .from(node)
        .filter(|edge| !is_call(edge))
        .map(|edge| {
            let (state, resolvable) = endpoint_state(&edge.to);
            SeamEdgeView {
                kind: edge.kind.name().to_owned(),
                outgoing: true,
                target: edge.to.resolved().and_then(resolve),
                display: edge.to.display_hint().map(str::to_owned),
                state,
                resolvable,
                site: edge.site,
                resolution: None,
                count: 1,
            }
        });
    let incoming = index
        .edges()
        .to(node)
        .filter(|edge| !is_call(edge))
        .map(|edge| SeamEdgeView {
            kind: edge.kind.name().to_owned(),
            outgoing: false,
            target: resolve(edge.from),
            display: None,
            state: "resolved".to_owned(),
            resolvable: true,
            site: edge.site,
            resolution: None,
            count: 1,
        });
    let mut out: Vec<SeamEdgeView> = outgoing.chain(incoming).collect();

    let reach = |id: Option<SeamId>| {
        id.and_then(|id| graph?.metrics(id))
            .map_or(0, |metrics| metrics.inclusive_lower)
    };
    // Grouped by (target, display, resolution); the first site is kept to navigate to.
    let mut callees: BTreeMap<CallKey, (u64, SeamEdgeView)> = BTreeMap::new();
    // Calls follow the active configuration, as the measures beside them do: a caller
    // the configuration leaves out — test code, by default — is not listed as one.
    let live = |id: SeamId| {
        index
            .node(id)
            .is_none_or(|other| other.membership != karet_seam::ConfigMembership::Inactive)
    };
    for edge in index
        .edges()
        .from(node)
        .filter(is_call)
        .filter(|edge| edge.to.resolved().is_none_or(live))
    {
        let resolution = edge
            .resolution
            .map_or("unresolved", karet_seam::Resolution::name);
        let target = edge.to.resolved().and_then(resolve);
        let display = edge.to.display_hint().map(str::to_owned);
        let (state, resolvable) = endpoint_state(&edge.to);
        let row = callees
            .entry((target.clone(), display.clone(), resolution))
            .or_insert_with(|| {
                (
                    reach(edge.to.resolved()),
                    SeamEdgeView {
                        kind: edge.kind.name().to_owned(),
                        outgoing: true,
                        target,
                        display,
                        state,
                        resolvable,
                        site: edge.site,
                        resolution: Some(resolution.to_owned()),
                        count: 0,
                    },
                )
            });
        row.1.count = row.1.count.saturating_add(1);
    }
    let mut callees: Vec<(u64, SeamEdgeView)> = callees.into_values().collect();
    callees.sort_by_key(|(reach, _)| std::cmp::Reverse(*reach));
    out.extend(callees.into_iter().map(|(_, view)| view));

    // Callers are listed in path order (then resolution), not by reach: a caller's reach
    // measures the caller, not what it asks of this function, and path order keeps the
    // callers from one module together and stable as their bodies change.
    let mut callers: BTreeMap<(Option<String>, &str), SeamEdgeView> = BTreeMap::new();
    for edge in index
        .edges()
        .to(node)
        .filter(is_call)
        .filter(|edge| live(edge.from))
    {
        let resolution = edge
            .resolution
            .map_or("unresolved", karet_seam::Resolution::name);
        let target = resolve(edge.from);
        let row = callers
            .entry((target.clone(), resolution))
            .or_insert_with(|| SeamEdgeView {
                kind: edge.kind.name().to_owned(),
                outgoing: false,
                target,
                display: None,
                state: "resolved".to_owned(),
                resolvable: true,
                site: edge.site,
                resolution: Some(resolution.to_owned()),
                count: 0,
            });
        row.count = row.count.saturating_add(1);
    }
    out.extend(callers.into_values());
    out
}

/// What one row of outgoing calls stands for: the target, or how it was written when it
/// has none, and how it was resolved.
type CallKey = (Option<String>, Option<String>, &'static str);

/// A function's call measures, when the graph holds it.
#[must_use]
pub(crate) fn metrics_of(graph: &CallGraph, node: SeamId) -> Option<SeamCallMetricsView> {
    let m = graph.metrics(node)?;
    Some(SeamCallMetricsView {
        code_lines: m.code_lines,
        span_lines: m.span_lines,
        inclusive: m.inclusive_lower,
        inclusive_upper: m.inclusive_upper,
        tree_weighted: m.tree_weighted,
        saturated: m.saturated,
        fan_in: m.fan_in,
        fan_out: m.fan_out,
        external_apis: m.external_apis,
        depth: m.depth,
        scc_size: m.scc_size,
        recursive: m.recursive,
    })
}

/// An endpoint's state name, and whether it may yet resolve.
fn endpoint_state(endpoint: &karet_seam::Endpoint) -> (String, bool) {
    let resolvable = match endpoint {
        karet_seam::Endpoint::Unresolved { resolvable, .. } => *resolvable,
        _ => true,
    };
    (endpoint.state().to_owned(), resolvable)
}

/// Summarize an index under the active configuration.
#[must_use]
pub(crate) fn summary_of(
    index: &SeamIndex,
    root: Option<&Path>,
    active: Option<&Configuration>,
    available: &[Configuration],
) -> SeamSummary {
    let package = match index.roots() {
        [] => String::new(),
        [single] => index
            .node(*single)
            .map(|node| node.name.clone())
            .unwrap_or_default(),
        // No member's name describes an index spanning several of them, and picking one
        // arbitrarily would read as a claim about what is on screen. The directory that
        // was indexed does describe it, and the count says how much it holds.
        _ => root.map(directory_name).unwrap_or_default(),
    };
    SeamSummary {
        package,
        packages: index.roots().len(),
        nodes: index.len(),
        files: index.files().len(),
        configuration: active
            .map(|configuration| configuration.name.clone())
            .unwrap_or_else(|| karet_seam::config::UNCONFIGURED.to_owned()),
        available_configurations: available
            .iter()
            .map(|configuration| configuration.name.clone())
            .collect(),
        variation_complete: active.is_some_and(Configuration::variation_is_complete),
        truncated_after: index.truncated_after(),
        unresolved_modules: index
            .unresolved_modules()
            .iter()
            .filter_map(|(id, candidates)| Some((index.path(*id)?.to_string(), candidates.clone())))
            .collect(),
    }
}

/// What to call the directory an index was built from.
///
/// Resolved when the path does not name itself — a session launched as `karet .` has a
/// root of `.`, whose file name is nothing at all, and a header reading "…" would say
/// less than the directory's actual name does.
fn directory_name(root: &Path) -> String {
    let named = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    };
    named(root)
        .or_else(|| root.canonicalize().ok().as_deref().and_then(named))
        .unwrap_or_default()
}

/// The configurations a package can be read under.
///
/// Until the manifest tier lands this is the honest minimum: the default build, plus the
/// test configuration, which is a configuration rather than a special case. Neither
/// claims to enumerate the package's features, so `variation_complete` stays false and
/// the header says the `variation` lens is incomplete.
#[must_use]
pub(crate) fn configurations_for(_root: &Path) -> Vec<Configuration> {
    vec![
        Configuration::unconfigured(),
        Configuration::unconfigured().for_target("test"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_with_no_configuration_still_summarizes() {
        let index = SeamIndex::new();
        let summary = summary_of(&index, None, None, &[]);
        assert_eq!(summary.nodes, 0);
        assert_eq!(summary.configuration, karet_seam::config::UNCONFIGURED);
        assert!(!summary.variation_complete);
        assert!(summary.is_complete());
        assert_eq!(summary.packages, 0);
    }

    #[test]
    fn a_workspace_is_named_for_its_directory_rather_than_one_arbitrary_member() {
        let mut index = SeamIndex::new();
        for name in ["alpha", "beta"] {
            let id = index.intern(karet_seam::SeamPath::new(vec![
                karet_seam::SeamSegment::new(name),
            ]));
            let file = index.intern_file(Path::new("Cargo.toml"));
            index.insert(Node {
                id,
                kind: karet_seam::NodeKind::Package,
                name: name.to_owned(),
                detail: None,
                location: karet_seam::SeamLocation {
                    file,
                    range: karet_core::Range::default(),
                    span: karet_core::Span::default(),
                    selection: karet_core::Range::default(),
                    header: karet_core::Range::default(),
                },
                parent: None,
                children: Vec::new(),
                facets: Vec::new(),
                visibility: None,
                rollups: karet_seam::Rollups::new(),
                membership: karet_seam::ConfigMembership::Active,
                provisional: false,
            });
        }

        let summary = summary_of(&index, Some(Path::new("/repo/myproject")), None, &[]);
        assert_eq!(summary.packages, 2);
        // Naming one member would read as a claim about what is on screen.
        assert_eq!(summary.package, "myproject");
    }

    #[test]
    fn a_lone_package_is_still_named_for_itself() {
        let mut index = SeamIndex::new();
        let id = index.intern(karet_seam::SeamPath::new(vec![
            karet_seam::SeamSegment::new("solo"),
        ]));
        let file = index.intern_file(Path::new("Cargo.toml"));
        index.insert(Node {
            id,
            kind: karet_seam::NodeKind::Package,
            name: "solo".to_owned(),
            detail: None,
            location: karet_seam::SeamLocation {
                file,
                range: karet_core::Range::default(),
                span: karet_core::Span::default(),
                selection: karet_core::Range::default(),
                header: karet_core::Range::default(),
            },
            parent: None,
            children: Vec::new(),
            facets: Vec::new(),
            visibility: None,
            rollups: karet_seam::Rollups::new(),
            membership: karet_seam::ConfigMembership::Active,
            provisional: false,
        });

        let summary = summary_of(&index, Some(Path::new("/repo/elsewhere")), None, &[]);
        assert_eq!(summary.packages, 1);
        assert_eq!(summary.package, "solo");
    }

    #[test]
    fn the_default_configuration_set_does_not_claim_completeness() {
        // Without a manifest the feature set is a guess, and saying otherwise would let
        // the `variation` lens imply an answer it does not have.
        let configurations = configurations_for(Path::new("."));
        assert_eq!(configurations.len(), 2);
        assert!(configurations.iter().all(|c| !c.variation_is_complete()));
        assert!(
            configurations
                .iter()
                .any(|c| c.target.as_deref() == Some("test"))
        );
    }

    #[test]
    fn a_root_that_does_not_name_itself_is_resolved() {
        // `karet .` has a root of `.`, and "…" in the header says less than the
        // directory's own name does.
        assert!(!directory_name(Path::new(".")).is_empty());
        assert_eq!(directory_name(Path::new("/repo/myproject")), "myproject");
    }

    #[test]
    fn edges_of_an_unknown_node_are_empty_rather_than_a_failure() {
        let index = SeamIndex::new();
        assert!(edges_of(&index, None, SeamId(0)).is_empty());
    }

    #[test]
    fn a_node_carries_the_lenses_its_language_does_not_read()
    -> Result<(), Box<dyn std::error::Error>> {
        if karet_seam::lang::java::language_id().is_none() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        let module = dir.path().join("shop");
        std::fs::create_dir_all(module.join("src/main/java"))?;
        std::fs::write(module.join("pom.xml"), "<project></project>\n")?;
        std::fs::write(module.join("src/main/java/Cart.java"), "class Cart {}\n")?;
        let index = karet_seam::index_workspace(dir.path(), karet_seam::IndexOptions::default())?;
        let cart = index
            .nodes()
            .find(|node| node.name == "Cart" && node.kind == karet_seam::NodeKind::Type)
            .ok_or("no Cart")?;
        let view = node_view(&index, cart).ok_or("no view")?;
        assert_eq!(
            view.unread_lenses,
            ["substitution", "variation", "boundary", "hazard"]
        );
        let package = index
            .roots()
            .first()
            .and_then(|id| index.node(*id))
            .ok_or("no root")?;
        assert!(
            node_view(&index, package)
                .ok_or("no view")?
                .unread_lenses
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn calls_are_one_row_per_target_after_the_structural_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        if karet_seam::lang::rust::language_id().is_none() {
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir_all(dir.path().join("src"))?;
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub trait T { fn go(&self); }\npub struct S;\nimpl T for S { fn go(&self) { small(); big(); big(); missing(); } }\n\
             fn small() {}\nfn big() { small(); small(); }\npub fn other(s: &S) { s.go(); }\n",
        )?;
        let index = karet_seam::index_workspace(dir.path(), karet_seam::IndexOptions::default())?;
        let graph = CallGraph::build(&index, &karet_seam::GraphOptions::default());
        let go = index
            .resolve(&"k::S::{impl T}::go".parse()?)
            .ok_or("no go")?;
        let edges = edges_of(&index, Some(&graph), go);

        let kinds: Vec<&str> = edges.iter().map(|edge| edge.kind.as_str()).collect();
        let first_call = kinds
            .iter()
            .position(|kind| *kind == "calls")
            .ok_or("no calls")?;
        assert!(
            kinds[..first_call].iter().all(|kind| *kind != "calls"),
            "structural edges come first: {kinds:?}"
        );
        let outgoing: Vec<(Option<&str>, u32)> = edges
            .iter()
            .filter(|edge| edge.kind == "calls" && edge.outgoing)
            .map(|edge| {
                (
                    edge.target.as_deref().or(edge.display.as_deref()),
                    edge.count,
                )
            })
            .collect();
        // Two calls to `big`, one row; the callee reaching more code first.
        assert_eq!(
            outgoing,
            [
                (Some("k::big"), 2),
                (Some("k::small"), 1),
                (Some("missing"), 1)
            ]
        );
        assert!(
            edges
                .iter()
                .filter(|edge| edge.kind == "calls")
                .all(|edge| edge.resolution.is_some())
        );

        // `other` calls `go` through the trait: an incoming row, after every outgoing one.
        let incoming = edges
            .iter()
            .rposition(|edge| edge.kind == "calls" && !edge.outgoing);
        let last_outgoing = edges
            .iter()
            .rposition(|edge| edge.kind == "calls" && edge.outgoing);
        assert!(incoming > last_outgoing, "{edges:?}");

        let measured = metrics_of(&graph, go).ok_or("no metrics")?;
        assert_eq!(measured.fan_out, 2);
        let type_node = index.resolve(&"k::S".parse()?).ok_or("no S")?;
        assert!(metrics_of(&graph, type_node).is_none());
        Ok(())
    }
}

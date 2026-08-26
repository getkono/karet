//! Projecting the index onto the wire model.
//!
//! One direction only. Nothing here reads a projection back into an index, because the
//! index is always built by walking source — so this is a lossy, presentation-shaped view
//! by design rather than a serialization format.

use std::path::Path;

use karet_seam::Configuration;
use karet_seam::SeamId;
use karet_seam::SeamIndex;
use karet_seam::model::Node;

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
#[must_use]
pub(crate) fn edges_of(index: &SeamIndex, node: SeamId) -> Vec<SeamEdgeView> {
    let resolve = |id: SeamId| index.path(id).map(ToString::to_string);
    let outgoing = index.edges().from(node).map(|edge| {
        let (state, resolvable) = endpoint_state(&edge.to);
        SeamEdgeView {
            kind: edge.kind.name().to_owned(),
            outgoing: true,
            target: edge.to.resolved().and_then(resolve),
            display: edge.to.display_hint().map(str::to_owned),
            state,
            resolvable,
            site: edge.site,
        }
    });
    let incoming = index.edges().to(node).map(|edge| SeamEdgeView {
        kind: edge.kind.name().to_owned(),
        outgoing: false,
        target: resolve(edge.from),
        display: None,
        state: "resolved".to_owned(),
        resolvable: true,
        site: edge.site,
    });
    outgoing.chain(incoming).collect()
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
        _ => root
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned(),
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
    fn edges_of_an_unknown_node_are_empty_rather_than_a_failure() {
        let index = SeamIndex::new();
        assert!(edges_of(&index, SeamId(0)).is_empty());
    }
}

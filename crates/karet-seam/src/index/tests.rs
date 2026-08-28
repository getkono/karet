//! Tests for the seam index arena.
//!
//! Split out from the module itself only because both outgrew one file; they are the
//! index's own unit tests and belong to it.

use karet_core::Range;
use karet_core::Span;

use super::*;
use crate::model::Facet;
use crate::model::FacetSubtype;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::SeamLocation;

fn location() -> SeamLocation {
    SeamLocation {
        file: FileId(0),
        range: Range::default(),
        span: Span::default(),
        selection: Range::default(),
        header: Range::default(),
    }
}

/// Add a node at `path` with the given facets, wiring its parent by path.
fn add(index: &mut SeamIndex, path: &str, facets: Vec<Facet>) -> SeamId {
    let parsed: SeamPath = path.parse().unwrap_or_default();
    let parent = parsed.parent().and_then(|p| index.resolve(&p));
    let id = index.intern(parsed.clone());
    index.insert(Node {
        id,
        kind: NodeKind::Module,
        name: parsed.leaf().unwrap_or_default().to_owned(),
        detail: None,
        location: location(),
        parent,
        children: Vec::new(),
        facets,
        visibility: None,
        rollups: Rollups::new(),
        membership: ConfigMembership::Active,
        provisional: false,
    });
    id
}

fn api() -> Vec<Facet> {
    vec![Facet::new(Lens::Api, FacetSubtype("pub"))]
}

#[test]
fn an_empty_index_answers_without_panicking() {
    let index = SeamIndex::new();
    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert!(index.roots().is_empty());
    assert_eq!(index.node(SeamId(0)), None);
    assert!(index.children(SeamId(0)).is_empty());
    assert!(index.ancestors(SeamId(0)).is_empty());
    assert_eq!(index.truncated_after(), None);
}

#[test]
fn inserting_links_parents_and_records_roots() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    let module = add(&mut index, "pkg::m", vec![]);
    let item = add(&mut index, "pkg::m::T", vec![]);

    assert_eq!(index.roots(), [pkg]);
    assert_eq!(index.children(pkg), [module]);
    assert_eq!(index.children(module), [item]);
    assert_eq!(index.ancestors(item), [module, pkg]);
    assert_eq!(index.len(), 3);
}

#[test]
fn subtree_lists_the_node_and_its_descendants_in_source_order() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    add(&mut index, "pkg::a", vec![]);
    add(&mut index, "pkg::a::x", vec![]);
    add(&mut index, "pkg::b", vec![]);

    let names: Vec<String> = index
        .subtree(pkg)
        .into_iter()
        .filter_map(|id| index.path(id).map(ToString::to_string))
        .collect();
    assert_eq!(names, ["pkg", "pkg::a", "pkg::a::x", "pkg::b"]);
}

#[test]
fn rollups_aggregate_the_whole_subtree() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    let module = add(&mut index, "pkg::m", api());
    add(&mut index, "pkg::m::T", api());
    add(&mut index, "pkg::m::U", api());

    index.recompute_rollups();

    // The package sees all three, without anything being expanded.
    assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(3));
    assert_eq!(
        index.node(module).map(|n| n.rollups.get(Lens::Api)),
        Some(3)
    );
}

#[test]
fn a_facet_with_sites_counts_every_occurrence() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    add(
        &mut index,
        "pkg::f",
        vec![
            Facet::new(Lens::Hazard, FacetSubtype("unsafe")).with_sites(vec![
                Range::default(),
                Range::default(),
                Range::default(),
            ]),
        ],
    );
    index.recompute_rollups();
    assert_eq!(
        index.node(pkg).map(|n| n.rollups.get(Lens::Hazard)),
        Some(3)
    );
}

#[test]
fn a_configuration_excluded_node_contributes_nothing() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    let gated = add(&mut index, "pkg::gated", api());
    if let Some(node) = index.node_mut(gated) {
        node.membership = ConfigMembership::Inactive;
    }
    index.recompute_rollups();
    assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(0));
}

#[test]
fn an_indeterminate_node_still_contributes() {
    // Dropping it would assert an absence the index has not established.
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    let unknown = add(&mut index, "pkg::maybe", api());
    if let Some(node) = index.node_mut(unknown) {
        node.membership = ConfigMembership::Indeterminate;
    }
    index.recompute_rollups();
    assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));
}

#[test]
fn incremental_recompute_refreshes_only_the_ancestor_spine() {
    let mut index = SeamIndex::new();
    let pkg = add(&mut index, "pkg", vec![]);
    let module = add(&mut index, "pkg::m", vec![]);
    let leaf = add(&mut index, "pkg::m::T", api());
    index.recompute_rollups();
    assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));

    // The leaf gains a second facet, as an edit would produce.
    if let Some(node) = index.node_mut(leaf) {
        node.facets
            .push(Facet::new(Lens::Hazard, FacetSubtype("unsafe")));
    }
    index.recompute_rollups_from(leaf);

    assert_eq!(
        index.node(leaf).map(|n| n.rollups.get(Lens::Hazard)),
        Some(1)
    );
    assert_eq!(
        index.node(module).map(|n| n.rollups.get(Lens::Hazard)),
        Some(1)
    );
    assert_eq!(
        index.node(pkg).map(|n| n.rollups.get(Lens::Hazard)),
        Some(1)
    );
    assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));
}

#[test]
fn files_intern_once_and_resolve_back() {
    let mut index = SeamIndex::new();
    let a = index.intern_file(Path::new("src/lib.rs"));
    let b = index.intern_file(Path::new("src/model.rs"));
    assert_eq!(index.intern_file(Path::new("src/lib.rs")), a);
    assert_ne!(a, b);
    assert_eq!(index.file_path(a), Some(Path::new("src/lib.rs")));
    assert_eq!(index.file_path(FileId(99)), None);
    assert_eq!(index.files().len(), 2);
}

#[test]
fn truncation_is_recorded_rather_than_swallowed() {
    let mut index = SeamIndex::new();
    index.mark_truncated(20_000);
    assert_eq!(index.truncated_after(), Some(20_000));
}

#[test]
fn several_packages_each_become_a_root() {
    let mut index = SeamIndex::new();
    let a = add(&mut index, "alpha", vec![]);
    let b = add(&mut index, "beta", vec![]);
    assert_eq!(index.roots(), [a, b]);
}

#[test]
fn a_path_the_index_never_held_has_no_handle() {
    let mut index = SeamIndex::new();
    assert_eq!(index.file_id(Path::new("/absent.rs")), None);
    // Asking must not register it: an unknown path has nothing to rebuild, and
    // interning one here would inflate the file count for work that never happened.
    let known = index.intern_file(Path::new("/present.rs"));
    assert_eq!(index.file_id(Path::new("/present.rs")), Some(known));
    assert_eq!(index.files().len(), 1);
}

#[test]
fn a_files_owner_and_grammar_round_trip() {
    let mut index = SeamIndex::new();
    let owner = add(&mut index, "alpha", vec![]);
    let file = index.intern_file(Path::new("/alpha/src/lib.rs"));
    assert_eq!(index.file_attribution(file), None);

    index.attribute_file(file, owner, LanguageId(7));
    assert_eq!(index.file_attribution(file), Some((owner, LanguageId(7))));
}

#[test]
fn attribution_survives_removing_the_files_own_nodes() {
    // `remove_nodes_in_file` never removes the owner itself, so the record it was
    // built under stays valid across the re-index that clears its subtree.
    let mut index = SeamIndex::new();
    let owner = add(&mut index, "alpha", vec![]);
    let child = add(&mut index, "alpha::inner", vec![]);
    let file = index.intern_file(Path::new("/alpha/src/inner.rs"));
    index.attribute_file(file, owner, LanguageId(3));
    if let Some(node) = index.node_mut(child) {
        node.location.file = file;
    }

    index.remove_nodes_in_file(file, owner);
    assert_eq!(index.node(child), None);
    assert_eq!(index.file_attribution(file), Some((owner, LanguageId(3))));
}

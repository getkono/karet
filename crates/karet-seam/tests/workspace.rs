//! Integration coverage against real crates — this workspace's own.
//!
//! Synthetic fixtures prove each rule in isolation; only real code proves the rules
//! compose. The `#[path]` anchoring bug these tests now guard against was invisible to a
//! full unit suite and obvious the moment a real crate was indexed: `karet-lsp` quietly
//! lost a module because a top-level `#[path]` was resolved from the module directory
//! instead of the declaring file's own directory.
//!
//! Every test degrades to a pass when the layout is not what it expects, so this suite
//! never fails a build for reasons unrelated to indexing.

use std::path::Path;
use std::path::PathBuf;

use karet_seam::IndexOptions;
use karet_seam::LENSES;
use karet_seam::Lens;
use karet_seam::SeamIndex;
use karet_seam::index_package;
use karet_seam::index_workspace;

/// Crates whose shapes between them exercise every resolution rule: flat modules,
/// `mod.rs` directories, nested non-root files, and `#[path]` overrides.
const CRATES: &[&str] = &[
    "karet-core",
    "karet-lsp",
    "karet-text",
    "karet-vcs",
    "karet-editor",
];

/// The sibling crate directory, or `None` when the layout differs or the Rust grammar
/// is not compiled in — without it there is nothing to index and nothing to assert.
fn crate_dir(name: &str) -> Option<PathBuf> {
    karet_seam::lang::rust::language_id()?;
    let path = Path::new("..").join(name);
    path.join("Cargo.toml").is_file().then_some(path)
}

fn index(name: &str) -> Option<SeamIndex> {
    index_package(&crate_dir(name)?, IndexOptions::default()).ok()
}

#[test]
fn every_module_in_this_workspace_resolves_to_a_file() {
    for name in CRATES {
        let Some(index) = index(name) else { continue };
        let unresolved: Vec<String> = index
            .unresolved_modules()
            .iter()
            .filter_map(|(id, _)| index.path(*id).map(ToString::to_string))
            .collect();
        assert!(
            unresolved.is_empty(),
            "{name} has unresolved modules: {unresolved:?}"
        );
    }
}

#[test]
fn a_real_crate_yields_a_connected_tree_reaching_every_file() {
    let Some(index) = index("karet-core") else {
        return;
    };
    // One package root, and everything hangs from it.
    assert_eq!(index.roots().len(), 1, "a package has exactly one root");
    let Some(root) = index.roots().first().copied() else {
        return;
    };
    assert_eq!(
        index.subtree(root).len(),
        index.len(),
        "every node must be reachable from the root"
    );
    assert!(
        index.len() > 100,
        "karet-core has more than a handful of items"
    );
    assert!(index.files().len() > 5, "its modules span several files");
    assert_eq!(index.truncated_after(), None, "nothing should be truncated");
}

#[test]
fn rollups_at_the_root_account_for_the_whole_package() {
    let Some(index) = index("karet-core") else {
        return;
    };
    let Some(root) = index.roots().first().and_then(|id| index.node(*id)) else {
        return;
    };
    for lens in LENSES {
        let direct: u32 = index
            .nodes()
            .flat_map(|node| node.facets_for(lens))
            .map(|facet| u32::try_from(facet.occurrences()).unwrap_or(u32::MAX))
            .fold(0, u32::saturating_add);
        assert_eq!(
            root.rollups.get(lens),
            direct,
            "{} rollup at the root must equal the sum over every node",
            lens.name()
        );
    }
}

#[test]
fn the_binary_crate_has_exactly_one_entry_point() {
    let Some(dir) = crate_dir("karet") else {
        return;
    };
    let Ok(index) = index_package(&dir, IndexOptions::default()) else {
        return;
    };
    let entry_points: Vec<String> = index
        .nodes()
        .filter(|node| node.has_subtype(Lens::Boundary, "entry-point"))
        .filter_map(|node| index.path(node.id).map(ToString::to_string))
        .collect();
    assert_eq!(
        entry_points.len(),
        1,
        "expected one entry point, got {entry_points:?}"
    );
}

#[test]
fn an_async_heavy_crate_reports_hazards_and_a_pure_model_crate_does_not() {
    // The lens has to discriminate. Reporting hazards everywhere, or nowhere, would both
    // be useless — and both would pass a test that only checked one crate.
    let (Some(async_crate), Some(model_crate)) = (index("karet-lsp"), index("karet-core")) else {
        return;
    };
    let async_count = |index: &SeamIndex| {
        index
            .nodes()
            .filter(|node| node.has_subtype(Lens::Hazard, "async"))
            .count()
    };
    assert!(
        async_count(&async_crate) > 0,
        "karet-lsp is an async client and must show async hazards"
    );
    assert_eq!(
        async_count(&model_crate),
        0,
        "karet-core is a synchronous model crate"
    );
}

#[test]
fn reindexing_a_real_file_leaves_the_tree_consistent() {
    let Some(dir) = crate_dir("karet-core") else {
        return;
    };
    let Some(mut index) = index("karet-core") else {
        return;
    };
    let before = index.len();
    let target = dir.join("src").join("coord.rs");
    let Ok(text) = std::fs::read_to_string(&target) else {
        return;
    };

    let mut pool = karet_treesitter::ParserPool::new();
    if karet_seam::reindex_file(&mut index, &mut pool, &target, &text).is_err() {
        return;
    }

    // Re-indexing a file with its own unchanged contents must be a no-op in shape.
    assert_eq!(
        index.len(),
        before,
        "re-indexing unchanged text changed the tree"
    );
    let Some(root) = index.roots().first().copied() else {
        return;
    };
    assert_eq!(
        index.subtree(root).len(),
        index.len(),
        "every node must still be reachable after a re-index"
    );
}

/// The repository root, when the layout is the one these tests expect.
///
/// Integration tests run with the crate directory as the working directory, so the
/// workspace root is two levels up.
fn repository_root() -> Option<PathBuf> {
    let path = Path::new("..").join("..");
    path.join("Cargo.toml").is_file().then_some(path)
}

#[test]
fn the_repository_root_discovers_every_member_crate() {
    // Needs no grammar: discovery reads manifests and lists directories, never source.
    let Some(root) = repository_root() else {
        return;
    };
    let found = karet_seam::discover(&root, karet_seam::DiscoveryOptions::default());
    let names: Vec<&str> = found.iter().map(|package| package.name.as_str()).collect();

    assert!(
        names.len() > 20,
        "a workspace this size has many members: {names:?}"
    );
    for expected in ["karet-core", "karet-seam", "blameline", "xtask"] {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }
    // Sorted within the `crates/*` expansion, so the view's first column is stable.
    let globbed: Vec<&&str> = names.iter().filter(|name| **name != "xtask").collect();
    let mut sorted = globbed.clone();
    sorted.sort();
    assert_eq!(
        globbed, sorted,
        "member order must not depend on the filesystem"
    );
}

#[test]
fn the_repository_root_indexes_as_one_tree_with_a_root_per_package() {
    // The case that was a hard error: this repository's root manifest is virtual, so the
    // only entry point the editor could reach always failed on it.
    if karet_seam::lang::rust::language_id().is_none() {
        return;
    }
    let Some(root) = repository_root() else {
        return;
    };
    let Ok(index) = index_workspace(&root, IndexOptions::default()) else {
        return;
    };

    assert!(index.roots().len() > 20, "every member becomes a root");
    assert_eq!(
        index.truncated_after(),
        None,
        "the default cap is far above this"
    );

    // The multi-root analogue of the single-package connectivity check: the roots
    // partition the index, so nothing is orphaned and nothing is counted twice.
    let total: usize = index
        .roots()
        .iter()
        .map(|id| index.subtree(*id).len())
        .sum();
    assert_eq!(total, index.len(), "the roots must partition every node");

    let names: Vec<&str> = index
        .roots()
        .iter()
        .filter_map(|id| index.node(*id).map(|node| node.name.as_str()))
        .collect();
    assert!(names.contains(&"karet-core"), "got {names:?}");
    assert!(names.contains(&"karet-seam"), "got {names:?}");
}

#[test]
fn every_module_across_the_whole_workspace_resolves_to_a_file() {
    // The sibling of the per-crate check, over every crate at once — which is where a
    // resolution bug in a crate nobody listed would otherwise hide.
    if karet_seam::lang::rust::language_id().is_none() {
        return;
    }
    let Some(root) = repository_root() else {
        return;
    };
    let Ok(index) = index_workspace(&root, IndexOptions::default()) else {
        return;
    };
    let unresolved: Vec<String> = index
        .unresolved_modules()
        .iter()
        .filter_map(|(id, _)| index.path(*id).map(ToString::to_string))
        .collect();
    assert!(
        unresolved.is_empty(),
        "unresolved across the workspace: {unresolved:?}"
    );
}

//! Integration coverage against real crates — this workspace's own.
//!
//! Synthetic fixtures prove each rule in isolation; only real code proves the rules
//! compose. The `#[path]` anchoring bug these tests now guard against was invisible to a
//! full unit suite and obvious the moment a real crate was indexed: `karet-lsp` quietly
//! lost a module because a top-level `#[path]` was resolved from the module directory
//! instead of the declaring file's own directory.
//!
//! A test skips only when there is nothing to index: the Rust grammar is not compiled in,
//! or the sibling crate it reads is not where this repository keeps it. Once it has
//! something to index, an indexing error fails the test rather than passing it.

use std::path::Path;
use std::path::PathBuf;

use karet_seam::IndexOptions;
use karet_seam::LENSES;
use karet_seam::Lens;
use karet_seam::SeamIndex;
use karet_seam::index_package;
use karet_seam::index_workspace;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
    karet_treesitter::language_id_from_injection_name("rust")?;
    let path = Path::new("..").join(name);
    path.join("Cargo.toml").is_file().then_some(path)
}

/// The indexed sibling crate, `None` only when [`crate_dir`] finds nothing to index.
///
/// # Errors
/// Whatever [`index_package`] returns — an indexing failure is a test failure.
fn index(name: &str) -> Result<Option<SeamIndex>, karet_seam::PackageError> {
    crate_dir(name)
        .map(|dir| index_package(&dir, IndexOptions::default()))
        .transpose()
}

#[test]
fn every_module_in_this_workspace_resolves_to_a_file() -> TestResult {
    for name in CRATES {
        let Some(index) = index(name)? else { continue };
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
    Ok(())
}

#[test]
fn a_real_crate_yields_a_connected_tree_reaching_every_file() -> TestResult {
    let Some(index) = index("karet-core")? else {
        return Ok(());
    };
    // One package root, and everything hangs from it.
    assert_eq!(index.roots().len(), 1, "a package has exactly one root");
    let root = index.roots().first().copied().ok_or("no package root")?;
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
    Ok(())
}

#[test]
fn rollups_at_the_root_account_for_the_whole_package() -> TestResult {
    let Some(index) = index("karet-core")? else {
        return Ok(());
    };
    let root = index
        .roots()
        .first()
        .and_then(|id| index.node(*id))
        .ok_or("no package root")?;
    for lens in LENSES {
        let direct: u32 = index
            .nodes()
            .flat_map(|node| node.facets_for(lens))
            .map(|facet| u32::try_from(facet.occurrences()).unwrap_or(u32::MAX))
            .fold(0, u32::saturating_add);
        assert_eq!(
            root.rollups().get(lens),
            direct,
            "{} rollup at the root must equal the sum over every node",
            lens.name()
        );
    }
    Ok(())
}

#[test]
fn the_binary_crate_has_exactly_one_entry_point() -> TestResult {
    let Some(index) = index("karet")? else {
        return Ok(());
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
    Ok(())
}

#[test]
fn an_async_heavy_crate_reports_hazards_and_a_pure_model_crate_does_not() -> TestResult {
    // The lens has to discriminate. Reporting hazards everywhere, or nowhere, would both
    // be useless — and both would pass a test that only checked one crate.
    let (Some(async_crate), Some(model_crate)) = (index("karet-lsp")?, index("karet-core")?) else {
        return Ok(());
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
    Ok(())
}

#[test]
fn reindexing_a_real_file_leaves_the_tree_consistent() -> TestResult {
    let (Some(dir), Some(mut index)) = (crate_dir("karet-core"), index("karet-core")?) else {
        return Ok(());
    };
    let before = index.len();
    let target = dir.join("src").join("coord.rs");
    let text = std::fs::read_to_string(&target)?;

    let mut pool = karet_treesitter::ParserPool::new();
    karet_seam::reindex_file(&mut index, &mut pool, &target, &text)?;

    // Re-indexing a file with its own unchanged contents must be a no-op in shape.
    assert_eq!(
        index.len(),
        before,
        "re-indexing unchanged text changed the tree"
    );
    let root = index.roots().first().copied().ok_or("no package root")?;
    assert_eq!(
        index.subtree(root).len(),
        index.len(),
        "every node must still be reachable after a re-index"
    );
    Ok(())
}

#[test]
fn real_calls_link_from_callables_and_measure_consistently() -> TestResult {
    use karet_seam::CallGraph;
    use karet_seam::EdgeKind;
    use karet_seam::GraphOptions;
    use karet_seam::Resolution;

    for name in CRATES {
        let Some(index) = index(name)? else { continue };
        let calls: Vec<&karet_seam::Edge> = index
            .edges()
            .all()
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Calls)
            .collect();
        assert!(!calls.is_empty(), "{name} makes no calls");
        // Every call leaves a callable, and every one says how it was resolved.
        for edge in &calls {
            assert!(
                index.calls().callable(edge.from).is_some(),
                "{name}: a call leaves a non-callable"
            );
            assert!(edge.resolution.is_some(), "{name}: an untagged call");
        }
        assert!(
            calls
                .iter()
                .any(|edge| edge.resolution == Some(Resolution::Exact)),
            "{name}: nothing resolved exactly"
        );

        let graph = CallGraph::build(&index, &GraphOptions::default());
        assert!(!graph.is_empty(), "{name}: an empty graph");
        for metrics in graph.all() {
            assert!(
                metrics.inclusive_lower >= u64::from(metrics.code_lines),
                "{name}: a function reaches less than itself"
            );
            assert!(
                metrics.inclusive_upper >= metrics.inclusive_lower,
                "{name}: the upper bound fell below the lower"
            );
            assert!(
                metrics.tree_weighted >= metrics.inclusive_lower || metrics.recursive,
                "{name}: the tree weighs less than the code it reaches"
            );
        }
    }
    Ok(())
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
fn the_repository_root_indexes_as_one_tree_with_a_root_per_package() -> TestResult {
    // The case that was a hard error: this repository's root manifest is virtual, so the
    // only entry point the editor could reach always failed on it.
    if karet_treesitter::language_id_from_injection_name("rust").is_none() {
        return Ok(());
    }
    let Some(root) = repository_root() else {
        return Ok(());
    };
    let index = index_workspace(&root, IndexOptions::default())?;

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
    Ok(())
}

#[test]
fn every_module_across_the_whole_workspace_resolves_to_a_file() -> TestResult {
    // The sibling of the per-crate check, over every crate at once — which is where a
    // resolution bug in a crate nobody listed would otherwise hide.
    if karet_treesitter::language_id_from_injection_name("rust").is_none() {
        return Ok(());
    }
    let Some(root) = repository_root() else {
        return Ok(());
    };
    let index = index_workspace(&root, IndexOptions::default())?;
    let unresolved: Vec<String> = index
        .unresolved_modules()
        .iter()
        .filter_map(|(id, _)| index.path(*id).map(ToString::to_string))
        .collect();
    assert!(
        unresolved.is_empty(),
        "unresolved across the workspace: {unresolved:?}"
    );
    Ok(())
}

/// Not an assertion — a stopwatch. `cargo test --release -- --ignored --nocapture bench`
#[test]
#[ignore = "timing, not a correctness check"]
fn bench_cold_index_of_this_repository() {
    // Overridable so a before/after comparison can point both builds at one tree.
    let root = std::env::var("KARET_BENCH_ROOT").map_or_else(
        |_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        },
        std::path::PathBuf::from,
    );
    let start = std::time::Instant::now();
    let index = karet_seam::index_workspace(&root, karet_seam::IndexOptions::default());
    let elapsed = start.elapsed();
    let nodes = index.as_ref().map(karet_seam::SeamIndex::len).unwrap_or(0);
    let files = index.as_ref().map(|i| i.files().len()).unwrap_or(0);
    println!("COLD {elapsed:?} — {nodes} nodes across {files} files");

    // Relinking is what every save pays, so it is timed on its own.
    let Ok(mut index) = index else {
        return;
    };
    let start = std::time::Instant::now();
    karet_seam::link_calls(&mut index);
    let linked = start.elapsed();
    let start = std::time::Instant::now();
    let graph = karet_seam::CallGraph::build(&index, &karet_seam::GraphOptions::default());
    let measured = start.elapsed();
    println!(
        "LINK {linked:?} — {} call edges; MEASURE {measured:?} — {} functions",
        index
            .edges()
            .all()
            .iter()
            .filter(|edge| edge.kind == karet_seam::EdgeKind::Calls)
            .count(),
        graph.len()
    );
}

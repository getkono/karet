//! Report tests over a small real package.

use super::*;
use crate::calls::graph::Bound;
use crate::calls::tree::TreeLimits;
use crate::calls::tree::call_tree;
use crate::id::SeamId;
use crate::package::IndexOptions;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A crate with a hub, a cycle, a heavy facade, a long leaf, dead code, a test-only
/// helper, and a function leaning on another module.
const LIB: &str = "\
mod store {
    pub(crate) fn get() -> u32 { 1 }
    pub fn put() -> u32 { get() }
}
mod engine {
    use crate::store;
    pub fn run() -> u32 { store::get() + store::get() + store::get() + store::put() }
    pub fn a() -> u32 { b() }
    fn b() -> u32 { a() }
}
pub fn facade() -> u32 {
    engine::run() + big()
}
fn big() -> u32 {
    let mut x = 0;
    x += 1;
    x += 1;
    x += 1;
    x += 1;
    x += 1;
    x
}
fn dead() -> u32 { 0 }
fn spin(n: u32) -> u32 { if n == 0 { 0 } else { spin(n - 1) } }
pub struct Shown;
impl std::fmt::Display for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(\"x\") }
}
fn only_tested() -> u32 { 2 }
#[cfg(test)]
mod tests {
    fn check() { super::only_tested(); }
}
";

fn index() -> Result<Option<crate::index::SeamIndex>, Box<dyn std::error::Error>> {
    if crate::lang::rust::language_id().is_none() {
        return Ok(None);
    }
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/lib.rs"), LIB)?;
    Ok(Some(crate::package::index_workspace(
        dir.path(),
        IndexOptions::default(),
    )?))
}

/// Options whose floors this small crate can reach.
fn small() -> ReportOptions {
    ReportOptions::default()
}

fn names(spots: &[Hotspot]) -> Vec<&str> {
    spots.iter().map(|spot| spot.path.as_str()).collect()
}

#[test]
fn a_report_finds_every_kind_of_hotspot_with_evidence() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let report = report(&index, &GraphOptions::default(), &[], &small());

    assert_eq!(report.hotspots.cycles.len(), 1);
    let cycle = report.hotspots.cycles.first().ok_or("no cycle")?;
    assert_eq!(cycle.members, ["k::engine::a", "k::engine::b"]);
    // A concrete path round it: from the first member, back to itself.
    assert_eq!(cycle.path, ["k::engine::a", "k::engine::b", "k::engine::a"]);

    // A function calling only itself is still called by nobody; a Display impl is called
    // by `format!`, which no call edge records, so its silence proves nothing.
    assert_eq!(names(&report.hotspots.unreachable), ["k::dead", "k::spin"]);
    assert_eq!(names(&report.hotspots.test_only), ["k::only_tested"]);
    assert!(
        report
            .hotspots
            .test_only
            .iter()
            .all(|spot| spot.why.contains("macro"))
    );
    for spot in report
        .hotspots
        .unreachable
        .iter()
        .chain(&report.hotspots.test_only)
    {
        assert!(spot.file.ends_with("lib.rs") && spot.line > 0, "{spot:?}");
    }
    // Floors keep a small crate's ordinary functions out of the hub and leaf lists.
    assert_eq!(report.thresholds.hub_fan_in, 5);
    assert_eq!(report.thresholds.large_leaf_lines, 50);
    assert!(report.hotspots.hubs.is_empty());
    assert!(report.hotspots.large_leaves.is_empty());
    Ok(())
}

#[test]
fn hubs_heavy_entry_points_and_large_leaves_are_found_past_their_floors() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // Four long leaves, a one-line facade over all of them, and a helper five
    // functions call.
    let mut lib = String::new();
    for leaf in 0..4 {
        lib.push_str(&format!("fn leaf{leaf}() -> u32 {{\n    let mut x = 0;\n"));
        for _ in 0..58 {
            lib.push_str("    x += 1;\n");
        }
        lib.push_str("    x\n}\n");
    }
    lib.push_str("pub fn facade() -> u32 { leaf0() + leaf1() + leaf2() + leaf3() }\n");
    lib.push_str("fn shared() -> u32 { 1 }\n");
    for caller in 0..5 {
        lib.push_str(&format!("pub fn user{caller}() -> u32 {{ shared() }}\n"));
    }
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/lib.rs"), lib)?;
    let index = crate::package::index_workspace(dir.path(), IndexOptions::default())?;
    let report = report(&index, &GraphOptions::default(), &[], &small());

    assert_eq!(names(&report.hotspots.hubs), ["k::shared"]);
    let hub = report.hotspots.hubs.first().ok_or("no hub")?;
    assert_eq!(hub.value, 5);
    assert!(hub.why.contains("5 functions"), "{}", hub.why);

    assert_eq!(names(&report.hotspots.heavy_entry_points), ["k::facade"]);
    let heavy = report
        .hotspots
        .heavy_entry_points
        .first()
        .ok_or("no heavy")?;
    // The evidence shows where the reach goes.
    assert!(heavy.excerpt.contains("k::leaf0"), "{}", heavy.excerpt);
    // Four 62-line leaves (signature, `let`, 58 steps, `x`, brace) and its own line.
    assert_eq!(heavy.value, 4 * 62 + 1);

    let leaves = names(&report.hotspots.large_leaves);
    assert_eq!(leaves.len(), 4, "{leaves:?}");
    assert!(
        report
            .hotspots
            .large_leaves
            .iter()
            .all(|spot| spot.value == 62)
    );
    Ok(())
}

#[test]
fn a_narrowed_report_keeps_whole_cycles_coupling_and_thresholds() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let whole = report(&index, &GraphOptions::default(), &[], &small());
    // Only `a` of the `a ↔ b` cycle, and only `store`'s functions.
    let scope: Vec<SeamId> = ["k::engine::a", "k::store::get", "k::store::put"]
        .iter()
        .filter_map(|path| index.resolve(&path.parse().ok()?))
        .collect();
    let narrowed = report(&index, &GraphOptions::default(), &scope, &small());

    let cycle = narrowed.hotspots.cycles.first().ok_or("no cycle")?;
    assert_eq!(cycle.members, ["k::engine::a", "k::engine::b"]);
    assert_eq!(cycle.path.len(), 3);
    assert_eq!(narrowed.thresholds, whole.thresholds);
    // `store` is still called by `engine`, which is outside the scope.
    let store = narrowed
        .coupling
        .modules
        .iter()
        .find(|m| m.path == "k::store")
        .ok_or("no store")?;
    assert_eq!(store.afferent, 1);
    assert!(
        narrowed
            .coupling
            .edges
            .iter()
            .any(|e| e.from == "k::engine" && e.to == "k::store")
    );
    Ok(())
}

#[test]
fn including_tests_is_said_in_the_configuration_and_empties_test_only() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let report = report(
        &index,
        &GraphOptions {
            configuration: crate::config::Configuration::unconfigured().for_target("test"),
            ..GraphOptions::default()
        },
        &[],
        &small(),
    );
    assert!(
        report.configuration.ends_with(" + test"),
        "{}",
        report.configuration
    );
    assert!(report.hotspots.test_only.is_empty());
    // A test function has no caller in code — its runner calls it — so it is not dead.
    assert_eq!(names(&report.hotspots.unreachable), ["k::dead", "k::spin"]);
    assert!(
        report
            .functions
            .iter()
            .any(|row| row.path == "k::tests::check"),
        "tests are measured under a test configuration"
    );
    Ok(())
}

#[test]
fn the_least_certain_list_ranks_by_uncertain_sites_then_path_and_is_capped() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    // `parse` has two candidates (ambiguous) and `nowhere` none (unresolved).
    let lib = "\
mod a { pub fn parse() -> u32 { 1 } }
mod b { pub fn parse() -> u32 { 2 } }
pub fn both() -> u32 { parse() + nowhere() }
pub fn guess() -> u32 { parse() }
pub fn lost() -> u32 { nowhere() }
pub fn sure() -> u32 { a::parse() }
";
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/lib.rs"), lib)?;
    let index = crate::package::index_workspace(dir.path(), IndexOptions::default())?;

    let whole = report(&index, &GraphOptions::default(), &[], &small());
    let listed: Vec<(&str, usize, usize, usize)> = whole
        .ledger
        .least_certain
        .iter()
        .map(|u| (u.path.as_str(), u.sites, u.ambiguous, u.unresolved))
        .collect();
    // Most uncertain sites first, ties by path; a function with none is not listed.
    assert_eq!(
        listed,
        [
            ("k::both", 2, 1, 1),
            ("k::guess", 1, 1, 0),
            ("k::lost", 1, 0, 1)
        ]
    );
    let rust = whole.ledger.languages.first().ok_or("no language")?;
    assert_eq!((rust.ambiguous, rust.unresolved), (2, 2), "{rust:?}");
    assert!(whole.markdown().contains("(at most 50)"));

    let capped = report(
        &index,
        &GraphOptions::default(),
        &[],
        &ReportOptions {
            least_certain_n: 2,
            ..ReportOptions::default()
        },
    );
    let kept: Vec<&str> = capped
        .ledger
        .least_certain
        .iter()
        .map(|u| u.path.as_str())
        .collect();
    assert_eq!(kept, ["k::both", "k::guess"]);
    assert_eq!(capped.thresholds.least_certain_n, 2);
    Ok(())
}

#[test]
fn hotspot_thresholds_follow_the_options() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    // With a percentile low enough, the floors are what decide.
    let report = report(
        &index,
        &GraphOptions::default(),
        &[],
        &ReportOptions {
            percentile: 0.1,
            ..ReportOptions::default()
        },
    );
    assert!((report.thresholds.percentile - 0.1).abs() < f64::EPSILON);
    assert_eq!(report.thresholds.heavy_inclusive, 200);
    assert!(report.thresholds.heavy_ratio >= 2.0);
    Ok(())
}

#[test]
fn coupling_counts_calls_between_modules() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let report = report(&index, &GraphOptions::default(), &[], &small());
    let c = &report.coupling;
    let edge = c
        .edges
        .iter()
        .find(|e| e.from == "k::engine" && e.to == "k::store")
        .ok_or("no engine → store edge")?;
    assert_eq!((edge.calls, edge.callers), (4, 1));

    let store = c
        .modules
        .iter()
        .find(|m| m.path == "k::store")
        .ok_or("no store")?;
    assert_eq!((store.afferent, store.efferent), (1, 0));
    assert_eq!(store.instability, Some(0.0));
    let engine = c
        .modules
        .iter()
        .find(|m| m.path == "k::engine")
        .ok_or("no engine")?;
    assert_eq!(engine.efferent, 1);
    assert!(engine.instability.is_some_and(|i| i > 0.0));

    // `store::get` is crate-visible, reached from another module.
    assert!(
        c.internal_reach.iter().any(|r| r.caller == "k::engine::run"
            && r.callee == "k::store::get"
            && r.visibility == "crate"),
        "{:?}",
        c.internal_reach
    );
    // `run` calls into `store` four times and its own module never.
    let envy = c
        .envious
        .iter()
        .find(|e| e.function == "k::engine::run")
        .ok_or("no envy")?;
    assert_eq!((envy.own_calls, envy.other_calls), (0, 4));
    assert_eq!(envy.other, "k::store");
    Ok(())
}

#[test]
fn the_ledger_counts_sites_not_edges() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let report = report(&index, &GraphOptions::default(), &[], &small());
    let rust = report.ledger.languages.first().ok_or("no language")?;
    assert!(rust.language.eq_ignore_ascii_case("rust"), "{rust:?}");
    assert_eq!(rust.test_detection, "cfg(test)");
    assert_eq!(
        rust.sites,
        rust.exact
            + rust.dispatch
            + rust.ambiguous
            + rust.too_ambiguous
            + rust.external
            + rust.unresolved
    );
    assert!(rust.exact >= 8, "{rust:?}");
    Ok(())
}

#[test]
fn a_scope_narrows_what_is_listed_but_not_what_is_measured() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let query = crate::query::parse("in:k::engine")?;
    let scope = crate::query::evaluate(&query, &index).nodes;
    let report = report(&index, &GraphOptions::default(), &scope, &small());
    let listed: Vec<&str> = report
        .functions
        .iter()
        .map(|row| row.path.as_str())
        .collect();
    assert!(
        listed.iter().all(|path| path.starts_with("k::engine::")),
        "{listed:?}"
    );
    // `run` still reaches into `store`, which is outside the scope.
    let run = report
        .functions
        .iter()
        .find(|row| row.path == "k::engine::run")
        .ok_or("no run")?;
    assert!(run.metrics.inclusive_lower > u64::from(run.metrics.code_lines));
    // Most inclusive first.
    let order: Vec<u64> = report
        .functions
        .iter()
        .map(|row| row.metrics.inclusive_lower)
        .collect();
    let mut sorted = order.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(order, sorted);
    Ok(())
}

#[test]
fn the_markdown_puts_definitions_and_hotspots_before_the_appendix() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let report = report(&index, &GraphOptions::default(), &[], &small());
    let markdown = report.markdown();
    let at = |heading: &str| markdown.find(heading).ok_or(format!("no {heading}"));
    let order = [
        at("## How to read this")?,
        at("## Summary")?,
        at("## Hotspots")?,
        at("## Module coupling")?,
        at("## Precision ledger")?,
        at("## Appendix: every function")?,
    ];
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
    assert!(markdown.contains("`k::engine::a` → `k::engine::b` → `k::engine::a`"));
    assert!(markdown.contains("| `k::facade` |"));
    // Deterministic: the same index renders the same document.
    assert_eq!(markdown, report.markdown());
    Ok(())
}

#[test]
fn a_call_tree_expands_each_function_once_and_respects_its_limits() -> TestResult {
    let Some(index) = index()? else {
        return Ok(());
    };
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let facade = index.resolve(&"k::facade".parse()?).ok_or("no facade")?;
    let tree =
        call_tree(&index, &graph, facade, Bound::Lower, TreeLimits::FULL).ok_or("no tree")?;
    let text = tree.render();
    assert!(text.contains("k::engine::run"), "{text}");
    assert!(text.contains("k::store::get ×3"), "{text}");
    // `get` is reached again through `put`, and is not expanded twice.
    assert!(text.contains("↺ above"), "{text}");
    assert!(tree.markdown().starts_with("# Call tree of `k::facade`"));

    // Width keeps the most inclusive callees: `big` outweighs `run`.
    let narrow = call_tree(
        &index,
        &graph,
        facade,
        Bound::Lower,
        TreeLimits {
            depth: None,
            width: Some(1),
        },
    )
    .ok_or("no tree")?;
    let kept: Vec<&str> = narrow
        .root
        .children
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(kept, ["k::big"]);
    assert_eq!(narrow.root.elided, 1);
    assert!(narrow.render().contains("… 1 more"));

    // Depth stops below the first level and says so where callees remain.
    let shallow = call_tree(
        &index,
        &graph,
        facade,
        Bound::Lower,
        TreeLimits {
            depth: Some(1),
            width: None,
        },
    )
    .ok_or("no tree")?;
    let run = shallow
        .root
        .children
        .iter()
        .find(|c| c.path == "k::engine::run")
        .ok_or("no run")?;
    assert!(run.truncated && run.children.is_empty());
    let big = shallow
        .root
        .children
        .iter()
        .find(|c| c.path == "k::big")
        .ok_or("no big")?;
    assert!(!big.truncated, "a leaf has nothing left to show");

    let outside = index.resolve(&"k::store".parse()?).ok_or("no store")?;
    assert!(call_tree(&index, &graph, outside, Bound::Lower, TreeLimits::FULL).is_none());
    Ok(())
}

#[test]
fn a_function_cut_off_by_depth_is_expanded_where_it_appears_again() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
    )?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    // `x` is reached at depth 2 through `a` — cut off there — and at depth 1 directly.
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn r() { a(); x(); }\nfn a() { x(); }\nfn x() { y(); }\nfn y() {}\n",
    )?;
    let index = crate::package::index_workspace(dir.path(), IndexOptions::default())?;
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let root = index.resolve(&"k::r".parse()?).ok_or("no r")?;
    let tree = call_tree(
        &index,
        &graph,
        root,
        Bound::Lower,
        TreeLimits {
            depth: Some(2),
            width: None,
        },
    )
    .ok_or("no tree")?;
    let direct = tree
        .root
        .children
        .iter()
        .find(|child| child.path == "k::x")
        .ok_or("no direct x")?;
    assert!(!direct.repeat, "{}", tree.render());
    assert!(
        direct.children.iter().any(|child| child.path == "k::y"),
        "{}",
        tree.render()
    );
    Ok(())
}

#[test]
fn a_percentile_is_nearest_rank_and_safe_on_nothing() {
    assert!((percentile(Vec::new(), 0.95)).abs() < f64::EPSILON);
    let values: Vec<f64> = (1..=20).map(f64::from).collect();
    assert!((percentile(values.clone(), 0.95) - 19.0).abs() < f64::EPSILON);
    assert!((percentile(values.clone(), 1.0) - 20.0).abs() < f64::EPSILON);
    assert!((percentile(values, 0.0) - 1.0).abs() < f64::EPSILON);
}

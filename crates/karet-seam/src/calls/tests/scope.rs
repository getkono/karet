//! Scope rules that only show across files and packages, and the graph's other answers.

use super::*;

#[test]
fn a_call_into_another_package_resolves_through_its_written_name() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"core-lib\", \"app\"]\n",
        ),
        (
            "core-lib/Cargo.toml",
            "[package]\nname = \"core-lib\"\nversion = \"0.1.0\"\n",
        ),
        ("core-lib/src/lib.rs", "pub fn shared() -> u32 { 1 }\n"),
        (
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        ),
        (
            "app/src/lib.rs",
            "use core_lib::shared;\n\
             pub fn imported() -> u32 { shared() }\n\
             pub fn qualified() -> u32 { core_lib::shared() }\n",
        ),
    ])?;
    let index = index(dir.path())?;
    assert_eq!(calls(&index, "app::imported")?, [exact("core-lib::shared")]);
    assert_eq!(
        calls(&index, "app::qualified")?,
        [exact("core-lib::shared")]
    );
    Ok(())
}

#[test]
fn a_free_name_from_an_unimported_package_is_only_a_candidate() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        ("Cargo.toml", "[workspace]\nmembers = [\"one\", \"two\"]\n"),
        (
            "one/Cargo.toml",
            "[package]\nname = \"one\"\nversion = \"0.1.0\"\n",
        ),
        ("one/src/lib.rs", "pub fn spawn() {}\n"),
        (
            "two/Cargo.toml",
            "[package]\nname = \"two\"\nversion = \"0.1.0\"\n",
        ),
        ("two/src/lib.rs", "pub fn go() { spawn() }\n"),
    ])?;
    let index = index(dir.path())?;
    assert_eq!(
        calls(&index, "two::go")?,
        [("one::spawn".to_owned(), Resolution::Ambiguous)]
    );
    Ok(())
}

#[test]
fn a_bare_name_in_a_rust_method_is_the_free_function_not_the_method() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "mod parser { pub fn parse(s: &str) -> u32 { s.len() as u32 } }\n\
         use parser::parse;\n\
         pub struct Config;\n\
         impl Config { pub fn parse(s: &str) -> u32 { parse(s) } }\n",
    )?;
    let index = index(dir.path())?;
    // Delegation, not recursion.
    assert_eq!(
        calls(&index, "k::Config::parse")?,
        [exact("k::parser::parse")]
    );
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let metrics = graph
        .metrics(id(&index, "k::Config::parse")?)
        .cloned()
        .ok_or("no metrics")?;
    assert!(!metrics.recursive);
    Ok(())
}

#[test]
fn a_test_module_shadows_the_file_it_sits_in() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub fn helper() -> u32 { 1 }\n\
         #[cfg(test)]\nmod tests {\n    fn helper() -> u32 { 2 }\n    fn t() -> u32 { helper() }\n}\n",
    )?;
    let index = index(dir.path())?;
    assert_eq!(calls(&index, "k::tests::t")?, [exact("k::tests::helper")]);
    Ok(())
}

#[test]
fn super_and_glob_imports_resolve() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "mod tools { pub fn sharpen() {} }\n\
         mod work { use crate::tools::*; pub fn go() { sharpen() } }\n",
    )?;
    let rust_index = index(dir.path())?;
    assert_eq!(
        calls(&rust_index, "k::work::go")?,
        [exact("k::tools::sharpen")]
    );

    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        ("pyproject.toml", "[project]\nname = \"app\"\n"),
        ("app/__init__.py", ""),
        (
            "app/shapes.py",
            "class Base:\n    def draw(self):\n        return 1\n\n\
             class Square(Base):\n    def draw(self):\n        return super().draw()\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let draw = calls(&index, "app::shapes::Square::draw")?;
    // Which parent is meant takes the hierarchy, so even the one candidate is only that;
    // `super()` itself is a builtin.
    assert_eq!(
        draw,
        [
            ("app::shapes::Base::draw".to_owned(), Resolution::Ambiguous),
            ("~super".to_owned(), Resolution::External)
        ]
    );
    Ok(())
}

#[test]
fn a_dotted_python_import_binds_its_first_segment() -> TestResult {
    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        ("pyproject.toml", "[project]\nname = \"app\"\n"),
        ("app/__init__.py", ""),
        ("app/util.py", "def helper():\n    return 2\n"),
        (
            "app/main.py",
            "import app.util\nimport os.path\n\ndef run():\n    return app.util.helper() + os.path.join()\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let run = calls(&index, "app::main::run")?;
    assert!(run.contains(&exact("app::util::helper")), "{run:?}");
    assert!(
        run.contains(&("~os::path::join".to_owned(), Resolution::External)),
        "{run:?}"
    );
    Ok(())
}

#[test]
fn a_kebab_case_module_resolves_through_a_namespace_import() -> TestResult {
    if !mapped("typescript") {
        return Ok(());
    }
    let dir = tree(&[
        ("package.json", "{\"name\": \"web\"}"),
        (
            "src/string-utils.ts",
            "export function fmt(s: string): string {\n  return s;\n}\n",
        ),
        (
            "src/api/client.ts",
            "export function get(): string {\n  return 'x';\n}\n",
        ),
        (
            "src/main.ts",
            "import * as su from './string-utils';\nimport * as api from './api/client';\n\
             export function run(): string {\n  return su.fmt('a') + api.get();\n}\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let paths: Vec<String> = index
        .nodes()
        .filter_map(|n| index.path(n.id).map(ToString::to_string))
        .collect();
    let run = paths
        .iter()
        .find(|p| p.ends_with("main::run"))
        .ok_or_else(|| format!("no run in {paths:?}"))?;
    let found = calls(&index, run)?;
    assert!(
        found
            .iter()
            .any(|(t, r)| t.ends_with("string-utils::fmt") && *r == Resolution::Exact),
        "{found:?}"
    );
    // An alias equal to the path's first segment still stands for the whole path.
    assert!(
        found
            .iter()
            .any(|(t, r)| t.ends_with("client::get") && *r == Resolution::Exact),
        "{found:?}"
    );
    Ok(())
}

#[test]
fn a_qualifier_does_not_reach_into_another_languages_package() -> TestResult {
    if crate::lang::rust::language_id().is_none() || !mapped("typescript") {
        return Ok(());
    }
    let dir = tree(&[
        (
            "Cargo.toml",
            "[package]\nname = \"svc\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub fn go() { api::get(); }\n"),
        ("api/package.json", "{\"name\": \"api\"}"),
        ("api/src/index.ts", "export function get() {}\n"),
    ])?;
    let index = index(dir.path())?;
    // A Rust path naming `api` means a crate nobody indexed, not the Node package.
    assert_eq!(
        calls(&index, "svc::go")?,
        [("~api::get".to_owned(), Resolution::External)]
    );
    Ok(())
}

#[test]
fn a_method_named_by_one_contract_dispatches_through_it() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub trait Shape { fn area(&self) -> u32; }\n\
         pub struct Square;\n\
         impl Shape for Square { fn area(&self) -> u32 { 4 } }\n\
         pub struct Circle;\n\
         impl Shape for Circle { fn area(&self) -> u32 { 3 } }\n\
         pub fn total(s: &dyn Shape) -> u32 { s.area() }\n",
    )?;
    let index = index(dir.path())?;
    let total = calls(&index, "k::total")?;
    assert!(
        total.contains(&("k::Shape::area".to_owned(), Resolution::Dispatch)),
        "{total:?}"
    );
    assert_eq!(
        total
            .iter()
            .filter(|(_, r)| *r == Resolution::DispatchImpl)
            .count(),
        2,
        "{total:?}"
    );
    Ok(())
}

#[test]
fn an_unrelated_method_of_the_same_name_keeps_the_call_ambiguous() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub trait Render { fn render(&self) -> u32; }\n\
         pub struct Card;\n\
         impl Render for Card { fn render(&self) -> u32 { 1 } }\n\
         pub struct Page;\n\
         impl Page { pub fn render(&self) -> u32 { 2 } }\n\
         pub fn show(p: &Page) -> u32 { p.render() }\n",
    )?;
    let index = index(dir.path())?;
    let show = calls(&index, "k::show")?;
    // `Page::render` implements nothing, so the call may be to it: nothing is established.
    assert!(
        show.iter().all(|(_, r)| *r == Resolution::Ambiguous),
        "{show:?}"
    );
    assert!(
        show.contains(&("k::Page::render".to_owned(), Resolution::Ambiguous)),
        "{show:?}"
    );
    Ok(())
}

#[test]
fn the_graph_answers_callers_reached_by_and_external_apis() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub fn top() { mid(); mid(); }\n\
         fn mid() { leaf(); let v: Vec<u8> = Vec::new(); v.len(); v.len(); }\n\
         fn leaf() {}\n\
         pub const X: u32 = { 1 };\n",
    )?;
    let index = index(dir.path())?;
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let (top, mid, leaf) = (
        id(&index, "k::top")?,
        id(&index, "k::mid")?,
        id(&index, "k::leaf")?,
    );
    assert_eq!(graph.callees(top, Bound::Lower), [(mid, 2)]);
    assert_eq!(graph.callers(leaf, Bound::Lower), [mid]);
    assert_eq!(graph.reached_by(leaf, Bound::Lower), [mid, top]);
    // `Vec::new` and `.len` — two distinct, however often.
    assert_eq!(graph.metrics(mid).map(|m| m.external_apis), Some(2));
    assert_eq!(graph.metrics(mid).map(|m| m.fan_in), Some(1));
    assert_eq!(graph.len(), 3);
    Ok(())
}

#[test]
fn a_resolver_earlier_in_the_chain_answers_first() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    struct Everything;
    impl CallResolver for Everything {
        fn resolve(
            &self,
            _: &crate::index::SeamIndex,
            _: SeamId,
            site: &site::CallSite,
        ) -> Option<Resolved> {
            // Claims only the sites it knows; the rest fall through.
            (site.name.as_deref() == Some("b")).then(|| Resolved::external("semantic::b"))
        }
    }
    let dir = rust("pub fn a() { b(); c(); }\nfn b() {}\nfn c() {}\n")?;
    let index = index(dir.path())?;
    let names = NameResolver::new(&index);
    let edges = super::super::resolve(&index, &[&Everything, &names]);
    let mut seen: Vec<(String, Resolution)> = edges
        .iter()
        .map(|edge| {
            let target = edge.to.display_hint().map_or_else(
                || {
                    edge.to
                        .resolved()
                        .and_then(|id| index.path(id))
                        .map(ToString::to_string)
                        .unwrap_or_default()
                },
                str::to_owned,
            );
            (target, edge.resolution.unwrap_or(Resolution::Unresolved))
        })
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            exact("k::c"),
            ("semantic::b".to_owned(), Resolution::External)
        ]
    );
    Ok(())
}

#[test]
fn calls_outside_any_function_are_counted_not_lost() -> TestResult {
    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        ("pyproject.toml", "[project]\nname = \"app\"\n"),
        ("app/__init__.py", ""),
        (
            "app/boot.py",
            "def setup():\n    return 1\n\nsetup()\nsetup()\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let loose: u32 = index
        .calls()
        .files()
        .iter()
        .map(|(_, calls)| calls.loose_sites)
        .sum();
    assert_eq!(loose, 2);
    Ok(())
}

//! End-to-end call-tier tests: real source, parsed, linked and measured.

use std::path::Path;

use karet_treesitter::LanguageId;
use karet_treesitter::Query;

use super::*;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::id::SeamId;
use crate::id::SeamPath;
use crate::package::IndexOptions;

mod scope;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Every grammar a registered mapping serves.
fn mapped_languages() -> Vec<(LanguageId, &'static dyn crate::lang::SeamLanguage)> {
    [
        "rust",
        "python",
        "javascript",
        "typescript",
        "tsx",
        "swift",
        "kotlin",
    ]
    .into_iter()
    .filter_map(karet_treesitter::language_id_from_injection_name)
    .filter_map(|id| Some((id, crate::lang::for_language(id)?)))
    .collect()
}

#[test]
fn every_call_query_compiles_and_names_only_known_captures() -> TestResult {
    for (language, mapping) in mapped_languages() {
        let Some(source) = mapping.call_query(language) else {
            return Err(format!("no call query for {language:?}").into());
        };
        let query = Query::compile(language, source)
            .map_err(|error| format!("call query for {language:?}: {error}"))?;
        for name in query.capture_names() {
            assert!(
                name.starts_with('_') || site::CAPTURES.contains(name),
                "{language:?} names an unknown capture `{name}`"
            );
        }
        assert!(
            query.capture_names().contains(&site::CALL),
            "{language:?} marks no calls"
        );
    }
    Ok(())
}

/// A tree of files under a temporary root.
fn tree(files: &[(&str, &str)]) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    for (path, text) in files {
        let path = dir.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(dir)
}

/// A one-crate workspace holding `lib`.
fn rust(lib: &str) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    tree(&[
        (
            "Cargo.toml",
            "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", lib),
    ])
}

fn index(root: &Path) -> Result<crate::index::SeamIndex, Box<dyn std::error::Error>> {
    Ok(crate::package::index_workspace(
        root,
        IndexOptions::default(),
    )?)
}

fn id(index: &crate::index::SeamIndex, path: &str) -> Result<SeamId, Box<dyn std::error::Error>> {
    let parsed: SeamPath = path.parse()?;
    index
        .resolve(&parsed)
        .filter(|id| index.node(*id).is_some())
        .ok_or_else(|| format!("no node at {path}").into())
}

/// The calls leaving `path`, as `(target path or display, resolution)`, sorted.
fn calls(
    index: &crate::index::SeamIndex,
    path: &str,
) -> Result<Vec<(String, Resolution)>, Box<dyn std::error::Error>> {
    let from = id(index, path)?;
    let mut out: Vec<(String, Resolution)> = index
        .edges()
        .from_of_kind(from, crate::edge::EdgeKind::Calls)
        .map(|edge| {
            let target = match &edge.to {
                Endpoint::Resolved(to) => {
                    index.path(*to).map(ToString::to_string).unwrap_or_default()
                },
                other => format!("~{}", other.display_hint().unwrap_or("?")),
            };
            (target, edge.resolution.unwrap_or(Resolution::Unresolved))
        })
        .collect();
    out.sort();
    Ok(out)
}

fn exact(target: &str) -> (String, Resolution) {
    (target.to_owned(), Resolution::Exact)
}

#[test]
fn rust_calls_resolve_through_every_tier() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "use std::collections::HashMap;\n\
         mod util {\n\
             pub fn helper() -> u32 { 1 }\n\
         }\n\
         use util::helper;\n\
         pub struct Widget;\n\
         impl Widget {\n\
             pub fn new() -> Self { Widget }\n\
             fn size(&self) -> u32 { helper() }\n\
             pub fn render(&self) -> u32 {\n\
                 let map: HashMap<u32, u32> = HashMap::new();\n\
                 self.size() + Self::new().size() + util::helper() + map.len() as u32\n\
             }\n\
         }\n\
         pub fn run() -> Option<u32> { Some(Widget::new().render()) }\n",
    )?;
    let index = index(dir.path())?;

    assert_eq!(
        calls(&index, "k::Widget::size")?,
        [exact("k::util::helper")]
    );
    let render = calls(&index, "k::Widget::render")?;
    assert!(render.contains(&exact("k::Widget::size")), "{render:?}");
    assert!(render.contains(&exact("k::Widget::new")), "{render:?}");
    assert!(render.contains(&exact("k::util::helper")), "{render:?}");
    // An imported path the index does not hold belongs to someone else.
    assert!(
        render.contains(&(
            "~std::collections::HashMap::new".to_owned(),
            Resolution::External
        )),
        "{render:?}"
    );
    // A method nobody here declares is someone else's type's.
    assert!(
        render.contains(&("~.len".to_owned(), Resolution::External)),
        "{render:?}"
    );
    let run = calls(&index, "k::run")?;
    assert!(
        run.contains(&("~Some".to_owned(), Resolution::External)),
        "{run:?}"
    );
    // Called on what `new()` returns, whose type names alone cannot give.
    assert!(
        run.contains(&("k::Widget::render".to_owned(), Resolution::Ambiguous)),
        "{run:?}"
    );
    Ok(())
}

#[test]
fn rust_lines_count_code_and_leave_out_comments_and_nested_functions() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "/// Documented.\n\
         pub fn outer() -> u32 {\n\
         \n\
             // a comment line\n\
             fn inner() -> u32 {\n\
                 1\n\
             }\n\
             let add = |x: u32| {\n\
                 x + 1\n\
             };\n\
             add(inner()) /* trailing */\n\
         }\n",
    )?;
    let index = index(dir.path())?;
    let outer = index
        .calls()
        .callable(id(&index, "k::outer")?)
        .cloned()
        .unwrap_or_default();
    let inner = index
        .calls()
        .callable(id(&index, "k::outer::inner")?)
        .cloned()
        .unwrap_or_default();
    // outer: signature, `let add`, `x + 1`, `};`, `add(…)`, `}` — the closure is outer's.
    assert_eq!(outer.code_lines, 6);
    assert_eq!(inner.code_lines, 3);
    assert_eq!(outer.span_lines, 11);
    assert!(outer.has_body && inner.has_body);
    Ok(())
}

#[test]
fn a_trait_call_reaches_its_declaration_and_every_implementation() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub trait Shape { fn area(&self) -> u32; }\n\
         pub struct Square;\n\
         impl Shape for Square { fn area(&self) -> u32 { 4 } }\n\
         pub struct Circle;\n\
         impl Shape for Circle { fn area(&self) -> u32 { 3 } }\n\
         pub fn total(shape: &dyn Shape) -> u32 { Shape::area(shape) }\n",
    )?;
    let index = index(dir.path())?;
    let total = calls(&index, "k::total")?;
    assert!(
        total.contains(&("k::Shape::area".to_owned(), Resolution::Dispatch)),
        "{total:?}"
    );
    let implementations = total
        .iter()
        .filter(|(_, resolution)| *resolution == Resolution::DispatchImpl)
        .count();
    assert_eq!(implementations, 2, "{total:?}");

    let graph = CallGraph::build(&index, &GraphOptions::default());
    let metrics = graph
        .metrics(id(&index, "k::total")?)
        .cloned()
        .ok_or("no metrics")?;
    // The declaration has no body; the implementations are only plausible.
    assert_eq!(metrics.inclusive_lower, 1);
    assert_eq!(metrics.inclusive_upper, 3);
    Ok(())
}

#[test]
fn same_named_functions_in_sibling_modules_are_ambiguous() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "mod a { pub fn parse() -> u32 { 1 } }\n\
         mod b { pub fn parse() -> u32 { 2 } }\n\
         pub fn go() -> u32 { parse() }\n\
         pub fn missing() -> u32 { nowhere() }\n",
    )?;
    let index = index(dir.path())?;
    let go = calls(&index, "k::go")?;
    assert_eq!(
        go,
        [
            ("k::a::parse".to_owned(), Resolution::Ambiguous),
            ("k::b::parse".to_owned(), Resolution::Ambiguous)
        ]
    );
    assert_eq!(
        calls(&index, "k::missing")?,
        [("~nowhere".to_owned(), Resolution::Unresolved)]
    );
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let metrics = graph
        .metrics(id(&index, "k::go")?)
        .cloned()
        .ok_or("no metrics")?;
    assert_eq!(metrics.inclusive_lower, 1);
    assert_eq!(
        metrics.inclusive_upper,
        3,
        "{:?} {:?}",
        graph.callees(id(&index, "k::go")?, Bound::Upper),
        graph.all()
    );
    assert_eq!(
        metrics.fan_out, 0,
        "ambiguous calls are not established ones"
    );
    Ok(())
}

#[test]
fn twins_gated_for_different_hosts_resolve_to_the_live_one() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "#[cfg(unix)]\nfn sep() -> char { '/' }\n\
         #[cfg(windows)]\nfn sep() -> char { '\\\\' }\n\
         pub fn join() -> char { sep() }\n",
    )?;
    let index = index(dir.path())?;
    let join = calls(&index, "k::join")?;
    assert_eq!(join.len(), 1, "{join:?}");
    assert_eq!(join.first().map(|(_, r)| *r), Some(Resolution::Exact));
    Ok(())
}

#[test]
fn test_code_is_left_out_unless_the_configuration_builds_tests() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub fn shared() -> u32 { 1 }\n\
         #[cfg(test)]\nmod tests {\n    fn check() { super::shared(); }\n}\n",
    )?;
    let index = index(dir.path())?;
    let shared = id(&index, "k::shared")?;
    let default = CallGraph::build(&index, &GraphOptions::default());
    assert_eq!(default.metrics(shared).map(|m| m.fan_in), Some(0));
    assert!(default.metrics(id(&index, "k::tests::check")?).is_none());

    let testing = CallGraph::build(
        &index,
        &GraphOptions {
            configuration: crate::config::Configuration::unconfigured().for_target("test"),
            ..GraphOptions::default()
        },
    );
    assert_eq!(testing.metrics(shared).map(|m| m.fan_in), Some(1));
    Ok(())
}

#[test]
fn recursion_is_measured_once_and_reported() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub fn even(n: u32) -> bool {\n    if n == 0 { true } else { odd(n - 1) }\n}\n\
         pub fn odd(n: u32) -> bool {\n    if n == 0 { false } else { even(n - 1) }\n}\n\
         pub fn main_entry() -> bool { even(4) }\n",
    )?;
    let index = index(dir.path())?;
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let even = graph
        .metrics(id(&index, "k::even")?)
        .cloned()
        .ok_or("no even")?;
    let odd = graph
        .metrics(id(&index, "k::odd")?)
        .cloned()
        .ok_or("no odd")?;
    let entry = graph
        .metrics(id(&index, "k::main_entry")?)
        .cloned()
        .ok_or("no entry")?;
    assert_eq!(even.scc, odd.scc);
    assert_eq!(even.scc_size, 2);
    assert!(even.recursive && !entry.recursive);
    assert_eq!(even.inclusive_lower, 6);
    assert_eq!(entry.inclusive_lower, 7);
    assert_eq!(entry.depth, 1);

    let reach = graph.reach(id(&index, "k::main_entry")?, Bound::Lower);
    assert_eq!(reach.len(), 2);
    // A recursive function reaches itself; a plain one does not.
    assert!(
        graph
            .reach(id(&index, "k::even")?, Bound::Lower)
            .contains(&id(&index, "k::even")?)
    );
    assert!(!reach.contains(&id(&index, "k::main_entry")?));
    Ok(())
}

#[test]
fn a_cached_build_links_exactly_as_a_cold_one() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    struct Replaying(std::sync::Mutex<Vec<crate::contribution::FileContribution>>);
    impl crate::parallel::IndexObserver for Replaying {
        fn cached(
            &self,
            file: &Path,
            stamp: crate::contribution::FileStamp,
        ) -> Option<crate::contribution::FileContribution> {
            self.0
                .lock()
                .ok()?
                .iter()
                .find(|c| c.file == file && c.matches(stamp))
                .cloned()
        }
    }
    struct Recording(std::sync::Mutex<Vec<crate::contribution::FileContribution>>);
    impl crate::parallel::IndexObserver for Recording {
        fn package_indexed(&self, indexed: &mut crate::parallel::IndexedPackage) {
            if let Ok(mut held) = self.0.lock() {
                held.extend(indexed.contributions.iter().cloned());
            }
        }
    }

    let dir = tree(&[
        (
            "Cargo.toml",
            "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
        ),
        (
            "src/lib.rs",
            "mod net;\npub struct W;\nimpl W { pub fn go(&self) { net::send(); } }\n",
        ),
        ("src/net.rs", "pub fn send() { helper() }\nfn helper() {}\n"),
    ])?;
    let recording = Recording(std::sync::Mutex::new(Vec::new()));
    let cold =
        crate::parallel::index_workspace_with(dir.path(), IndexOptions::default(), &recording)?;
    let stored = recording.0.into_inner().unwrap_or_default();
    assert!(stored.iter().any(|c| !c.callables.is_empty()));
    let warm = crate::parallel::index_workspace_with(
        dir.path(),
        IndexOptions::default(),
        &Replaying(std::sync::Mutex::new(stored)),
    )?;
    for path in ["k::W::go", "k::net::send"] {
        assert_eq!(calls(&cold, path)?, calls(&warm, path)?, "{path}");
    }
    assert_eq!(calls(&cold, "k::W::go")?, [exact("k::net::send")]);
    Ok(())
}

#[test]
fn reindexing_a_file_relinks_the_calls_into_it() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        (
            "Cargo.toml",
            "[package]\nname = \"k\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "mod net;\npub fn go() { net::send(); }\n"),
        ("src/net.rs", "pub fn send() {}\n"),
    ])?;
    let mut index = index(dir.path())?;
    assert_eq!(calls(&index, "k::go")?, [exact("k::net::send")]);

    let net = dir.path().join("src/net.rs");
    let mut pool = karet_treesitter::ParserPool::new();
    crate::package::reindex_file(
        &mut index,
        &mut pool,
        &net,
        "pub fn send() { more() }\nfn more() {}\n",
    )?;
    // The edit replaced `send`, and the call into it must follow.
    assert_eq!(calls(&index, "k::go")?, [exact("k::net::send")]);
    assert_eq!(calls(&index, "k::net::send")?, [exact("k::net::more")]);

    crate::package::reindex_file(&mut index, &mut pool, &net, "pub fn renamed() {}\n")?;
    assert_eq!(
        calls(&index, "k::go")?,
        [("~net::send".to_owned(), Resolution::Unresolved)]
    );
    Ok(())
}

#[test]
fn reach_and_callers_terms_follow_established_calls_only() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub fn a() { b() }\nfn b() { c() }\nfn c() {}\n\
         mod x { pub fn d() {} }\nmod y { pub fn d() {} }\npub fn e() { d() }\n",
    )?;
    let index = index(dir.path())?;
    let names = |query: &str| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let parsed = crate::query::parse(query)?;
        let mut out: Vec<String> = crate::query::evaluate(&parsed, &index)
            .nodes
            .iter()
            .filter_map(|id| index.path(*id).map(ToString::to_string))
            .collect();
        out.sort();
        Ok(out)
    };
    assert_eq!(names("reach:k::a")?, ["k::b", "k::c"]);
    assert_eq!(names("callers:k::c")?, ["k::a", "k::b"]);
    assert_eq!(names("reach:k::a !c")?, ["k::b"]);
    // Ambiguous calls are not followed.
    assert!(names("reach:k::e")?.is_empty());
    assert!(names("reach:k::nothing")?.is_empty());
    Ok(())
}

#[test]
fn python_calls_resolve_and_tests_are_left_out() -> TestResult {
    if crate::lang::python::language_id().is_none() {
        return Ok(());
    }
    let dir = tree(&[
        ("pyproject.toml", "[project]\nname = \"app\"\n"),
        ("app/__init__.py", ""),
        (
            "app/core.py",
            "\"\"\"Module docs.\"\"\"\nimport os\nfrom app.util import helper as h\n\n\
             class Engine:\n    \"\"\"Docs.\"\"\"\n    def start(self):\n        # comment\n        \
             return self.step() + h() + len(os.listdir('.'))\n\n    def step(self):\n        \
             return 1\n",
        ),
        ("app/util.py", "def helper():\n    return 2\n"),
        (
            "app/tests/test_core.py",
            "from app.core import Engine\n\ndef test_start():\n    Engine().start()\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let start = calls(&index, "app::core::Engine::start")?;
    assert!(
        start.contains(&exact("app::core::Engine::step")),
        "{start:?}"
    );
    assert!(start.contains(&exact("app::util::helper")), "{start:?}");
    assert!(
        start.contains(&("~len".to_owned(), Resolution::External)),
        "{start:?}"
    );
    let engine = index
        .calls()
        .callable(id(&index, "app::core::Engine::start")?)
        .cloned()
        .unwrap_or_default();
    // `def start(self):` and the return; the comment is not code.
    assert_eq!(engine.code_lines, 2);

    let graph = CallGraph::build(&index, &GraphOptions::default());
    let start = id(&index, "app::core::Engine::start")?;
    assert_eq!(
        graph.metrics(start).map(|m| m.fan_in),
        Some(0),
        "a test is not a caller"
    );
    Ok(())
}

#[test]
fn typescript_calls_resolve_through_this_imports_and_new() -> TestResult {
    if !mapped("typescript") {
        return Ok(());
    }
    let dir = tree(&[
        ("package.json", "{\"name\": \"web\"}"),
        (
            "src/widget.ts",
            "import { format as fmt } from './strings';\n\
             export class Widget {\n  render(): string {\n    return this.label() + fmt('x');\n  }\n  \
             label(): string { return 'w'; }\n}\n\
             export function make(): string {\n  // build one\n  return new Widget().render();\n}\n",
        ),
        (
            "src/strings.ts",
            "export function format(s: string): string {\n  return s;\n}\n",
        ),
    ])?;
    let index = index(dir.path())?;
    let paths: Vec<String> = index
        .nodes()
        .filter_map(|n| index.path(n.id).map(ToString::to_string))
        .collect();
    let render = paths
        .iter()
        .find(|p| p.ends_with("Widget::render"))
        .ok_or_else(|| format!("no render in {paths:?}"))?;
    let found = calls(&index, render)?;
    assert!(
        found
            .iter()
            .any(|(t, r)| t.ends_with("Widget::label") && *r == Resolution::Exact),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(t, r)| t.ends_with("strings::format") && *r == Resolution::Exact),
        "{found:?}"
    );
    let make = paths
        .iter()
        .find(|p| p.ends_with("::make"))
        .ok_or("no make")?;
    let made = calls(&index, make)?;
    assert!(
        made.iter().any(|(t, _)| t.ends_with("Widget::render")),
        "{made:?}"
    );
    assert!(
        made.iter().any(|(t, _)| t.ends_with("::Widget")),
        "{made:?}"
    );
    Ok(())
}

/// Whether a language mapping is compiled in for the grammar named `name`.
fn mapped(name: &str) -> bool {
    karet_treesitter::language_id_from_injection_name(name)
        .and_then(crate::lang::for_language)
        .is_some()
}

#[test]
fn swift_and_kotlin_resolve_self_and_free_calls() -> TestResult {
    if mapped("swift") {
        let dir = tree(&[
            ("Package.swift", "// swift-tools-version:5.9\n"),
            (
                "Sources/App/Widget.swift",
                "struct Widget {\n    func render() -> Int {\n        return self.size() + helper()\n    }\n    \
                 func measure() -> Int {\n        return size()\n    }\n    \
                 func size() -> Int { return 1 }\n}\n\nfunc helper() -> Int {\n    return 2\n}\n\n\
                 func size() -> Int {\n    return 3\n}\n",
            ),
        ])?;
        let index = index(dir.path())?;
        let paths: Vec<String> = index
            .nodes()
            .filter_map(|n| index.path(n.id).map(ToString::to_string))
            .collect();
        let render = paths
            .iter()
            .find(|p| p.ends_with("Widget::render"))
            .ok_or_else(|| format!("{paths:?}"))?;
        let found = calls(&index, render)?;
        assert!(
            found
                .iter()
                .any(|(t, r)| t.ends_with("Widget::size") && *r == Resolution::Exact),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|(t, r)| t.ends_with("::helper") && *r == Resolution::Exact),
            "{found:?}"
        );
        assert_bare_call_reaches_own_member(&index, &paths)?;
    }
    if mapped("kotlin") {
        let dir = tree(&[
            ("build.gradle.kts", "plugins {}\n"),
            (
                "src/main/kotlin/app/Widget.kt",
                "package app\n\nclass Widget {\n    fun render(): Int {\n        return this.size() + helper()\n    }\n    \
                 fun measure(): Int {\n        return size()\n    }\n    \
                 fun size(): Int = 1\n}\n\nfun helper(): Int {\n    return 2\n}\n\nfun size(): Int {\n    return 3\n}\n",
            ),
        ])?;
        let index = index(dir.path())?;
        let paths: Vec<String> = index
            .nodes()
            .filter_map(|n| index.path(n.id).map(ToString::to_string))
            .collect();
        let render = paths
            .iter()
            .find(|p| p.ends_with("Widget::render"))
            .ok_or_else(|| format!("{paths:?}"))?;
        let found = calls(&index, render)?;
        assert!(
            found
                .iter()
                .any(|(t, r)| t.ends_with("Widget::size") && *r == Resolution::Exact),
            "{found:?}"
        );
        assert!(
            found
                .iter()
                .any(|(t, r)| t.ends_with("::helper") && *r == Resolution::Exact),
            "{found:?}"
        );
        assert_bare_call_reaches_own_member(&index, &paths)?;
    }
    Ok(())
}

/// A bare `size()` inside `Widget::measure` is the implicit receiver's member, and
/// beats the file-level `size()` the fixture also declares.
fn assert_bare_call_reaches_own_member(
    index: &crate::index::SeamIndex,
    paths: &[String],
) -> TestResult {
    let measure = paths
        .iter()
        .find(|p| p.ends_with("Widget::measure"))
        .ok_or_else(|| format!("{paths:?}"))?;
    let found = calls(index, measure)?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found
            .iter()
            .all(|(t, r)| t.ends_with("Widget::size") && *r == Resolution::Exact),
        "{found:?}"
    );
    Ok(())
}

#[test]
fn too_many_candidates_count_in_neither_bound() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let mut lib = String::new();
    for n in 0..=MAX_CANDIDATES_FOR_TEST {
        lib.push_str(&format!(
            "pub struct T{n};\nimpl T{n} {{ pub fn get(&self) {{}} }}\n"
        ));
    }
    lib.push_str("pub fn call(x: &T0) { x.get() }\n");
    let dir = rust(&lib)?;
    let index = index(dir.path())?;
    let found = calls(&index, "k::call")?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found.first().map(|(_, r)| *r), Some(Resolution::Ambiguous));
    assert!(
        found.first().is_some_and(|(t, _)| t.starts_with("~get ×")),
        "{found:?}"
    );
    let graph = CallGraph::build(&index, &GraphOptions::default());
    let metrics = graph
        .metrics(id(&index, "k::call")?)
        .cloned()
        .ok_or("no metrics")?;
    assert_eq!(metrics.inclusive_upper, metrics.inclusive_lower);
    Ok(())
}

const MAX_CANDIDATES_FOR_TEST: usize = resolve::MAX_CANDIDATES;

#[test]
fn a_unique_method_name_is_exact_only_on_a_named_receiver() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub struct Picker;\n\
         impl Picker { pub fn filter(&self) -> u32 { 1 } }\n\
         pub fn named(p: &Picker) -> u32 { p.filter() }\n\
         pub fn chained(xs: &[u32]) -> usize { xs.iter().filter(|x| **x > 1).count() }\n",
    )?;
    let index = index(dir.path())?;
    assert_eq!(calls(&index, "k::named")?, [exact("k::Picker::filter")]);
    // A chain's value is a library iterator; the lone `filter` here is a coincidence.
    let chained = calls(&index, "k::chained")?;
    assert!(
        chained.contains(&("k::Picker::filter".to_owned(), Resolution::Ambiguous)),
        "{chained:?}"
    );
    Ok(())
}

#[test]
fn an_impl_trait_return_does_not_make_a_method_dispatch() -> TestResult {
    if crate::lang::rust::language_id().is_none() {
        return Ok(());
    }
    let dir = rust(
        "pub struct Store;\n\
         impl Store { pub fn items(&self) -> impl Iterator<Item = u32> { 0..1 } }\n\
         pub struct Other;\n\
         impl Other { pub fn items(&self) -> u32 { 2 } }\n\
         pub fn use_it(s: &Store) -> u32 { Store::items(s).sum() }\n",
    )?;
    let index = index(dir.path())?;
    let found = calls(&index, "k::use_it")?;
    assert!(found.contains(&exact("k::Store::items")), "{found:?}");
    assert!(
        !found
            .iter()
            .any(|(_, r)| matches!(r, Resolution::Dispatch | Resolution::DispatchImpl)),
        "{found:?}"
    );
    Ok(())
}

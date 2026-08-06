//! Grammar-backed extraction into the neutral core symbol model.

use std::collections::HashMap;

use karet_core::LineCol;
use karet_core::Range;
use karet_core::Span;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredTree;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_injection_name;
use karet_treesitter::outline_query;

mod names;
mod structured;
#[cfg(test)]
mod web_tests;

use names::clean_name;
use names::clean_subroutine_name;
use names::kind_rank;
use names::symbol_kind;

#[derive(Debug)]
struct Candidate {
    span: Span,
    symbol: Symbol,
    heading_level: Option<u8>,
}

/// Cached query runner that extracts hierarchical document symbols.
#[derive(Default)]
pub struct OutlineExtractor {
    queries: HashMap<LanguageId, Option<Query>>,
}

impl OutlineExtractor {
    /// Create an empty outline-query cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Extract ordered, hierarchical symbols from `tree` and its matching `text`.
    ///
    /// A language without an outline query, or a query that cannot compile,
    /// degrades to an empty outline. Recoverable parse errors do not suppress the
    /// declarations tree-sitter can still identify.
    #[must_use]
    pub fn analyze(&mut self, tree: &SyntaxTree, text: &str) -> Vec<Symbol> {
        structured::analyze(tree.language(), text)
            .unwrap_or_else(|| finish(self.candidates(tree, text)))
    }

    /// Extract one ordered outline from a root tree and its injected languages.
    ///
    /// Host and injected declarations share document coordinates, so component
    /// scripts naturally nest under host sections. Markdown and MDX are deliberate
    /// exceptions: fenced examples are illustrative code, not document symbols.
    /// MDX keeps its heading tree and merges its own declarations at the top level.
    #[must_use]
    pub fn analyze_layers(&mut self, tree: &LayeredTree, text: &str) -> Vec<Symbol> {
        if let Some(symbols) = structured::analyze(tree.root().language(), text) {
            return symbols;
        }
        let root_candidates = self.candidates(tree.root(), text);
        if language_id_from_injection_name("markdown") == Some(tree.root().language()) {
            return finish(root_candidates);
        }
        if language_id_from_injection_name("mdx") == Some(tree.root().language()) {
            let (headings, declarations) = root_candidates
                .into_iter()
                .partition(|candidate| candidate.heading_level.is_some());
            let mut symbols = finish(headings);
            symbols.extend(finish(declarations));
            symbols.sort_by_key(|symbol| symbol.range.start);
            return symbols;
        }

        let mut candidates = root_candidates;
        for layer in tree.children() {
            candidates.extend(self.candidates(layer, text));
        }
        finish(candidates)
    }

    fn candidates(&mut self, tree: &SyntaxTree, text: &str) -> Vec<Candidate> {
        let lang = tree.language();
        self.queries.entry(lang).or_insert_with(|| {
            outline_query(lang).and_then(|source| Query::compile(lang, &source).ok())
        });
        let Some(query) = self.queries.get(&lang).and_then(Option::as_ref) else {
            return Vec::new();
        };
        let capture_names = query.capture_names();
        let dynamic_headings = capture_names.contains(&"definition.heading");
        let starts = line_starts(text);
        let mut candidates = Vec::new();
        let mut heading_styles = Vec::new();
        for matched in tree.matches(query, text) {
            let mut definition = None;
            let mut name = None;
            let mut kind = SymbolKind::Variable;
            let mut heading_level = None;
            let mut subroutine_name = false;
            for capture in matched.captures {
                let Some(capture_name) = capture_names.get(capture.capture as usize) else {
                    continue;
                };
                if *capture_name == "name" {
                    name = Some(capture.span);
                } else if let Some(suffix) = capture_name.strip_prefix("definition.") {
                    definition = Some(capture.span);
                    kind = symbol_kind(suffix);
                    subroutine_name = suffix == "subroutine";
                    heading_level = suffix
                        .strip_prefix("heading.")
                        .and_then(|level| level.parse().ok());
                }
            }
            let (Some(definition), Some(name_span)) = (definition, name) else {
                continue;
            };
            let Some(raw_name) = text.get(name_span.start.0..name_span.end.0) else {
                continue;
            };
            let name = if subroutine_name {
                clean_subroutine_name(raw_name)
            } else {
                clean_name(raw_name)
            };
            if name.is_empty() {
                continue;
            }
            if heading_level.is_none() && dynamic_headings {
                heading_level = dynamic_heading_level(text, definition, &mut heading_styles);
            }
            candidates.push(Candidate {
                span: definition,
                heading_level,
                symbol: Symbol {
                    name,
                    kind,
                    detail: None,
                    range: to_range(&starts, text, definition),
                    selection_range: to_range(&starts, text, name_span),
                    container_name: None,
                    children: Vec::new(),
                },
            });
        }
        candidates
    }
}

fn finish(mut candidates: Vec<Candidate>) -> Vec<Symbol> {
    candidates.sort_by_key(|candidate| {
        (
            candidate.span.start.0,
            usize::MAX - candidate.span.end.0,
            kind_rank(candidate.symbol.kind),
        )
    });
    candidates
        .dedup_by(|left, right| left.span == right.span && left.symbol.name == right.symbol.name);
    let mut index = 0;
    if candidates
        .iter()
        .all(|candidate| candidate.heading_level.is_some())
    {
        nest_headings(&mut candidates, &mut index, None, None)
    } else {
        nest(&mut candidates, &mut index, None, None)
    }
}

fn dynamic_heading_level(text: &str, span: Span, styles: &mut Vec<String>) -> Option<u8> {
    let source = text.get(span.start.0..span.end.0)?;
    let markers: Vec<char> = source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let mut chars = trimmed.chars();
            let first = chars.next()?;
            (trimmed.len() >= 2 && first.is_ascii_punctuation() && chars.all(|ch| ch == first))
                .then_some(first)
        })
        .collect();
    let marker = *markers.first()?;
    let style = format!(
        "{}:{marker}",
        if markers.len() > 1 { "over" } else { "under" }
    );
    let index = styles
        .iter()
        .position(|candidate| *candidate == style)
        .unwrap_or_else(|| {
            styles.push(style);
            styles.len() - 1
        });
    u8::try_from(index + 1).ok()
}

fn nest_headings(
    candidates: &mut [Candidate],
    index: &mut usize,
    parent_level: Option<u8>,
    container: Option<&str>,
) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    while *index < candidates.len() {
        let level = candidates[*index].heading_level.unwrap_or(1);
        if parent_level.is_some_and(|parent| level <= parent) {
            break;
        }
        let mut symbol = std::mem::replace(&mut candidates[*index].symbol, empty_symbol());
        *index += 1;
        symbol.container_name = container.map(str::to_owned);
        let name = symbol.name.clone();
        symbol.children = nest_headings(candidates, index, Some(level), Some(&name));
        symbols.push(symbol);
    }
    symbols
}

fn nest(
    candidates: &mut [Candidate],
    index: &mut usize,
    parent: Option<Span>,
    container: Option<&str>,
) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    while *index < candidates.len() {
        let span = candidates[*index].span;
        if parent.is_some_and(|parent| !strictly_contains(parent, span)) {
            break;
        }
        let mut symbol = std::mem::replace(&mut candidates[*index].symbol, empty_symbol());
        *index += 1;
        symbol.container_name = container.map(str::to_owned);
        let name = symbol.name.clone();
        symbol.children = nest(candidates, index, Some(span), Some(&name));
        symbols.push(symbol);
    }
    symbols
}

fn strictly_contains(parent: Span, child: Span) -> bool {
    parent.start.0 <= child.start.0
        && child.end.0 <= parent.end.0
        && (parent.start.0 < child.start.0 || child.end.0 < parent.end.0)
}

fn empty_symbol() -> Symbol {
    Symbol {
        name: String::new(),
        kind: SymbolKind::Variable,
        detail: None,
        range: Range::default(),
        selection_range: Range::default(),
        container_name: None,
        children: Vec::new(),
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

fn to_range(starts: &[usize], text: &str, span: Span) -> Range {
    Range {
        start: line_col(starts, text, span.start.0),
        end: line_col(starts, text, span.end.0),
    }
}

fn line_col(starts: &[usize], text: &str, byte: usize) -> LineCol {
    let byte = byte.min(text.len());
    let line_index = starts
        .partition_point(|start| *start <= byte)
        .saturating_sub(1);
    let line = u32::try_from(line_index).unwrap_or(u32::MAX);
    let column = text
        .get(starts[line_index]..byte)
        .map_or(0, |slice| slice.chars().count());
    LineCol::new(line, u32::try_from(column).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_treesitter::ParserPool;
    use karet_treesitter::language_id_from_path;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn symbols(path: &str, source: &str) -> TestResult<Vec<Symbol>> {
        let language = language_id_from_path(Path::new(path)).ok_or("missing test grammar")?;
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, language, source)?;
        Ok(OutlineExtractor::new().analyze(&tree, source))
    }

    fn names(symbols: &[Symbol]) -> Vec<&str> {
        fn collect<'a>(symbols: &'a [Symbol], output: &mut Vec<&'a str>) {
            for symbol in symbols {
                output.push(&symbol.name);
                collect(&symbol.children, output);
            }
        }
        let mut output = Vec::new();
        collect(symbols, &mut output);
        output
    }

    fn assert_outline_names(
        path: &str,
        source: &str,
        expected: &[&str],
    ) -> TestResult<Vec<Symbol>> {
        let extracted = symbols(path, source)?;
        let actual = names(&extracted);
        for name in expected {
            assert!(
                actual.contains(name),
                "{path}: missing {name:?}: {extracted:#?}"
            );
        }
        Ok(extracted)
    }

    fn assert_symbol_container(symbols: &[Symbol], child: &str, parent: &str) {
        fn find<'a>(symbols: &'a [Symbol], name: &str) -> Option<&'a Symbol> {
            symbols
                .iter()
                .find(|symbol| symbol.name == name)
                .or_else(|| {
                    symbols
                        .iter()
                        .find_map(|symbol| find(&symbol.children, name))
                })
        }
        let symbol = find(symbols, child);
        assert_eq!(
            symbol.and_then(|symbol| symbol.container_name.as_deref()),
            Some(parent),
            "{child:?} should be nested under {parent:?}: {symbols:#?}"
        );
    }

    #[test]
    fn bundled_languages_extract_recoverable_non_ascii_declarations() -> TestResult {
        let cases = [
            (
                "lib.rs",
                "mod café { pub struct Thé; pub fn brew() {} } ???",
                "café",
            ),
            ("app.py", "class Café:\n  def brew(self): pass\n???", "Café"),
            (
                "app.js",
                "class Café { brew() {} }\nfunction serve() {}\n???",
                "Café",
            ),
            ("app.jsx", "function Café() { return <main/> }\n???", "Café"),
            (
                "app.ts",
                "interface Café { brew(): void }\nfunction serve() {}\n???",
                "Café",
            ),
            ("app.tsx", "function Café() { return <main/> }\n???", "Café"),
            (
                "main.go",
                "package café\ntype Thé struct{}\nfunc brew() {}\n???",
                "Thé",
            ),
            (
                "main.c",
                "struct Cafe { int cups; };\nvoid brew(void) {}\n???",
                "Cafe",
            ),
            (
                "main.cpp",
                "namespace café { class Thé {}; void brew() {} }\n???",
                "Thé",
            ),
            (
                "Main.cs",
                "namespace Café { class Thé { void Brew() {} } }\n???",
                "Café",
            ),
            ("Main.java", "class Café { void brew() {} }\n???", "Café"),
            (
                "main.rb",
                "module Café\n class Thé\n  def brew; end\n end\nend\n???",
                "Café",
            ),
            (
                "main.php",
                "<?php namespace Café; class Thé { function brew() {} } ???",
                "Café",
            ),
            ("tool.sh", "function café() { echo thé; }\n???", "café"),
            (
                "data.json",
                "{\"café\": {\"nested\": []}, \"broken\": [}",
                "café",
            ),
            (
                "data.yaml",
                "café:\n  nested:\n    - thé\nbroken: [\n",
                "café",
            ),
            (
                "Cargo.toml",
                "[\"café\"]\nname='thé'\n[[\"café\".items]]\n???",
                "café",
            ),
            (
                "page.html",
                "<main><section><h1>Café</h1></section></main><",
                "main",
            ),
            (
                "style.css",
                ".café { color: red; }\n@keyframes tourné { from {} }\n???",
                ".café",
            ),
            (
                "schema.sql",
                "CREATE SCHEMA café; CREATE TABLE café.thé (id int); ???",
                "café",
            ),
            (
                "schema.graphql",
                "\"Café schema\" type Coffee { roast: String } ???",
                "Coffee",
            ),
            (
                "schema.proto",
                "// Café service\nmessage Coffee { string roast = 1; } ???",
                "Coffee",
            ),
            (
                "Dockerfile",
                "# Café image\nFROM rust:latest AS builder\nRUN broken &&\n???",
                "builder",
            ),
            (
                "Makefile",
                "# Café build\nbuild: dep\n\t@echo ok\n???",
                "build",
            ),
            (
                "CMakeLists.txt",
                "# Café build\nfunction(café arg)\nendfunction()\n???",
                "café",
            ),
        ];
        for (path, source, expected) in cases {
            let extracted = symbols(path, source)?;
            assert!(
                names(&extracted).contains(&expected),
                "{path}: {extracted:#?}"
            );
        }
        Ok(())
    }

    #[test]
    fn every_bundled_outline_language_accepts_empty_input() -> TestResult {
        for path in [
            "lib.rs",
            "app.py",
            "app.js",
            "app.jsx",
            "app.ts",
            "app.tsx",
            "main.go",
            "main.c",
            "main.cpp",
            "Main.cs",
            "Main.java",
            "main.rb",
            "main.php",
            "tool.sh",
            "data.json",
            "data.jsonc",
            "data.yaml",
            "Cargo.toml",
            "page.html",
            "style.css",
            "schema.sql",
            "schema.graphql",
            "schema.proto",
            "Dockerfile",
            "Containerfile",
            "Makefile",
            "GNUmakefile",
            "rules.mk",
            "CMakeLists.txt",
            "module.cmake",
            "guide.rst",
            "guide.adoc",
            "paper.tex",
            "script.zsh",
            "script.fish",
            "profile.ps1",
            "build.cmd",
            "Main.kt",
            "Main.swift",
            "build.sbt",
            "init.lua",
            "Main.hs",
            "Main.lhs",
            "main.ml",
            "main.mli",
            "app.ex",
            "app.erl",
            "main.dart",
            "analysis.r",
            "main.zig",
            "tool.pl",
            "core.clj",
            "core.cljs",
            "core.cljc",
            "init.el",
            "plugin.vim",
        ] {
            assert!(symbols(path, "")?.is_empty(), "{path}");
        }
        Ok(())
    }

    #[test]
    fn nested_symbols_keep_ranges_selection_and_container_names() -> TestResult {
        let source = "mod café {\n  struct Thé;\n  fn brew() {}\n}\n";
        let extracted = symbols("lib.rs", source)?;
        let module = extracted
            .iter()
            .find(|symbol| symbol.name == "café")
            .ok_or("module")?;
        assert_eq!(module.selection_range.start, LineCol::new(0, 4));
        let child = module
            .children
            .iter()
            .find(|symbol| symbol.name == "Thé")
            .ok_or("nested struct")?;
        assert_eq!(child.container_name.as_deref(), Some("café"));
        assert!(child.range.start <= child.selection_range.start);
        assert!(child.selection_range.end <= child.range.end);
        Ok(())
    }

    #[test]
    fn query_and_schema_languages_keep_nested_declarations() -> TestResult {
        let sql = symbols(
            "schema.sql",
            "CREATE VIEW café.orders AS WITH récent AS (SELECT 1) SELECT * FROM récent; ???",
        )?;
        let view = sql
            .iter()
            .find(|symbol| symbol.name == "café.orders")
            .ok_or("SQL view")?;
        assert!(names(&view.children).contains(&"récent"), "{sql:#?}");

        let graphql = symbols(
            "schema.graphql",
            "\"Café\" type Coffee { roast: String } query Brew { coffee { roast } } ???",
        )?;
        let object = graphql
            .iter()
            .find(|symbol| symbol.name == "Coffee")
            .ok_or("GraphQL type")?;
        assert!(names(&object.children).contains(&"roast"), "{graphql:#?}");
        assert!(names(&graphql).contains(&"Brew"), "{graphql:#?}");

        let proto = symbols(
            "schema.proto",
            "// Café\nmessage Coffee { message Roast {} } service Brewer { rpc Brew (Coffee) returns (Coffee); } ???",
        )?;
        let message = proto
            .iter()
            .find(|symbol| symbol.name == "Coffee")
            .ok_or("Protobuf message")?;
        assert!(names(&message.children).contains(&"Roast"), "{proto:#?}");
        let service = proto
            .iter()
            .find(|symbol| symbol.name == "Brewer")
            .ok_or("Protobuf service")?;
        assert!(names(&service.children).contains(&"Brew"), "{proto:#?}");
        Ok(())
    }

    #[test]
    fn build_languages_expose_stages_targets_and_nested_blocks() -> TestResult {
        let container = symbols(
            "Containerfile",
            "# Café\nFROM rust AS builder\nRUN cargo build\nFROM alpine AS runtime\nCOPY --from=builder /app /app\n???",
        )?;
        assert_eq!(
            names(&container),
            vec!["builder", "runtime"],
            "{container:#?}"
        );

        let make = symbols(
            "GNUmakefile",
            "# Café\nifdef DEBUG\nbuild: compile\n\t@echo done\nendif\ndefine banner\nhello\nendef\n???",
        )?;
        let condition = make
            .iter()
            .find(|symbol| symbol.name.contains("DEBUG"))
            .ok_or("Make condition")?;
        assert!(names(&condition.children).contains(&"build"), "{make:#?}");
        assert!(names(&make).contains(&"banner"), "{make:#?}");

        let cmake = symbols(
            "CMakeLists.txt",
            "# Café\nif(ENABLED)\n  function(café arg)\n  endfunction()\n  add_executable(app main.cpp)\nendif()\n???",
        )?;
        let condition = cmake
            .iter()
            .find(|symbol| symbol.name == "ENABLED")
            .ok_or("CMake condition")?;
        assert!(names(&condition.children).contains(&"café"), "{cmake:#?}");
        assert!(names(&condition.children).contains(&"app"), "{cmake:#?}");
        Ok(())
    }

    #[test]
    fn document_markup_headings_preserve_hierarchy_and_unicode_ranges() -> TestResult {
        let rst = symbols(
            "guide.rst",
            "==========\nCafé guide\n==========\n\nSetup\n==========\n\nThé\n~~~\n\nUsage\n==========\n\n???\n",
        )?;
        let guide = rst.first().ok_or("reStructuredText title")?;
        assert_eq!(guide.name, "Café guide");
        assert_eq!(guide.selection_range.end.col, 10);
        assert_eq!(names(&guide.children), vec!["Setup", "Thé", "Usage"]);
        assert_eq!(
            guide.children[0].children[0].container_name.as_deref(),
            Some("Setup")
        );

        let asciidoc = symbols(
            "guide.adoc",
            "= Café guide\n\n== Setup\n\n=== Thé\n\n== Usage\n\n???\n",
        )?;
        let guide = asciidoc.first().ok_or("AsciiDoc title")?;
        assert_eq!(guide.name, "Café guide");
        assert_eq!(names(&guide.children), vec!["Setup", "Thé", "Usage"]);
        assert_eq!(
            guide.children[0].children[0].container_name.as_deref(),
            Some("Setup")
        );

        let latex = symbols(
            "paper.tex",
            "\\part{Café}\n\\chapter{Setup}\n\\section{Thé}\n\\subsection{Brew}\n\\chapter{Usage}\n???\n",
        )?;
        let part = latex.first().ok_or("LaTeX part")?;
        assert_eq!(
            names(std::slice::from_ref(part)),
            vec!["Café", "Setup", "Thé", "Brew", "Usage"]
        );
        assert_eq!(
            part.children[0].children[0].container_name.as_deref(),
            Some("Setup")
        );
        Ok(())
    }

    #[test]
    fn shell_languages_expose_named_declarations_without_control_flow() -> TestResult {
        let zsh = symbols(
            "script.zsh",
            "# Café\nouter() {\n  echo ok\n}\ninner() {\n  echo ok\n}\nif true; then echo no; fi\n???\n",
        )?;
        let outer = zsh.first().ok_or("Zsh function")?;
        assert_eq!(outer.name, "outer");
        assert_eq!(names(&zsh), vec!["outer", "inner"]);

        let fish = symbols(
            "script.fish",
            "function café\n  function inner\n    echo ok\n  end\nend\nif true\nend\n???\n",
        )?;
        let outer = fish.first().ok_or("Fish function")?;
        assert_eq!(outer.name, "café");
        assert_eq!(names(&outer.children), vec!["inner"]);

        let powershell = symbols(
            "profile.ps1",
            "# Café\nclass Cafe { [string] Brew() { return 'ok' } }\nfunction Serve { function Inner { } }\nfilter Clean { }\nenum Roast { Light; Dark }\nif ($true) { }\n???\n",
        )?;
        let class = powershell.first().ok_or("PowerShell class")?;
        assert_eq!(class.name, "Cafe");
        assert_eq!(names(&class.children), vec!["Brew"]);
        let serve = powershell
            .iter()
            .find(|symbol| symbol.name == "Serve")
            .ok_or("PowerShell function")?;
        assert_eq!(names(&serve.children), vec!["Inner"]);
        assert!(names(&powershell).contains(&"Clean"));
        assert!(names(&powershell).contains(&"Roast"));

        let batch = symbols(
            "build.cmd",
            "@echo off\n:build café target\necho ok\ngoto :eof\n:package\necho done\n???\n",
        )?;
        assert_eq!(names(&batch), vec!["build", "package"]);
        assert_eq!(batch[0].selection_range.start, LineCol::new(1, 0));
        Ok(())
    }

    #[test]
    fn jvm_native_and_functional_languages_extract_recoverable_unicode() -> TestResult {
        for (path, source, expected, nested) in [
            (
                "Main.kt",
                "package café\nclass Cafe()\nfun brew() { fun inner() {} }\n???\n",
                &["café", "Cafe", "brew", "inner"][..],
                Some(("inner", "brew")),
            ),
            (
                "Main.swift",
                "class Café { func brew() { func inner() {} } }\n???\n",
                &["Café", "brew", "inner"],
                Some(("inner", "brew")),
            ),
            (
                "build.sbt",
                "package café\nclass Thé { def brew = { def inner = 1 } }\n???\n",
                &["café", "Thé", "brew", "inner"],
                Some(("inner", "brew")),
            ),
            (
                "init.lua",
                "function café() local function inner() end end\n???\n",
                &["café", "inner"],
                Some(("inner", "café")),
            ),
            (
                "Main.hs",
                "module Café where\ndata Thé = Thé\nbrew x = let inner y = y in inner x\n???\n",
                &["Café", "Thé", "brew", "inner"],
                Some(("inner", "brew")),
            ),
            (
                "Main.lhs",
                "> module Café where\n> café x = x\n\nrecoverable prose ???\n",
                &["Café", "café"],
                None,
            ),
        ] {
            let extracted = assert_outline_names(path, source, expected)?;
            if let Some((child, parent)) = nested {
                assert_symbol_container(&extracted, child, parent);
            }
        }
        Ok(())
    }

    #[test]
    fn beam_ml_and_dart_languages_extract_recoverable_unicode() -> TestResult {
        for (path, source, expected, nested) in [
            (
                "main.ml",
                "module Café = struct let brew x = let inner y = y in inner x end\n???\n",
                &["Café", "brew", "inner"][..],
                Some(("inner", "brew")),
            ),
            (
                "main.mli",
                "module Café : sig val brew : unit -> unit end\n???\n",
                &["Café"],
                None,
            ),
            (
                "app.ex",
                "defmodule Cafe do\n  def café do\n    defp inner, do: :ok\n  end\nend\n???\n",
                &["Cafe", "café", "inner"],
                Some(("inner", "café")),
            ),
            (
                "app.erl",
                "-module('café').\nbrew() -> inner().\ninner() -> ok.\n???\n",
                &["café", "brew", "inner"],
                None,
            ),
            (
                "main.dart",
                "// Café\nclass Cafe { void brew() {} }\nvoid serve() { void inner() {} }\n???\n",
                &["Cafe", "brew", "serve"],
                Some(("brew", "Cafe")),
            ),
        ] {
            let extracted = assert_outline_names(path, source, expected)?;
            if let Some((child, parent)) = nested {
                assert_symbol_container(&extracted, child, parent);
            }
        }
        Ok(())
    }

    #[test]
    fn scripting_and_lisp_languages_extract_recoverable_unicode() -> TestResult {
        for (path, source, expected, nested) in [
            (
                "analysis.r",
                "café <- function() { inner <- function() {} }\n???\n",
                &["café", "inner"][..],
                Some(("inner", "café")),
            ),
            (
                "main.zig",
                "const @\"café\" = struct { fn brew() void {} };\n???\n",
                &["café", "brew"],
                Some(("brew", "café")),
            ),
            (
                "tool.pl",
                "# Café\npackage Cafe; sub brew { sub inner {} }\n???\n",
                &["Cafe", "brew", "inner"],
                Some(("inner", "brew")),
            ),
            (
                "core.cljc",
                "(ns café.core)\n(defrecord Thé [kind])\n(defn brew [] (defn inner [] nil))\n???\n",
                &["café.core", "Thé", "brew", "inner"],
                Some(("inner", "brew")),
            ),
            (
                "init.el",
                "(defun café () (defun inner () nil))\n???\n",
                &["café", "inner"],
                Some(("inner", "café")),
            ),
            (
                "plugin.vim",
                "\" Café\nfunction! cafe#brew()\n  function! s:inner()\n  endfunction\nendfunction\n???\n",
                &["cafe#brew", "s:inner"],
                Some(("s:inner", "cafe#brew")),
            ),
        ] {
            let extracted = assert_outline_names(path, source, expected)?;
            if let Some((child, parent)) = nested {
                assert_symbol_container(&extracted, child, parent);
            }
        }
        Ok(())
    }
}

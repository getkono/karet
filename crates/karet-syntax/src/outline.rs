//! Grammar-backed extraction into the neutral core symbol model.

use std::collections::HashMap;

use karet_core::LineCol;
use karet_core::Range;
use karet_core::Span;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_treesitter::LanguageId;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use karet_treesitter::outline_query;

#[derive(Debug)]
struct Candidate {
    span: Span,
    symbol: Symbol,
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
        let lang = tree.language();
        self.queries.entry(lang).or_insert_with(|| {
            outline_query(lang).and_then(|source| Query::compile(lang, &source).ok())
        });
        let Some(query) = self.queries.get(&lang).and_then(Option::as_ref) else {
            return Vec::new();
        };
        let capture_names = query.capture_names();
        let starts = line_starts(text);
        let mut candidates = Vec::new();
        for matched in tree.matches(query, text) {
            let mut definition = None;
            let mut name = None;
            let mut kind = SymbolKind::Variable;
            for capture in matched.captures {
                let Some(capture_name) = capture_names.get(capture.capture as usize) else {
                    continue;
                };
                if *capture_name == "name" {
                    name = Some(capture.span);
                } else if let Some(suffix) = capture_name.strip_prefix("definition.") {
                    definition = Some(capture.span);
                    kind = symbol_kind(suffix);
                }
            }
            let (Some(definition), Some(name_span)) = (definition, name) else {
                continue;
            };
            let Some(raw_name) = text.get(name_span.start.0..name_span.end.0) else {
                continue;
            };
            let name = clean_name(raw_name);
            if name.is_empty() {
                continue;
            }
            candidates.push(Candidate {
                span: definition,
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
        candidates.sort_by_key(|candidate| {
            (
                candidate.span.start.0,
                usize::MAX - candidate.span.end.0,
                kind_rank(candidate.symbol.kind),
            )
        });
        candidates.dedup_by(|left, right| {
            left.span == right.span && left.symbol.name == right.symbol.name
        });
        let mut index = 0;
        nest(&mut candidates, &mut index, None, None)
    }
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

fn clean_name(raw: &str) -> String {
    raw.split_once('{')
        .map_or(raw, |(head, _)| head)
        .trim()
        .trim_matches(['"', '\'', '[', ']'])
        .trim()
        .to_owned()
}

fn symbol_kind(name: &str) -> SymbolKind {
    match name {
        "class" => SymbolKind::Class,
        "method" => SymbolKind::Method,
        "function" | "macro" => SymbolKind::Function,
        "interface" => SymbolKind::Interface,
        "module" | "namespace" => SymbolKind::Module,
        "constant" => SymbolKind::Constant,
        "field" => SymbolKind::Field,
        "type" => SymbolKind::Struct,
        "array" => SymbolKind::Array,
        "object" => SymbolKind::Object,
        _ => SymbolKind::Variable,
    }
}

fn kind_rank(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Method => 0,
        SymbolKind::Function => 1,
        _ => 2,
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
}

//! Layered web and template outline tests.

use std::path::Path;

use karet_core::Symbol;
use karet_treesitter::LayeredParser;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_path;

use super::OutlineExtractor;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn symbols(path: &str, source: &str) -> TestResult<Vec<Symbol>> {
    let language = language_id_from_path(Path::new(path)).ok_or("missing test grammar")?;
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, language, source)?;
    Ok(OutlineExtractor::new().analyze(&tree, source))
}

fn layered_symbols(path: &str, source: &str) -> TestResult<Vec<Symbol>> {
    let language = language_id_from_path(Path::new(path)).ok_or("missing test grammar")?;
    let mut parser = LayeredParser::new();
    let tree = parser.parse(language, source)?;
    Ok(OutlineExtractor::new().analyze_layers(&tree, source))
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
    assert_eq!(
        find(symbols, child).and_then(|symbol| symbol.container_name.as_deref()),
        Some(parent),
        "{child:?} should be nested under {parent:?}: {symbols:#?}"
    );
}

#[test]
fn web_outline_languages_accept_empty_input() -> TestResult {
    for path in [
        "style.scss",
        "style.sass",
        "style.less",
        "view.erb",
        "component.mdx",
    ] {
        assert!(symbols(path, "")?.is_empty(), "{path}");
    }
    Ok(())
}

#[test]
fn stylesheet_languages_expose_useful_rules_and_declarations() -> TestResult {
    let scss = symbols(
        "style.scss",
        "@mixin roast($x) { color: $x; }\n@function brew($x) { @return $x; }\n.card { .title { color: red; } }\n@media (width > 1px) { .wide {} }\n???\n",
    )?;
    for expected in ["roast", "brew", ".card", ".title", ".wide"] {
        assert!(names(&scss).contains(&expected), "{expected}: {scss:#?}");
    }
    assert_symbol_container(&scss, ".title", ".card");

    let sass = symbols(
        "style.sass",
        "@mixin roast($x)\n  color: $x\n@function brew($x)\n  @return $x\n.card\n  .title\n    color: red\n???\n",
    )?;
    for expected in ["roast", "brew", ".card", ".title"] {
        assert!(names(&sass).contains(&expected), "{expected}: {sass:#?}");
    }

    let less = symbols(
        "style.less",
        ".mixin(@x) { color: @x; }\n.card { .title { color: red; } }\n@media (min-width: 1px) { .wide {} }\n???\n",
    )?;
    for expected in ["mixin", ".card", ".title", ".wide"] {
        assert!(names(&less).contains(&expected), "{expected}: {less:#?}");
    }
    assert_symbol_container(&less, ".title", ".card");
    Ok(())
}

#[test]
fn component_outlines_merge_sections_scripts_and_styles() -> TestResult {
    let vue = layered_symbols(
        "Card.vue",
        "<template><main><section>Card</section></main></template>\n<script setup lang=\"ts\">function café() {}</script>\n<style lang=\"scss\">.card { .title {} }</style>\n",
    )?;
    for expected in [
        "template", "main", "section", "script", "café", "style", ".card",
    ] {
        assert!(names(&vue).contains(&expected), "{expected}: {vue:#?}");
    }
    assert_symbol_container(&vue, "café", "script");
    assert_symbol_container(&vue, ".card", "style");

    let svelte = layered_symbols(
        "Card.svelte",
        "<script lang=\"ts\">function brew() {}</script>\n<main><section>Card</section></main>\n<style>.card { color: red; }</style>\n",
    )?;
    for expected in ["script", "brew", "style", ".card"] {
        assert!(
            names(&svelte).contains(&expected),
            "{expected}: {svelte:#?}"
        );
    }
    assert_symbol_container(&svelte, "brew", "script");
    assert_symbol_container(&svelte, ".card", "style");
    Ok(())
}

#[test]
fn host_templates_merge_injected_declarations_in_document_order() -> TestResult {
    let html = layered_symbols(
        "page.xhtml",
        "<main><script>function brew() {}</script><section>Body</section></main>",
    )?;
    assert_eq!(names(&html), vec!["main", "brew", "section"]);
    assert_symbol_container(&html, "brew", "main");

    let erb = layered_symbols(
        "view.erb",
        "<main><% def café; end %><section>Body</section></main>",
    )?;
    for expected in ["main", "café", "section"] {
        assert!(names(&erb).contains(&expected), "{expected}: {erb:#?}");
    }

    let php = layered_symbols(
        "view.php",
        "<main><?php function café() {} ?><section>Body</section></main>",
    )?;
    for expected in ["main", "café", "section"] {
        assert!(names(&php).contains(&expected), "{expected}: {php:#?}");
    }
    Ok(())
}

#[test]
fn mdx_keeps_headings_and_exports_but_excludes_fenced_examples() -> TestResult {
    let mdx = layered_symbols(
        "guide.mdx",
        "# Café\n\nexport function Card() { return <main /> }\n\n## Usage\n\n```js\nfunction fakeExample() {}\n```\n\nexport const Brew = () => <Card />\n",
    )?;
    assert_eq!(names(&mdx), vec!["Café", "Usage", "Card", "Brew"]);
    assert!(!names(&mdx).contains(&"fakeExample"), "{mdx:#?}");
    assert_eq!(mdx[0].children[0].container_name.as_deref(), Some("Café"));
    assert!(mdx.iter().any(|symbol| symbol.name == "Card"));
    assert!(mdx.iter().any(|symbol| symbol.name == "Brew"));
    Ok(())
}

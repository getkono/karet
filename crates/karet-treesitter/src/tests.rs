use super::*;

#[test]
fn language_ids_compare() {
    assert_eq!(LanguageId(1), LanguageId(1));
    assert_ne!(LanguageId(1), LanguageId(2));
}

#[test]
fn error_displays() {
    assert_eq!(TsError::UnknownLanguage.to_string(), "unknown language");
}

#[test]
fn unknown_language_has_no_highlights() {
    assert!(highlights_query(LanguageId(60000)).is_none());
    assert!(injections_query(LanguageId(60000)).is_none());
    assert!(semantic_query(LanguageId(60000)).is_none());
}

#[test]
fn every_registered_semantic_query_compiles() -> Result<(), TsError> {
    for grammar in registry::all() {
        let Some(source) = semantic_query(grammar.id) else {
            continue;
        };
        let query = Query::compile(grammar.id, source)?;
        assert!(
            query
                .capture_names()
                .iter()
                .any(|name| name.starts_with("semantic.")),
            "{} semantic query has no semantic captures",
            grammar.name
        );
    }
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn injections_query_compiles_for_grammars_that_ship_one() -> Result<(), TsError> {
    let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let src = injections_query(lang).ok_or(TsError::UnknownLanguage)?;
    let query = Query::compile(lang, &src)?;
    assert!(query.capture_names().contains(&"injection.content"));
    Ok(())
}

#[cfg(feature = "lang-python")]
#[test]
fn grammar_without_injections_reports_none() -> Result<(), TsError> {
    // Python's grammar ships no injections query; that is not an error.
    let lang = language_id_from_injection_name("python").ok_or(TsError::UnknownLanguage)?;
    assert!(highlights_query(lang).is_some());
    assert!(injections_query(lang).is_none());
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn clean_trees_report_no_error_lines() -> Result<(), TsError> {
    let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, lang, "fn main() { let x = 1; }\n")?;
    assert!(tree.error_lines().is_empty());
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn error_lines_cover_the_broken_lines_only() -> Result<(), TsError> {
    let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let mut pool = ParserPool::new();
    // Line 2 (0-based) is broken inside its block; the neighbours are fine.
    let src = "fn ok() {}\n\nfn broken() { let x = ; }\n\nfn also_ok() {}\n";
    let tree = SyntaxTree::parse(&mut pool, lang, src)?;
    let errors = tree.error_lines();
    assert!(
        !errors.is_empty(),
        "the malformed source must report errors"
    );
    assert!(
        errors.iter().any(|&(s, e)| s <= 2 && 2 <= e),
        "line 2 is broken: {errors:?}"
    );
    assert!(
        errors.iter().all(|&(s, _)| s != 0),
        "line 0 parsed cleanly: {errors:?}"
    );
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn missing_nodes_count_as_errors() -> Result<(), TsError> {
    let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let mut pool = ParserPool::new();
    // An unclosed brace makes the parser insert a zero-width missing "}".
    let tree = SyntaxTree::parse(&mut pool, lang, "fn f() {\n    let x = 1;\n")?;
    assert!(
        !tree.error_lines().is_empty(),
        "a missing closing brace is an outright error"
    );
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn layered_error_lines_union_the_layers() -> Result<(), TsError> {
    let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let mut parser = LayeredParser::new();
    let clean = parser.parse(lang, "fn main() {}\n")?;
    assert!(clean.error_lines().is_empty());
    let broken = parser.parse(lang, "fn broken( {{{\n")?;
    assert!(!broken.error_lines().is_empty());
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn parses_rust_and_runs_highlights() -> Result<(), TsError> {
    let lang =
        language_id_from_path(std::path::Path::new("main.rs")).ok_or(TsError::UnknownLanguage)?;
    let src = "fn main() { let x = 1; }";
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, lang, src)?;
    let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
    let query = Query::compile(lang, query_src)?;
    let caps = tree.captures(&query, src);
    assert!(!caps.is_empty(), "rust highlights should match something");
    assert!(query.capture_names().contains(&"keyword"));
    // Every capture index is within range.
    assert!(
        caps.iter()
            .all(|c| (c.capture as usize) < query.capture_names().len())
    );
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn incremental_reparse_matches_full() -> Result<(), TsError> {
    // Insert "let z=1;" before the closing brace and reparse incrementally;
    // the captures must be identical to a fresh full parse of the new text.
    let Some(lang) = language_id_from_path(std::path::Path::new("x.rs")) else {
        return Ok(());
    };
    let old = "fn main() {}";
    let new = "fn main() {let z=1;}";
    let mut pool = ParserPool::new();
    let mut tree = SyntaxTree::parse(&mut pool, lang, old)?;
    // The insertion happens at byte 11 (before '}'), 8 bytes long, same line.
    tree.edit(&Edit {
        start_byte: 11,
        old_end_byte: 11,
        new_end_byte: 19,
        start_point: (0, 11),
        old_end_point: (0, 11),
        new_end_point: (0, 19),
    });
    tree.reparse_with(&mut pool, |byte| new.as_bytes().get(byte..).unwrap_or(&[]))?;

    let full = SyntaxTree::parse(&mut pool, lang, new)?;
    let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
    let query = Query::compile(lang, query_src)?;
    assert_eq!(
        tree.captures(&query, new),
        full.captures(&query, new),
        "incremental reparse must match a full parse"
    );
    Ok(())
}

#[cfg(feature = "lang-markdown")]
#[test]
fn parses_markdown_and_compiles_block_query() -> Result<(), TsError> {
    let lang =
        language_id_from_path(std::path::Path::new("README.md")).ok_or(TsError::UnknownLanguage)?;
    let src = "# Title\n\nSome `code` and a [link](http://x).\n";
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, lang, src)?;
    let query_src = highlights_query(lang).ok_or(TsError::UnknownLanguage)?;
    let query = Query::compile(lang, query_src)?;
    // The block grammar should at least capture the heading text.
    let caps = tree.captures(&query, src);
    assert!(query.capture_names().contains(&"text.title"));
    assert!(
        caps.iter()
            .all(|c| (c.capture as usize) < query.capture_names().len())
    );
    Ok(())
}

/// The `(row, byte-column)` of `byte` in `text` — for building test edits.
#[cfg(feature = "lang-markdown")]
fn point_of(text: &str, byte: usize) -> (usize, usize) {
    let before = text.get(..byte).unwrap_or("");
    let row = before.matches('\n').count();
    let col = before.rfind('\n').map_or(byte, |i| byte - i - 1);
    (row, col)
}

#[cfg(feature = "lang-markdown")]
fn layer_langs(tree: &LayeredTree) -> Vec<LanguageId> {
    tree.children().iter().map(SyntaxTree::language).collect()
}

#[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
#[test]
fn markdown_injects_fenced_code_into_its_language() -> Result<(), TsError> {
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let src = "# Title\n\n```rust\nfn main() {}\n```\n";

    let mut parser = LayeredParser::new();
    let tree = parser.parse(md, src)?;

    assert_eq!(tree.root().language(), md);
    // The fence's info string names rust, so its content becomes a rust layer.
    assert!(
        layer_langs(&tree).contains(&rust),
        "expected an embedded rust layer, got {:?}",
        layer_langs(&tree)
    );
    // Root is always the first layer.
    assert_eq!(tree.layers().count(), tree.children().len() + 1);
    Ok(())
}

#[cfg(feature = "lang-markdown")]
#[test]
fn markdown_injects_its_own_inline_grammar() -> Result<(), TsError> {
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let inline =
        language_id_from_injection_name("markdown_inline").ok_or(TsError::UnknownLanguage)?;
    let mut parser = LayeredParser::new();
    let tree = parser.parse(md, "Some *emphasis* and a [link](http://x).\n")?;
    assert!(layer_langs(&tree).contains(&inline));
    Ok(())
}

#[cfg(feature = "lang-markdown")]
#[test]
fn unknown_fence_language_injects_nothing() -> Result<(), TsError> {
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let mut parser = LayeredParser::new();
    // No grammar for `brainfuck`; the fence stays plain text rather than erroring.
    let tree = parser.parse(md, "```brainfuck\n+++.\n```\n")?;
    assert!(
        tree.children().is_empty(),
        "an unresolvable fence language must yield no layer"
    );
    // A resolvable fence over the same shape does produce one.
    let rust = parser.parse(md, "```rust\nfn f() {}\n```\n")?;
    assert!(!rust.children().is_empty());
    Ok(())
}

#[cfg(all(
    feature = "lang-html",
    feature = "lang-javascript",
    feature = "lang-css"
))]
#[test]
fn html_injects_script_and_style() -> Result<(), TsError> {
    let html = language_id_from_injection_name("html").ok_or(TsError::UnknownLanguage)?;
    let js = language_id_from_injection_name("javascript").ok_or(TsError::UnknownLanguage)?;
    let css = language_id_from_injection_name("css").ok_or(TsError::UnknownLanguage)?;

    let mut parser = LayeredParser::new();
    let tree = parser.parse(
        html,
        "<script>let x = 1;</script><style>a { color: red }</style>",
    )?;
    let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
    assert!(
        langs.contains(&js),
        "expected a javascript layer: {langs:?}"
    );
    assert!(langs.contains(&css), "expected a css layer: {langs:?}");
    Ok(())
}

#[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
#[test]
fn reparse_discovers_a_newly_typed_code_fence() -> Result<(), TsError> {
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;

    let old = "text\n";
    let new = "text\n\n```rust\nfn f() {}\n```\n";
    let mut parser = LayeredParser::new();
    let mut tree = parser.parse(md, old)?;
    assert!(!layer_langs(&tree).contains(&rust), "no fence yet");

    // Append the fence — the edit turns a paragraph into an injected rust region.
    let edit = Edit {
        start_byte: old.len(),
        old_end_byte: old.len(),
        new_end_byte: new.len(),
        start_point: point_of(old, old.len()),
        old_end_point: point_of(old, old.len()),
        new_end_point: point_of(new, new.len()),
    };
    parser.reparse(&mut tree, &[edit], new)?;

    assert!(
        layer_langs(&tree).contains(&rust),
        "reparse must discover the new fence, got {:?}",
        layer_langs(&tree)
    );
    // And it agrees with a cold parse of the same text.
    let fresh = parser.parse(md, new)?;
    let (mut a, mut b) = (layer_langs(&tree), layer_langs(&fresh));
    a.sort_unstable_by_key(|l| l.0);
    b.sort_unstable_by_key(|l| l.0);
    assert_eq!(a, b, "incremental layers must match a full layered parse");
    Ok(())
}

#[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
#[test]
fn deleting_a_fence_drops_its_layer() -> Result<(), TsError> {
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let old = "```rust\nfn f() {}\n```\n";
    let new = "";

    let mut parser = LayeredParser::new();
    let mut tree = parser.parse(md, old)?;
    assert!(layer_langs(&tree).contains(&rust));

    parser.reparse(
        &mut tree,
        &[Edit {
            start_byte: 0,
            old_end_byte: old.len(),
            new_end_byte: 0,
            start_point: (0, 0),
            old_end_point: point_of(old, old.len()),
            new_end_point: (0, 0),
        }],
        new,
    )?;
    assert!(
        !layer_langs(&tree).contains(&rust),
        "the rust layer must vanish with its fence"
    );
    Ok(())
}

#[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
#[test]
fn rust_doc_comment_is_markdown() -> Result<(), TsError> {
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let mut parser = LayeredParser::new();
    let tree = parser.parse(rust, "/// Adds *one*.\npub fn f() {}\n")?;
    let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
    assert!(
        langs.contains(&md),
        "doc comment must inject markdown: {langs:?}"
    );
    // A plain `//` comment is not markdown.
    let plain = parser.parse(rust, "// not *markdown*\npub fn f() {}\n")?;
    assert!(!plain.children().iter().any(|c| c.language() == md));
    Ok(())
}

#[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
#[test]
fn rust_doctest_fence_in_a_doc_comment_is_rust() -> Result<(), TsError> {
    // The headline case: a doctest fence spans several `///` lines, each its own
    // `line_comment` node. Only a *combined* markdown injection can see the fence,
    // and markdown must then recursively inject rust back into it.
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let src = "\
/// Adds one.
///
/// ```rust
/// let y = 1 + 1;
/// assert_eq!(y, 2);
/// ```
pub fn add_one() {}
";
    let mut parser = LayeredParser::new();
    let tree = parser.parse(rust, src)?;
    let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
    assert!(langs.contains(&md), "expected a markdown layer: {langs:?}");

    // The doctest body must come back as a *nested* rust layer covering the fence
    // body — not merely some rust layer (a macro injection would also be rust).
    let fence_body = src.find("let y").ok_or(TsError::ParseFailed)?;
    let doctest = tree
        .children()
        .iter()
        .find(|c| c.language() == rust && c.span().start.0 >= fence_body);
    let doctest = doctest.ok_or(TsError::ParseFailed)?;
    assert!(
        doctest.span().end.0 <= src.find("pub fn").unwrap_or(src.len()),
        "the doctest layer must stay inside the doc comment"
    );
    Ok(())
}

#[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
#[test]
fn layers_are_ordered_shallowest_first() -> Result<(), TsError> {
    // rust (root) → markdown (doc comment, depth 1) → rust (doctest fence, depth 2).
    // The nested rust layer must come *after* the markdown layer that produced it,
    // so a capture merge can let the deeper layer win an exact-range tie.
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let md = language_id_from_injection_name("markdown").ok_or(TsError::UnknownLanguage)?;
    let src = "/// ```rust\n/// let y = 1;\n/// ```\npub fn f() {}\n";

    let mut parser = LayeredParser::new();
    let tree = parser.parse(rust, src)?;
    let langs: Vec<_> = tree.children().iter().map(SyntaxTree::language).collect();
    let md_at = langs.iter().position(|l| *l == md);
    let nested_rust_at = tree
        .children()
        .iter()
        .position(|c| c.language() == rust && c.span().start.0 < src.len());
    let (Some(md_at), Some(rust_at)) = (md_at, nested_rust_at) else {
        return Err(TsError::ParseFailed);
    };
    assert!(
        md_at < rust_at,
        "markdown (depth 1) must precede the doctest rust layer (depth 2): {langs:?}"
    );
    Ok(())
}

#[cfg(feature = "lang-rust")]
#[test]
fn self_injecting_grammar_terminates() -> Result<(), TsError> {
    // Rust's own injections query re-parses macro token trees as rust. Without the
    // depth cap and identity guard this descends forever.
    let rust = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
    let mut parser = LayeredParser::new();
    let tree = parser.parse(rust, "macro_rules! m { () => { m!(); } }\nfn f() { m!(); }")?;
    assert!(tree.children().len() < MAX_INJECTION_LAYERS);
    Ok(())
}

#[test]
fn parse_ranges_rejects_an_empty_range_list() {
    let mut pool = ParserPool::new();
    let err = SyntaxTree::parse_ranges(&mut pool, LanguageId(60000), "x", &[]);
    assert!(matches!(err, Err(TsError::ParseFailed)));
}

#[test]
fn point_at_resolves_rows_and_columns() {
    let text = "ab\ncd\n";
    let starts = line_starts(text);
    assert_eq!(starts, vec![0, 3, 6]);
    assert_eq!(
        point_at(&starts, 0),
        tree_sitter::Point { row: 0, column: 0 }
    );
    assert_eq!(
        point_at(&starts, 4),
        tree_sitter::Point { row: 1, column: 1 }
    );
    // End of buffer sits at the start of the (empty) trailing line.
    assert_eq!(
        point_at(&starts, 6),
        tree_sitter::Point { row: 2, column: 0 }
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn detects_rust_by_extension_and_name() {
    let p = std::path::Path::new("src/lib.rs");
    assert!(language_id_from_path(p).is_some());
    assert_eq!(language_name_from_path(p), Some("Rust"));
}

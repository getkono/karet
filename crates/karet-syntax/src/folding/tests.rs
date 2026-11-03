use std::path::Path;

use karet_treesitter::ParserPool;
use karet_treesitter::TsError;
use karet_treesitter::language_id_from_path;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn parse(path: &str, src: &str) -> Result<SyntaxTree, TsError> {
    let lang = language_id_from_path(Path::new(path)).ok_or(TsError::UnknownLanguage)?;
    SyntaxTree::parse(&mut ParserPool::new(), lang, src)
}

/// The regions as `(start, end, kind)` triples, for terse assertions.
fn triples(folds: &FoldRegions) -> Vec<(u32, u32, FoldKind)> {
    folds
        .regions()
        .iter()
        .map(|r| (r.start, r.end, r.kind))
        .collect()
}

#[test]
fn fold_finds_multiline_regions() -> TestResult {
    // A function whose body spans several lines: the fn item (and its block) are
    // multi-line and must yield a fold; the one-line `let` inside must not.
    let src = "fn main() {\n    let x = 42;\n    let y = x + 1;\n}\n";
    let tree = parse("x.rs", src)?;
    assert_eq!(triples(&fold(&tree)), [(0, 3, FoldKind::Code)]);
    Ok(())
}

#[test]
fn fold_is_empty_for_single_line_source() -> TestResult {
    let tree = parse("x.rs", "let x = 1;")?;
    assert!(fold(&tree).regions().is_empty());
    Ok(())
}

#[test]
fn folder_joins_comment_and_import_runs() -> TestResult {
    let src = "\
use std::fmt;

use crate::a;
use crate::b;
// one
// two
fn f() {
    let x = 1; // trailing
    // inside
}
/* lone */
";
    let tree = parse("x.rs", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(
        triples(&folds),
        [
            // The blank line between the import groups does not split them.
            (0, 3, FoldKind::Imports),
            (4, 5, FoldKind::Comment),
            (6, 9, FoldKind::Code),
        ],
        "a trailing comment, a one-line run and a one-line block fold nothing"
    );
    Ok(())
}

#[test]
fn folder_keeps_the_longest_region_per_header_line() -> TestResult {
    // `fn` and its `block` share line 0; the multi-line `use` list is nested
    // inside the import run that starts on the same line as it.
    let src = "use a::{\n    b,\n};\nuse c;\nfn f(\n    x: u8,\n) {\n    x;\n}\n";
    let tree = parse("x.rs", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(
        triples(&folds),
        [
            (0, 3, FoldKind::Imports),
            (4, 8, FoldKind::Code),
            (6, 8, FoldKind::Code),
        ]
    );
    Ok(())
}

#[test]
fn folder_classifies_a_block_comment_spanning_lines() -> TestResult {
    let src = "/*\n * doc\n */\nint main(void) {\n    return 0;\n}\n";
    let tree = parse("x.c", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(
        triples(&folds),
        [(0, 2, FoldKind::Comment), (3, 5, FoldKind::Code)]
    );
    Ok(())
}

#[test]
fn folder_includes_do_not_claim_the_line_after_them() -> TestResult {
    let src = "#include <a.h>\n#include <b.h>\nint x;\n";
    let tree = parse("x.c", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(triples(&folds), [(0, 1, FoldKind::Imports)]);
    Ok(())
}

#[test]
fn folder_uses_the_python_query() -> TestResult {
    let src = "import os\nfrom a import b\n\ndef f():\n    return {\n        1: 2,\n    }\n";
    let tree = parse("x.py", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(
        triples(&folds),
        [
            (0, 1, FoldKind::Imports),
            (3, 6, FoldKind::Code),
            (4, 6, FoldKind::Code),
        ]
    );
    Ok(())
}

#[test]
fn folder_falls_back_to_every_multiline_node_without_a_query() -> TestResult {
    let src = "[a]\nb = [\n  1,\n]\n";
    let tree = parse("x.toml", src)?;
    let folds = Folder::new().analyze(&tree, src);
    assert_eq!(triples(&folds), triples(&fold(&tree)));
    assert!(!folds.regions().is_empty());
    Ok(())
}

#[test]
fn indent_folds_nest_and_leave_trailing_blank_lines_out() {
    let src = "a\n  b\n    c\n\n  d\n\ne\n";
    assert_eq!(
        triples(&fold_by_indent(src, 4)),
        [(0, 4, FoldKind::Code), (1, 2, FoldKind::Code)]
    );
}

#[test]
fn indent_folds_count_tabs_to_the_next_stop_and_close_at_eof() {
    // A tab is as deep as four spaces at width 4, so line 2 is a sibling of 1.
    let src = "a\n\tb\n    c\n\t  d";
    assert_eq!(
        triples(&fold_by_indent(src, 4)),
        [(0, 3, FoldKind::Code), (2, 3, FoldKind::Code)]
    );
    // Width 0 is treated as 1, so the tab is shallower than four spaces.
    assert_eq!(
        triples(&fold_by_indent("a\n\tb\n    c", 0)),
        [(0, 2, FoldKind::Code), (1, 2, FoldKind::Code)]
    );
}

#[test]
fn indent_folds_nothing_in_flat_or_empty_text() {
    assert!(fold_by_indent("", 4).regions().is_empty());
    assert!(
        fold_by_indent("a\nb\r\n\n  \nc\r\n", 4)
            .regions()
            .is_empty()
    );
}

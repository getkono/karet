//! Tree-sitter function/block detection used to narrow blame to a single function.
//!
//! Reuses the shared `karet-treesitter` parse host. blameline bundles a small
//! per-language query that captures function-like nodes as `@function`; the
//! innermost captured node containing a target line gives the range to blame.

use std::path::Path;

use karet_treesitter::LanguageId;
use karet_treesitter::ParserPool;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_path;

use crate::LineRange;

/// The 1-based line range of the innermost function/block enclosing `line`
/// (also 1-based, the numbering [`LineRange`] uses), if the language is
/// supported and an enclosing function exists.
///
/// This is a pure tree-sitter helper — no git access — so it is independently
/// testable. Returns `None` (caller should fall back to whole-file blame) when the
/// language has no bundled grammar/query, `line` is `0` or past the end of
/// `source`, or `line` is not inside any function.
#[must_use]
pub fn enclosing_function_range(source: &str, file: &Path, line: u32) -> Option<LineRange> {
    let index = line.checked_sub(1)?;
    let lang = language_id_from_path(file)?;
    let query_src = function_query(lang)?;
    let query = Query::compile(lang, query_src).ok()?;
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, lang, source).ok()?;

    let starts = line_start_offsets(source);
    let target = *starts.get(index as usize)?;

    // Pick the smallest captured span that contains the target byte (innermost fn).
    let mut best: Option<(usize, usize)> = None;
    for cap in tree.captures(&query, source) {
        let (start, end) = (cap.span.start.0, cap.span.end.0);
        if start <= target && target < end && best.is_none_or(|(bs, be)| (end - start) < (be - bs))
        {
            best = Some((start, end));
        }
    }
    let (start_byte, end_byte) = best?;
    Some(LineRange {
        start: byte_to_line(&starts, start_byte) + 1,
        end: byte_to_line(&starts, end_byte.saturating_sub(1)) + 1,
    })
}

/// Function-like node types captured as `@function` for a resolved grammar.
///
/// Keyed on [`LanguageId`] through `karet_treesitter::languages`' typed
/// constants — never on display names, whose spelling belongs to a presentation
/// catalogue this crate cannot see (dispatching on them silently broke
/// JSX/`.mli` narrowing). A malformed/unknown query simply yields `None`
/// upstream (the query fails to compile), degrading to whole-file blame.
fn function_query(lang: LanguageId) -> Option<&'static str> {
    use karet_treesitter::languages;
    const JS: &str = "[(function_declaration) (method_definition) (arrow_function) \
                       (function_expression)] @function";
    // (query, grammars it applies to) — a grammar not compiled into this build
    // has a `None` constant and matches nothing. JSX parses with the JavaScript
    // grammar, so it needs no entry of its own.
    const QUERIES: &[(&str, &[Option<LanguageId>])] = &[
        ("(function_item) @function", &[languages::RUST]),
        (
            "(function_definition) @function",
            &[languages::PYTHON, languages::BASH, languages::ZSH],
        ),
        (
            JS,
            &[languages::JAVASCRIPT, languages::TYPESCRIPT, languages::TSX],
        ),
        (
            "[(function_declaration) (method_declaration)] @function",
            &[languages::GO],
        ),
        (
            "(function_definition) @function",
            &[languages::C, languages::CPP],
        ),
        (
            "(method_declaration) @function",
            &[languages::JAVA, languages::CSHARP],
        ),
        (
            "[(method) (singleton_method)] @function",
            &[languages::RUBY],
        ),
        (
            "[(function_definition) (method_declaration)] @function",
            &[languages::PHP],
        ),
    ];
    QUERIES
        .iter()
        .find_map(|(query, ids)| ids.contains(&Some(lang)).then_some(*query))
}

/// Byte offset of the start of each line (index 0 is byte 0).
fn line_start_offsets(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter(|(_, b)| *b == b'\n')
            .map(|(i, _)| i + 1),
    );
    starts
}

/// The 0-based line index containing byte offset `byte`.
fn byte_to_line(starts: &[usize], byte: usize) -> u32 {
    let idx = starts.partition_point(|&o| o <= byte).saturating_sub(1);
    u32::try_from(idx).unwrap_or(u32::MAX)
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;

    const SRC: &str =
        "fn first() {\n    let a = 1;\n}\n\nfn second() {\n    let b = 2;\n    let c = 3;\n}\n";
    //  1-based lines: 1 `fn first`, 2 `let a`, 3 `}`, 4 blank, 5 `fn second`, 6 `let b`, 7 `let c`, 8 `}`

    #[test]
    fn finds_innermost_enclosing_function() {
        // Line 6 (`let b`) is inside `second()`, which spans lines 5..=8.
        assert_eq!(
            enclosing_function_range(SRC, Path::new("x.rs"), 6),
            Some(LineRange { start: 5, end: 8 })
        );
        // Line 2 (`let a`) is inside `first()`, spanning lines 1..=3.
        assert_eq!(
            enclosing_function_range(SRC, Path::new("x.rs"), 2),
            Some(LineRange { start: 1, end: 3 })
        );
    }

    /// The parameter numbers lines the way the returned range does: a
    /// function's own first and last lines narrow to that function. Falsified
    /// by the parameter being read 0-based: line 3 would land on the blank
    /// line and line 8 past the last function.
    #[test]
    fn a_function_s_boundary_lines_narrow_to_it() {
        let range = |line| enclosing_function_range(SRC, Path::new("x.rs"), line);
        assert_eq!(range(1), Some(LineRange { start: 1, end: 3 }));
        assert_eq!(range(3), Some(LineRange { start: 1, end: 3 }));
        assert_eq!(range(5), Some(LineRange { start: 5, end: 8 }));
        assert_eq!(range(8), Some(LineRange { start: 5, end: 8 }));
    }

    #[test]
    fn line_outside_any_function_is_none() {
        // Line 4 is the blank line between the two functions.
        assert_eq!(enclosing_function_range(SRC, Path::new("x.rs"), 4), None);
    }

    /// There is no line 0 in 1-based numbering: it narrows to nothing rather
    /// than to the first line's function.
    #[test]
    fn line_zero_is_none() {
        assert_eq!(enclosing_function_range(SRC, Path::new("x.rs"), 0), None);
    }

    #[test]
    fn unsupported_language_is_none() {
        assert_eq!(
            enclosing_function_range(SRC, Path::new("x.unknownext"), 5),
            None
        );
    }

    #[test]
    fn line_start_offsets_and_byte_to_line_round_trip() {
        let starts = line_start_offsets("ab\ncd\n\nef");
        assert_eq!(starts, vec![0, 3, 6, 7]);
        assert_eq!(byte_to_line(&starts, 0), 0);
        assert_eq!(byte_to_line(&starts, 4), 1);
        assert_eq!(byte_to_line(&starts, 8), 3);
    }
}

/// One case per bundled query, for every grammar this build compiled in: a
/// line inside a function body narrows to that function's whole range, and a
/// line between functions narrows to nothing. Each path's extension resolves
/// through the same alias table the query does (`.jsx`/`.tsx`, `.cs`), which
/// is where narrowing silently broke before.
#[cfg(all(test, feature = "treesitter"))]
mod language_tests {
    use super::*;

    /// `(path, source, body line, expected function range, line outside every
    /// function)`, all 1-based.
    type Case = (&'static str, &'static str, u32, LineRange, u32);

    const fn range(start: u32, end: u32) -> LineRange {
        LineRange { start, end }
    }

    #[allow(clippy::vec_init_then_push)] // each push is gated on its grammar's feature
    fn cases() -> Vec<Case> {
        let mut cases: Vec<Case> = Vec::new();
        #[cfg(feature = "lang-rust")]
        cases.push(("a.rs", "use x;\nfn f() {\n    1;\n}\n", 3, range(2, 4), 1));
        #[cfg(feature = "lang-python")]
        cases.push((
            "a.py",
            "import x\ndef f():\n    return 1\n",
            3,
            range(2, 3),
            1,
        ));
        #[cfg(feature = "lang-javascript")]
        cases.push((
            "a.js",
            "let x;\nfunction f() {\n  return 1;\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-javascript")]
        cases.push((
            "a.jsx",
            "let x;\nconst f = () => {\n  return <a/>;\n};\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-typescript")]
        cases.push((
            "a.ts",
            "let x: number;\nfunction f(): number {\n  return 1;\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-typescript")]
        cases.push((
            "a.tsx",
            "let x;\nconst f = function () {\n  return <a/>;\n};\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-go")]
        cases.push((
            "a.go",
            "package m\nfunc f() int {\n\treturn 1\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-c")]
        cases.push((
            "a.c",
            "int x;\nint f(void) {\n  return 1;\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-cpp")]
        cases.push((
            "a.cpp",
            "int x;\nint f() {\n  return 1;\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-java")]
        cases.push((
            "A.java",
            "class A {\n  int f() {\n    return 1;\n  }\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-ruby")]
        cases.push(("a.rb", "x = 1\ndef f\n  1\nend\n", 3, range(2, 4), 1));
        #[cfg(feature = "lang-csharp")]
        cases.push((
            "A.cs",
            "class A {\n  int F() {\n    return 1;\n  }\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-php")]
        cases.push((
            "a.php",
            "<?php\nfunction f() {\n  return 1;\n}\n",
            3,
            range(2, 4),
            1,
        ));
        #[cfg(feature = "lang-bash")]
        cases.push(("a.sh", "x=1\nf() {\n  echo 1\n}\n", 3, range(2, 4), 1));
        if zsh_compiled_in() {
            cases.push(("a.zsh", "x=1\nf() {\n  echo 1\n}\n", 3, range(2, 4), 1));
        }
        cases
    }

    /// blameline has no `lang-zsh` feature of its own: the Zsh grammar is
    /// compiled in only when workspace feature unification turns on
    /// `karet-treesitter/lang-zsh`, so its case is gated on the grammar
    /// resolving at run time rather than on a feature.
    fn zsh_compiled_in() -> bool {
        language_id_from_path(Path::new("a.zsh")).is_some()
    }

    #[test]
    fn every_bundled_query_narrows_to_the_enclosing_function() {
        for (path, source, inside, want, outside) in cases() {
            assert_eq!(
                enclosing_function_range(source, Path::new(path), inside),
                Some(want),
                "{path}: line {inside}"
            );
            assert_eq!(
                enclosing_function_range(source, Path::new(path), outside),
                None,
                "{path}: line {outside}"
            );
        }
    }

    /// The table covers every narrowing grammar `all-narrowing-languages`
    /// turns on, plus Zsh whenever unification compiled it in, so a new query
    /// cannot land untested under `--all-features`.
    #[cfg(feature = "all-narrowing-languages")]
    #[test]
    fn the_table_covers_every_narrowing_language() {
        assert_eq!(cases().len(), 14 + usize::from(zsh_compiled_in()));
    }

    /// A line past the end of the source has no start offset, so it narrows
    /// to nothing rather than to the last function.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_line_past_the_end_is_none() {
        assert_eq!(
            enclosing_function_range("fn f() {\n}\n", Path::new("a.rs"), 10),
            None
        );
    }
}

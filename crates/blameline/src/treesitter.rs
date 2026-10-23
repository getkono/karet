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
use karet_treesitter::language_id_from_injection_name;
use karet_treesitter::language_id_from_path;

use crate::LineRange;

/// The 1-based line range of the innermost function/block enclosing `line`
/// (0-based), if the language is supported and an enclosing function exists.
///
/// This is a pure tree-sitter helper — no git access — so it is independently
/// testable. Returns `None` (caller should fall back to whole-file blame) when the
/// language has no bundled grammar/query or `line` is not inside any function.
#[must_use]
pub fn enclosing_function_range(source: &str, file: &Path, line: u32) -> Option<LineRange> {
    let lang = language_id_from_path(file)?;
    let query_src = function_query(lang)?;
    let query = Query::compile(lang, query_src).ok()?;
    let mut pool = ParserPool::new();
    let tree = SyntaxTree::parse(&mut pool, lang, source).ok()?;

    let starts = line_start_offsets(source);
    let target = *starts.get(line as usize)?;

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
/// Keyed on [`LanguageId`] via the injection-name resolver — never on display
/// names, whose spelling belongs to a presentation catalogue this crate cannot
/// see (dispatching on them silently broke JSX/`.mli` narrowing). A
/// malformed/unknown query simply yields `None` upstream (the query fails to
/// compile), degrading to whole-file blame.
fn function_query(lang: LanguageId) -> Option<&'static str> {
    const JS: &str = "[(function_declaration) (method_definition) (arrow_function) \
                       (function_expression)] @function";
    // (query, grammar aliases it applies to) — resolved against the grammars
    // actually compiled into this build.
    const QUERIES: &[(&str, &[&str])] = &[
        ("(function_item) @function", &["rust"]),
        (
            "(function_definition) @function",
            &["python", "bash", "zsh"],
        ),
        (JS, &["javascript", "jsx", "typescript", "tsx"]),
        (
            "[(function_declaration) (method_declaration)] @function",
            &["go"],
        ),
        ("(function_definition) @function", &["c", "cpp"]),
        ("(method_declaration) @function", &["java", "c_sharp"]),
        ("[(method) (singleton_method)] @function", &["ruby"]),
        (
            "[(function_definition) (method_declaration)] @function",
            &["php"],
        ),
    ];
    QUERIES.iter().find_map(|(query, aliases)| {
        aliases
            .iter()
            .any(|alias| language_id_from_injection_name(alias) == Some(lang))
            .then_some(*query)
    })
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
    //  0-based lines: 0 `fn first`, 1 `let a`, 2 `}`, 3 blank, 4 `fn second`, 5 `let b`, 6 `let c`, 7 `}`

    #[test]
    fn finds_innermost_enclosing_function() {
        // Line 5 (`let b`) is inside `second()`, which spans 1-based lines 5..=8.
        assert_eq!(
            enclosing_function_range(SRC, Path::new("x.rs"), 5),
            Some(LineRange { start: 5, end: 8 })
        );
        // Line 1 (`let a`) is inside `first()`, spanning 1-based lines 1..=3.
        assert_eq!(
            enclosing_function_range(SRC, Path::new("x.rs"), 1),
            Some(LineRange { start: 1, end: 3 })
        );
    }

    #[test]
    fn line_outside_any_function_is_none() {
        // Line 3 (0-based) is the blank line between the two functions.
        assert_eq!(enclosing_function_range(SRC, Path::new("x.rs"), 3), None);
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

    /// `(path, source, 0-based body line, expected 1-based function range,
    /// 0-based line outside every function)`.
    type Case = (&'static str, &'static str, u32, LineRange, u32);

    const fn range(start: u32, end: u32) -> LineRange {
        LineRange { start, end }
    }

    #[allow(clippy::vec_init_then_push)] // each push is gated on its grammar's feature
    fn cases() -> Vec<Case> {
        let mut cases: Vec<Case> = Vec::new();
        #[cfg(feature = "lang-rust")]
        cases.push(("a.rs", "use x;\nfn f() {\n    1;\n}\n", 2, range(2, 4), 0));
        #[cfg(feature = "lang-python")]
        cases.push((
            "a.py",
            "import x\ndef f():\n    return 1\n",
            2,
            range(2, 3),
            0,
        ));
        #[cfg(feature = "lang-javascript")]
        cases.push((
            "a.js",
            "let x;\nfunction f() {\n  return 1;\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-javascript")]
        cases.push((
            "a.jsx",
            "let x;\nconst f = () => {\n  return <a/>;\n};\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-typescript")]
        cases.push((
            "a.ts",
            "let x: number;\nfunction f(): number {\n  return 1;\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-typescript")]
        cases.push((
            "a.tsx",
            "let x;\nconst f = function () {\n  return <a/>;\n};\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-go")]
        cases.push((
            "a.go",
            "package m\nfunc f() int {\n\treturn 1\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-c")]
        cases.push((
            "a.c",
            "int x;\nint f(void) {\n  return 1;\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-cpp")]
        cases.push((
            "a.cpp",
            "int x;\nint f() {\n  return 1;\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-java")]
        cases.push((
            "A.java",
            "class A {\n  int f() {\n    return 1;\n  }\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-ruby")]
        cases.push(("a.rb", "x = 1\ndef f\n  1\nend\n", 2, range(2, 4), 0));
        #[cfg(feature = "lang-csharp")]
        cases.push((
            "A.cs",
            "class A {\n  int F() {\n    return 1;\n  }\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-php")]
        cases.push((
            "a.php",
            "<?php\nfunction f() {\n  return 1;\n}\n",
            2,
            range(2, 4),
            0,
        ));
        #[cfg(feature = "lang-bash")]
        cases.push(("a.sh", "x=1\nf() {\n  echo 1\n}\n", 2, range(2, 4), 0));
        if zsh_compiled_in() {
            cases.push(("a.zsh", "x=1\nf() {\n  echo 1\n}\n", 2, range(2, 4), 0));
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
            enclosing_function_range("fn f() {\n}\n", Path::new("a.rs"), 9),
            None
        );
    }
}

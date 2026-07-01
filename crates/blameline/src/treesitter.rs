//! Tree-sitter function/block detection used to narrow blame to a single function.
//!
//! Reuses the shared `karet-treesitter` parse host. blameline bundles a small
//! per-language query that captures function-like nodes as `@function`; the
//! innermost captured node containing a target line gives the range to blame.

use std::path::Path;

use karet_treesitter::{
    ParserPool, Query, SyntaxTree, language_id_from_path, language_name_from_path,
};

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
    let query_src = function_query(language_name_from_path(file)?)?;
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

/// Function-like node types captured as `@function` for `language_name` (the
/// `karet-treesitter` display name). A malformed/unknown query simply yields `None`
/// upstream (the query fails to compile), degrading to whole-file blame.
fn function_query(language_name: &str) -> Option<&'static str> {
    const JS: &str = "[(function_declaration) (method_definition) (arrow_function) \
                       (function_expression)] @function";
    Some(match language_name {
        "Rust" => "(function_item) @function",
        "Python" => "(function_definition) @function",
        "JavaScript" | "TypeScript" | "TSX" => JS,
        "Go" => "[(function_declaration) (method_declaration)] @function",
        "C" | "C++" => "(function_definition) @function",
        "Java" | "C#" => "(method_declaration) @function",
        "Ruby" => "[(method) (singleton_method)] @function",
        "PHP" => "[(function_definition) (method_declaration)] @function",
        "Bash" => "(function_definition) @function",
        _ => return None,
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

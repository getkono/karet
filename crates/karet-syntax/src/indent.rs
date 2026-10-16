//! How far "one level deeper" goes, read from the file before the setting.
//!
//! An indent query says *that* a new line sits one level inside the line its
//! enclosing node starts on; this module says *how deep* that is. A file
//! indented with tabs, or two spaces, keeps its own style even when the
//! configured unit differs: the body lines the node already has decide first,
//! then the file's prevailing indentation step, and only then the setting.

use std::collections::BTreeMap;

use karet_core::BytePos;
use karet_treesitter::SyntaxTree;

/// The longest indentation step, in spaces, counted as a nesting level rather
/// than a continuation aligned under something on the line above.
const MAX_SPACE_STEP: usize = 8;

/// The leading blanks (spaces and tabs) of `line`.
fn leading(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

/// The indentation the start of `text`'s line beginning at `line_start` has.
pub(crate) fn line_indent(text: &str, line_start: usize) -> Option<&str> {
    let line = text.get(line_start..)?;
    Some(leading(line.split('\n').next().unwrap_or(line)))
}

/// Whether the character at `at` is string or comment content rather than code:
/// it sits inside a comment, or inside a string that opened before it. A line
/// that opens a string (a Python docstring) is code; one that continues it is not.
pub(crate) fn in_literal(tree: &SyntaxTree, at: BytePos) -> bool {
    tree.named_ancestors_at(at).iter().any(|node| {
        node.kind.contains("comment") || (node.kind.contains("string") && node.span.start < at)
    })
}

/// The indentation one level inside the node that starts on the line at
/// `line_start`, for a break at byte `caret`.
///
/// The shallowest indentation, deeper than the anchor line's, among the
/// non-blank lines from the one after the anchor through the caret's: the body
/// the node already has. A line is skipped when `literal` holds for the byte its
/// first non-blank character sits at, so the inside of a multi-line string or
/// comment never counts as body. With none, the anchor line's indentation plus
/// the file's own step ([`file_unit`], which skips the same lines), or plus
/// `unit` when the file shows none.
pub(crate) fn deeper(
    text: &str,
    line_start: usize,
    caret: usize,
    unit: &str,
    literal: impl Fn(usize) -> bool,
) -> Option<String> {
    let base = line_indent(text, line_start)?;
    let mut offset = line_start;
    let mut body: Option<&str> = None;
    for (row, line) in text.get(line_start..)?.split('\n').enumerate() {
        if row > 0 && offset > caret {
            break;
        }
        let ws = leading(line);
        let solid = ws.len() < line.trim_end_matches('\r').len();
        if row > 0
            && solid
            && ws.len() > base.len()
            && ws.starts_with(base)
            && !literal(offset + ws.len())
        {
            body = Some(body.map_or(ws, |b| if ws.len() < b.len() { ws } else { b }));
        }
        offset += line.len() + 1;
    }
    if let Some(body) = body {
        return Some(body.to_owned());
    }
    Some(format!(
        "{base}{}",
        file_unit(text, &literal).unwrap_or(unit)
    ))
}

/// The file's prevailing indentation step: the commonest way a non-blank line
/// goes deeper than the one before it — one tab, or one to [`MAX_SPACE_STEP`]
/// spaces (the narrower step on a tie). `None` when no line steps in.
///
/// A line is skipped, as a blank one is, when `literal` holds for the byte its
/// first non-blank character sits at: the ` * ` under a `/**` sits one space in
/// from it, so a file full of doc comments would otherwise step by one.
pub(crate) fn file_unit(text: &str, literal: impl Fn(usize) -> bool) -> Option<&'static str> {
    const SPACES: &str = "        ";
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut prev: Option<&str> = None;
    let mut offset = 0;
    for raw in text.split('\n') {
        let start = offset;
        offset += raw.len() + 1;
        let line = raw.trim_end_matches('\r');
        let ws = leading(line);
        if ws.len() == line.len() || literal(start + ws.len()) {
            continue;
        }
        if let Some(step) = prev.and_then(|p| ws.strip_prefix(p))
            && (step == "\t"
                || (!step.is_empty()
                    && step.len() <= MAX_SPACE_STEP
                    && step.bytes().all(|b| b == b' ')))
        {
            *counts.entry(step).or_default() += 1;
        }
        prev = Some(ws);
    }
    let (step, _) = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.len().cmp(&a.0.len())))?;
    if step == "\t" {
        Some("\t")
    } else {
        SPACES.get(..step.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_unit_is_the_commonest_inward_step() {
        assert_eq!(file_unit("a\n\tb\n\t\tc\n", code), Some("\t"));
        assert_eq!(
            file_unit("a:\n  b:\n    c: 1\nd:\n  e: 2\n", code),
            Some("  ")
        );
        assert_eq!(file_unit("f {\n    g {\n        h\n", code), Some("    "));
        // Blank lines neither step nor break the chain.
        assert_eq!(file_unit("a\n\n   \n  b\n", code), Some("  "));
        // A tie goes to the narrower step; outdents and wide alignment count for nothing.
        assert_eq!(file_unit("a\n    b\nc\n  d\n", code), Some("  "));
        assert_eq!(file_unit("call(a,\n                 b)\n", code), None);
        assert_eq!(file_unit("flat\nlines\n", code), None);
        assert_eq!(file_unit("", code), None);
        // CRLF line ends are not indentation.
        assert_eq!(file_unit("a\r\n  b\r\n", code), Some("  "));
    }

    #[test]
    fn file_unit_skips_lines_that_start_inside_a_literal() {
        // Two doc comments, each a one-space ` * ` step, against two four-space
        // code steps: counting the comments would make the one-space step win.
        let text = "/**\n * a\n */\nf {\n    /**\n     * b\n     */\n    g {\n        h\n";
        let comment = |at: usize| {
            text.get(at..)
                .is_some_and(|rest| rest.starts_with("/*") || rest.starts_with('*'))
        };
        assert_eq!(file_unit(text, code), Some(" "));
        // A skipped line neither steps nor breaks the chain, as a blank one.
        assert_eq!(file_unit(text, comment), Some("    "));
        // A file whose only steps are inside comments shows no step at all.
        let text = "/**\n * a\n */\nf\n";
        assert_eq!(file_unit(text, |at| at < 12), None);
    }

    /// No byte is inside a string or comment.
    fn code(_: usize) -> bool {
        false
    }

    #[test]
    fn deeper_prefers_the_nodes_existing_body() {
        // Tab-indented Go body, with a four-space setting.
        assert_eq!(
            deeper("func f() {\n\tx()", 0, 15, "    ", code).as_deref(),
            Some("\t")
        );
        // The shallowest body line, not a continuation aligned deeper.
        let text = "fn f() {\n    g(a,\n      b);";
        assert_eq!(
            deeper(text, 0, text.len(), "  ", code).as_deref(),
            Some("    ")
        );
        // Lines after the caret do not count.
        let text = "a {\n  b\n}\nif x {\n\t\ty\n}";
        assert_eq!(deeper(text, 10, 16, "    ", code).as_deref(), Some("  "));
    }

    #[test]
    fn deeper_skips_lines_that_start_inside_a_literal() {
        // The shallower line starts inside a string, so the code line decides.
        let text = "def f():\n    x = \"\"\"\n  s\n  \"\"\"\n    y";
        let inside = |at: usize| (21..30).contains(&at);
        assert_eq!(
            deeper(text, 0, text.len(), "\t", inside).as_deref(),
            Some("    ")
        );
        // A shallower comment line does not set the body either.
        assert_eq!(
            deeper("f {\n   // c\n  g", 0, 18, "\t", |at| at == 7).as_deref(),
            Some("  ")
        );
    }

    #[test]
    fn deeper_without_a_body_steps_by_the_files_unit_then_the_setting() {
        let text = "def g():\n  pass\n\ndef f():";
        assert_eq!(
            deeper(text, 17, text.len(), "    ", code).as_deref(),
            Some("  ")
        );
        assert_eq!(
            deeper("  if x {", 0, 8, "\t", code).as_deref(),
            Some("  \t")
        );
        assert_eq!(deeper("x", 9, 9, "\t", code), None);
    }
}

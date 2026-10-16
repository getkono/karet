//! How far "one level deeper" goes, read from the file before the setting.
//!
//! An indent query says *that* a new line sits one level inside the line its
//! enclosing node starts on; this module says *how deep* that is. A file
//! indented with tabs, or two spaces, keeps its own style even when the
//! configured unit differs: the body lines the node already has decide first,
//! then the file's prevailing indentation step, and only then the setting.

use std::collections::BTreeMap;

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

/// The indentation one level inside the node that starts on the line at
/// `line_start`, for a break at byte `caret`.
///
/// The shallowest indentation, deeper than the anchor line's, among the
/// non-blank lines from the one after the anchor through the caret's: the body
/// the node already has. With none, the anchor line's indentation plus the
/// file's own step ([`file_unit`]), or plus `unit` when the file shows none.
pub(crate) fn deeper(text: &str, line_start: usize, caret: usize, unit: &str) -> Option<String> {
    let base = line_indent(text, line_start)?;
    let mut offset = line_start;
    let mut body: Option<&str> = None;
    for (row, line) in text.get(line_start..)?.split('\n').enumerate() {
        if row > 0 && offset > caret {
            break;
        }
        let ws = leading(line);
        let solid = ws.len() < line.trim_end_matches('\r').len();
        if row > 0 && solid && ws.len() > base.len() && ws.starts_with(base) {
            body = Some(body.map_or(ws, |b| if ws.len() < b.len() { ws } else { b }));
        }
        offset += line.len() + 1;
    }
    if let Some(body) = body {
        return Some(body.to_owned());
    }
    Some(format!("{base}{}", file_unit(text).unwrap_or(unit)))
}

/// The file's prevailing indentation step: the commonest way a non-blank line
/// goes deeper than the one before it — one tab, or one to [`MAX_SPACE_STEP`]
/// spaces (the narrower step on a tie). `None` when no line steps in.
pub(crate) fn file_unit(text: &str) -> Option<&'static str> {
    const SPACES: &str = "        ";
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut prev: Option<&str> = None;
    for line in text.lines() {
        let ws = leading(line);
        if ws.len() == line.len() {
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
        assert_eq!(file_unit("a\n\tb\n\t\tc\n"), Some("\t"));
        assert_eq!(file_unit("a:\n  b:\n    c: 1\nd:\n  e: 2\n"), Some("  "));
        assert_eq!(file_unit("f {\n    g {\n        h\n"), Some("    "));
        // Blank lines neither step nor break the chain.
        assert_eq!(file_unit("a\n\n   \n  b\n"), Some("  "));
        // A tie goes to the narrower step; outdents and wide alignment count for nothing.
        assert_eq!(file_unit("a\n    b\nc\n  d\n"), Some("  "));
        assert_eq!(file_unit("call(a,\n                 b)\n"), None);
        assert_eq!(file_unit("flat\nlines\n"), None);
        assert_eq!(file_unit(""), None);
    }

    #[test]
    fn deeper_prefers_the_nodes_existing_body() {
        // Tab-indented Go body, with a four-space setting.
        assert_eq!(
            deeper("func f() {\n\tx()", 0, 15, "    ").as_deref(),
            Some("\t")
        );
        // The shallowest body line, not a continuation aligned deeper.
        let text = "fn f() {\n    g(a,\n      b);";
        assert_eq!(deeper(text, 0, text.len(), "  ").as_deref(), Some("    "));
        // Lines after the caret do not count.
        let text = "a {\n  b\n}\nif x {\n\t\ty\n}";
        assert_eq!(deeper(text, 10, 16, "    ").as_deref(), Some("  "));
    }

    #[test]
    fn deeper_without_a_body_steps_by_the_files_unit_then_the_setting() {
        let text = "def g():\n  pass\n\ndef f():";
        assert_eq!(deeper(text, 17, text.len(), "    ").as_deref(), Some("  "));
        assert_eq!(deeper("  if x {", 0, 8, "\t").as_deref(), Some("  \t"));
        assert_eq!(deeper("x", 9, 9, "\t"), None);
    }
}

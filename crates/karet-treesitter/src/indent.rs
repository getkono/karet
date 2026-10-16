//! Indentation for a new line, from a grammar's indent query.
//!
//! An indent query names up to three captures:
//!
//! - `@indent` marks a node whose inner lines sit one level deeper than the line
//!   the node starts on (a block, an argument list, a Python `def`).
//! - `@outdent` marks a closing token (`}`, `)`, `end`). A node whose last child
//!   is one is *closed*, so a caret just past it is outside the node; and a new
//!   line that begins with one lands at its node's own level rather than inside.
//! - `@open` marks an opening token directly inside an `ERROR` node. Code being
//!   typed is often unbalanced — `fn f() {` with no `}` yet parses as an error,
//!   not a block — so an `@open` token no later `@outdent` sibling closes counts
//!   as an enclosing node starting at that token.
//!
//! The indentation is relative, not absolute: it is the indentation of the line
//! the innermost enclosing `@indent` node starts on, plus one level. A file whose
//! existing indentation is off the grammar's idea of nesting therefore keeps its
//! own style instead of being renumbered from the root.

use karet_core::BytePos;
use tree_sitter::Node;
use tree_sitter::StreamingIterator;

use crate::LanguageId;
use crate::Query;
use crate::SyntaxTree;
#[cfg(any(
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-json",
    feature = "lang-go",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-yaml",
    feature = "lang-lua"
))]
use crate::registry;

/// The karet-authored indent query for `lang`, if one ships for its grammar.
pub(crate) fn query_source(_lang: LanguageId) -> Option<&'static str> {
    #[cfg(feature = "lang-rust")]
    if _lang == registry::RUST {
        return Some(include_str!("../queries/indents/rust.scm"));
    }
    #[cfg(feature = "lang-python")]
    if _lang == registry::PYTHON {
        return Some(include_str!("../queries/indents/python.scm"));
    }
    #[cfg(feature = "lang-javascript")]
    if _lang == registry::JAVASCRIPT {
        return Some(include_str!("../queries/indents/javascript.scm"));
    }
    #[cfg(feature = "lang-typescript")]
    if _lang == registry::TYPESCRIPT || _lang == registry::TSX {
        return Some(include_str!("../queries/indents/typescript.scm"));
    }
    #[cfg(feature = "lang-json")]
    if _lang == registry::JSON {
        return Some(include_str!("../queries/indents/json.scm"));
    }
    #[cfg(feature = "lang-go")]
    if _lang == registry::GO {
        return Some(include_str!("../queries/indents/go.scm"));
    }
    #[cfg(feature = "lang-c")]
    if _lang == registry::C {
        return Some(include_str!("../queries/indents/c.scm"));
    }
    #[cfg(feature = "lang-cpp")]
    if _lang == registry::CPP {
        return Some(include_str!("../queries/indents/cpp.scm"));
    }
    #[cfg(feature = "lang-yaml")]
    if _lang == registry::YAML {
        return Some(include_str!("../queries/indents/yaml.scm"));
    }
    #[cfg(feature = "lang-lua")]
    if _lang == registry::programming::LUA {
        return Some(include_str!("../queries/indents/lua.scm"));
    }
    None
}

/// Where a new line's indentation comes from: the line to copy it from, and
/// whether to go one level deeper than that line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentAnchor {
    /// The 0-based row whose leading whitespace is the base indentation.
    pub row: u32,
    /// The byte offset where that row starts.
    pub line_start: BytePos,
    /// Whether the new line goes one indentation level deeper than `row`.
    pub deeper: bool,
}

/// The nodes captured as `@indent` and `@outdent` near the caret.
struct Captured<'tree> {
    indents: Vec<Node<'tree>>,
    outdents: Vec<Node<'tree>>,
    opens: Vec<Node<'tree>>,
}

impl SyntaxTree {
    /// The [`IndentAnchor`] for a line break inserted over `before..after` (an
    /// empty range for a bare caret), according to `query` — an indent query for
    /// this tree's language (see [`indents_query`](crate::indents_query)).
    ///
    /// `text` must be the source this tree was parsed from, *before* the break
    /// is inserted. `None` when no `@indent` node encloses the break, so the
    /// caller falls back to copying the current line's indentation.
    #[must_use]
    pub fn newline_indent(
        &self,
        query: &Query,
        text: &str,
        before: BytePos,
        after: BytePos,
    ) -> Option<IndentAnchor> {
        let before = before.0.min(text.len());
        let after = after.0.clamp(before, text.len());
        let rest = text.get(after..)?;
        let captured = capture(self, query, text, after, rest)?;
        let encloses = |node: &Node<'_>| {
            node.start_byte() < before
                && (before < node.end_byte()
                    || (before == node.end_byte() && !closed(*node, &captured.outdents)))
        };
        // A line that begins with a closer lands at the level of the node it closes.
        let next = first_solid(rest, after);
        let closer = captured
            .outdents
            .iter()
            .filter(|token| token.start_byte() == next && !token.is_missing())
            .find_map(|token| {
                let parent = token.parent()?;
                captured
                    .indents
                    .iter()
                    .any(|n| n.id() == parent.id())
                    .then_some(parent)
            })
            .filter(encloses);
        if let Some(node) = closer {
            return Some(anchor(node, false));
        }
        captured
            .indents
            .iter()
            .copied()
            .filter(encloses)
            .chain(unclosed_open(&captured, before))
            .max_by_key(|node| (node.start_byte(), std::cmp::Reverse(node.end_byte())))
            .map(|node| anchor(node, true))
    }
}

/// The last `@open` token before `before`, inside an `ERROR` node that reaches
/// the caret, that no later `@outdent` sibling closes.
fn unclosed_open<'tree>(captured: &Captured<'tree>, before: usize) -> Option<Node<'tree>> {
    let in_error = |token: &Node<'_>| {
        token.end_byte() <= before
            && token
                .parent()
                .is_some_and(|p| p.is_error() && p.start_byte() < before && before <= p.end_byte())
    };
    let mut tokens: Vec<(Node<'tree>, bool)> = captured
        .opens
        .iter()
        .filter(|t| in_error(t))
        .map(|t| (*t, true))
        .chain(
            captured
                .outdents
                .iter()
                .filter(|t| in_error(t) && !t.is_missing())
                .map(|t| (*t, false)),
        )
        .collect();
    tokens.sort_by_key(|(t, _)| t.start_byte());
    tokens.dedup_by_key(|(t, _)| t.id());
    let mut stack = Vec::new();
    for (token, opens) in tokens {
        if opens {
            stack.push(token);
        } else {
            stack.pop();
        }
    }
    stack.pop()
}

/// Run `query` over the part of `tree` from the top of the file to the end of
/// the line `rest` (the text from `after` on) starts on.
fn capture<'tree>(
    tree: &'tree SyntaxTree,
    query: &Query,
    text: &str,
    after: usize,
    rest: &str,
) -> Option<Captured<'tree>> {
    let names = query.capture_names();
    let indent = names.iter().position(|n| *n == "indent")?;
    let outdent = names.iter().position(|n| *n == "outdent");
    let open = names.iter().position(|n| *n == "open");
    let mut cursor = tree_sitter::QueryCursor::new();
    // Nothing past the caret's line matters: the closer the new line would begin
    // with is on it. The `@open` tokens of an unbalanced prefix sit anywhere
    // before the caret, so the range starts at the top of the file.
    let end = rest.find('\n').map_or(text.len(), |n| after + n);
    cursor.set_byte_range(0..end.saturating_add(1));
    let root = tree.tree.root_node();
    let mut it = cursor.captures(&query.inner, root, text.as_bytes());
    let mut out = Captured {
        indents: Vec::new(),
        outdents: Vec::new(),
        opens: Vec::new(),
    };
    while let Some((m, idx)) = it.next() {
        let Some(cap) = m.captures.get(*idx) else {
            continue;
        };
        let index = cap.index as usize;
        if index == indent {
            out.indents.push(cap.node);
        } else if Some(index) == outdent {
            out.outdents.push(cap.node);
        } else if Some(index) == open {
            out.opens.push(cap.node);
        }
    }
    Some(out)
}

/// Whether `node` ends with a real (not parser-inserted) `@outdent` token, so a
/// caret right after it is outside the node.
fn closed(node: Node<'_>, outdents: &[Node<'_>]) -> bool {
    let Some(last) = u32::try_from(node.child_count())
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| node.child(i))
    else {
        return false;
    };
    !last.is_missing() && outdents.iter().any(|o| o.id() == last.id())
}

/// The byte offset of the first non-blank character of `rest` (which starts at
/// byte `from`), or its end when it is all blank.
fn first_solid(rest: &str, from: usize) -> usize {
    rest.char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map_or(from + rest.len(), |(i, _)| from + i)
}

/// The anchor on the line `node` starts on.
fn anchor(node: Node<'_>, deeper: bool) -> IndentAnchor {
    let start = node.start_position();
    IndentAnchor {
        row: u32::try_from(start.row).unwrap_or(u32::MAX),
        line_start: BytePos(node.start_byte() - start.column),
        deeper,
    }
}

#[cfg(all(
    test,
    feature = "lang-rust",
    feature = "lang-python",
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-json",
    feature = "lang-go",
    feature = "lang-c",
    feature = "lang-cpp",
    feature = "lang-yaml",
    feature = "lang-lua",
    feature = "lang-toml"
))]
mod tests {
    use super::*;
    use crate::ParserPool;
    use crate::TsError;
    use crate::language_id_from_injection_name;

    /// The indentation a line break over `<|>…<|>` (or at a lone `<|>`) in
    /// `marked` gets with a four-space unit, or `None` for the copy-indent
    /// fallback.
    fn enter(lang: &str, marked: &str) -> Result<Option<String>, TsError> {
        let lang = language_id_from_injection_name(lang).ok_or(TsError::UnknownLanguage)?;
        let before = marked.find("<|>").ok_or(TsError::ParseFailed)?;
        let text = marked.replacen("<|>", "", 1);
        let after = text.find("<|>").unwrap_or(before);
        let text = text.replacen("<|>", "", 1);
        let tree = SyntaxTree::parse(&mut ParserPool::new(), lang, &text)?;
        let query = Query::compile(lang, query_source(lang).ok_or(TsError::UnknownLanguage)?)?;
        let anchor = tree.newline_indent(&query, &text, BytePos(before), BytePos(after));
        Ok(anchor.map(|a| {
            let ws: String = text[a.line_start.0..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            if a.deeper { format!("{ws}    ") } else { ws }
        }))
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    #[test]
    fn every_bundled_indent_query_compiles() -> Result<(), TsError> {
        let names = [
            "rust",
            "python",
            "javascript",
            "typescript",
            "tsx",
            "json",
            "go",
            "c",
            "cpp",
            "yaml",
            "lua",
        ];
        for name in names {
            let lang = language_id_from_injection_name(name).ok_or(TsError::UnknownLanguage)?;
            let source = query_source(lang).ok_or(TsError::UnknownLanguage)?;
            let query = Query::compile(lang, source)?;
            assert!(query.capture_names().contains(&"indent"), "{name}");
        }
        // A grammar karet ships no indent query for keeps the copy-indent fallback.
        let toml = language_id_from_injection_name("toml").ok_or(TsError::UnknownLanguage)?;
        assert!(query_source(toml).is_none());
        Ok(())
    }

    #[test]
    fn rust_indents_inside_delimiters_relative_to_the_opening_line() -> Result<(), TsError> {
        assert_eq!(enter("rust", "fn f() {<|>\n}")?, some("    "));
        assert_eq!(
            enter("rust", "fn f() {\n    let x = 1;<|>\n}")?,
            some("    ")
        );
        assert_eq!(
            enter("rust", "impl A {\n    fn f() {<|>\n    }\n}")?,
            some("        ")
        );
        assert_eq!(enter("rust", "fn f() {\n    g(a,<|>\n}")?, some("        "));
        // Relative, not absolute: an off-grid block keeps its own base.
        assert_eq!(
            enter("rust", "fn f() {\n  if x {<|>\n  }\n}")?,
            some("      ")
        );
        Ok(())
    }

    #[test]
    fn a_closer_after_the_caret_lands_at_its_node_level() -> Result<(), TsError> {
        assert_eq!(enter("rust", "fn f() {<|>}")?, some(""));
        assert_eq!(
            enter("rust", "    fn f() {\n        x<|>    }")?,
            some("    ")
        );
        assert_eq!(enter("python", "foo(<|>)")?, some(""));
        // A selection's end decides what the new line starts with.
        assert_eq!(enter("rust", "fn f() {<|>xx<|>}")?, some(""));
        Ok(())
    }

    #[test]
    fn a_caret_past_a_closed_node_is_outside_it() -> Result<(), TsError> {
        assert_eq!(enter("rust", "fn f() {}<|>")?, None);
        assert_eq!(enter("rust", "fn f() {\n    g(a)<|>\n}")?, some("    "));
        assert_eq!(enter("python", "x = 1<|>")?, None);
        Ok(())
    }

    #[test]
    fn unbalanced_openers_being_typed_still_indent() -> Result<(), TsError> {
        assert_eq!(enter("rust", "fn f() {<|>")?, some("    "));
        assert_eq!(enter("go", "func f() {<|>")?, some("    "));
        assert_eq!(enter("lua", "function f()<|>")?, some("    "));
        assert_eq!(enter("c", "int f() {<|>")?, some("    "));
        assert_eq!(enter("javascript", "const o = {<|>")?, some("    "));
        Ok(())
    }

    #[test]
    fn python_bodies_without_closers_stay_indented() -> Result<(), TsError> {
        assert_eq!(enter("python", "def f():<|>")?, some("    "));
        assert_eq!(enter("python", "def f():\n    return 1<|>")?, some("    "));
        assert_eq!(
            enter("python", "class A:\n    def f(self):<|>")?,
            some("        ")
        );
        assert_eq!(enter("python", "if x:\n    a()\nelse:<|>")?, some("    "));
        Ok(())
    }

    #[test]
    fn js_ts_go_c_cpp_and_json_indent_their_blocks() -> Result<(), TsError> {
        assert_eq!(enter("javascript", "function f() {<|>\n}")?, some("    "));
        assert_eq!(
            enter("javascript", "const o = {\n  a: 1,<|>\n};")?,
            some("    ")
        );
        assert_eq!(enter("typescript", "interface A {<|>\n}")?, some("    "));
        assert_eq!(enter("tsx", "function f() {<|>\n}")?, some("    "));
        assert_eq!(enter("go", "func f() {<|>\n}")?, some("    "));
        assert_eq!(enter("go", "switch x {\ncase 1:<|>\n}")?, some("    "));
        assert_eq!(enter("c", "int f() {<|>\n}")?, some("    "));
        assert_eq!(enter("cpp", "namespace a {<|>\n}")?, some("    "));
        assert_eq!(enter("json", "{\n    \"a\": 1,<|>\n}")?, some("    "));
        assert_eq!(enter("json", "{\"a\": [<|>]}")?, some(""));
        Ok(())
    }

    #[test]
    fn yaml_indents_under_a_key_only_for_a_nested_block() -> Result<(), TsError> {
        assert_eq!(enter("yaml", "a:<|>")?, some("    "));
        assert_eq!(enter("yaml", "a:\n  b: 1<|>")?, some("    "));
        assert_eq!(enter("yaml", "a: 1<|>")?, None);
        Ok(())
    }

    #[test]
    fn lua_blocks_close_with_end() -> Result<(), TsError> {
        assert_eq!(enter("lua", "function f()\n  x()<|>\nend")?, some("    "));
        assert_eq!(enter("lua", "if x then<|>\nend")?, some("    "));
        assert_eq!(enter("lua", "if x then\n  y()<|>end")?, some(""));
        Ok(())
    }

    #[test]
    fn out_of_range_positions_do_not_panic() -> Result<(), TsError> {
        let lang = language_id_from_injection_name("rust").ok_or(TsError::UnknownLanguage)?;
        let text = "fn f() {}";
        let tree = SyntaxTree::parse(&mut ParserPool::new(), lang, text)?;
        let query = Query::compile(lang, query_source(lang).ok_or(TsError::UnknownLanguage)?)?;
        assert_eq!(
            tree.newline_indent(&query, text, BytePos(99), BytePos(3)),
            None
        );
        // A query with no `@indent` capture yields nothing rather than failing.
        let empty = Query::compile(lang, "(block) @other")?;
        assert_eq!(
            tree.newline_indent(&empty, text, BytePos(8), BytePos(8)),
            None
        );
        Ok(())
    }
}

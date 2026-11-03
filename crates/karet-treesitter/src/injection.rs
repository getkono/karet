//! Language injection: discovering the embedded-language regions of a tree.
//!
//! A grammar's injections query marks nodes whose *text* is written in another
//! language — a markdown code fence, an HTML `<script>` body, a Rust doc comment.
//! Each match names a language, either statically (`#set! injection.language "css"`)
//! or dynamically (an `@injection.language` capture whose text is read from the
//! source, as a code fence's info string is).
//!
//! Everything tree-sitter-shaped stays in this module: the public surface is the
//! neutral [`InjectionRegion`].

use karet_core::BytePos;
use karet_core::Span;

use crate::LanguageId;
use crate::Query;
use crate::language_id_from_injection_name;

/// The capture naming the region to re-parse in another language.
const CONTENT: &str = "injection.content";
/// The capture whose *text* names the language (e.g. a code fence's info string).
const LANGUAGE: &str = "injection.language";

/// One embedded-language region discovered by an injections query.
///
/// `ranges` is normally a single span, but a content node whose named children are
/// excluded (tree-sitter's default when `injection.include-children` is unset) yields
/// the gaps *between* those children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectionRegion {
    /// The embedded language.
    pub lang: LanguageId,
    /// The byte ranges of the embedded text, in document coordinates, ascending.
    pub ranges: Vec<Span>,
    /// Whether every region of this pattern should be parsed as one tree (`#set!
    /// injection.combined`) rather than one tree per region.
    pub combined: bool,
}

/// Run `query` (an injections query) over `tree` and collect every resolvable region.
///
/// Regions naming a language with no grammar compiled in are skipped, so an unknown
/// code fence (` ```jsdoc `) simply renders as plain text.
pub(crate) fn extract(tree: &tree_sitter::Tree, query: &Query, text: &str) -> Vec<InjectionRegion> {
    use tree_sitter::StreamingIterator;

    let names = query.inner.capture_names();
    // A query with no `@injection.content` cannot inject anything.
    let Some(content_idx) = names.iter().position(|n| *n == CONTENT) else {
        return Vec::new();
    };
    let language_idx = names.iter().position(|n| *n == LANGUAGE);

    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query.inner, tree.root_node(), text.as_bytes());
    let mut out = Vec::new();

    while let Some(m) = matches.next() {
        let props = Properties::read(query, m.pattern_index);

        // A dynamic `@injection.language` capture wins over a static `#set!`, because
        // a grammar that offers both means the capture to specialize the default.
        let dynamic = language_idx
            .and_then(|i| m.captures.iter().find(|c| c.index as usize == i))
            .and_then(|c| c.node.utf8_text(text.as_bytes()).ok());
        let Some(lang) = dynamic
            .or(props.language.as_deref())
            .and_then(language_id_from_injection_name)
        else {
            continue;
        };

        for cap in m
            .captures
            .iter()
            .filter(|c| c.index as usize == content_idx)
        {
            let ranges = content_ranges(cap.node, props.include_children);
            if !ranges.is_empty() {
                out.push(InjectionRegion {
                    lang,
                    ranges,
                    combined: props.combined,
                });
            }
        }
    }
    if is_rustdoc_markdown(tree, text) {
        add_rustdoc_doctests(tree, text, &mut out);
    }
    let included = tree.included_ranges();
    for region in &mut out {
        region.ranges = clip_to_included(&region.ranges, &included);
    }
    out.retain(|r| !r.ranges.is_empty());
    out
}

/// `ranges` restricted to the bytes `tree`'s own layer parses.
///
/// A node of an injected layer spans the host text between that layer's included
/// ranges — a code block in a doc comment covers the next line's `///` — so a region
/// cut from it would hand the embedded language the host's comment markers too.
fn clip_to_included(ranges: &[Span], included: &[tree_sitter::Range]) -> Vec<Span> {
    let mut out = Vec::new();
    for span in ranges {
        for r in included {
            out.extend(non_empty(
                span.start.0.max(r.start_byte),
                span.end.0.min(r.end_byte),
            ));
        }
    }
    out
}

/// Whether `tree` is the markdown layer of a Rust doc comment.
///
/// Rust's doc-comment injection hands markdown the comment's *content*, which starts
/// immediately after the `///`, `//!`, `/**` or `/*!` marker, so a markdown tree whose
/// first included range follows one of those markers is rustdoc. A markdown file
/// parsed on its own includes the whole text from byte 0 and never qualifies.
fn is_rustdoc_markdown(tree: &tree_sitter::Tree, text: &str) -> bool {
    if crate::languages::RUST.is_none() || tree.language().name() != Some("markdown") {
        return false;
    }
    let Some(first) = tree.included_ranges().first().map(|r| r.start_byte) else {
        return false;
    };
    let before = text.get(..first).unwrap_or("");
    ["///", "//!", "/**", "/*!"]
        .iter()
        .any(|marker| before.ends_with(marker))
}

/// Inject Rust into every code block rustdoc compiles as a doctest that the query did
/// not already resolve.
///
/// Rustdoc treats a bare fence, and one whose info string holds only doctest
/// attributes (` ```no_run `, ` ```should_panic,edition2021 `), as Rust. Markdown's
/// own injections query resolves a fence by its first word, so it finds no grammar
/// for those and would leave the doctest unhighlighted. Rustdoc also compiles an
/// indented code block as Rust, and markdown's query never injects one at all.
///
/// Rustdoc strips a doc comment's shared indentation before markdown reads it, so
/// `///     x` after `/// Text.` is indented 4 columns to rustdoc, not 5. The markdown
/// layer sees the unstripped text, and an indented block counts as a doctest only
/// when it sits 4 columns beyond its doc comment's shared indentation.
fn add_rustdoc_doctests(tree: &tree_sitter::Tree, text: &str, out: &mut Vec<InjectionRegion>) {
    let Some(rust) = crate::languages::RUST else {
        return;
    };
    let included = tree.included_ranges();
    let mut indented = Vec::new();
    collect_kind(tree.root_node(), "indented_code_block", &mut indented);
    for block in indented {
        if !is_rustdoc_indented_block(&included, text, block.start_byte()) {
            continue;
        }
        let ranges = content_ranges(block, false);
        if !ranges.is_empty() {
            out.push(InjectionRegion {
                lang: rust,
                ranges,
                combined: false,
            });
        }
    }
    let mut fences = Vec::new();
    collect_kind(tree.root_node(), "fenced_code_block", &mut fences);
    for fence in fences {
        let mut cursor = fence.walk();
        let children: Vec<_> = fence.named_children(&mut cursor).collect();
        let info = children
            .iter()
            .find(|c| c.kind() == "info_string")
            .and_then(|c| c.utf8_text(text.as_bytes()).ok())
            .unwrap_or("");
        let Some(content) = children.iter().find(|c| c.kind() == "code_fence_content") else {
            continue;
        };
        let (start, end) = (content.start_byte(), content.end_byte());
        let already = out.iter().any(|r| {
            r.ranges
                .first()
                .is_some_and(|s| s.start.0 >= start && s.start.0 < end)
        });
        if already || !rustdoc_fence_is_rust(info) {
            continue;
        }
        let ranges = content_ranges(*content, false);
        if !ranges.is_empty() {
            out.push(InjectionRegion {
                lang: rust,
                ranges,
                combined: false,
            });
        }
    }
}

/// Whether the indented code block starting at byte `start` of a rustdoc markdown
/// layer (whose included ranges are `included`) is still indented 4 columns once
/// rustdoc strips its doc comment's shared indentation.
fn is_rustdoc_indented_block(included: &[tree_sitter::Range], text: &str, start: usize) -> bool {
    // The block's first line: the first included byte at or after `start` (a node can
    // begin in the host text between two ranges), back to its line's start.
    let Some(index) = included.iter().position(|r| start < r.end_byte) else {
        return false;
    };
    let range_start = included.get(index).map_or(start, |r| r.start_byte);
    let byte = start.max(range_start);
    let line_start = text
        .get(range_start..byte)
        .and_then(|s| s.rfind('\n'))
        .map_or(range_start, |nl| range_start + nl + 1);
    let line = text.get(line_start..).unwrap_or("");
    let shared = doc_comment_of(included, text, index)
        .filter_map(|r| text.get(r.start_byte..r.end_byte))
        .flat_map(str::lines)
        .filter(|l| !l.trim().is_empty())
        .map(leading_indent)
        .min()
        .unwrap_or(0);
    // Rustdoc removes `shared` whitespace characters; markdown then measures the rest
    // in columns, a tab advancing to the next multiple of 4.
    let mut column = 0;
    for c in line.chars().skip(shared) {
        match c {
            ' ' => column += 1,
            '\t' => column += 4 - column % 4,
            _ => break,
        }
    }
    column >= 4
}

/// The included ranges of the one doc comment `included[index]` belongs to.
///
/// A rustdoc markdown layer combines every doc comment of the file, but rustdoc
/// strips indentation per item: `///` (or `//!`) lines separated by nothing but
/// whitespace are one comment, and anything else between two ranges — code, a
/// block comment's `*/` — ends it.
fn doc_comment_of<'r>(
    included: &'r [tree_sitter::Range],
    text: &str,
    index: usize,
) -> impl Iterator<Item = &'r tree_sitter::Range> {
    let joined = |i: usize| {
        let (Some(a), Some(b)) = (included.get(i), included.get(i + 1)) else {
            return false;
        };
        let marker = text.get(a.end_byte..b.start_byte).unwrap_or("").trim();
        marker == "///" || marker == "//!"
    };
    let first = (0..index)
        .rev()
        .take_while(|&i| joined(i))
        .last()
        .unwrap_or(index);
    let last = (index..included.len())
        .take_while(|&i| joined(i))
        .last()
        .map_or(index, |i| i + 1);
    included.get(first..=last).unwrap_or(&[]).iter()
}

/// The number of leading space and tab characters of `line`, as rustdoc counts them.
fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Every node of `kind` at or beneath `node`, in document order.
fn collect_kind<'t>(node: tree_sitter::Node<'t>, kind: &str, out: &mut Vec<tree_sitter::Node<'t>>) {
    if node.kind() == kind {
        out.push(node);
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_kind(child, kind, out);
    }
}

/// Whether rustdoc compiles a fence with this info string as Rust: the string is
/// empty, or every word in it (split on commas and whitespace, ignoring a `{…}`
/// attribute block) is `rust` or a doctest attribute. A word naming another
/// language (` ```text `, ` ```sh,no_run `) makes the fence not Rust.
fn rustdoc_fence_is_rust(info: &str) -> bool {
    let info = info.split('{').next().unwrap_or("");
    info.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .all(is_rustdoc_word)
}

/// `rust`, or an attribute rustdoc reads from a doctest fence.
fn is_rustdoc_word(word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    matches!(
        word.as_str(),
        "rust"
            | "ignore"
            | "no_run"
            | "should_panic"
            | "compile_fail"
            | "test_harness"
            | "standalone_crate"
            | "allow_fail"
    ) || word.strip_prefix("ignore-").is_some_and(|t| !t.is_empty())
        || word.strip_prefix("edition").is_some_and(digits)
        || word
            .strip_prefix('e')
            .is_some_and(|code| code.len() == 4 && digits(code))
}

/// The `#set!` directives attached to one injection pattern.
#[derive(Default)]
struct Properties {
    language: Option<String>,
    combined: bool,
    include_children: bool,
}

impl Properties {
    fn read(query: &Query, pattern_index: usize) -> Self {
        let mut props = Self::default();
        for p in query.inner.property_settings(pattern_index) {
            match &*p.key {
                "injection.language" => {
                    props.language = p.value.as_deref().map(str::to_owned);
                },
                "injection.combined" => props.combined = true,
                "injection.include-children" => props.include_children = true,
                _ => {},
            }
        }
        props
    }
}

impl crate::LayeredTree {
    /// The languages whose layers contain `byte`, innermost first and ending with the
    /// root's language — e.g. `[javascript, html]` inside an HTML `<script>` body.
    ///
    /// A layer contains a byte when one of its included ranges does (half-open, so
    /// the byte just past a region belongs to its parent). Callers that need a
    /// property the innermost language lacks (a paragraph's `markdown_inline` layer
    /// has no comment syntax) walk outward through the rest.
    #[must_use]
    pub fn languages_at(&self, byte: BytePos) -> Vec<LanguageId> {
        self.layers_at(byte)
            .into_iter()
            .map(crate::SyntaxTree::language)
            .collect()
    }

    /// The layers that contain `byte`, innermost first and ending with the root —
    /// the trees behind [`languages_at`](Self::languages_at), for a caller that
    /// must also inspect each layer's nodes at `byte` (e.g. to tell that an
    /// injected layer sits inside a host comment).
    #[must_use]
    pub fn layers_at(&self, byte: BytePos) -> Vec<&crate::SyntaxTree> {
        // Layers are stored breadth-first, so the deepest containing layer is last.
        let mut out: Vec<&crate::SyntaxTree> = self
            .children()
            .iter()
            .rev()
            .filter(|layer| {
                layer
                    .tree
                    .included_ranges()
                    .iter()
                    .any(|r| r.start_byte <= byte.0 && byte.0 < r.end_byte)
            })
            .collect();
        out.push(self.root());
        out
    }
}

/// The byte ranges of a content node.
///
/// With `include_children`, that is simply the node's span. Without it — tree-sitter's
/// default — the node's *named children* are carved out, leaving the text between
/// them. This is what lets a grammar inject only the literal chunks of a node that
/// also contains structured sub-nodes.
fn content_ranges(node: tree_sitter::Node, include_children: bool) -> Vec<Span> {
    let (start, end) = (node.start_byte(), node.end_byte());
    if include_children || node.named_child_count() == 0 {
        return non_empty(start, end).into_iter().collect();
    }

    let mut ranges = Vec::new();
    let mut pos = start;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        ranges.extend(non_empty(pos, child.start_byte()));
        pos = child.end_byte();
    }
    ranges.extend(non_empty(pos, end));
    ranges
}

/// `[start, end)` as a [`Span`], or `None` when empty or inverted.
fn non_empty(start: usize, end: usize) -> Option<Span> {
    (end > start).then_some(Span {
        start: BytePos(start),
        end: BytePos(end),
    })
}

/// Sort and merge `ranges` into ascending, non-overlapping spans.
///
/// `tree_sitter::Parser::set_included_ranges` rejects anything else, and combined
/// injections concatenate ranges from independent matches, so overlap is possible.
pub(crate) fn normalize(mut ranges: Vec<Span>) -> Vec<Span> {
    ranges.retain(|s| s.end.0 > s.start.0);
    karet_core::merge_spans(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    #[test]
    fn normalize_sorts_and_merges() {
        let got = normalize(vec![span(10, 15), span(0, 4), span(3, 8)]);
        assert_eq!(got, vec![span(0, 8), span(10, 15)]);
    }

    #[test]
    fn normalize_merges_touching_ranges() {
        assert_eq!(normalize(vec![span(0, 4), span(4, 9)]), vec![span(0, 9)]);
    }

    #[test]
    fn normalize_drops_empty_ranges() {
        assert!(normalize(vec![span(5, 5), span(9, 2)]).is_empty());
    }

    #[cfg(all(feature = "lang-html", feature = "lang-javascript"))]
    #[test]
    fn languages_at_lists_the_containing_layers_innermost_first() -> Result<(), crate::TsError> {
        let lang =
            |name| language_id_from_injection_name(name).ok_or(crate::TsError::UnknownLanguage);
        let (html, js) = (lang("html")?, lang("javascript")?);
        let src = "<p>hi</p>\n<script>\nlet x = 1;\n</script>\n";
        let tree = crate::LayeredParser::new().parse(html, src)?;
        let at = |needle: &str| BytePos(src.find(needle).unwrap_or(0));
        assert_eq!(tree.languages_at(at("let")), vec![js, html]);
        assert_eq!(tree.languages_at(at("<p>")), vec![html]);
        // The byte just past the script body belongs to the HTML again.
        assert_eq!(tree.languages_at(at("</script>")), vec![html]);
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn included_ranges_leave_out_the_gaps_a_span_covers() -> Result<(), crate::TsError> {
        // Two doc comments combine into one markdown layer whose span runs over the
        // code between them; its included ranges must not.
        let lang =
            |name| language_id_from_injection_name(name).ok_or(crate::TsError::UnknownLanguage);
        let (rust, md) = (lang("rust")?, lang("markdown")?);
        let src = "/// a\nfn f() {}\n/// b\nfn g() {}\n";
        let tree = crate::LayeredParser::new().parse(rust, src)?;
        let root = tree.root().included_ranges();
        assert_eq!(root.len(), 1);
        assert!(root.iter().all(|r| r.start.0 == 0 && r.end.0 >= src.len()));
        let layer = tree
            .children()
            .iter()
            .find(|c| c.language() == md)
            .ok_or(crate::TsError::UnknownLanguage)?;
        let ranges = layer.included_ranges();
        let texts: Vec<_> = ranges
            .iter()
            .filter_map(|r| src.get(r.start.0..r.end.0))
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.iter().all(|t| !t.contains("fn")), "{texts:?}");
        assert!(
            ranges
                .iter()
                .zip(ranges.iter().skip(1))
                .all(|(a, b)| a.end <= b.start)
        );
        let gap = src.find("fn f").unwrap_or_default();
        assert!(layer.span().start.0 <= gap && gap < layer.span().end.0);
        Ok(())
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn languages_at_reaches_a_fenced_code_block_and_inline_prose() -> Result<(), crate::TsError> {
        let lang =
            |name| language_id_from_injection_name(name).ok_or(crate::TsError::UnknownLanguage);
        let (md, rust, inline) = (lang("markdown")?, lang("rust")?, lang("markdown_inline")?);
        let src = "Some prose.\n\n```rust\nfn main() {}\n```\n";
        let tree = crate::LayeredParser::new().parse(md, src)?;
        let at = |needle: &str| BytePos(src.find(needle).unwrap_or(0));
        assert_eq!(tree.languages_at(at("fn main")), vec![rust, md]);
        assert_eq!(tree.languages_at(at("prose")), vec![inline, md]);
        Ok(())
    }

    #[cfg(all(feature = "lang-markdown", feature = "lang-rust"))]
    #[test]
    fn layers_at_returns_the_trees_behind_languages_at() -> Result<(), crate::TsError> {
        let rust =
            language_id_from_injection_name("rust").ok_or(crate::TsError::UnknownLanguage)?;
        let src = "/// Adds *one*.\npub fn f() {}\n";
        let tree = crate::LayeredParser::new().parse(rust, src)?;
        let byte = BytePos(src.find("Adds").unwrap_or(0));
        let layers = tree.layers_at(byte);
        let langs: Vec<_> = layers.iter().map(|l| l.language()).collect();
        assert_eq!(langs, tree.languages_at(byte));
        // The root comes last, and at a doc comment's text it is a comment node.
        let root = layers.last().ok_or(crate::TsError::UnknownLanguage)?;
        assert!(
            root.named_ancestors_at(byte)
                .iter()
                .any(|n| n.kind == "line_comment")
        );
        assert_eq!(
            tree.layers_at(BytePos(src.find("pub").unwrap_or(0))).len(),
            1
        );
        Ok(())
    }

    /// How many Rust layers the doctest fence `info` yields inside a doc comment
    /// written with `open` (and closed by `close` on the last line).
    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    fn doctest_layers(
        info: &str,
        open: &str,
        cont: &str,
        close: &str,
    ) -> Result<usize, crate::TsError> {
        let rust =
            language_id_from_injection_name("rust").ok_or(crate::TsError::UnknownLanguage)?;
        let src =
            format!("{open} ```{info}\n{cont} let y = 1;\n{cont} ```{close}\npub fn f() {{}}\n");
        let tree = crate::LayeredParser::new().parse(rust, &src)?;
        let (body, end) = (
            src.find("let y").unwrap_or(0),
            src.find("pub fn").unwrap_or(0),
        );
        Ok(tree
            .children()
            .iter()
            .filter(|c| c.language() == rust && c.span().start.0 >= body && c.span().end.0 <= end)
            .count())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rustdoc_doctest_fences_are_rust() -> Result<(), crate::TsError> {
        // Rustdoc compiles a bare fence, and one holding only doctest attributes, as
        // Rust; a fence naming another language it leaves alone.
        for info in [
            "",
            "rust,no_run",
            "rust,compile_fail",
            "rust,ignore",
            "no_run",
            "ignore",
            "should_panic,edition2018",
            "compile_fail,E0277",
        ] {
            for (open, cont, close) in [("///", "///", ""), ("//!", "//!", ""), ("/**", "", "\n*/")]
            {
                let got = doctest_layers(info, open, cont, close)?;
                assert_eq!(got, 1, "{open} ```{info}");
            }
        }
        for info in ["text", "sh,no_run"] {
            assert_eq!(doctest_layers(info, "///", "///", "")?, 0, "```{info}");
        }
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rustdoc_multiline_code_blocks_are_rust_without_comment_markers() -> Result<(), crate::TsError>
    {
        // Rustdoc compiles an indented code block as a doctest too. Every line of a
        // multi-line block is Rust, and the next line's `///` stays the host's: it
        // must not reach the Rust layer as a nested doc comment.
        let rust =
            language_id_from_injection_name("rust").ok_or(crate::TsError::UnknownLanguage)?;
        for (open, cont, close) in [("///", "///", ""), ("//!", "//!", ""), ("/**", "", "\n*/")] {
            let indented = format!(
                "{open} Adds one.\n{cont}\n{cont}     let y = 1;\n{cont}     let z = y;{close}\npub fn f() {{}}\n"
            );
            let fenced = format!(
                "{open} ```\n{cont} let y = 1;\n{cont} let z = y;\n{cont} ```{close}\npub fn f() {{}}\n"
            );
            for src in [indented, fenced] {
                let tree = crate::LayeredParser::new().parse(rust, &src)?;
                let end = src.find("pub fn").unwrap_or(0);
                let layers: Vec<_> = tree
                    .children()
                    .iter()
                    .filter(|c| c.language() == rust && c.span().end.0 <= end)
                    .collect();
                assert_eq!(layers.len(), 1, "{src}");
                for line in ["let y", "let z"] {
                    let at = BytePos(src.find(line).unwrap_or(0));
                    let langs = tree.languages_at(at);
                    assert_eq!(langs.first(), Some(&rust), "{line} in {src}");
                    // Rust (the doctest), markdown, then the host: no layer between.
                    assert_eq!(langs.len(), 3, "{line} in {src}");
                }
            }
        }
        Ok(())
    }

    /// Whether the text at `needle` in the Rust source `src` lies in a Rust doctest.
    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    fn is_doctest(src: &str, needle: &str) -> Result<bool, crate::TsError> {
        let rust =
            language_id_from_injection_name("rust").ok_or(crate::TsError::UnknownLanguage)?;
        let tree = crate::LayeredParser::new().parse(rust, src)?;
        let at = BytePos(src.find(needle).ok_or(crate::TsError::UnknownLanguage)?);
        let langs = tree.languages_at(at);
        Ok(langs.len() > 2 && langs.first() == Some(&rust))
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rustdoc_indented_blocks_are_measured_past_the_shared_indent() -> Result<(), crate::TsError> {
        // Rustdoc strips the 1-space indent every line shares, leaving `x` indented 3
        // columns: a paragraph, though markdown alone sees 4 and a code block.
        let src = "/// Text.\n///\n///    let x = 1;\npub fn f() {}\n";
        assert!(!is_doctest(src, "let x")?, "{src}");
        // With no shared indent, the same 4 columns are a doctest.
        let src = "///Text.\n///\n///    let x = 1;\npub fn f() {}\n";
        assert!(is_doctest(src, "let x")?, "{src}");
        // A tab reaches column 4 past the shared indent too.
        let src = "/// Text.\n///\n/// \tlet x = 1;\npub fn f() {}\n";
        assert!(is_doctest(src, "let x")?, "{src}");
        // A `///` line indented in the source continues the same doc comment.
        let src = "/// Text.\n///\n    ///    let x = 1;\npub fn f() {}\n";
        assert!(!is_doctest(src, "let x")?, "{src}");
        Ok(())
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn rustdoc_shared_indent_is_per_doc_comment() -> Result<(), crate::TsError> {
        // One markdown layer spans every doc comment in the file, but each item's
        // comment has its own shared indent: `f`'s is 0, `g`'s is 1.
        let src = "///Text.\n///\n///    let x = 1;\npub fn f() {}\n\n\
                   /// Text.\n///\n///    let y = 2;\npub fn g() {}\n";
        assert!(is_doctest(src, "let x")?, "{src}");
        assert!(!is_doctest(src, "let y")?, "{src}");
        let src = "/// Text.\n///\n///    let y = 2;\npub fn g() {}\n\n\
                   ///Text.\n///\n///    let x = 1;\npub fn f() {}\n";
        assert!(is_doctest(src, "let x")?, "{src}");
        assert!(!is_doctest(src, "let y")?, "{src}");
        Ok(())
    }

    #[test]
    fn leading_indent_counts_spaces_and_tabs_as_one_each() {
        assert_eq!(leading_indent("  \t x"), 4);
        assert_eq!(leading_indent("x  "), 0);
        assert_eq!(leading_indent(""), 0);
    }

    #[cfg(all(feature = "lang-rust", feature = "lang-markdown"))]
    #[test]
    fn a_bare_fence_in_a_markdown_file_stays_plain() -> Result<(), crate::TsError> {
        // The rustdoc default belongs to doc comments only.
        let lang =
            |name| language_id_from_injection_name(name).ok_or(crate::TsError::UnknownLanguage);
        let (md, rust) = (lang("markdown")?, lang("rust")?);
        let src = "```\nlet y = 1;\n```\n\n```no_run\nlet z = 2;\n```\n\n    let w = 3;\n";
        let tree = crate::LayeredParser::new().parse(md, src)?;
        assert!(!tree.children().iter().any(|c| c.language() == rust));
        Ok(())
    }

    #[test]
    fn rustdoc_fences_without_another_language_are_rust() {
        for info in [
            "",
            "rust",
            "no_run",
            "ignore",
            "should_panic,edition2018",
            "rust,compile_fail,E0277",
            "compile_fail E0599",
            "ignore-wasm32",
            "edition2024 {.example}",
            "  Rust , NO_RUN ",
        ] {
            assert!(rustdoc_fence_is_rust(info), "{info:?}");
        }
    }

    #[test]
    fn rustdoc_fences_naming_another_language_are_not_rust() {
        for info in [
            "text",
            "sh",
            "sh,no_run",
            "toml",
            "edition",
            "ignore-",
            "E12",
            "rusty",
        ] {
            assert!(!rustdoc_fence_is_rust(info), "{info:?}");
        }
    }

    #[test]
    fn non_empty_rejects_degenerate() {
        assert_eq!(non_empty(2, 5), Some(span(2, 5)));
        assert_eq!(non_empty(5, 5), None);
        assert_eq!(non_empty(6, 5), None);
    }
}

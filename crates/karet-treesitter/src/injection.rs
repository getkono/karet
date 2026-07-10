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
    out
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
    ranges.sort_by_key(|s| (s.start.0, s.end.0));
    let mut out: Vec<Span> = Vec::with_capacity(ranges.len());
    for span in ranges {
        match out.last_mut() {
            // Overlapping or touching — extend rather than emit a second range.
            Some(last) if span.start.0 <= last.end.0 => {
                last.end = BytePos(last.end.0.max(span.end.0));
            },
            _ => out.push(span),
        }
    }
    out
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

    #[test]
    fn non_empty_rejects_degenerate() {
        assert_eq!(non_empty(2, 5), Some(span(2, 5)));
        assert_eq!(non_empty(5, 5), None);
        assert_eq!(non_empty(6, 5), None);
    }
}

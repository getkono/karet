//! The tree-sitter highlighter: run a grammar's highlights query and flatten the
//! (overlapping) captures into ordered, non-overlapping [`HighlightSpan`]s.

use std::collections::HashMap;

use karet_core::BytePos;
use karet_core::Span;
use karet_core::TokenId;
use karet_treesitter::Edit;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredTree;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use karet_treesitter::merge_spans;
use karet_treesitter::shift_span;

use crate::HighlightSpan;
use crate::Highlights;
use crate::SyntaxError;
use crate::map::map_capture;

/// Computes [`Highlights`] from a [`SyntaxTree`] using a grammar's highlights query.
pub struct Highlighter {
    query: Query,
    /// Capture-index → (token, specificity), precomputed from the query's capture names.
    lut: Vec<Option<(TokenId, u8)>>,
}

/// How specific a capture name is: the number of dot-separated segments.
///
/// Grammars refine a general capture with a narrower one over the *same* node —
/// tree-sitter-rust captures `(line_comment) @comment` and then
/// `(line_comment (doc_comment)) @comment.documentation`. Ranking by specificity lets
/// the narrower name win regardless of the order the grammar happens to declare them.
fn specificity(capture_name: &str) -> u8 {
    let segments = capture_name.split('.').count();
    u8::try_from(segments).unwrap_or(u8::MAX)
}

impl Highlighter {
    /// Build a highlighter for `lang`.
    ///
    /// # Errors
    /// Returns [`SyntaxError::UnsupportedLanguage`] if no grammar/highlights query is
    /// compiled in for `lang`, or [`SyntaxError::Query`] if the query fails to compile.
    pub fn new(lang: LanguageId) -> Result<Self, SyntaxError> {
        let source =
            karet_treesitter::highlights_query(lang).ok_or(SyntaxError::UnsupportedLanguage)?;
        let query = Query::compile(lang, source).map_err(|e| SyntaxError::Query(e.to_string()))?;
        let lut: Vec<Option<(TokenId, u8)>> = query
            .capture_names()
            .iter()
            .map(|n| map_capture(n).map(|token| (token, specificity(n))))
            .collect();
        Ok(Self { query, lut })
    }

    /// Highlight `text` using its parsed `tree`.
    ///
    /// `tree` must have been parsed (with `karet-treesitter`) for the same language
    /// this highlighter was built for, from the same `text`.
    ///
    /// # Errors
    /// This currently never errors; the result is `Ok` for API symmetry with future
    /// query-time failures.
    pub fn highlight(&self, tree: &SyntaxTree, text: &str) -> Result<Highlights, SyntaxError> {
        let mut caps: Vec<Cap> = Vec::new();
        self.collect(tree, text, (0, None), &mut caps);
        Ok(Highlights::from_sorted_spans(flatten(caps)))
    }

    /// Append this grammar's captures over `tree` to `out`, stamped with `layer`:
    /// every capture, or with `range` only those of the matches intersecting it.
    fn collect(
        &self,
        tree: &SyntaxTree,
        text: &str,
        (layer, range): (u32, Option<Span>),
        out: &mut Vec<Cap>,
    ) {
        let captures = match range {
            Some(range) => tree.captures_in(&self.query, text, range),
            None => tree.captures(&self.query, text),
        };
        for rc in captures {
            if let Some(Some((token, specificity))) = self.lut.get(rc.capture as usize) {
                out.push(Cap {
                    start: rc.span.start.0,
                    end: rc.span.end.0,
                    token: *token,
                    layer,
                    specificity: *specificity,
                    key: rc.capture,
                });
            }
        }
    }
}

/// Highlights a [`LayeredTree`], merging the captures of every injected language into
/// one span list.
///
/// Layers share the document's byte coordinates, so merging is just concatenation plus
/// the usual innermost-wins flatten: a fenced block's rust `keyword` span is smaller
/// than markdown's `text.literal` span over the whole fence, so the keyword paints over
/// it. Where the embedded grammar says nothing, the parent's color shows through.
#[derive(Default)]
pub struct LayeredHighlighter {
    /// One highlighter per language; `None` records "no grammar/query compiled in", so
    /// a missing grammar is not retried on every keystroke.
    by_lang: HashMap<LanguageId, Option<Highlighter>>,
}

impl LayeredHighlighter {
    /// Create an empty layered highlighter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Highlight every layer of `tree` against `text` and merge the result.
    ///
    /// A layer whose language has no highlights query simply contributes nothing, so an
    /// exotic embedded language degrades to plain text rather than failing the document.
    pub fn highlight(&mut self, tree: &LayeredTree, text: &str) -> Highlights {
        Highlights::from_sorted_spans(flatten(self.caps(tree, text, None)))
    }

    /// Re-highlight `tree` after an incremental reparse, re-running the highlights
    /// queries only where the syntax may have changed.
    ///
    /// `previous` is this highlighter's output for the text before `edits`, and
    /// `changed` the ranges [`LayeredParser::reparse_changed`] reported for them.
    /// The result equals [`highlight`](Self::highlight) over the new tree: every
    /// changed range is widened to whole lines and then to the full extent of each
    /// capture that reaches it, so a construct the edit touched (a block comment,
    /// a multi-line string) is redone whole; outside those regions `previous` is
    /// kept, moved by the edits. Where the regions cover half the document or
    /// more, the whole document is highlighted afresh, which is then cheaper.
    ///
    /// [`LayeredParser::reparse_changed`]: karet_treesitter::LayeredParser::reparse_changed
    pub fn rehighlight(
        &mut self,
        tree: &LayeredTree,
        text: &str,
        previous: &Highlights,
        edits: &[Edit],
        changed: &[Span],
    ) -> Highlights {
        let most = |regions: &[Span]| {
            let covered: usize = regions.iter().map(|s| s.end.0 - s.start.0).sum();
            covered.saturating_mul(2) >= text.len()
        };
        let lines = merge_spans(changed.iter().map(|s| whole_lines(text, *s)).collect());
        if most(&lines) {
            return self.highlight(tree, text);
        }
        let mut wide = lines.clone();
        for range in &lines {
            wide.extend(self.caps(tree, text, Some(*range)).iter().map(Cap::span));
        }
        let regions = merge_spans(wide.into_iter().map(|s| whole_lines(text, s)).collect());
        if most(&regions) {
            return self.highlight(tree, text);
        }

        let mut moved: Vec<HighlightSpan> = previous.all().to_vec();
        for edit in edits {
            for span in &mut moved {
                span.span = shift_span(span.span, edit);
            }
        }
        let mut out: Vec<HighlightSpan> = Vec::with_capacity(moved.len());
        let mut kept = moved.iter().peekable();
        let mut from = 0;
        for region in regions {
            keep(&mut out, &mut kept, from, region.start.0);
            for fresh in flatten(self.caps(tree, text, Some(region))) {
                let start = fresh.span.start.0.max(region.start.0);
                let end = fresh.span.end.0.min(region.end.0);
                if end > start {
                    emit(&mut out, start, end, fresh.token);
                }
            }
            from = region.end.0;
        }
        keep(&mut out, &mut kept, from, text.len());
        Highlights::from_sorted_spans(out)
    }

    /// Every layer's captures over `tree`, or with `range` only the matches
    /// intersecting it, from the layers that reach it.
    fn caps(&mut self, tree: &LayeredTree, text: &str, range: Option<Span>) -> Vec<Cap> {
        let mut caps: Vec<Cap> = Vec::new();
        for (index, layer) in tree.layers().enumerate() {
            if let Some(range) = range {
                let span = layer.span();
                if span.end.0 < range.start.0 || range.end.0 < span.start.0 {
                    continue;
                }
            }
            let lang = layer.language();
            let entry = self
                .by_lang
                .entry(lang)
                .or_insert_with(|| Highlighter::new(lang).ok());
            if let Some(highlighter) = entry {
                // `layers()` is ordered shallowest-first, so the index doubles as depth
                // for tie-breaking.
                let layer_ord = u32::try_from(index).unwrap_or(u32::MAX);
                highlighter.collect(layer, text, (layer_ord, range), &mut caps);
            }
        }
        caps
    }
}

/// `span` widened to the start of its first line and past the newline ending
/// its last, clamped to `text`.
fn whole_lines(text: &str, span: Span) -> Span {
    let bytes = text.as_bytes();
    let end = span.end.0.min(bytes.len());
    let start = span.start.0.min(end);
    let first = bytes
        .get(..start)
        .and_then(|before| before.iter().rposition(|b| *b == b'\n'))
        .map_or(0, |newline| newline + 1);
    let last = bytes
        .get(end..)
        .and_then(|after| after.iter().position(|b| *b == b'\n'))
        .map_or(bytes.len(), |newline| end + newline + 1);
    Span {
        start: BytePos(first),
        end: BytePos(last),
    }
}

/// Emit the parts of the spans `kept` yields that fall in `from..to`, consuming
/// every span that ends by `to`.
fn keep<'a>(
    out: &mut Vec<HighlightSpan>,
    kept: &mut std::iter::Peekable<impl Iterator<Item = &'a HighlightSpan>>,
    from: usize,
    to: usize,
) {
    while let Some(span) = kept.peek() {
        let (start, end) = (span.span.start.0.max(from), span.span.end.0.min(to));
        if end > start {
            emit(out, start, end, span.token);
        }
        if span.span.end.0 > to {
            return;
        }
        kept.next();
    }
}

/// An intermediate capture: a byte range, its token, and the three keys that break an
/// exact same-range tie — producing layer, capture-name specificity, capture index.
struct Cap {
    start: usize,
    end: usize,
    token: TokenId,
    /// Index of the producing layer; deeper (larger) layers are more specific.
    layer: u32,
    /// Dot-segment count of the capture name; see [`specificity`].
    specificity: u8,
    key: u32,
}

impl Cap {
    /// The byte range this capture covers.
    fn span(&self) -> Span {
        Span {
            start: BytePos(self.start),
            end: BytePos(self.end),
        }
    }
}

/// Flatten overlapping captures into ordered, non-overlapping spans where the
/// innermost (smallest) capture wins for its sub-range — so e.g. a `string.escape`
/// paints over the enclosing `string`. Adjacent same-token spans are coalesced.
fn flatten(mut caps: Vec<Cap>) -> Vec<HighlightSpan> {
    caps.retain(|c| c.end > c.start);
    // At equal start, the longer (outer) capture opens first. For an identical range the
    // most specific reading wins, in order: the deeper layer (an injected language's
    // view of a node beats its host's), then the narrower capture name
    // (`comment.documentation` beats `comment`), then the lower capture index as a
    // deterministic last resort.
    caps.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| b.end.cmp(&a.end))
            .then_with(|| b.layer.cmp(&a.layer))
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut out: Vec<HighlightSpan> = Vec::new();
    let mut stack: Vec<Cap> = Vec::new();
    let mut i = 0usize;
    let mut pos = match caps.first() {
        Some(c) => c.start,
        None => return out,
    };

    while i < caps.len() || !stack.is_empty() {
        let next_start = caps.get(i).map(|c| c.start);
        let top_end = stack.last().map(|c| c.end);
        let boundary = match (next_start, top_end) {
            (Some(s), Some(e)) => s.min(e),
            (Some(s), None) => s,
            (None, Some(e)) => e,
            (None, None) => break,
        };

        if boundary > pos {
            if let Some(top) = stack.last() {
                emit(&mut out, pos, boundary, top.token);
            }
            pos = boundary;
        }

        // Close every capture that ends at or before the cursor.
        while stack.last().is_some_and(|c| c.end <= pos) {
            stack.pop();
        }
        // Open every capture starting exactly here. Skip one with the same range as
        // the current innermost (its lower-key sibling already claimed the range).
        while let Some(c) = caps.get(i) {
            if c.start != pos {
                break;
            }
            let dup = stack
                .last()
                .is_some_and(|t| t.start == c.start && t.end == c.end);
            if !dup {
                stack.push(Cap {
                    start: c.start,
                    end: c.end,
                    token: c.token,
                    layer: c.layer,
                    specificity: c.specificity,
                    key: c.key,
                });
            }
            i += 1;
        }
    }

    out
}

/// Append `[start, end)` with `token`, coalescing with an adjacent same-token span.
fn emit(out: &mut Vec<HighlightSpan>, start: usize, end: usize, token: TokenId) {
    if let Some(last) = out.last_mut()
        && last.token == token
        && last.span.end.0 == start
    {
        last.span.end = BytePos(end);
        return;
    }
    out.push(HighlightSpan {
        span: Span {
            start: BytePos(start),
            end: BytePos(end),
        },
        token,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: usize, end: usize, token: TokenId, key: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer: 0,
            specificity: 1,
            key,
        }
    }

    /// A capture from injected layer `layer`.
    fn cap_at(start: usize, end: usize, token: TokenId, layer: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer,
            specificity: 1,
            key: 0,
        }
    }

    /// A capture whose name has `specificity` dot-separated segments.
    fn cap_spec(start: usize, end: usize, token: TokenId, specificity: u8, key: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer: 0,
            specificity,
            key,
        }
    }

    fn spans(caps: Vec<Cap>) -> Vec<(usize, usize, u16)> {
        flatten(caps)
            .into_iter()
            .map(|s| (s.span.start.0, s.span.end.0, s.token.0))
            .collect()
    }

    #[test]
    fn disjoint_captures_preserved() {
        let out = spans(vec![
            cap(0, 3, TokenId::KEYWORD, 0),
            cap(4, 9, TokenId::FUNCTION, 1),
        ]);
        assert_eq!(
            out,
            vec![(0, 3, TokenId::KEYWORD.0), (4, 9, TokenId::FUNCTION.0)]
        );
    }

    #[test]
    fn inner_capture_overrides_outer() {
        // A string [0,10) with an escape [3,5) inside it.
        let out = spans(vec![
            cap(0, 10, TokenId::STRING, 0),
            cap(3, 5, TokenId::new(12), 1), // StringEscape id
        ]);
        assert_eq!(
            out,
            vec![
                (0, 3, TokenId::STRING.0),
                (3, 5, 12),
                (5, 10, TokenId::STRING.0),
            ]
        );
    }

    #[test]
    fn same_range_lower_key_wins() {
        let out = spans(vec![
            cap(0, 4, TokenId::FUNCTION, 1),
            cap(0, 4, TokenId::VARIABLE, 0),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::VARIABLE.0)]);
    }

    #[test]
    fn adjacent_same_token_coalesced() {
        let out = spans(vec![
            cap(0, 3, TokenId::KEYWORD, 0),
            cap(3, 6, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(out, vec![(0, 6, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn zero_length_captures_dropped() {
        assert!(spans(vec![cap(5, 5, TokenId::KEYWORD, 0)]).is_empty());
    }

    #[test]
    fn specificity_counts_dot_segments() {
        assert_eq!(specificity("comment"), 1);
        assert_eq!(specificity("comment.documentation"), 2);
        assert_eq!(specificity("keyword.control.import"), 3);
    }

    #[test]
    fn narrower_capture_name_wins_an_exact_range_tie() {
        // tree-sitter-rust captures `(line_comment) @comment` *before*
        // `(line_comment (doc_comment)) @comment.documentation`, both on the same node.
        // The narrower name must win despite its higher capture index.
        let doc = karet_core::StandardToken::CommentDoc.id();
        let out = spans(vec![
            cap_spec(0, 8, TokenId::COMMENT, 1, 0),
            cap_spec(0, 8, doc, 2, 1),
        ]);
        assert_eq!(out, vec![(0, 8, doc.0)]);
    }

    #[test]
    fn layer_outranks_specificity() {
        // An injected language's plain `keyword` beats the host's narrower name: the
        // deeper layer is the more authoritative reading of that text.
        let out = spans(vec![
            cap_spec(0, 4, TokenId::COMMENT, 3, 0),
            Cap {
                start: 0,
                end: 4,
                token: TokenId::KEYWORD,
                layer: 1,
                specificity: 1,
                key: 9,
            },
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn deeper_layer_wins_an_exact_range_tie() {
        // The host grammar and an injected one both claim [0,4): the injected layer's
        // reading of that text is the more specific one.
        let out = spans(vec![
            cap_at(0, 4, TokenId::STRING, 0),
            cap_at(0, 4, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
        // Order of submission must not matter.
        let out = spans(vec![
            cap_at(0, 4, TokenId::KEYWORD, 1),
            cap_at(0, 4, TokenId::STRING, 0),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn embedded_spans_paint_over_the_hosts_coarse_span() {
        // markdown tags the whole fence `text.literal`; the embedded rust layer tags a
        // keyword inside it. The keyword wins its sub-range, the rest stays raw.
        let raw = karet_core::StandardToken::MarkupRaw.id();
        let out = spans(vec![
            cap_at(0, 20, raw, 0),
            cap_at(5, 7, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(
            out,
            vec![(0, 5, raw.0), (5, 7, TokenId::KEYWORD.0), (7, 20, raw.0)]
        );
    }

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    #[test]
    fn whole_lines_reaches_both_line_ends() {
        let text = "ab\ncd\nef";
        assert_eq!(whole_lines(text, span(4, 4)), span(3, 6));
        assert_eq!(whole_lines(text, span(1, 4)), span(0, 6));
        assert_eq!(whole_lines(text, span(7, 99)), span(6, 8));
        assert_eq!(whole_lines("", span(0, 0)), span(0, 0));
    }

    #[test]
    fn keep_clips_spans_to_the_window_and_holds_one_that_runs_past_it() {
        let spans: Vec<HighlightSpan> = [(0, 4), (6, 12)]
            .into_iter()
            .map(|(s, e)| HighlightSpan {
                span: span(s, e),
                token: TokenId::KEYWORD,
            })
            .collect();
        let mut kept = spans.iter().peekable();
        let mut out = Vec::new();
        keep(&mut out, &mut kept, 2, 8);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].span, out[1].span), (span(2, 4), span(6, 8)));
        // The span running past the window is still there for the next one.
        keep(&mut out, &mut kept, 10, 20);
        assert_eq!(out.get(2).map(|s| s.span), Some(span(10, 12)));
        assert!(kept.next().is_none());
    }

    /// A deterministic pseudo-random stream (an LCG), so a failure replays.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, bound: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or_default() % bound.max(1)
        }
    }

    fn point_of(text: &str, byte: usize) -> karet_core::BytePoint {
        let before = text.get(..byte).unwrap_or("");
        let row = before.matches('\n').count();
        let col = before.rfind('\n').map_or(byte, |i| byte - i - 1);
        karet_core::BytePoint::new(row, col)
    }

    /// Replace `text[start..end]` with `with` in place, returning the edit.
    fn splice(text: &mut String, start: usize, end: usize, with: &str) -> Edit {
        let old = text.clone();
        text.replace_range(start..end, with);
        let new_end = start + with.len();
        Edit {
            start_byte: start,
            old_end_byte: end,
            new_end_byte: new_end,
            start_point: point_of(&old, start),
            old_end_point: point_of(&old, end),
            new_end_point: point_of(text, new_end),
            replaced: old.get(start..end).unwrap_or("").to_owned(),
        }
    }

    /// Edit `source` (ASCII) `steps` times as `lang` — bursts of one to three
    /// random insertions and deletions of syntax-bearing text per reparse — and
    /// require every incremental re-highlight to equal a full highlight of the
    /// same tree. Returns how many steps ran incrementally (did not fall back).
    fn incremental_matches_full(
        lang: &str,
        source: &str,
        seed: u64,
        steps: usize,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        const PIECES: &[&str] = &[
            "x",
            " ",
            "\n",
            "{",
            "}",
            "(",
            ")",
            "\"",
            "'",
            "/*",
            "*/",
            "//",
            "///",
            "#",
            "`",
            "```",
            "```rust\n",
            "*",
            "_",
            "fn f() {}\n",
            "let a = 1;",
            "- item\n",
            "| a |\n",
            "|---|\n",
            "> ",
            "<b>",
            "\\",
        ];
        let lang = karet_treesitter::language_id_from_injection_name(lang)
            .ok_or("grammar not compiled in")?;
        let mut parser = karet_treesitter::LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut text = source.to_owned();
        let mut tree = parser.parse(lang, &text)?;
        let mut previous = highlighter.highlight(&tree, &text);
        let mut rng = Rng(seed);
        let mut incremental = 0;
        for step in 0..steps {
            let mut edits = Vec::new();
            for _ in 0..=rng.below(3) {
                let start = rng.below(text.len() + 1);
                let edit = if rng.below(3) == 0 && start < text.len() {
                    let end = (start + 1 + rng.below(6)).min(text.len());
                    splice(&mut text, start, end, "")
                } else {
                    let piece = PIECES.get(rng.below(PIECES.len())).unwrap_or(&"x");
                    splice(&mut text, start, start, piece)
                };
                edits.push(edit);
            }
            let changed = parser.reparse_changed(&mut tree, &edits, &text)?;
            let full = highlighter.highlight(&tree, &text);
            let fresh = highlighter.rehighlight(&tree, &text, &previous, &edits, &changed);
            assert_eq!(
                fresh.all(),
                full.all(),
                "step {step} (seed {seed}) diverged on {text:?}"
            );
            let covered: usize = changed.iter().map(|s| s.end.0 - s.start.0).sum();
            if covered * 2 < text.len() {
                incremental += 1;
            }
            previous = fresh;
        }
        Ok(incremental)
    }

    const RUST: &str = "//! Crate docs.\n\nuse std::fmt;\n\n/// Adds *one*.\n///\n/// ```\n/// \
                        let y = add(1);\n/// ```\npub fn add(x: i32) -> i32 {\n    x + 1\n}\n\n/* a \
                        block\n   comment */\nconst S: &str = \"multi\nline\";\n\nstruct Point { x: \
                        f64, y: f64 }\n\nimpl fmt::Display for Point {\n    fn fmt(&self, f: &mut \
                        fmt::Formatter) -> fmt::Result {\n        write!(f, \"({}, {})\", self.x, \
                        self.y)\n    }\n}\n\nfn main() {\n    let p = Point { x: 1.0, y: 2.0 };\n    \
                        println!(\"{p}\");\n}\n";

    const MARKDOWN: &str = "# Title\n\nSome *emphasis* and `code` here.\n\n```rust\nfn \
                            main() {\n    let x = 1;\n}\n```\n\n- a list item\n- another \
                            **bold** one\n\n> a quote\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```\nplain \
                            fence\n```\n\nClosing paragraph with a [link](https://example.com).\n";

    #[test]
    fn incremental_rust_highlighting_equals_a_full_highlight()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incremental = 0;
        for seed in 1..=6 {
            incremental += incremental_matches_full("rust", RUST, seed, 60)?;
        }
        assert!(incremental > 0, "some steps must take the incremental path");
        Ok(())
    }

    #[test]
    fn incremental_markdown_highlighting_equals_a_full_highlight()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incremental = 0;
        for seed in 1..=6 {
            incremental += incremental_matches_full("markdown", MARKDOWN, seed, 60)?;
        }
        assert!(incremental > 0, "some steps must take the incremental path");
        Ok(())
    }

    #[test]
    fn incremental_html_highlighting_equals_a_full_highlight()
    -> Result<(), Box<dyn std::error::Error>> {
        let html = "<!doctype html>\n<html>\n<head>\n<style>\nbody { color: red; }\n</style>\n\
                    </head>\n<body>\n<p class=\"x\">Hello <b>world</b></p>\n<script>\nconst a \
                    = 1;\nfunction f() { return a; }\n</script>\n</body>\n</html>\n";
        for seed in 1..=4 {
            incremental_matches_full("html", html, seed, 60)?;
        }
        Ok(())
    }

    #[test]
    fn incremental_highlighting_equals_a_full_highlight_across_grammars()
    -> Result<(), Box<dyn std::error::Error>> {
        let samples: &[(&str, &str, u64)] = &[
            (
                "python",
                "@decorator(\n  a,\n  b)\ndef foo(x):\n    \"\"\"doc\n    string\"\"\"\n    \
                 return Foo(x) + len(x)\n\nclass Bar:\n    CONST = 1\n",
                3,
            ),
            (
                "javascript",
                "/** doc\n * more */\nconst A_B = require('x');\nfunction f(a) {\n  return `t \
                 ${a}\n  more`;\n}\nconsole.log(f(1));\n",
                3,
            ),
            (
                "c",
                "#include <stdio.h>\n#define MAX 10\n/* c\n comment */\nint main(void) {\n  \
                 printf(\"%d\\n\", MAX);\n}\n",
                3,
            ),
            (
                "yaml",
                "a: 1\nb:\n  - x\n  - y\nc: |\n  block\n  text\nd: \"q\"\n",
                3,
            ),
            // Elixir colours `@doc` heredocs by a predicate over the whole
            // multi-line node, so only widening a region to every capture
            // reaching it keeps this grammar exact.
            (
                "elixir",
                "defmodule M do\n  @doc \"\"\"\n  doc\n  \"\"\"\n  def f(_x), do: :ok\nend\n",
                10,
            ),
        ];
        for &(lang, source, seeds) in samples {
            for seed in 1..=seeds {
                incremental_matches_full(lang, source, seed, 80)?;
            }
        }
        Ok(())
    }

    #[test]
    fn opening_a_block_comment_recolours_past_the_edit() -> Result<(), Box<dyn std::error::Error>> {
        let rust = karet_treesitter::language_id_from_injection_name("rust")
            .ok_or("grammar not compiled in")?;
        let mut text = format!(
            "fn a() {{}}\n{}fn z() {{}}\n",
            "fn b() { let v = 1; }\n".repeat(20)
        );
        let mut parser = karet_treesitter::LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut tree = parser.parse(rust, &text)?;
        let previous = highlighter.highlight(&tree, &text);
        let edit = splice(&mut text, 0, 0, "/* ");
        let edits = [edit];
        let changed = parser.reparse_changed(&mut tree, &edits, &text)?;
        let fresh = highlighter.rehighlight(&tree, &text, &previous, &edits, &changed);
        let last = text.rfind("fn z").unwrap_or_default();
        assert_eq!(
            fresh
                .spans_in(span(last, last + 2))
                .first()
                .map(|s| s.token),
            Some(TokenId::COMMENT),
            "the unclosed comment swallows the rest of the file"
        );
        Ok(())
    }
}

//! Soft-wrapping the render model into painted lines.
//!
//! The output is renderer-agnostic: each line is a run of [`TextSpan`]s carrying a
//! semantic [`TokenId`], which a consumer resolves to a color (and bold/italic) through
//! `karet-theme`. Widths are measured in terminal columns, not bytes or `char`s.

use karet_core::StandardToken;
use karet_core::TokenId;
use unicode_width::UnicodeWidthStr;

use crate::Block;
use crate::Inline;
use crate::MarkdownDocument;

/// A styled run of text within a wrapped line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSpan {
    /// The text of the run.
    pub text: String,
    /// The semantic class to color it with, or `None` for default foreground.
    pub token: Option<TokenId>,
}

/// One wrapped, painted line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrappedLine {
    /// The styled runs, left to right.
    pub spans: Vec<TextSpan>,
}

impl WrappedLine {
    /// The line's plain text, with styling discarded.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The line's display width in terminal columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }
}

/// Ties a source line to the wrapped line it produced.
///
/// One anchor is emitted per *top-level* block, at the block's first wrapped line. Lines
/// inside a block (a soft-wrapped paragraph, the body of a code fence) are not anchored
/// individually; [`WrappedDocument`]'s projections interpolate between anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The 0-based line in the markdown source.
    pub source_line: usize,
    /// The 0-based index into [`WrappedDocument::lines`].
    pub wrapped_line: usize,
}

/// A width-wrapped document, ready to be painted line by line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrappedDocument {
    /// The painted lines, top to bottom.
    pub lines: Vec<WrappedLine>,
    /// Source-line anchors, one per top-level block, ascending on both axes.
    pub anchors: Vec<Anchor>,
}

impl WrappedDocument {
    /// The wrapped line that best corresponds to 0-based `source_line`.
    ///
    /// Clamped to the last line; `0` for an empty document.
    #[must_use]
    pub fn wrapped_line_for_source(&self, source_line: usize) -> usize {
        project(
            &self.anchors,
            source_line,
            |a| a.source_line,
            |a| a.wrapped_line,
            self.lines.len().saturating_sub(1),
        )
    }

    /// The 0-based source line that best corresponds to `wrapped_line`.
    ///
    /// Unclamped at the top end — the source's length is not known here, so a caller that
    /// needs a valid line index clamps it against its own buffer.
    #[must_use]
    pub fn source_line_for_wrapped(&self, wrapped_line: usize) -> usize {
        project(
            &self.anchors,
            wrapped_line,
            |a| a.wrapped_line,
            |a| a.source_line,
            usize::MAX,
        )
    }
}

/// Project `input` from one anchor axis onto the other, interpolating proportionally
/// between the two anchors that bracket it, and clamping the result to `limit`.
///
/// Total by construction: an empty `anchors`, or an `input` below the first anchor,
/// projects to `0`; past the last anchor the final block extends one-for-one.
fn project(
    anchors: &[Anchor],
    input: usize,
    from: impl Fn(&Anchor) -> usize,
    to: impl Fn(&Anchor) -> usize,
    limit: usize,
) -> usize {
    let above = anchors.partition_point(|a| from(a) <= input);
    let Some(lo) = above.checked_sub(1).and_then(|i| anchors.get(i)) else {
        return 0;
    };
    let offset = input.saturating_sub(from(lo));
    let projected = match anchors.get(above) {
        // Bracketed by two anchors: scale the offset by the ratio of the two spans. The
        // `max(1)` divisor is unreachable (anchors are strictly ascending on the source
        // axis) but keeps the division total.
        Some(hi) => {
            let span_from = from(hi).saturating_sub(from(lo)).max(1);
            let span_to = to(hi).saturating_sub(to(lo));
            to(lo).saturating_add(offset.saturating_mul(span_to) / span_from)
        },
        // Past the last anchor: no `hi` to scale against, so extend one-for-one.
        None => to(lo).saturating_add(offset),
    };
    projected.min(limit)
}

/// The bullet used for unordered list items.
const BULLET: &str = "• ";
/// The rule drawn for a thematic break.
const RULE: char = '─';
/// The gutter drawn to the left of a block quote.
const QUOTE_GUTTER: &str = "▌ ";

/// Wrap `doc` to `width` terminal columns.
pub(crate) fn wrap(doc: &MarkdownDocument, width: u16) -> WrappedDocument {
    // A zero-width viewport would make every wrap loop spin; one column always
    // terminates, and the caller sees (unhelpful but finite) output.
    let width = usize::from(width).max(1);
    let mut lines = Vec::new();
    let mut anchors = Vec::new();
    // The top level is wrapped here rather than through `wrap_blocks` so each block can
    // be anchored to its source line. The anchor is stamped *after* the separator, so it
    // points at the block's first real line rather than the blank one above it.
    for (index, block) in doc.blocks.iter().enumerate() {
        if index > 0 {
            lines.push(WrappedLine::default());
        }
        if let Some(source_line) = doc.block_line(index) {
            anchors.push(Anchor {
                source_line,
                wrapped_line: lines.len(),
            });
        }
        wrap_block(block, width, &[], &mut lines);
    }
    // A trailing blank line is an artifact of the between-blocks separator.
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    WrappedDocument { lines, anchors }
}

/// Wrap a sequence of blocks, each prefixed by `prefix` (a quote gutter, a list
/// indent), separated by a blank line.
fn wrap_blocks(blocks: &[Block], width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            out.push(prefixed_line(prefix, Vec::new()));
        }
        wrap_block(block, width, prefix, out);
    }
}

fn wrap_block(block: &Block, width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    let indent = prefix_width(prefix);
    let inner = width.saturating_sub(indent).max(1);

    match block {
        Block::Heading { level, content } => {
            let marker = TextSpan {
                text: format!("{} ", "#".repeat(usize::from(*level).min(6))),
                token: Some(StandardToken::MarkupHeading.id()),
            };
            let mut runs = vec![marker];
            flatten(content, Some(StandardToken::MarkupHeading.id()), &mut runs);
            wrap_runs(&runs, inner, prefix, out);
        },
        Block::Paragraph(content) => {
            let mut runs = Vec::new();
            flatten(content, None, &mut runs);
            wrap_runs(&runs, inner, prefix, out);
        },
        Block::CodeBlock { lang, code } => {
            for line in code_lines(lang.as_deref(), code) {
                out.push(prefixed_line(prefix, line));
            }
        },
        Block::List(items) => {
            for item in items {
                // The bullet occupies the first line; continuation lines align under it.
                let mut bullet = prefix.to_vec();
                bullet.push(TextSpan {
                    text: BULLET.to_owned(),
                    token: Some(StandardToken::MarkupListMarker.id()),
                });
                let mut continuation = prefix.to_vec();
                continuation.push(TextSpan {
                    text: " ".repeat(BULLET.width()),
                    token: None,
                });

                let start = out.len();
                wrap_blocks(item, width, &continuation, out);
                // Swap the continuation indent on the item's first line for the bullet.
                if let Some(first) = out.get_mut(start) {
                    replace_prefix(first, prefix.len(), &bullet);
                }
            }
        },
        Block::Quote(blocks) => {
            let mut gutter = prefix.to_vec();
            gutter.push(TextSpan {
                text: QUOTE_GUTTER.to_owned(),
                token: Some(StandardToken::MarkupQuote.id()),
            });
            wrap_blocks(blocks, width, &gutter, out);
        },
        Block::Rule => out.push(prefixed_line(
            prefix,
            vec![TextSpan {
                text: RULE.to_string().repeat(inner),
                token: Some(StandardToken::MarkupListMarker.id()),
            }],
        )),
    }
}

/// Replace the first `old_len` spans of `line` with `new`.
fn replace_prefix(line: &mut WrappedLine, old_len: usize, new: &[TextSpan]) {
    if line.spans.len() < old_len + 1 {
        return;
    }
    line.spans.splice(..=old_len, new.iter().cloned());
}

/// The display width of a prefix.
fn prefix_width(prefix: &[TextSpan]) -> usize {
    prefix.iter().map(|s| s.text.width()).sum()
}

/// A line consisting of `prefix` followed by `spans`.
fn prefixed_line(prefix: &[TextSpan], spans: Vec<TextSpan>) -> WrappedLine {
    let mut all = prefix.to_vec();
    all.extend(spans);
    WrappedLine { spans: all }
}

/// Flatten inlines into styled runs, inheriting `token` where an inline sets none.
fn flatten(inlines: &[Inline], token: Option<TokenId>, out: &mut Vec<TextSpan>) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push(TextSpan {
                text: text.clone(),
                token,
            }),
            Inline::Code(text) => out.push(TextSpan {
                text: text.clone(),
                token: Some(StandardToken::MarkupRaw.id()),
            }),
            // Emphasis inside a heading stays a heading: the outer token wins, because a
            // theme colors headings as a unit.
            Inline::Emphasis(children) => flatten(
                children,
                token.or(Some(StandardToken::MarkupItalic.id())),
                out,
            ),
            Inline::Strong(children) => {
                flatten(
                    children,
                    token.or(Some(StandardToken::MarkupBold.id())),
                    out,
                );
            },
            Inline::Link { text, .. } => out.push(TextSpan {
                text: text.clone(),
                token: Some(StandardToken::MarkupLink.id()),
            }),
        }
    }
}

/// Greedily wrap `runs` to `width` columns, preserving each run's token.
///
/// Breaks at whitespace; a single word longer than the line is emitted whole and allowed
/// to overflow rather than being cut mid-grapheme. An embedded `\n` (a hard break) ends
/// the line.
fn wrap_runs(runs: &[TextSpan], width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    let mut line: Vec<TextSpan> = Vec::new();
    let mut used = 0usize;

    // The space that separated the last word from the one pushed to the next line must
    // not linger at the end of this one.
    let flush = |line: &mut Vec<TextSpan>, used: &mut usize, out: &mut Vec<WrappedLine>| {
        trim_trailing_space(line);
        out.push(prefixed_line(prefix, std::mem::take(line)));
        *used = 0;
    };

    for run in runs {
        for (index, segment) in run.text.split('\n').enumerate() {
            if index > 0 {
                flush(&mut line, &mut used, out); // a hard break
            }
            for word in words(segment) {
                let w = word.width();
                let is_space = word.chars().all(char::is_whitespace);
                // Never start a line with the space that separated two words.
                if used == 0 && is_space {
                    continue;
                }
                if used > 0 && used + w > width && !is_space {
                    flush(&mut line, &mut used, out);
                }
                // Re-check: a leading space can appear after the flush above.
                if used == 0 && is_space {
                    continue;
                }
                push_word(&mut line, word, run.token);
                used += w;
            }
        }
    }
    if !line.is_empty() || out.is_empty() {
        flush(&mut line, &mut used, out);
    }
}

/// Drop trailing whitespace from a completed line, discarding runs it empties.
fn trim_trailing_space(line: &mut Vec<TextSpan>) {
    while let Some(last) = line.last_mut() {
        let trimmed = last.text.trim_end();
        if trimmed.len() == last.text.len() {
            return; // nothing to trim
        }
        last.text.truncate(trimmed.len());
        if !last.text.is_empty() {
            return;
        }
        line.pop();
    }
}

/// Append `word` to `line`, coalescing with a preceding run of the same token.
fn push_word(line: &mut Vec<TextSpan>, word: &str, token: Option<TokenId>) {
    match line.last_mut() {
        Some(last) if last.token == token => last.text.push_str(word),
        _ => line.push(TextSpan {
            text: word.to_owned(),
            token,
        }),
    }
}

/// Split `text` into alternating word and whitespace chunks, preserving both.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (index, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if in_space.is_some_and(|s| s != space) {
            out.push(&text[start..index]);
            start = index;
        }
        in_space = Some(space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The painted lines of a code block: highlighted when a grammar is available for
/// `lang`, otherwise a single raw-markup run per line.
fn code_lines(lang: Option<&str>, code: &str) -> Vec<Vec<TextSpan>> {
    #[cfg(feature = "highlight")]
    if let Some(lines) = crate::highlight::code_lines(lang, code) {
        return lines;
    }
    let _ = lang;
    code.lines()
        .map(|line| {
            vec![TextSpan {
                text: line.to_owned(),
                token: Some(StandardToken::MarkupRaw.id()),
            }]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn lines(source: &str, width: u16) -> Vec<String> {
        wrap(&parse::parse(source), width)
            .lines
            .iter()
            .map(WrappedLine::text)
            .collect()
    }

    #[test]
    fn words_preserves_whitespace_chunks() {
        assert_eq!(words("ab  cd"), vec!["ab", "  ", "cd"]);
        assert_eq!(words(""), Vec::<&str>::new());
        assert_eq!(words(" a"), vec![" ", "a"]);
    }

    #[test]
    fn paragraph_wraps_at_word_boundaries() {
        assert_eq!(lines("alpha beta gamma\n", 11), vec!["alpha beta", "gamma"]);
    }

    #[test]
    fn a_word_longer_than_the_line_is_not_split() {
        // Overflowing beats cutting a word (or a grapheme) in half.
        assert_eq!(lines("abcdefgh ij\n", 4), vec!["abcdefgh", "ij"]);
    }

    #[test]
    fn zero_width_terminates() {
        // A degenerate viewport must not spin; it just produces narrow output.
        assert!(!lines("a b c\n", 0).is_empty());
    }

    #[test]
    fn heading_carries_its_marker_and_token() {
        let doc = wrap(&parse::parse("## Title\n"), 40);
        let Some(first) = doc.lines.first() else {
            return;
        };
        assert_eq!(first.text(), "## Title");
        assert!(
            first
                .spans
                .iter()
                .all(|s| s.token == Some(StandardToken::MarkupHeading.id()))
        );
    }

    #[test]
    fn emphasis_and_code_get_their_own_tokens() {
        let doc = wrap(&parse::parse("a *b* `c`\n"), 40);
        let spans: Vec<_> = doc.lines.iter().flat_map(|l| l.spans.iter()).collect();
        assert!(
            spans
                .iter()
                .any(|s| s.token == Some(StandardToken::MarkupItalic.id()) && s.text == "b")
        );
        assert!(
            spans
                .iter()
                .any(|s| s.token == Some(StandardToken::MarkupRaw.id()) && s.text == "c")
        );
    }

    #[test]
    fn list_bullets_the_first_line_and_indents_the_rest() {
        let out = lines("- alpha beta gamma\n", 10);
        assert_eq!(out.first().map(String::as_str), Some("• alpha"));
        // Continuation lines align under the bullet's text, not its marker.
        assert_eq!(out.get(1).map(String::as_str), Some("  beta"));
    }

    #[test]
    fn quote_prefixes_every_line_with_a_gutter() {
        let out = lines("> alpha beta\n", 20);
        assert!(out.iter().all(|l| l.starts_with(QUOTE_GUTTER)));
    }

    #[test]
    fn rule_fills_the_width() {
        let out = lines("---\n", 5);
        assert_eq!(out.first().map(String::as_str), Some("─────"));
    }

    #[test]
    fn code_block_lines_are_raw_markup_without_a_grammar() {
        let doc = wrap(&parse::parse("```\nlet x;\n```\n"), 40);
        let Some(first) = doc.lines.first() else {
            return;
        };
        assert_eq!(first.text(), "let x;");
        assert_eq!(
            first.spans.first().and_then(|s| s.token),
            Some(StandardToken::MarkupRaw.id())
        );
    }

    #[test]
    fn width_is_measured_in_terminal_columns() {
        // A CJK glyph is two columns wide, so only one fits in a width of 3.
        let out = lines("世 界\n", 3);
        assert_eq!(out.len(), 2, "got {out:?}");
    }

    #[test]
    fn blocks_are_separated_by_a_blank_line_with_no_trailing_blank() {
        let out = lines("a\n\nb\n", 20);
        assert_eq!(out, vec!["a".to_owned(), String::new(), "b".to_owned()]);
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn a_rust_fence_is_syntax_highlighted_end_to_end() {
        use karet_core::TokenId;

        let doc = wrap(&parse::parse("```rust\nfn main() {}\n```\n"), 40);
        let tokens: Vec<_> = doc
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter_map(|s| s.token)
            .collect();
        if tokens.is_empty() {
            return; // no rust grammar compiled into this build
        }
        // `fn` is a keyword, not undifferentiated raw markup.
        assert!(
            tokens.contains(&TokenId::KEYWORD),
            "the fence should be highlighted as rust, got {tokens:?}"
        );
    }

    #[cfg(feature = "highlight")]
    #[test]
    fn an_unknown_fence_language_falls_back_to_raw_markup() {
        let doc = wrap(&parse::parse("```brainfuck\n+++.\n```\n"), 40);
        let Some(first) = doc.lines.first() else {
            return;
        };
        assert_eq!(first.text(), "+++.");
        assert_eq!(
            first.spans.first().and_then(|s| s.token),
            Some(StandardToken::MarkupRaw.id())
        );
    }

    fn wrapped(source: &str, width: u16) -> WrappedDocument {
        wrap(&parse::parse(source), width)
    }

    #[test]
    fn one_anchor_per_top_level_block_at_its_first_line() {
        // "# Title" / "" / "Some text." — the anchor skips the separator blank.
        let doc = wrapped("# Title\n\nSome text.\n", 40);
        assert_eq!(
            doc.anchors,
            vec![
                Anchor {
                    source_line: 0,
                    wrapped_line: 0
                },
                Anchor {
                    source_line: 2,
                    wrapped_line: 2
                },
            ]
        );
    }

    #[test]
    fn nested_blocks_do_not_add_anchors() {
        let doc = wrapped("- one\n  - two\n\n> quoted\n", 40);
        assert_eq!(
            doc.anchors.len(),
            2,
            "the list and the quote, nothing inside"
        );
    }

    #[test]
    fn anchors_ascend_on_both_axes() {
        let doc = wrapped("a\n\n# b\n\n---\n\n> c\n\n- d\n", 20);
        assert!(
            doc.anchors
                .windows(2)
                .all(|w| w[0].source_line < w[1].source_line
                    && w[0].wrapped_line < w[1].wrapped_line),
            "{:?}",
            doc.anchors
        );
    }

    #[test]
    fn projections_hit_anchors_exactly() {
        let doc = wrapped("# Title\n\nSome text.\n", 40);
        for anchor in &doc.anchors {
            assert_eq!(
                doc.wrapped_line_for_source(anchor.source_line),
                anchor.wrapped_line
            );
            assert_eq!(
                doc.source_line_for_wrapped(anchor.wrapped_line),
                anchor.source_line
            );
        }
    }

    #[test]
    fn an_empty_document_projects_everything_to_the_top() {
        let doc = wrapped("", 40);
        assert!(doc.anchors.is_empty());
        assert_eq!(doc.wrapped_line_for_source(7), 0);
        assert_eq!(doc.source_line_for_wrapped(7), 0);
    }

    #[test]
    fn a_single_anchor_extends_one_for_one() {
        // One block, so there is no `hi` to interpolate against.
        let doc = wrapped("alpha\nbravo\ncharlie\n", 40);
        assert_eq!(doc.anchors.len(), 1);
        // Source lines beyond the block still map forward, clamped to the last line.
        assert_eq!(doc.wrapped_line_for_source(0), 0);
        assert_eq!(doc.source_line_for_wrapped(2), 2);
    }

    #[test]
    fn a_source_line_below_the_first_anchor_maps_to_the_top() {
        let doc = wrapped("\n\n# Late\n", 40);
        assert_eq!(doc.anchors.first().map(|a| a.source_line), Some(2));
        assert_eq!(doc.wrapped_line_for_source(0), 0);
        assert_eq!(doc.wrapped_line_for_source(1), 0);
    }

    #[test]
    fn a_source_line_past_the_end_clamps_to_the_last_wrapped_line() {
        let doc = wrapped("# Title\n\nSome text.\n", 40);
        let last = doc.lines.len().saturating_sub(1);
        assert_eq!(doc.wrapped_line_for_source(9_999), last);
    }

    #[test]
    fn a_wrapped_line_past_the_end_is_not_clamped() {
        // The source's length is unknown here, so the caller clamps; we just extend.
        let doc = wrapped("# Title\n\nSome text.\n", 40);
        assert!(doc.source_line_for_wrapped(9_999) > 2);
    }

    #[test]
    fn interpolation_lands_strictly_inside_the_block_that_owns_the_line() {
        // A paragraph on source lines 2-3 that soft-wraps into several rendered lines,
        // bracketed by headings, so both anchors exist.
        let doc = wrapped(
            "# H\n\nlorem ipsum dolor\nsit amet consectetur\n\n## Tail\n",
            12,
        );
        let para = doc.wrapped_line_for_source(2);
        let tail = doc.wrapped_line_for_source(5);
        let inner = doc.wrapped_line_for_source(3);
        assert!(
            para < inner && inner < tail,
            "source line 3 must render between the paragraph start and the tail heading: \
             {para} < {inner} < {tail}"
        );
        // And back: the interpolated row belongs to the paragraph, not the tail heading.
        let back = doc.source_line_for_wrapped(inner);
        assert!((2..5).contains(&back), "expected 2..5, got {back}");
    }

    #[test]
    fn round_tripping_a_source_line_stays_within_its_block() {
        let doc = wrapped(
            "# H\n\nlorem ipsum dolor\nsit amet consectetur\n\n## Tail\n",
            12,
        );
        for source_line in 0..6 {
            let back = doc.source_line_for_wrapped(doc.wrapped_line_for_source(source_line));
            assert!(
                back <= source_line.max(2),
                "{source_line} round-tripped to {back}"
            );
        }
    }

    #[test]
    fn a_zero_width_wrap_still_projects_without_panicking() {
        let doc = wrapped("# H\n\ntext\n", 0);
        let _ = doc.wrapped_line_for_source(usize::MAX);
        let _ = doc.source_line_for_wrapped(usize::MAX);
    }
}

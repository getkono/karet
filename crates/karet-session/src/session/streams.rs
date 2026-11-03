//! The session's two outbound streams: [`Event`]s and render-only document
//! snapshots. Every module answers through these.

use super::*;

impl Session {
    pub(super) fn emit(&self, id: Option<RequestId>, event: Event) {
        self.events.emit(id, event);
    }

    /// Push a render-only snapshot of `doc_id` on the snapshot stream. `cursor` is
    /// `Some` only for undo/redo, carrying the caret the editor should jump to; every
    /// other publish passes `None` and leaves the UI's cursor untouched.
    pub(super) fn publish(&self, doc_id: DocumentId, cursor: Option<CursorState>) {
        if let Some(doc) = self.store.docs.get(&doc_id) {
            let snapshot = Arc::new(DocSnapshot {
                version: doc.buffer.version(),
                buffer: doc.buffer.content_snapshot(),
                highlights: self.viewport_highlights(doc_id, doc),
                folds: doc.display_folds(),
                semantic_blocks: doc.syntax.semantic_blocks().clone(),
                syntax_error_lines: doc.syntax.error_lines().clone(),
                table_lines: doc.syntax.table_lines().clone(),
                language: doc.language,
                dirty: doc.buffer.is_dirty(),
                cursor,
            });
            self.snapshots.send((doc_id, snapshot, 0)).ok();
        }
    }

    /// The highlight spans a snapshot of `doc` should carry.
    ///
    /// A view paints highlights per rendered line, so it needs spans only for
    /// the lines it displays — its viewport, and the sticky-scroll headers of the
    /// blocks it is inside, which can sit any distance above it — plus, within
    /// bracket matching's reach, the string and comment spans that hide a
    /// bracket. When a client
    /// has declared a viewport, narrowing to those lines is what keeps the
    /// answer to a keystroke about a screenful rather than a whole file's worth
    /// of spans — the difference between a usable and an unusable connection
    /// once the two are not on the same machine.
    ///
    /// The narrowed slice keeps its `Arc` while its spans are unchanged, so a
    /// republish for anything else (a save, a diagnostic, a scroll inside the
    /// window) does not look like new highlights to a connection that compares
    /// pointers.
    ///
    /// With no viewport declared the whole set is shared as-is, at no cost:
    /// that is the local default, and the `Arc` clone is free.
    fn viewport_highlights(&self, doc_id: DocumentId, doc: &Document) -> Arc<Highlights> {
        let highlights = doc.display_highlights();
        let mut windows = self
            .highlight_windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(window) = windows.get_mut(&doc_id) else {
            return highlights;
        };
        let spans = narrow_highlights(
            &highlights,
            &doc.buffer,
            window.lines,
            doc.syntax.semantic_blocks(),
        );
        if let Some(previous) = &window.narrowed
            && previous.all() == spans.as_slice()
        {
            return Arc::clone(previous);
        }
        let narrowed = Arc::new(Highlights::from_sorted_spans(spans));
        window.narrowed = Some(Arc::clone(&narrowed));
        narrowed
    }
}

/// The spans of `highlights` a view of `first..=last` of `buffer` can read: those
/// touching the window, and those on the header lines of every block in
/// `blocks` that opens above the window and is still open inside it, since
/// sticky scroll paints those headers however far above the window they are.
fn narrow_highlights(
    highlights: &Highlights,
    buffer: &karet_text::TextBuffer,
    (first, last): (u32, u32),
    blocks: &karet_core::SemanticBlocks,
) -> Vec<karet_core::HighlightSpan> {
    let last_line = u32::try_from(buffer.line_count().saturating_sub(1)).unwrap_or(u32::MAX);
    // A viewport can outrun the buffer when an edit shortened the document
    // before the client's next scroll; clamping is what keeps that a
    // no-highlight frame instead of an empty one.
    let lines = |from: u32, to: u32| {
        let start = buffer.line_to_byte_range(from.min(last_line) as usize)?;
        let end = buffer.line_to_byte_range(to.min(last_line) as usize)?;
        Some(karet_core::Span::new(start.start, end.end))
    };
    let Some(window) = lines(first, last) else {
        return highlights.all().to_vec();
    };
    let mut spans: Vec<karet_core::HighlightSpan> = blocks
        .blocks()
        .iter()
        .filter(|block| block.header_start < first && block.scope_end >= first)
        .filter_map(|block| lines(block.header_start, block.header_end.min(first)))
        .flat_map(|header| highlights.spans_in(header).iter().copied())
        .collect();
    // The editor's bracket matcher and depth colouring scan up to
    // `BRACKET_REACH` lines past the lines on screen, and read the spans there
    // to tell a bracket in a string or comment from one in code. Only the
    // spans that hide a bracket change that answer, so only those travel.
    if let Some(reach) = lines(
        first.saturating_sub(BRACKET_REACH),
        last.saturating_add(BRACKET_REACH),
    ) {
        spans.extend(
            highlights
                .spans_in(reach)
                .iter()
                .filter(|span| {
                    (span.span.end <= window.start || span.span.start >= window.end)
                        && hides_brackets(span.token)
                        && buffer
                            .slice(span.span)
                            .is_some_and(|text| text.contains(['(', ')', '[', ']', '{', '}']))
                })
                .copied(),
        );
    }
    if spans.is_empty() {
        return highlights.spans_in(window).to_vec();
    }
    spans.extend_from_slice(highlights.spans_in(window));
    // Headers nest and can touch the window, so one span can be read twice.
    spans.sort_by_key(|span| (span.span.start, span.span.end));
    spans.dedup();
    spans
}

/// How many lines past a view the editor's bracket matching reads highlights
/// (`karet_editor`'s `SCAN_LINES`).
const BRACKET_REACH: u32 = 10_000;

/// Whether `token` marks text whose brackets are not code, as the editor's
/// bracket matching reads it: strings and comments.
fn hides_brackets(token: karet_core::TokenId) -> bool {
    use karet_core::StandardToken;

    [
        StandardToken::String,
        StandardToken::StringEscape,
        StandardToken::Comment,
        StandardToken::CommentDoc,
        StandardToken::CommentMark,
    ]
    .iter()
    .any(|hiding| hiding.id() == token)
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;
    use karet_core::HighlightSpan;
    use karet_core::Span;
    use karet_core::TokenId;

    use super::*;

    /// One keyword span per line of a `lines`-line buffer whose lines are each
    /// `"fn x\n"` (five bytes).
    fn one_span_per_line(lines: usize) -> (karet_text::TextBuffer, Arc<Highlights>) {
        let buffer = karet_text::TextBuffer::from_text(&"fn x\n".repeat(lines));
        let spans = (0..lines)
            .map(|line| HighlightSpan {
                span: Span::new(BytePos(line * 5), BytePos(line * 5 + 2)),
                token: TokenId::KEYWORD,
            })
            .collect();
        (buffer, Arc::new(Highlights::from_sorted_spans(spans)))
    }

    fn lines_of(spans: &[HighlightSpan]) -> Vec<usize> {
        spans.iter().map(|span| span.span.start.0 / 5).collect()
    }

    fn no_blocks() -> karet_core::SemanticBlocks {
        karet_core::SemanticBlocks::new(Vec::new())
    }

    /// The headline property of a viewport: only spans on the declared lines
    /// travel, so a keystroke's answer is a screenful rather than a file.
    #[test]
    fn narrowing_keeps_only_the_spans_on_the_declared_lines() {
        let (buffer, highlights) = one_span_per_line(1_000);

        let narrowed = narrow_highlights(&highlights, &buffer, (400, 402), &no_blocks());

        assert_eq!(lines_of(&narrowed), vec![400, 401, 402]);
    }

    /// A viewport past the end of a document an edit just shortened clamps to
    /// its last line rather than dropping every span.
    #[test]
    fn a_viewport_past_the_end_clamps_to_the_last_line() {
        let (buffer, highlights) = one_span_per_line(10);

        let narrowed = narrow_highlights(&highlights, &buffer, (8, 5_000), &no_blocks());

        assert_eq!(lines_of(&narrowed), vec![8, 9]);
    }

    /// Sticky scroll paints the header of every block the view is inside, however
    /// far above the window it opened; its spans travel with the window, while a
    /// block that closed before the window contributes nothing.
    #[test]
    fn narrowing_keeps_the_headers_of_blocks_open_into_the_window() {
        let (buffer, highlights) = one_span_per_line(1_000);
        let blocks = karet_core::SemanticBlocks::new(vec![
            karet_core::SemanticBlock {
                header_start: 10,
                header_end: 11,
                scope_end: 900,
            },
            karet_core::SemanticBlock {
                header_start: 20,
                header_end: 20,
                scope_end: 30,
            },
        ]);

        let narrowed = narrow_highlights(&highlights, &buffer, (400, 402), &blocks);

        assert_eq!(lines_of(&narrowed), vec![10, 11, 400, 401, 402]);
    }

    /// Bracket matching reads, up to `BRACKET_REACH` lines past the view,
    /// whether a bracket sits in a string or comment. A comment hiding a
    /// bracket within that reach travels with the window; one hiding none, and
    /// one beyond the reach, do not.
    #[test]
    fn narrowing_keeps_the_spans_that_hide_brackets_within_reach() {
        let beyond = 402 + BRACKET_REACH as usize + 50;
        let text: String = (0..beyond + 1)
            .map(|line| match line {
                5 => "// (\n",
                6 => "// x\n",
                l if l == beyond => "// )\n",
                _ => "fn x\n",
            })
            .collect();
        let buffer = karet_text::TextBuffer::from_text(&text);
        let comment = |line: usize| HighlightSpan {
            span: Span::new(BytePos(line * 5), BytePos(line * 5 + 4)),
            token: karet_core::StandardToken::Comment.id(),
        };
        let keyword = |line: usize| HighlightSpan {
            span: Span::new(BytePos(line * 5), BytePos(line * 5 + 2)),
            token: TokenId::KEYWORD,
        };
        let highlights = Highlights::from_sorted_spans(vec![
            comment(5),
            comment(6),
            keyword(400),
            keyword(401),
            keyword(402),
            comment(beyond),
        ]);

        let narrowed = narrow_highlights(&highlights, &buffer, (400, 402), &no_blocks());

        assert_eq!(lines_of(&narrowed), vec![5, 400, 401, 402]);
    }
}

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
            self.snapshots.send((doc_id, snapshot)).ok();
        }
    }

    /// The highlight spans a snapshot of `doc` should carry.
    ///
    /// A view resolves highlights per rendered line, so spans outside what a
    /// client is displaying are never read. When a client has declared a
    /// viewport, narrowing to it is what keeps the answer to a keystroke about a
    /// screenful rather than a whole file's worth of spans — the difference
    /// between a usable and an unusable connection once the two are not on the
    /// same machine.
    ///
    /// With no viewport declared the whole set is shared as-is, at no cost:
    /// that is the local default, and the `Arc` clone is free.
    fn viewport_highlights(&self, doc_id: DocumentId, doc: &Document) -> Arc<Highlights> {
        let highlights = doc.display_highlights();
        let Some(lines) = self.viewport_lines(doc_id) else {
            return highlights;
        };
        narrow_highlights(highlights, &doc.buffer, lines)
    }
}

/// `highlights` cut down to the spans that touch `first..=last` of `buffer`.
fn narrow_highlights(
    highlights: Arc<Highlights>,
    buffer: &karet_text::TextBuffer,
    (first, last): (u32, u32),
) -> Arc<Highlights> {
    let last_line = u32::try_from(buffer.line_count().saturating_sub(1)).unwrap_or(u32::MAX);
    // A viewport can outrun the buffer when an edit shortened the document
    // before the client's next scroll; clamping is what keeps that a
    // no-highlight frame instead of an empty one.
    let Some(start) = buffer.line_to_byte_range(first.min(last_line) as usize) else {
        return highlights;
    };
    let Some(end) = buffer.line_to_byte_range(last.min(last_line) as usize) else {
        return highlights;
    };
    let span = karet_core::Span::new(start.start, end.end);
    Arc::new(Highlights::from_sorted_spans(
        highlights.spans_in(span).to_vec(),
    ))
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

    fn lines_of(highlights: &Highlights) -> Vec<usize> {
        highlights
            .all()
            .iter()
            .map(|span| span.span.start.0 / 5)
            .collect()
    }

    /// The headline property of a viewport: only spans on the declared lines
    /// travel, so a keystroke's answer is a screenful rather than a file.
    #[test]
    fn narrowing_keeps_only_the_spans_on_the_declared_lines() {
        let (buffer, highlights) = one_span_per_line(1_000);

        let narrowed = narrow_highlights(highlights, &buffer, (400, 402));

        assert_eq!(lines_of(&narrowed), vec![400, 401, 402]);
    }

    /// A viewport past the end of a document an edit just shortened clamps to
    /// its last line rather than dropping every span.
    #[test]
    fn a_viewport_past_the_end_clamps_to_the_last_line() {
        let (buffer, highlights) = one_span_per_line(10);

        let narrowed = narrow_highlights(highlights, &buffer, (8, 5_000));

        assert_eq!(lines_of(&narrowed), vec![8, 9]);
    }
}

//! The semantic-token overlay on each document: adopting a server's answer,
//! keeping it aligned across edits, dropping it when its server goes, and
//! laying it over the tree-sitter highlights a snapshot carries.
//!
//! A document's own [`highlights`](Document::highlights) stay tree-sitter's
//! alone -- spell checking and every other reader of the syntax classes sees
//! exactly what it saw before -- and the overlay is applied only where a
//! snapshot is published for display (see [`crate::highlight::overlay_semantic`]).

use karet_core::HighlightSpan;
use karet_core::Span;

use super::*;
use crate::lsp::SlotKey;

/// One document's semantic tokens, as byte spans in the document's text.
pub(super) struct SemanticLayer {
    /// The slot whose server answered; its retirement takes the layer.
    server: SlotKey,
    /// The buffer version `spans` describe.
    version: u64,
    /// Ordered, non-overlapping semantic spans.
    spans: Arc<Highlights>,
}

impl Document {
    /// Keep the overlay aligned with the text across `edits`, as the syntax
    /// highlights are until the worker answers. A version change with no
    /// edits to map through -- a reload replacing the text -- drops it
    /// instead: its spans describe text that is gone.
    pub(super) fn translate_semantic_tokens(&mut self, edits: Option<&[AppliedEdit]>) {
        let version = self.buffer.version();
        let Some(layer) = self.semantic_tokens.as_mut() else {
            return;
        };
        match edits {
            Some(edits) => {
                for edit in edits {
                    layer.spans = Arc::new(layer.spans.translate(
                        BytePos(edit.start_byte),
                        BytePos(edit.old_end_byte),
                        BytePos(edit.new_end_byte),
                    ));
                }
                layer.version = version;
            },
            None if layer.version != version => self.semantic_tokens = None,
            None => {},
        }
    }

    /// The highlights a snapshot paints: the syntax highlights with the
    /// semantic overlay laid over them, when there is one.
    pub(super) fn display_highlights(&self) -> Arc<Highlights> {
        match &self.semantic_tokens {
            Some(layer) => Arc::new(crate::highlight::overlay_semantic(
                &self.highlights,
                &layer.spans,
            )),
            None => Arc::clone(&self.highlights),
        }
    }
}

impl Session {
    /// Adopt an [`LspUpdate::SemanticTokens`]: the server's tokens for a
    /// document at an LSP version, replacing the document's overlay and
    /// republishing it.
    ///
    /// Dropped when the document has moved past that version (the next one
    /// is asked for as soon as the server has it), and when semantic
    /// highlighting has been turned off since the request went out.
    pub(super) fn adopt_semantic_tokens(&mut self, update: LspUpdate) {
        let LspUpdate::SemanticTokens {
            server,
            path,
            version,
            tokens,
            ..
        } = update
        else {
            return;
        };
        if !self.config.settings.editor.semantic_highlighting.enabled {
            return;
        }
        let Some(&doc_id) = self.store.by_path.get(&path) else {
            return;
        };
        let Some(document) = self.store.docs.get_mut(&doc_id) else {
            return;
        };
        if crate::lsp::version_i32(document.buffer.version()) != version {
            return;
        }
        let spans = byte_spans(&document.buffer, &tokens);
        document.semantic_tokens = Some(SemanticLayer {
            server,
            version: document.buffer.version(),
            spans: Arc::new(spans),
        });
        self.publish(doc_id, None);
    }

    /// Drop every overlay `server` laid, republishing the documents that
    /// carried one: the slot was retired, or its connection is gone for good.
    pub(super) fn clear_semantic_tokens_of(&mut self, server: &SlotKey) {
        self.clear_semantic_tokens(|layer| layer.server == *server);
    }

    /// Apply `editor.semanticHighlighting.enabled` live: the owning servers
    /// start or stop fetching, and turning it off takes every overlay down.
    pub(super) fn set_semantic_highlighting(&mut self, enabled: bool) {
        self.lsp.set_semantic_highlighting(enabled);
        if !enabled {
            self.clear_semantic_tokens(|_| true);
        }
    }

    fn clear_semantic_tokens(&mut self, matching: impl Fn(&SemanticLayer) -> bool) {
        let affected: Vec<DocumentId> = self
            .store
            .docs
            .iter_mut()
            .filter(|(_, document)| document.semantic_tokens.as_ref().is_some_and(&matching))
            .map(|(&doc_id, document)| {
                document.semantic_tokens = None;
                doc_id
            })
            .collect();
        for doc_id in affected {
            self.publish(doc_id, None);
        }
    }
}

/// `tokens` (UTF-16 columns) as byte spans of `buffer`, in order and not
/// overlapping; a token that does not land inside the text is dropped.
fn byte_spans(buffer: &TextBuffer, tokens: &[karet_core::SemanticToken]) -> Highlights {
    let mut spans: Vec<HighlightSpan> = Vec::with_capacity(tokens.len());
    for token in tokens {
        let line = token.start.line;
        let end_col = token.start.col.saturating_add(token.length);
        let start = buffer.utf16_to_line_col(line, token.start.col);
        let end = buffer.utf16_to_line_col(line, end_col);
        let (Ok(start), Ok(end)) = (buffer.line_col_to_byte(start), buffer.line_col_to_byte(end))
        else {
            continue;
        };
        let after_previous = spans.last().is_none_or(|last| last.span.end <= start);
        if start < end && after_previous {
            spans.push(HighlightSpan {
                span: Span { start, end },
                token: token.token,
            });
        }
    }
    Highlights::from_sorted_spans(spans)
}

#[cfg(test)]
mod tests {
    use karet_core::SemanticToken;
    use karet_core::SemanticTokenModifiers as M;
    use karet_core::SemanticTokenType as T;
    use karet_core::TokenId;

    use super::*;

    fn token(line: u32, col: u32, length: u32) -> SemanticToken {
        SemanticToken {
            start: LineCol::new(line, col),
            length,
            token: TokenId::semantic(T::Variable, M::NONE),
        }
    }

    /// UTF-16 columns land on the right bytes past a character outside the
    /// basic plane, and a token past the text or overlapping its predecessor
    /// is dropped rather than painted out of order.
    #[test]
    fn tokens_convert_from_utf16_columns_to_byte_spans() {
        // `𝑥` is two UTF-16 units and four UTF-8 bytes.
        let buffer = TextBuffer::from_text("let 𝑥 = y;\nz\n");
        let spans = byte_spans(
            &buffer,
            &[
                token(0, 4, 2),
                token(0, 4, 1),
                token(0, 9, 1),
                token(1, 0, 1),
                token(7, 0, 3),
            ],
        );
        let bytes: Vec<(usize, usize)> = spans
            .all()
            .iter()
            .map(|s| (s.span.start.0, s.span.end.0))
            .collect();
        assert_eq!(bytes, [(4, 8), (11, 12), (14, 15)]);
    }
}

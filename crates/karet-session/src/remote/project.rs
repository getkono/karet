//! Projecting the local snapshot stream onto the wire.
//!
//! [`DocSnapshot`] shares a rope and `Arc`s with the session's own document; none
//! of that can cross a connection, and re-sending all of it per keystroke would
//! defeat the point of the split. This turns each snapshot into the *difference*
//! from what the client was last told.
//!
//! Two cheap facts make that possible. The `Arc`s in a snapshot are the document's
//! own, so a producer that did not recompute yields the identical pointer and
//! [`Arc::ptr_eq`] settles "unchanged" without comparing contents. And the client
//! announces the versions it produced itself, so text is only ever sent when the
//! backend moved it.

use std::collections::HashMap;
use std::sync::Arc;

use karet_core::Decoration;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextBuffer;

use super::delta::minimal_change;
use crate::api::DocumentId;
use crate::api::HighlightSlice;
use crate::api::RenderUpdate;
use crate::api::TextUpdate;
use crate::local::DocSnapshot;

/// What the client was last told about one document.
struct Sent {
    /// The text at the version the client is known to hold. Kept as a buffer
    /// rather than a `String` so retaining it is an `Arc` bump, not a copy.
    text: TextBuffer,
    /// The version that text is at.
    version: u64,
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    semantic_blocks: Arc<SemanticBlocks>,
    decorations: Arc<Vec<Decoration>>,
    syntax_error_lines: Arc<Vec<(u32, u32)>>,
    language: Option<&'static str>,
    dirty: bool,
}

/// Tracks what a connection has been told, per document.
#[derive(Default)]
pub(super) struct Projection {
    sent: HashMap<DocumentId, Sent>,
    /// Versions this client produced with its own edits, so the backend knows
    /// not to send the text back.
    client_versions: HashMap<DocumentId, u64>,
}

impl Projection {
    /// Record that the client's own edit produced `version` of `doc`.
    ///
    /// Without this every keystroke would look like a backend edit and echo the
    /// document back at the client that just typed into it.
    pub(super) fn client_reached(&mut self, doc: DocumentId, version: u64) {
        let entry = self.client_versions.entry(doc).or_default();
        *entry = (*entry).max(version);
    }

    /// Forget a closed document.
    pub(super) fn forget(&mut self, doc: DocumentId) {
        self.sent.remove(&doc);
        self.client_versions.remove(&doc);
    }

    /// Discard everything, so the next projection of each document is complete.
    ///
    /// Used when a client attaches without a resumable position: it holds no
    /// replicas, so it must be told everything from scratch.
    pub(super) fn reset(&mut self) {
        self.sent.clear();
        self.client_versions.clear();
    }

    /// Project `snapshot` into the update this client still needs.
    ///
    /// `None` when the client already knows everything in it — a snapshot minted
    /// for a producer that changed nothing this connection can see.
    pub(super) fn project(
        &mut self,
        doc: DocumentId,
        snapshot: &DocSnapshot,
    ) -> Option<RenderUpdate> {
        let text = self.text_update(doc, snapshot);
        let previous = self.sent.get(&doc);
        let update = RenderUpdate {
            version: snapshot.version,
            text,
            highlights: changed_arc(previous.map(|sent| &sent.highlights), &snapshot.highlights)
                .map(|highlights| HighlightSlice {
                    // The backend narrows a snapshot's spans to the declared
                    // viewport before publishing, so the slice is already scoped;
                    // the range is left open because the client replaces rather
                    // than merges and never needs to know where it ended.
                    range: None,
                    highlights: highlights.as_ref().clone(),
                }),
            folds: changed_arc(previous.map(|sent| &sent.folds), &snapshot.folds)
                .map(|folds| folds.as_ref().clone()),
            semantic_blocks: changed_arc(
                previous.map(|sent| &sent.semantic_blocks),
                &snapshot.semantic_blocks,
            )
            .map(|blocks| blocks.as_ref().clone()),
            decorations: changed_arc(
                previous.map(|sent| &sent.decorations),
                &snapshot.decorations,
            )
            .map(|decorations| decorations.as_ref().clone()),
            syntax_error_lines: changed_arc(
                previous.map(|sent| &sent.syntax_error_lines),
                &snapshot.syntax_error_lines,
            )
            .map(|lines| lines.as_ref().clone()),
            language: (previous.map(|sent| sent.language) != Some(snapshot.language))
                .then_some(snapshot.language)
                .flatten()
                .map(str::to_owned),
            dirty: snapshot.dirty,
            cursor: snapshot.cursor.clone(),
        };
        // `dirty` rides every update and so cannot make one worth sending on its
        // own — but a save clears it while touching nothing else, and the client
        // must hear about that or its tab keeps showing an unsaved marker.
        let dirty_changed = previous.is_none_or(|sent| sent.dirty != snapshot.dirty);
        self.remember(doc, snapshot);
        if update.is_empty() && !dirty_changed {
            return None;
        }
        Some(update)
    }

    /// Decide what, if anything, to say about `doc`'s text.
    fn text_update(&self, doc: DocumentId, snapshot: &DocSnapshot) -> TextUpdate {
        let Some(sent) = self.sent.get(&doc) else {
            // First sight of this document: the client has no replica at all.
            return TextUpdate::Full(snapshot.buffer.text());
        };
        if sent.version == snapshot.version {
            return TextUpdate::Unchanged;
        }
        // The client produced this version itself, so it already has the text.
        if self
            .client_versions
            .get(&doc)
            .is_some_and(|known| *known >= snapshot.version)
        {
            return TextUpdate::Unchanged;
        }
        // The backend moved the text: format-on-save, a rename, a reload, an undo.
        match minimal_change(&sent.text.text(), &snapshot.buffer.text(), sent.version) {
            Some(change) => TextUpdate::Change(Box::new(change)),
            // Equal text at a different version — nothing to say about it.
            None => TextUpdate::Unchanged,
        }
    }

    fn remember(&mut self, doc: DocumentId, snapshot: &DocSnapshot) {
        self.sent.insert(
            doc,
            Sent {
                text: snapshot.buffer.content_snapshot(),
                version: snapshot.version,
                highlights: snapshot.highlights.clone(),
                folds: snapshot.folds.clone(),
                semantic_blocks: snapshot.semantic_blocks.clone(),
                decorations: snapshot.decorations.clone(),
                syntax_error_lines: snapshot.syntax_error_lines.clone(),
                language: snapshot.language,
                dirty: snapshot.dirty,
            },
        );
    }
}

/// `Some(current)` when the producer recomputed this data, `None` when it did not.
///
/// Pointer identity, not equality: the session shares one `Arc` per producer
/// output, so an unchanged pointer is proof nothing was recomputed — and proving
/// it costs a comparison rather than a walk of every span.
fn changed_arc<'a, T>(previous: Option<&Arc<T>>, current: &'a Arc<T>) -> Option<&'a Arc<T>> {
    match previous {
        Some(previous) if Arc::ptr_eq(previous, current) => None,
        _ => Some(current),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use karet_text::TextBuffer;

    use super::*;

    fn snapshot(version: u64, text: &str) -> DocSnapshot {
        DocSnapshot {
            version,
            buffer: TextBuffer::from_bytes(text.as_bytes()).unwrap_or_default(),
            highlights: Arc::default(),
            folds: Arc::default(),
            semantic_blocks: Arc::default(),
            decorations: Arc::default(),
            syntax_error_lines: Arc::default(),
            language: None,
            dirty: false,
            cursor: None,
        }
    }

    const DOC: DocumentId = DocumentId(1);

    /// A client that has never heard of a document has no replica to delta
    /// against, so the first thing it is told must be the whole text.
    #[test]
    fn the_first_projection_of_a_document_carries_its_text() {
        let mut projection = Projection::default();

        let update = projection.project(DOC, &snapshot(1, "alpha\n"));

        let Some(update) = update else {
            return;
        };
        assert_eq!(update.text, TextUpdate::Full("alpha\n".to_owned()));
    }

    /// The steady state. Producers that recomputed nothing share the same `Arc`s,
    /// so a republish at the same version has nothing to say.
    #[test]
    fn a_republish_with_nothing_changed_is_dropped() {
        let mut projection = Projection::default();
        let first = snapshot(1, "alpha\n");
        let _ = projection.project(DOC, &first);

        let again = DocSnapshot {
            highlights: first.highlights.clone(),
            folds: first.folds.clone(),
            semantic_blocks: first.semantic_blocks.clone(),
            decorations: first.decorations.clone(),
            syntax_error_lines: first.syntax_error_lines.clone(),
            ..snapshot(1, "alpha\n")
        };

        assert!(projection.project(DOC, &again).is_none());
    }

    /// The client typed this. Echoing the text back at it would double every
    /// keystroke's cost and, worse, fight its optimistic replica.
    #[test]
    fn an_edit_the_client_made_is_not_echoed_back() {
        let mut projection = Projection::default();
        let _ = projection.project(DOC, &snapshot(1, "alpha\n"));
        projection.client_reached(DOC, 2);

        let update = projection.project(DOC, &snapshot(2, "omega\n"));

        let Some(update) = update else {
            return;
        };
        assert_eq!(update.text, TextUpdate::Unchanged);
        assert_eq!(update.version, 2);
    }

    /// An edit the client did not make — a format, a rename, an undo — has to
    /// reach it, and as an edit rather than a document.
    #[test]
    fn a_backend_edit_is_sent_as_a_change_not_a_document() {
        let mut projection = Projection::default();
        let _ = projection.project(DOC, &snapshot(1, "alpha\n"));

        let update = projection.project(DOC, &snapshot(2, "omega\n"));

        let Some(update) = update else {
            return;
        };
        let TextUpdate::Change(change) = update.text else {
            return;
        };
        assert_eq!(change.base_version, 1);
        assert_eq!(change.edits.len(), 1);
    }

    /// A save clears the unsaved marker while touching nothing else. It is
    /// precisely the update a naive "did anything change?" test would drop.
    #[test]
    fn a_change_of_only_the_dirty_flag_is_still_sent() {
        let mut projection = Projection::default();
        let first = snapshot(1, "alpha\n");
        let _ = projection.project(
            DOC,
            &DocSnapshot {
                dirty: true,
                ..snapshot(1, "alpha\n")
            },
        );

        let update = projection.project(
            DOC,
            &DocSnapshot {
                highlights: first.highlights.clone(),
                folds: first.folds.clone(),
                semantic_blocks: first.semantic_blocks.clone(),
                decorations: first.decorations.clone(),
                syntax_error_lines: first.syntax_error_lines.clone(),
                dirty: false,
                ..snapshot(1, "alpha\n")
            },
        );

        let Some(update) = update else {
            return;
        };
        assert!(!update.dirty);
    }

    /// The recovery path. After a client discards a diverged replica, the backend
    /// must stop deltaing against state the client no longer has.
    #[test]
    fn forgetting_a_document_makes_the_next_projection_complete_again() {
        let mut projection = Projection::default();
        let _ = projection.project(DOC, &snapshot(1, "alpha\n"));
        projection.client_reached(DOC, 1);

        projection.forget(DOC);
        let update = projection.project(DOC, &snapshot(1, "alpha\n"));

        let Some(update) = update else {
            return;
        };
        assert_eq!(update.text, TextUpdate::Full("alpha\n".to_owned()));
    }

    /// A reattaching client that could not resume holds nothing, so everything it
    /// is told next must stand on its own.
    #[test]
    fn resetting_makes_every_document_complete_again() {
        let mut projection = Projection::default();
        let _ = projection.project(DOC, &snapshot(1, "alpha\n"));
        let _ = projection.project(DocumentId(2), &snapshot(1, "beta\n"));

        projection.reset();

        let Some(first) = projection.project(DOC, &snapshot(1, "alpha\n")) else {
            return;
        };
        let Some(second) = projection.project(DocumentId(2), &snapshot(1, "beta\n")) else {
            return;
        };
        assert!(matches!(first.text, TextUpdate::Full(_)));
        assert!(matches!(second.text, TextUpdate::Full(_)));
    }

    /// A version bump with identical text (a no-op edit, a re-save) is not worth an
    /// edit that changes nothing.
    #[test]
    fn a_version_bump_with_identical_text_says_nothing_about_the_text() {
        let mut projection = Projection::default();
        let _ = projection.project(DOC, &snapshot(1, "alpha\n"));

        let update = projection.project(DOC, &snapshot(2, "alpha\n"));

        let text = update.map(|update| update.text);
        assert!(
            matches!(text, None | Some(TextUpdate::Unchanged)),
            "{text:?}"
        );
    }
}

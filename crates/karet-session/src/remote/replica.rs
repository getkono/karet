//! The client's per-document replica.
//!
//! A remote client holds no authoritative state, but it cannot hold *none*: to
//! echo a keystroke without waiting for the network it needs the text in front of
//! it. So it keeps a replica — derived, discardable, and rebuilt from the server
//! whenever the two disagree.
//!
//! The replica's job is to turn a stream of [`RenderUpdate`]s back into the
//! [`DocSnapshot`]s the presentation layer already renders from. That is what
//! keeps remote mode additive: the rendering code cannot tell which mode it is in.

use std::sync::Arc;

use karet_core::CursorState;
use karet_core::Decoration;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextBuffer;

use crate::api::RenderUpdate;
use crate::api::TextUpdate;
use crate::local::DocSnapshot;

/// One document as the client knows it.
#[derive(Default)]
pub(super) struct Replica {
    /// The text, advanced by the client's own edits and by the backend's.
    buffer: TextBuffer,
    /// Spans for the window the client declared. Replaced wholesale rather than
    /// merged: a slice is scoped to a version, and merging one into a cache built
    /// from another is how stale highlights survive an edit.
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    semantic_blocks: Arc<SemanticBlocks>,
    decorations: Arc<Vec<Decoration>>,
    syntax_error_lines: Arc<Vec<(u32, u32)>>,
    /// Interned so the snapshot can carry the `&'static str` the renderer wants.
    language: Option<&'static str>,
    dirty: bool,
}

impl Replica {
    /// Apply `update` and mint the snapshot it produces.
    ///
    /// `None` when the update could not be applied — a backend edit against text
    /// the replica has diverged from. The caller resynchronizes rather than
    /// rendering something the backend never said.
    pub(super) fn apply(&mut self, update: RenderUpdate) -> Option<Arc<DocSnapshot>> {
        match update.text {
            TextUpdate::Unchanged => {},
            TextUpdate::Full(text) => {
                self.buffer = TextBuffer::from_bytes(text.as_bytes()).ok()?;
            },
            TextUpdate::Change(change) => {
                self.buffer
                    .apply(&change, karet_text::EditContext::default())
                    .ok()?;
            },
        }
        if let Some(slice) = update.highlights {
            self.highlights = Arc::new(slice.highlights);
        }
        if let Some(folds) = update.folds {
            self.folds = Arc::new(folds);
        }
        if let Some(blocks) = update.semantic_blocks {
            self.semantic_blocks = Arc::new(blocks);
        }
        if let Some(decorations) = update.decorations {
            self.decorations = Arc::new(decorations);
        }
        if let Some(lines) = update.syntax_error_lines {
            self.syntax_error_lines = Arc::new(lines);
        }
        if let Some(language) = update.language {
            self.language = intern_language(&language);
        }
        self.dirty = update.dirty;
        Some(self.snapshot(update.version, update.cursor))
    }

    /// Apply the client's own edit, so the echo does not wait for the network.
    ///
    /// The presentation layer applies the same change to its own buffer; keeping
    /// the replica in step means the next backend update — which is relative to
    /// this version — lands on the same text the backend has.
    pub(super) fn apply_local(
        &mut self,
        change: &karet_core::Change,
        cause: karet_text::EditCause,
    ) -> Option<Arc<DocSnapshot>> {
        let applied = self
            .buffer
            .apply(
                change,
                karet_text::EditContext {
                    cause,
                    ..Default::default()
                },
            )
            .ok()?;
        self.dirty = self.buffer.is_dirty();
        Some(self.snapshot(applied.version, None))
    }

    fn snapshot(&self, version: u64, cursor: Option<CursorState>) -> Arc<DocSnapshot> {
        Arc::new(DocSnapshot {
            version,
            buffer: self.buffer.content_snapshot(),
            highlights: self.highlights.clone(),
            folds: self.folds.clone(),
            semantic_blocks: self.semantic_blocks.clone(),
            decorations: self.decorations.clone(),
            syntax_error_lines: self.syntax_error_lines.clone(),
            language: self.language,
            dirty: self.dirty,
            cursor,
        })
    }
}

/// Resolve a language name to the `'static` string a snapshot carries.
///
/// The renderer's language field is `&'static str` because in local mode it comes
/// straight from the [`karet_filetype`] registry. A name that arrived over a
/// connection has no such lifetime, so it is matched back against that same
/// registry — the one authority both modes name languages from. An unknown name
/// renders as no language, exactly as an unrecognized file does.
fn intern_language(name: &str) -> Option<&'static str> {
    karet_filetype::all_file_types()
        .iter()
        .map(karet_filetype::FileType::name)
        .find(|known| *known == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::HighlightSlice;

    fn full(version: u64, text: &str) -> RenderUpdate {
        RenderUpdate {
            text: TextUpdate::Full(text.to_owned()),
            ..RenderUpdate::at(version)
        }
    }

    #[test]
    fn a_full_update_establishes_the_text() {
        let mut replica = Replica::default();

        let snapshot = replica.apply(full(1, "hello\n"));

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.buffer.text(), "hello\n");
        assert_eq!(snapshot.version, 1);
    }

    /// The overwhelmingly common update: the client already applied the edit, so
    /// the backend confirms a version and ships derived data.
    #[test]
    fn an_unchanged_update_keeps_the_text_and_refreshes_derived_data() {
        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {}\n"));

        let snapshot = replica.apply(RenderUpdate {
            highlights: Some(HighlightSlice::default()),
            dirty: true,
            ..RenderUpdate::at(2)
        });

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.buffer.text(), "fn main() {}\n");
        assert!(snapshot.dirty);
    }

    #[test]
    fn a_change_update_advances_the_text() -> Result<(), Box<dyn std::error::Error>> {
        let mut replica = Replica::default();
        let base = replica
            .apply(full(1, "alpha\n"))
            .ok_or("an initial snapshot")?
            .version;
        let change =
            super::super::delta::minimal_change("alpha\n", "beta\n", base).ok_or("a change")?;

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Change(Box::new(change)),
            ..RenderUpdate::at(base + 1)
        });

        let Some(snapshot) = snapshot else {
            return Ok(());
        };
        assert_eq!(snapshot.buffer.text(), "beta\n");
        Ok(())
    }

    /// A change against text the replica has diverged from must be refused, not
    /// half-applied — the caller resynchronizes instead.
    #[test]
    fn a_change_against_a_diverged_replica_is_refused() {
        let mut replica = Replica::default();
        let Some(snapshot) = replica.apply(full(1, "short\n")) else {
            return;
        };
        let bad = karet_core::Change::new(
            snapshot.version,
            vec![karet_core::TextEdit {
                range: karet_core::Range {
                    start: karet_core::LineCol { line: 400, col: 0 },
                    end: karet_core::LineCol { line: 400, col: 9 },
                },
                new_text: "nope".to_owned(),
            }],
        );

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Change(Box::new(bad)),
            ..RenderUpdate::at(2)
        });

        assert!(snapshot.is_none());
    }

    /// The echo path: a local edit must advance the replica immediately, with no
    /// backend round trip involved.
    #[test]
    fn a_local_edit_advances_the_replica_without_the_backend() {
        let mut replica = Replica::default();
        let Some(initial) = replica.apply(full(1, "alpha\n")) else {
            return;
        };
        let base = initial.version;
        let Some(change) = super::super::delta::minimal_change("alpha\n", "alphax\n", base) else {
            return;
        };

        let snapshot = replica.apply_local(&change, karet_text::EditCause::Type);

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.buffer.text(), "alphax\n");
        assert!(snapshot.version > base);
    }

    /// A highlight slice is scoped to a window and a version. Replacing rather
    /// than merging is what stops spans computed for one version from surviving
    /// into another.
    #[test]
    fn a_highlight_slice_replaces_rather_than_merges() {
        use karet_core::HighlightSpan;
        use karet_core::Span;

        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {}\n"));
        let Ok(span) = Span::new(karet_core::BytePos(0), karet_core::BytePos(2)) else {
            return;
        };
        let _ = replica.apply(RenderUpdate {
            highlights: Some(HighlightSlice {
                range: None,
                highlights: Highlights::from_sorted_spans(vec![HighlightSpan {
                    span,
                    token: karet_core::TokenId::KEYWORD,
                }]),
            }),
            ..RenderUpdate::at(1)
        });

        let snapshot = replica.apply(RenderUpdate {
            highlights: Some(HighlightSlice::default()),
            ..RenderUpdate::at(1)
        });

        let Some(snapshot) = snapshot else {
            return;
        };
        assert!(snapshot.highlights.all().is_empty());
    }

    /// Derived data the backend did not resend must persist: a `None` field means
    /// "unchanged", and dropping it would blank folds on every keystroke.
    #[test]
    fn omitted_derived_data_persists_across_updates() {
        use karet_core::FoldRegion;

        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {\n}\n"));
        let _ = replica.apply(RenderUpdate {
            folds: Some(FoldRegions::from_sorted_regions(vec![FoldRegion {
                start: 0,
                end: 1,
            }])),
            ..RenderUpdate::at(1)
        });

        let snapshot = replica.apply(RenderUpdate::at(2));

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.folds.regions().len(), 1);
    }

    #[test]
    fn a_known_language_name_survives_the_wire_as_a_static_str() {
        let mut replica = Replica::default();

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Full("fn main() {}\n".to_owned()),
            language: Some("Rust".to_owned()),
            ..RenderUpdate::at(1)
        });

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.language, Some("Rust"));
    }

    /// A name this build has no grammar for renders as no language, exactly as an
    /// unrecognized file does — never as a dangling string.
    #[test]
    fn an_unknown_language_name_renders_as_no_language() {
        let mut replica = Replica::default();

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Full("x\n".to_owned()),
            language: Some("Bogolang".to_owned()),
            ..RenderUpdate::at(1)
        });

        let Some(snapshot) = snapshot else {
            return;
        };
        assert_eq!(snapshot.language, None);
    }
}

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

use std::ops::RangeInclusive;
use std::sync::Arc;

use karet_core::CursorState;
use karet_core::FoldRegions;
use karet_core::Highlights;
use karet_core::SemanticBlocks;
use karet_text::TextBuffer;

use crate::api::RenderUpdate;
use crate::api::TextUpdate;
use crate::local::DocSnapshot;

/// One document as the client knows it.
#[derive(Default)]
pub(super) struct Replica {
    /// The text, advanced by the client's own edits and by the backend's.
    ///
    /// Its version is kept equal to the *document's* version rather than to a
    /// count of this buffer's own edits — see [`Replica::apply`]. Everything
    /// downstream depends on that: the presentation layer bases its next edit on
    /// the version a snapshot reports, and the backend derives its deltas from
    /// the version it last sent.
    buffer: TextBuffer,
    /// Spans for the window the client declared. Replaced wholesale rather than
    /// merged: a slice is scoped to a version, and merging one into a cache built
    /// from another is how stale highlights survive an edit.
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    semantic_blocks: Arc<SemanticBlocks>,
    syntax_error_lines: Arc<Vec<(u32, u32)>>,
    table_lines: Arc<Vec<RangeInclusive<u32>>>,
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
        // An update that moves no text but names an older version than the
        // replica holds was overtaken by a local edit on the way here: the client
        // typed while it was in flight. Its derived data is still the newest the
        // backend has, but its version and cursor describe text the replica has
        // already moved past.
        let stale =
            matches!(update.text, TextUpdate::Unchanged) && update.version < self.buffer.version();
        match update.text {
            TextUpdate::Unchanged => {},
            // The text exactly as sent: it is the backend's in-memory text, which
            // the backend's deltas are computed against. Decoding it as file bytes
            // would strip a leading U+FEFF and normalize line endings, moving
            // every later edit and highlight on the lines that changed.
            TextUpdate::Full(text) => {
                self.buffer = TextBuffer::from_text(&text);
            },
            TextUpdate::Change(change) => {
                self.buffer
                    .apply(&change, karet_text::EditContext::default())
                    .ok()?;
            },
        }
        // The text is now what the backend says `update.version` holds, so the
        // replica must number it that way. A buffer rebuilt from a full copy
        // counts from its own zero, and a change can carry the document past more
        // than one version; either gap is silent and permanent. The presentation
        // layer bases its next edit on the version a snapshot reports, so a
        // replica numbering from zero refused every edit that followed — no echo,
        // no warning, and the stale replica text overwriting the tab on the next
        // snapshot. A stale update must not move it back, though: the next
        // keystroke is based on the version the replica last reported, and a
        // replica numbered backwards refuses it and forces a full resync.
        if !stale {
            self.buffer.set_version(update.version);
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
        if let Some(lines) = update.syntax_error_lines {
            self.syntax_error_lines = Arc::new(lines);
        }
        if let Some(lines) = update.table_lines {
            self.table_lines = Arc::new(lines);
        }
        if let Some(language) = update.language {
            self.language = language.as_deref().and_then(intern_language);
        }
        if stale {
            return Some(self.snapshot(self.buffer.version(), None));
        }
        self.dirty = update.dirty;
        Some(self.snapshot(update.version, update.cursor))
    }

    /// Apply the client's own edit, so the echo does not wait for the network.
    ///
    /// The presentation layer applies the same change to its own buffer; keeping
    /// the replica in step means the next backend update — which is relative to
    /// this version — lands on the same text the backend has.
    ///
    /// `None` when the change does not fit the version this replica is at. The
    /// caller resynchronizes: a replica that cannot place the client's own edit
    /// cannot place the backend's next one either, and going quiet about it is
    /// how a document ends up silently swallowing every keystroke.
    pub(super) fn apply_local(
        &mut self,
        change: &karet_core::Change,
        cause: karet_core::EditCause,
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
            syntax_error_lines: self.syntax_error_lines.clone(),
            table_lines: self.table_lines.clone(),
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

    /// What a test returns: an update that should apply and did not fails the
    /// test rather than letting it pass without asserting anything.
    type TestResult = Result<(), &'static str>;

    #[test]
    fn a_full_update_establishes_the_text() -> TestResult {
        let mut replica = Replica::default();

        let snapshot = replica.apply(full(1, "hello\n")).ok_or("a snapshot")?;

        assert_eq!(snapshot.buffer.text(), "hello\n");
        assert_eq!(snapshot.version, 1);
        Ok(())
    }

    /// The backend's deltas are computed against its in-memory text, byte for
    /// byte. A replica that dropped a leading byte-order mark or normalized line
    /// endings would place every later edit on line 0 one column off, or on the
    /// wrong side of a `\r`.
    #[test]
    fn a_full_update_keeps_a_byte_order_mark_and_crlf_as_sent() -> TestResult {
        let text = "\u{feff}one\r\ntwo\r\n";
        let mut replica = Replica::default();

        let snapshot = replica.apply(full(1, text)).ok_or("a snapshot")?;

        assert_eq!(snapshot.buffer.text(), text);
        Ok(())
    }

    /// The overwhelmingly common update: the client already applied the edit, so
    /// the backend confirms a version and ships derived data.
    #[test]
    fn an_unchanged_update_keeps_the_text_and_refreshes_derived_data() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {}\n"));

        let snapshot = replica.apply(RenderUpdate {
            highlights: Some(HighlightSlice::default()),
            dirty: true,
            ..RenderUpdate::at(2)
        });

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.buffer.text(), "fn main() {}\n");
        assert!(snapshot.dirty);
        Ok(())
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

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.buffer.text(), "beta\n");
        Ok(())
    }

    /// A change against text the replica has diverged from must be refused, not
    /// half-applied — the caller resynchronizes instead.
    #[test]
    fn a_change_against_a_diverged_replica_is_refused() -> TestResult {
        let mut replica = Replica::default();
        let snapshot = replica.apply(full(1, "short\n")).ok_or("a snapshot")?;
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
        Ok(())
    }

    /// The echo path: a local edit must advance the replica immediately, with no
    /// backend round trip involved.
    #[test]
    fn a_local_edit_advances_the_replica_without_the_backend() -> TestResult {
        let mut replica = Replica::default();
        let base = replica
            .apply(full(1, "alpha\n"))
            .ok_or("a snapshot")?
            .version;
        let change =
            super::super::delta::minimal_change("alpha\n", "alphax\n", base).ok_or("a change")?;

        let snapshot = replica
            .apply_local(&change, karet_core::EditCause::Type)
            .ok_or("the echo")?;

        assert_eq!(snapshot.buffer.text(), "alphax\n");
        assert!(snapshot.version > base);
        Ok(())
    }

    /// A render the backend sent before a local edit reached it arrives after
    /// the replica moved on. It must not number the replica backwards, or the
    /// next keystroke — based on the newer version — is refused and every
    /// keystroke on a slow link forces a resync.
    #[test]
    fn a_render_overtaken_by_a_local_edit_does_not_move_the_version_back() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(full(3, "alpha\n"));
        let typed =
            super::super::delta::minimal_change("alpha\n", "alphax\n", 3).ok_or("a change")?;
        let echoed = replica
            .apply_local(&typed, karet_core::EditCause::Type)
            .ok_or("the echo")?;
        assert_eq!(echoed.version, 4);

        let stale = replica
            .apply(RenderUpdate {
                dirty: true,
                ..RenderUpdate::at(3)
            })
            .ok_or("a snapshot")?;

        assert_eq!(stale.version, 4);
        assert_eq!(stale.buffer.version(), 4);
        assert_eq!(stale.buffer.text(), "alphax\n");
        let next =
            super::super::delta::minimal_change("alphax\n", "alphaxy\n", 4).ok_or("a change")?;
        let typed_again = replica
            .apply_local(&next, karet_core::EditCause::Type)
            .ok_or("the next keystroke must still apply")?;
        assert_eq!(typed_again.buffer.text(), "alphaxy\n");
        Ok(())
    }

    /// A highlight slice is scoped to a window and a version. Replacing rather
    /// than merging is what stops spans computed for one version from surviving
    /// into another.
    #[test]
    fn a_highlight_slice_replaces_rather_than_merges() -> TestResult {
        use karet_core::HighlightSpan;
        use karet_core::Span;

        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {}\n"));
        let span = Span::new(karet_core::BytePos(0), karet_core::BytePos(2));
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

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert!(snapshot.highlights.all().is_empty());
        Ok(())
    }

    /// Derived data the backend did not resend must persist: a `None` field means
    /// "unchanged", and dropping it would blank folds on every keystroke.
    #[test]
    fn omitted_derived_data_persists_across_updates() -> TestResult {
        use karet_core::FoldRegion;

        let mut replica = Replica::default();
        let _ = replica.apply(full(1, "fn main() {\n}\n"));
        let _ = replica.apply(RenderUpdate {
            folds: Some(FoldRegions::from_sorted_regions(vec![FoldRegion {
                start: 0,
                end: 1,
                kind: karet_core::FoldKind::Code,
            }])),
            ..RenderUpdate::at(1)
        });

        let snapshot = replica.apply(RenderUpdate::at(2));

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.folds.regions().len(), 1);
        Ok(())
    }

    #[test]
    fn a_known_language_name_survives_the_wire_as_a_static_str() -> TestResult {
        let mut replica = Replica::default();

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Full("fn main() {}\n".to_owned()),
            language: Some(Some("Rust".to_owned())),
            ..RenderUpdate::at(1)
        });

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.language, Some("Rust"));
        Ok(())
    }

    /// A name this build has no grammar for renders as no language, exactly as an
    /// unrecognized file does — never as a dangling string.
    #[test]
    fn an_unknown_language_name_renders_as_no_language() -> TestResult {
        let mut replica = Replica::default();

        let snapshot = replica.apply(RenderUpdate {
            text: TextUpdate::Full("x\n".to_owned()),
            language: Some(Some("Bogolang".to_owned())),
            ..RenderUpdate::at(1)
        });

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.language, None);
        Ok(())
    }

    /// Renaming a file to an unrecognized extension clears its language. An
    /// update that could only *set* one would leave the tab labelled with the
    /// language the file used to be.
    #[test]
    fn a_language_can_be_cleared_as_well_as_set() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(RenderUpdate {
            text: TextUpdate::Full("fn main() {}\n".to_owned()),
            language: Some(Some("Rust".to_owned())),
            ..RenderUpdate::at(1)
        });

        let snapshot = replica.apply(RenderUpdate {
            language: Some(None),
            ..RenderUpdate::at(2)
        });

        let snapshot = snapshot.ok_or("a snapshot")?;
        assert_eq!(snapshot.language, None);
        Ok(())
    }

    /// The property the presentation layer depends on: a snapshot's version and
    /// the version of the buffer inside it must be the same number. The app bases
    /// its next edit on the former and applies it to the latter.
    #[test]
    fn a_full_update_numbers_the_replica_by_the_document_not_by_itself() -> TestResult {
        let mut replica = Replica::default();

        let snapshot = replica
            .apply(full(7, "alpha\n"))
            .ok_or("a full update must produce a snapshot")?;

        assert_eq!(snapshot.version, 7);
        assert_eq!(snapshot.buffer.version(), 7);
        Ok(())
    }

    /// The regression a resync used to cause. A replica rebuilt from a full copy
    /// at a non-zero version must still accept the edits that follow: numbering
    /// from its own zero made every later keystroke fail silently, and the stale
    /// text then overwrote what the user had typed.
    #[test]
    fn a_replica_rebuilt_at_a_non_zero_version_still_takes_the_next_local_edit() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(full(7, "alpha\n"));
        let change =
            super::super::delta::minimal_change("alpha\n", "alphax\n", 7).ok_or("a change")?;

        let snapshot = replica
            .apply_local(&change, karet_core::EditCause::Type)
            .ok_or("an edit based on the version the replica reported must apply")?;

        assert_eq!(snapshot.buffer.text(), "alphax\n");
        assert_eq!(snapshot.version, 8);
        Ok(())
    }

    /// A backend change that skips versions (the projection derives it from the
    /// version it last *sent*, which can lag the document) must still leave the
    /// replica numbered by the document.
    #[test]
    fn a_change_carrying_the_document_past_several_versions_lands_on_the_new_one() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(full(3, "alpha\n"));
        let change =
            super::super::delta::minimal_change("alpha\n", "beta\n", 3).ok_or("a change")?;

        let snapshot = replica
            .apply(RenderUpdate {
                text: TextUpdate::Change(Box::new(change)),
                ..RenderUpdate::at(9)
            })
            .ok_or("a well-based change must apply")?;

        assert_eq!(snapshot.buffer.text(), "beta\n");
        assert_eq!(snapshot.buffer.version(), 9);
        Ok(())
    }

    /// An update that says nothing about the language keeps the one in force —
    /// that is what `None` means for every other field too.
    #[test]
    fn an_update_silent_about_the_language_keeps_it() -> TestResult {
        let mut replica = Replica::default();
        let _ = replica.apply(RenderUpdate {
            text: TextUpdate::Full("fn main() {}\n".to_owned()),
            language: Some(Some("Rust".to_owned())),
            ..RenderUpdate::at(1)
        });

        let snapshot = replica.apply(RenderUpdate::at(2)).ok_or("a snapshot")?;

        assert_eq!(snapshot.language, Some("Rust"));
        Ok(())
    }
}

//! The wire form of a document's render-only state.
//!
//! In local mode the UI renders from [`DocSnapshot`](crate::local::DocSnapshot),
//! which shares a rope and `Arc`s across the actor boundary and never leaves the
//! process. A remote client cannot receive that, so the backend projects the same
//! information into [`RenderUpdate`] — owned, serializable, and *delta-shaped*:
//! every field is `Option`, and `None` means "unchanged since the last update for
//! this document". A keystroke's update therefore carries a highlight slice and
//! little else.
//!
//! The client keeps the text itself as a replica and applies its own edits
//! optimistically, so [`TextUpdate`] describes only what the *backend* did to the
//! text — usually nothing.

use karet_core::Change;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::Span;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;

/// What the backend did to a document's text to reach a version.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum TextUpdate {
    /// Nothing the client does not already know.
    ///
    /// The overwhelmingly common case: the client applied this edit itself and
    /// the backend is only confirming the version and shipping derived data.
    #[default]
    Unchanged,
    /// Apply this change to reach the update's version.
    ///
    /// A backend-originated edit the client did not make — format-on-save, an
    /// LSP rename or code action, undo/redo, a reload after the file changed on
    /// disk. Sent as a change rather than whole text so an undo on a large file
    /// costs one edit, not one document.
    Change(Box<Change>),
    /// Replace the replica's text wholesale.
    ///
    /// Used where no change can express the transition: the initial open, a
    /// resync after the client fell too far behind, a re-decode under a new
    /// encoding, or a CBOR document the backend rendered to text.
    Full(String),
}

/// Highlight spans covering part of a document.
///
/// Highlights are the one piece of derived data scoped to what the client can
/// actually see: [`karet_editor`] resolves them per rendered line, so a client
/// never needs spans for lines outside its viewport. The backend answers
/// [`Command::SetViewport`](crate::api::Command::SetViewport) with the spans for
/// that window plus a margin, and the client *replaces* rather than merges —
/// which is why a slice can never go stale against a newer version.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HighlightSlice {
    /// The byte range these spans cover, or `None` for the whole document.
    ///
    /// Only spans starting inside the range are present. Rendering a line
    /// outside it yields no highlight rather than a wrong one.
    pub range: Option<Span>,
    /// The spans themselves, in byte order.
    pub highlights: Highlights,
}

/// A document's render-only state at one version, as a delta.
///
/// The serializable counterpart of [`DocSnapshot`](crate::local::DocSnapshot).
/// A `None` field means "unchanged"; the client keeps the previous value. The
/// backend tracks what it last sent per document so it can leave fields out.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct RenderUpdate {
    /// The document version this update reflects.
    pub version: u64,
    /// What the backend did to the text (usually [`TextUpdate::Unchanged`]).
    pub text: TextUpdate,
    /// Highlight spans for the client's viewport.
    pub highlights: Option<HighlightSlice>,
    /// Foldable regions. Whole-document: a fold above the viewport changes which
    /// lines are visible inside it.
    pub folds: Option<FoldRegions>,
    /// Semantic blocks for sticky scroll. Whole-document: the header that stays
    /// pinned usually sits above the viewport.
    pub semantic_blocks: Option<SemanticBlocks>,
    /// Decorations merged across producers (diagnostics, VCS markers, …).
    pub decorations: Option<Vec<Decoration>>,
    /// Inclusive 0-based line ranges covered by syntax errors.
    pub syntax_error_lines: Option<Vec<(u32, u32)>>,
    /// The display language name, when it is first known or changes.
    pub language: Option<String>,
    /// Whether the buffer has unsaved changes.
    pub dirty: bool,
    /// A caret for the client to move to, set only for undo/redo so the edit
    /// site is revealed. `None` leaves the client's own placement alone.
    pub cursor: Option<CursorState>,
}

impl RenderUpdate {
    /// An update carrying nothing but a version — the shape a pure scroll or an
    /// acknowledged local edit produces before any derived data is recomputed.
    #[must_use]
    pub fn at(version: u64) -> Self {
        Self {
            version,
            ..Self::default()
        }
    }

    /// Whether this update would change anything the client renders.
    ///
    /// The backend drops empty updates rather than spending a frame on them; an
    /// update is never empty when the backend touched the text or moved the caret.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.text, TextUpdate::Unchanged)
            && self.highlights.is_none()
            && self.folds.is_none()
            && self.semantic_blocks.is_none()
            && self.decorations.is_none()
            && self.syntax_error_lines.is_none()
            && self.language.is_none()
            && self.cursor.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_carrying_only_a_version_is_empty() {
        assert!(RenderUpdate::at(7).is_empty());
        assert_eq!(RenderUpdate::at(7).version, 7);
    }

    #[test]
    fn a_backend_originated_edit_is_never_empty() {
        let update = RenderUpdate {
            text: TextUpdate::Change(Box::new(Change::new(3, Vec::new()))),
            ..RenderUpdate::at(4)
        };

        assert!(!update.is_empty());
    }

    /// `dirty` is a plain bool, not an `Option`, so it rides every update. It must
    /// not on its own make an update worth sending — the save flag flips with the
    /// text, which is already reported.
    #[test]
    fn the_dirty_flag_alone_does_not_make_an_update_worth_sending() {
        let update = RenderUpdate {
            dirty: true,
            ..RenderUpdate::at(1)
        };

        assert!(update.is_empty());
    }

    #[test]
    fn derived_data_makes_an_update_worth_sending() {
        let update = RenderUpdate {
            folds: Some(FoldRegions::default()),
            ..RenderUpdate::at(1)
        };

        assert!(!update.is_empty());
    }

    #[test]
    fn a_render_update_round_trips_through_serde() -> Result<(), serde_json::Error> {
        let update = RenderUpdate {
            text: TextUpdate::Full("fn main() {}\n".to_owned()),
            highlights: Some(HighlightSlice::default()),
            language: Some("Rust".to_owned()),
            dirty: true,
            ..RenderUpdate::at(12)
        };

        let restored: RenderUpdate = serde_json::from_str(&serde_json::to_string(&update)?)?;

        assert_eq!(restored.version, 12);
        assert_eq!(restored.text, TextUpdate::Full("fn main() {}\n".to_owned()));
        assert_eq!(restored.language.as_deref(), Some("Rust"));
        assert!(restored.dirty);
        Ok(())
    }
}

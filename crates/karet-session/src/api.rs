//! The in-process contract between the presentation layer and the backend: the
//! [`Command`]s a client submits and the [`Event`]s the backend emits.
//!
//! This module carries only neutral `karet-core` (plus a few engine) types, so it
//! is the designated extraction point for a future dependency-light
//! `karet-protocol` crate when the client-server split is undertaken.

use karet_core::{
    Change, CompletionItem, CursorState, Decoration, Diagnostic, Hover, LineCol, Location, Symbol,
};
use karet_search::{FileHit, SearchQuery};
use karet_syntax::HighlightSpan;
use std::path::PathBuf;

/// Identifies an open document within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

/// Identifies a view (editor pane) within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewId(pub u64);

/// Correlates a [`Command`] with the [`Event`] that answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Which producer a [`Event::DecorationsChanged`] batch belongs to, so the client
/// can replace one producer's decoration layer atomically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecorationLayer {
    /// Version-control markers (git gutter, blame).
    Vcs,
    /// Debugger markers (breakpoints, current line).
    Dap,
    /// Search-match highlights.
    Search,
    /// Language-server decorations.
    Lsp,
}

/// A request submitted by the presentation layer to the backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Command {
    /// Open a document.
    OpenDocument {
        /// The file path to open.
        path: PathBuf,
        /// An explicit language id, or `None` to detect from the path.
        language: Option<String>,
    },
    /// Close a document.
    CloseDocument {
        /// The document to close.
        doc: DocumentId,
    },
    /// Apply an atomic change to a document.
    ApplyChange {
        /// The target document.
        doc: DocumentId,
        /// The change to apply.
        change: Change,
    },
    /// Save a document to disk.
    Save {
        /// The document to save.
        doc: DocumentId,
    },
    /// Request completions at a position.
    Completion {
        /// The target document.
        doc: DocumentId,
        /// The position to complete at.
        position: LineCol,
    },
    /// Request hover information at a position.
    Hover {
        /// The target document.
        doc: DocumentId,
        /// The position to hover.
        position: LineCol,
    },
    /// Resolve the definition of the symbol at a position.
    Definition {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
    },
    /// Request the document's symbols.
    DocumentSymbols {
        /// The target document.
        doc: DocumentId,
    },
    /// Search workspace symbols.
    WorkspaceSymbols {
        /// The query string.
        query: String,
    },
    /// Rename the symbol at a position.
    Rename {
        /// The target document.
        doc: DocumentId,
        /// The position of the symbol.
        position: LineCol,
        /// The new name.
        new_name: String,
    },
    /// Format a document as part of saving it.
    FormatOnSave {
        /// The document to format.
        doc: DocumentId,
    },
    /// Run a workspace search.
    Search {
        /// The search query and options.
        query: SearchQuery,
    },
    /// Report the client's cursor/selection state for a view.
    SetCursor {
        /// The target document.
        doc: DocumentId,
        /// The view whose cursors changed.
        view: ViewId,
        /// The new cursor state.
        cursors: CursorState,
    },
}

/// A message emitted by the backend to the presentation layer. When it answers a
/// [`Command`], it is delivered with that command's [`RequestId`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// A document was opened at the given version.
    Opened {
        /// The opened document.
        doc: DocumentId,
        /// Its initial version.
        version: u64,
    },
    /// A change was applied, producing a new version.
    Applied {
        /// The document.
        doc: DocumentId,
        /// The resulting version.
        version: u64,
    },
    /// A document was saved.
    Saved {
        /// The saved document.
        doc: DocumentId,
    },
    /// A document was closed.
    Closed {
        /// The closed document.
        doc: DocumentId,
    },
    /// New diagnostics were published for a document.
    DiagnosticsPublished {
        /// The document.
        doc: DocumentId,
        /// The full diagnostic set for the document.
        diagnostics: Vec<Diagnostic>,
    },
    /// A producer's decoration layer changed.
    DecorationsChanged {
        /// The document.
        doc: DocumentId,
        /// Which producer's layer this replaces.
        layer: DecorationLayer,
        /// The new decorations for that layer.
        decorations: Vec<Decoration>,
    },
    /// Updated syntax highlight spans for a document.
    Highlights {
        /// The document.
        doc: DocumentId,
        /// The highlight spans.
        spans: Vec<HighlightSpan>,
    },
    /// Resolved document symbols.
    Symbols {
        /// The document.
        doc: DocumentId,
        /// The symbols.
        symbols: Vec<Symbol>,
    },
    /// Completion results answering a [`Command::Completion`].
    Completions {
        /// The completion items.
        items: Vec<CompletionItem>,
    },
    /// Hover result answering a [`Command::Hover`].
    HoverResult {
        /// The hover, if any.
        hover: Option<Hover>,
    },
    /// Definition locations answering a [`Command::Definition`].
    Definitions {
        /// The resolved locations.
        locations: Vec<Location>,
    },
    /// Search results answering a [`Command::Search`].
    SearchResults {
        /// The per-file hits.
        hits: Vec<FileHit>,
    },
    /// Progress on a long-running operation.
    Progress {
        /// A human-readable status message.
        message: String,
        /// Percent complete (0–100), if known.
        percent: Option<u8>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_payloads_construct() {
        assert_eq!(DocumentId(1), DocumentId(1));
        assert_ne!(RequestId(1), RequestId(2));
        let _cmd = Command::Save { doc: DocumentId(7) };
        let _ev = Event::Saved { doc: DocumentId(7) };
        assert_eq!(DecorationLayer::Vcs, DecorationLayer::Vcs);
    }
}

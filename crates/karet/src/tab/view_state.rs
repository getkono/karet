//! Viewport state for tabs with custom render models.

use karet_markdown::WrappedDocument;

/// How a diff tab is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// One column: removals then additions.
    Unified,
    /// Two columns: old on the left, new on the right.
    SideBySide,
}

/// View-local state for a rendered Markdown preview beside a code editor.
#[derive(Default)]
pub(crate) struct MarkdownPreviewState {
    /// The parsed and wrapped render model.
    pub(crate) wrapped: WrappedDocument,
    /// The `(document version, wrap width)` represented by [`Self::wrapped`].
    pub(crate) rendered: Option<(u64, u16)>,
    /// The first visible wrapped line.
    pub(crate) scroll: u16,
}

/// The responsive arrangement last used to draw a commit-like view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommitLayoutMode {
    /// Metadata, file index, and diff cards form one vertical document.
    Stacked,
    /// Metadata precedes a pinned file rail beside the diff cards.
    Wide,
}

/// View-local navigation state shared by commit and compare tabs.
#[derive(Debug, Default)]
pub(crate) struct CommitViewState {
    /// Vertical offset in the current layout's virtual document.
    pub(crate) scroll: u16,
    /// Horizontal offset for diff lines wider than the visible pane.
    pub(crate) column: u16,
    /// The layout used by the previous frame, for resize-aware anchor remapping.
    pub(crate) layout: Option<CommitLayoutMode>,
    /// Per-file card-header offsets from the previous frame.
    pub(crate) file_anchors: Vec<u16>,
    /// First file shown in the wide layout's pinned rail.
    pub(crate) rail_offset: usize,
}

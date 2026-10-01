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

/// What a wrapped markdown preview was built at: `(document version, wrap width, image
/// generation, icon style)`. The generation moves when an image's reserved size does,
/// and the icon style leads every image chip, so a change in any re-wraps the preview.
pub(crate) type PreviewKey = (u64, u16, u64, karet_filetype::IconStyle);

/// View-local state for a rendered Markdown preview beside a code editor.
#[derive(Default)]
pub(crate) struct MarkdownPreviewState {
    /// The parsed and wrapped render model.
    pub(crate) wrapped: WrappedDocument,
    /// The key [`Self::wrapped`] was built at.
    pub(crate) rendered: Option<PreviewKey>,
    /// The first visible wrapped line.
    pub(crate) scroll: u16,
}

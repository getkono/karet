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
    /// The `(document version, wrap width, image generation)` represented by
    /// [`Self::wrapped`]; the generation moves when an image's reserved size does.
    pub(crate) rendered: Option<(u64, u16, u64)>,
    /// The first visible wrapped line.
    pub(crate) scroll: u16,
}

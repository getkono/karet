//! Last-frame render geometry recorded for mouse hit-testing: clickable
//! regions, drag states, and per-pane frames. Rebuilt by the renderer every
//! frame; the mouse handlers resolve events against them.

use super::*;

/// A clickable tab region in the tab strip, recorded during the last render.
#[derive(Clone, Copy)]
pub(crate) struct TabHit {
    /// First column of the tab (inclusive).
    pub(crate) start: u16,
    /// One past the last column of the tab (exclusive).
    pub(crate) end: u16,
    /// Column of the close (×) glyph.
    pub(crate) close: u16,
}

pub(crate) use karet_widgets::breadcrumbs::BreadcrumbHit;

/// A clickable changed-file row from a commit or compare view's last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommitFileHit {
    /// The rendered row in screen coordinates.
    pub(crate) rect: Rect,
    /// The changed file's index in the tab.
    pub(crate) file: usize,
    /// The layout-specific scroll offset that puts its card header at the top.
    pub(crate) scroll: u16,
}

/// A clickable disclosure control in a commit or compare file-card header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommitCollapseHit {
    /// The rendered disclosure cell in screen coordinates.
    pub(crate) rect: Rect,
    /// The changed file's index in the tab.
    pub(crate) file: usize,
}

/// A visible link run in the focused Markdown preview's last rendered frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkdownLinkHit {
    /// The rendered cells occupied by this run.
    pub(crate) rect: Rect,
    /// The renderer-neutral target from the Markdown source.
    pub(crate) target: String,
}

/// Lightweight field currently owning a mouse selection drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextFieldTarget {
    SearchFind,
    SearchReplace,
    Commit,
}

/// A rendered pane's clickable regions, recorded during the last frame for mouse
/// hit-testing (which pane a click lands in, and its tab strip / content).
#[derive(Clone)]
pub(crate) struct PaneFrame {
    /// The pane this frame belongs to.
    pub(crate) pane: PaneId,
    /// The pane's tab strip row.
    pub(crate) tabstrip_rect: Rect,
    /// Per-tab clickable regions within the strip.
    pub(crate) tab_hits: Vec<TabHit>,
    /// Format-specific actions right-aligned in the pane's tab strip.
    pub(crate) action_hits: Vec<(u16, u16, Command)>,
    /// The pane's breadcrumb row (zero-sized when the active tab has no path).
    pub(crate) breadcrumb_rect: Rect,
    /// Per-segment clickable regions within the breadcrumb row.
    pub(crate) breadcrumb_hits: Vec<BreadcrumbHit>,
    /// The pane's content (editor) area.
    pub(crate) content_rect: Rect,
    /// The exact editable editor viewport within the content area.
    pub(crate) editor_rect: Rect,
    /// Changed-file rows clickable within the pane's commit-like view.
    pub(crate) commit_file_hits: Vec<CommitFileHit>,
    /// File-card disclosure controls clickable within the pane's commit-like view.
    pub(crate) commit_collapse_hits: Vec<CommitCollapseHit>,
}

/// An in-progress tab drag: the pane it started from and the current drop target
/// (a pane plus which zone of it), used to preview and apply a move/split on release.
#[derive(Clone, Copy)]
pub(crate) struct TabDrag {
    /// The pane the dragged tab started in (and is still in until dropped).
    pub(crate) from_pane: PaneId,
    /// The current drop target: a pane and the zone the cursor is over, if any.
    pub(crate) hover: Option<(PaneId, DropZone)>,
}

/// An in-progress drag of a pane split boundary.
#[derive(Clone, Copy)]
pub(crate) struct PaneResize {
    /// Stable identity and geometry of the boundary when dragging began.
    pub(crate) divider: PaneDivider,
}

/// A clickable toast card, recorded during the last render for click hit-testing.
#[derive(Clone, Copy)]
pub(crate) struct ToastHit {
    /// The card rectangle (a click anywhere on it dismisses the notification).
    pub(crate) rect: Rect,
    /// The notification the card shows.
    pub(crate) id: NotificationId,
}

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
    /// The Search panel's include-glob field.
    SearchIncludes,
    /// The Search panel's exclude-glob field.
    SearchExcludes,
    Commit,
    /// The in-file find bar's query field.
    FindQuery,
    /// The in-file find bar's replacement field.
    FindReplace,
    /// The explorer's inline rename / new-name field.
    ExplorerRename,
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
    /// Read-only surfaces in this pane whose rows the pointer can select.
    pub(crate) select_regions: Vec<super::select::SelectRegion>,
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

/// The view a scrollbar drives, naming the offset a pointer gesture writes to.
///
/// One variant per *offset*, not per bar: several bars can share one, and they should
/// — the merge-conflict view paints three, but its side panes copy the merged
/// editor's offset every frame, so all three grab the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollSurface {
    /// The focused tab's own content, vertically. Which offset that is depends on
    /// the tab kind, exactly as it does for the wheel.
    TabRows,
    /// The focused tab's own content, horizontally.
    TabColumns,
    /// The Markdown preview beside a code tab (not the standalone preview tab,
    /// which is [`TabRows`](Self::TabRows)).
    EditorPreview,
    /// A pull request's commit list, which scrolls independently of its conversation.
    GithubPullRequestCommits,
    /// The Explorer file tree.
    Explorer,
    /// The symbol outline panel.
    Outline,
    /// The workspace-search results list.
    SearchResults,
    /// The workspace-spelling results list.
    SpellingResults,
    /// The Todos results list.
    TodoResults,
    /// The Debug panel's section list.
    DebugResults,
    /// The Source-Control changes list.
    ScmChanges,
    /// The Source-Control commit log.
    ScmCommits,
    /// The completion popup.
    Completion,
}

/// A scrollbar track from the last frame, and the view it scrolls.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollHit {
    /// Where the bar was painted, and the extent it was painted from.
    pub(crate) track: ScrollTrack,
    /// What a gesture on it moves.
    pub(crate) surface: ScrollSurface,
}

/// Every scrollbar track the last frame painted, in draw order.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollHits(Vec<ScrollHit>);

impl ScrollHits {
    /// Record what a paint pass put on screen against the view it scrolls.
    ///
    /// Both axes are taken at once because that is what [`ScrollTracks::paint`]
    /// returns; an axis that reserved no track contributes nothing.
    pub(crate) fn record(&mut self, painted: PaintedTracks, vertical: ScrollSurface) {
        self.record_axis(painted.vertical, vertical);
    }

    /// Record a two-axis paint, whose axes move different offsets.
    pub(crate) fn record_both(
        &mut self,
        painted: PaintedTracks,
        vertical: ScrollSurface,
        horizontal: ScrollSurface,
    ) {
        self.record_axis(painted.vertical, vertical);
        self.record_axis(painted.horizontal, horizontal);
    }

    /// Record one track directly — for the side-by-side diff, whose horizontal track
    /// is two independently scaled halves painted through [`ScrollBar`] by hand.
    pub(crate) fn record_track(&mut self, track: Option<ScrollTrack>, surface: ScrollSurface) {
        self.record_axis(track, surface);
    }

    /// Keep a track only if it has a thumb to grab. A bar for content that fits is
    /// suppressed, but its column stays reserved — recording it would make a click on
    /// an empty groove jump the view to the top.
    fn record_axis(&mut self, track: Option<ScrollTrack>, surface: ScrollSurface) {
        if let Some(track) = track
            && track.thumb_span().is_some()
        {
            self.0.push(ScrollHit { track, surface });
        }
    }

    /// The track under `(x, y)`, topmost first.
    ///
    /// Entries arrive in draw order, so searching backwards makes the completion
    /// popup and the overlaying outline win over the editor bar they float above.
    pub(crate) fn at(&self, x: u16, y: u16) -> Option<ScrollHit> {
        self.0
            .iter()
            .rev()
            .find(|hit| hit.track.contains(x, y))
            .copied()
    }

    /// The track driving `surface`, if the last frame painted one. Tests reach for a
    /// bar by what it scrolls; the mouse only ever reaches for one by where it is.
    #[cfg(test)]
    pub(crate) fn of(&self, surface: ScrollSurface) -> Option<ScrollHit> {
        self.0
            .iter()
            .rev()
            .find(|hit| hit.surface == surface)
            .copied()
    }
}

/// An in-progress scrollbar-thumb drag.
///
/// The track is captured by value at the grab rather than re-read each event: the
/// extent shifts under a live drag — a background fetch grows a list, a wrapped view
/// re-measures — and re-reading would make the thumb wander under a pointer that had
/// not moved. [`PaneResize`] stores the divider it grabbed for the same reason.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollDrag {
    /// What the drag scrolls.
    pub(crate) surface: ScrollSurface,
    /// The track as it was when the thumb was grabbed.
    pub(crate) track: ScrollTrack,
    /// How far along the track the pointer was at the grab.
    pub(crate) origin_cell: u16,
    /// The scroll position at that moment — the anchor a motionless press returns to
    /// exactly, instead of lurching by however many lines a cell stands for.
    pub(crate) origin_position: usize,
}

/// A clickable toast card, recorded during the last render for click hit-testing.
#[derive(Clone, Copy)]
pub(crate) struct ToastHit {
    /// The card rectangle (a click anywhere on it dismisses the notification).
    pub(crate) rect: Rect,
    /// The notification the card shows.
    pub(crate) id: NotificationId,
}

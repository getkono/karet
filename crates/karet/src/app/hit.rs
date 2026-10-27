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
pub(crate) use karet_widgets::breadcrumbs::BreadcrumbTarget;

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

/// A clickable directory row of a commit or compare view's changed-file index.
///
/// Carries the path rather than a row index because the fold it toggles outlives
/// the frame: the rows are rebuilt every paint, and an index would name a
/// different directory the moment anything above it folds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitDirHit {
    /// The rendered row in screen coordinates.
    pub(crate) rect: Rect,
    /// The directory the row folds — a compacted chain's deepest directory.
    pub(crate) path: PathBuf,
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
    /// The language-server badge's columns in the breadcrumb row, when painted.
    ///
    /// Recorded for every pane, not just the focused one: the badge reports the
    /// pane's own file, so clicking a background pane's badge has to work without
    /// focusing it first.
    pub(crate) lsp_badge_hit: Option<(u16, u16)>,
    /// The pane's content (editor) area.
    pub(crate) content_rect: Rect,
    /// The exact editable editor viewport within the content area.
    pub(crate) editor_rect: Rect,
    /// The Markdown preview beside a code tab, so a wheel notch over it scrolls the
    /// preview rather than the source. Zero-sized when no preview is on screen.
    pub(crate) markdown_preview_rect: Rect,
    /// Changed-file rows clickable within the pane's commit-like view.
    pub(crate) commit_file_hits: Vec<CommitFileHit>,
    /// Directory rows of that view's changed-file index, and the folds they toggle.
    pub(crate) commit_dir_hits: Vec<CommitDirHit>,
    /// The wide commit layout's file rail, so a wheel notch over it scrolls the rail
    /// rather than the document behind it. Zero-sized when no rail is on screen.
    pub(crate) commit_rail_rect: Rect,
    /// File-card disclosure controls clickable within the pane's commit-like view.
    pub(crate) commit_collapse_hits: Vec<CommitCollapseHit>,
    /// Read-only surfaces in this pane whose rows the pointer can select.
    pub(crate) select_regions: Vec<super::select::SelectRegion>,
    /// Visible committed-attribution text, for click routing.
    pub(crate) blame_rect: Option<Rect>,
    /// The commit view's signature badge, for double-click hit-testing. `None`
    /// when no badge is on screen.
    pub(crate) commit_badge_rect: Option<Rect>,
    /// The rect reserved for a Kitty image placement, if any.
    pub(crate) image_area: Option<Rect>,
    /// Where the find-in-file bar's editable fields landed (focused pane only).
    pub(crate) find_rects: FindBarRects,
    /// Visible Markdown link runs. A background pane's links are as live as the
    /// focused pane's: the Ctrl+click that follows one focuses the pane first.
    pub(crate) markdown_link_hits: Vec<MarkdownLinkHit>,
}

#[cfg(test)]
impl PaneFrame {
    /// A frame for `pane` that recorded nothing, for tests to fill in.
    pub(crate) fn empty(pane: PaneId) -> Self {
        Self {
            pane,
            tabstrip_rect: Rect::default(),
            tab_hits: Vec::new(),
            action_hits: Vec::new(),
            breadcrumb_rect: Rect::default(),
            breadcrumb_hits: Vec::new(),
            lsp_badge_hit: None,
            content_rect: Rect::default(),
            editor_rect: Rect::default(),
            markdown_preview_rect: Rect::default(),
            commit_file_hits: Vec::new(),
            commit_dir_hits: Vec::new(),
            commit_rail_rect: Rect::default(),
            commit_collapse_hits: Vec::new(),
            select_regions: Vec::new(),
            blame_rect: None,
            commit_badge_rect: None,
            image_area: None,
            find_rects: FindBarRects::default(),
            markdown_link_hits: Vec::new(),
        }
    }
}

/// Where the find bar's two editable fields landed, for pointer hit-testing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FindBarRects {
    /// The query field.
    pub(crate) query: Rect,
    /// The replacement field, when the replace row is shown.
    pub(crate) replace: Option<Rect>,
}

/// Every hit region and rect the last frame recorded for the mouse.
///
/// [`ui::draw`](crate::ui::draw) builds one locally and assigns it to
/// [`App::last_frame`] once, at the end of the frame, so no region can outlive
/// the frame that painted it and none is reset piecemeal. A surface the frame did
/// not paint (a hidden sidebar, the panes under the GitHub view, the chrome rows
/// in Zen Mode) records nothing and so claims no clicks.
#[derive(Clone, Default)]
pub(crate) struct FrameHits {
    /// The chrome row carrying the top-level view switcher.
    pub(crate) view_chrome_rect: Rect,
    /// Column spans of the view-switcher buttons on the chrome row.
    pub(crate) view_hits: Vec<(u16, u16, View)>,
    /// The sidebar, header included.
    pub(crate) sidebar_rect: Rect,
    /// The column of the sidebar's drag divider; `0` with no sidebar shown.
    pub(crate) sidebar_divider_x: u16,
    /// The sidebar's content area below its header.
    pub(crate) sidebar_content_rect: Rect,
    /// The header panel-switcher cells.
    pub(crate) panel_hits: Vec<(u16, u16, SidebarPanel)>,
    /// The explorer header toolbar-button cells `(start, end, command)` (new
    /// file / new folder / refresh / collapse all).
    pub(crate) header_action_hits: Vec<(u16, u16, Command)>,
    /// The main content area: the panes, or the view drawn in their place.
    pub(crate) main_rect: Rect,
    /// The pane that held focus when the frame was painted.
    pub(crate) focused_pane: Option<PaneId>,
    /// Each painted pane's clickable regions.
    pub(crate) panes: Vec<PaneFrame>,
    /// Draggable split boundaries.
    pub(crate) pane_dividers: Vec<PaneDivider>,
    /// The status bar.
    pub(crate) status_rect: Rect,
    /// Clickable status-bar segments `(start, end, command)`.
    pub(crate) status_hits: Vec<(u16, u16, Command)>,
    /// Clickable toast cards.
    pub(crate) toast_hits: Vec<ToastHit>,
    /// Every scrollbar track painted, in draw order.
    pub(crate) scroll: ScrollHits,
}

impl FrameHits {
    /// The frame of the pane that held focus when this frame was painted.
    #[must_use]
    pub(crate) fn focused_frame(&self) -> Option<&PaneFrame> {
        let focused = self.focused_pane?;
        self.panes.iter().find(|frame| frame.pane == focused)
    }

    /// The focused pane's exact editable editor viewport; zero-sized when no
    /// editor is on screen.
    #[must_use]
    pub(crate) fn editor_rect(&self) -> Rect {
        self.focused_frame()
            .map_or_else(Rect::default, |frame| frame.editor_rect)
    }

    /// The focused pane's in-editor Markdown preview; zero-sized when none is shown.
    /// The mouse reads every pane's preview through [`PaneFrame`]; tests ask for
    /// the focused one.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn markdown_preview_rect(&self) -> Rect {
        self.focused_frame()
            .map_or_else(Rect::default, |frame| frame.markdown_preview_rect)
    }

    /// The focused pane's visible committed-attribution text.
    #[must_use]
    pub(crate) fn blame_rect(&self) -> Option<Rect> {
        self.focused_frame().and_then(|frame| frame.blame_rect)
    }

    /// The focused commit view's signature badge.
    #[must_use]
    pub(crate) fn commit_badge_rect(&self) -> Option<Rect> {
        self.focused_frame()
            .and_then(|frame| frame.commit_badge_rect)
    }

    /// The focused pane's Kitty image placement rect.
    #[must_use]
    pub(crate) fn image_area(&self) -> Option<Rect> {
        self.focused_frame().and_then(|frame| frame.image_area)
    }

    /// Where the focused pane's find bar fields landed.
    #[must_use]
    pub(crate) fn find_rects(&self) -> FindBarRects {
        self.focused_frame()
            .map_or_else(FindBarRects::default, |frame| frame.find_rects)
    }

    /// Every visible Markdown link run, in every pane.
    pub(crate) fn markdown_link_hits(&self) -> impl Iterator<Item = &MarkdownLinkHit> {
        self.panes
            .iter()
            .flat_map(|frame| frame.markdown_link_hits.iter())
    }

    /// The focused pane's frame, created empty when the last frame painted none,
    /// so a test can place geometry without drawing.
    #[cfg(test)]
    pub(crate) fn focused_frame_mut(&mut self, focused: PaneId) -> &mut PaneFrame {
        self.focused_pane = Some(focused);
        let index = match self.panes.iter().position(|frame| frame.pane == focused) {
            Some(index) => index,
            None => {
                self.panes.push(PaneFrame::empty(focused));
                self.panes.len() - 1
            },
        };
        &mut self.panes[index]
    }
}

#[cfg(test)]
impl App {
    /// The focused pane's last-frame geometry, created empty when no frame has
    /// been painted, so a test can place a rect without drawing.
    pub(crate) fn focused_frame_mut(&mut self) -> &mut PaneFrame {
        let focused = self.focus_pane();
        self.last_frame.focused_frame_mut(focused)
    }
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
    /// A GitHub page's own content. Distinct from [`TabRows`](Self::TabRows) because
    /// the GitHub surface is not a tab: routing it through the focused tab would
    /// scroll whatever document happens to sit behind it.
    GithubPage,
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
    /// The Problems results list.
    ProblemResults,
    /// The Hierarchy panel's tree.
    HierarchyResults,
    /// The Debug panel's section list.
    DebugResults,
    /// A commit or compare view's changed-file rail, which scrolls independently of
    /// the diff beside it so a long file list can be read ahead of the card on screen.
    CommitFileRail,
    /// The Source-Control changes list.
    ScmChanges,
    /// The Source-Control commit log.
    ScmCommits,
    /// The Source-Control commit-message box.
    ScmCommitInput,
    /// The running commit's console log.
    CommitConsole,
    /// The running commit's console log, horizontally.
    CommitConsoleColumns,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame for `pane` whose editor and blame label sit at `x`.
    fn pane_at(pane: u64, x: u16) -> PaneFrame {
        PaneFrame {
            editor_rect: Rect::new(x, 1, 10, 5),
            blame_rect: Some(Rect::new(x, 2, 4, 1)),
            markdown_link_hits: vec![MarkdownLinkHit {
                rect: Rect::new(x, 3, 2, 1),
                target: format!("pane-{pane}.md"),
            }],
            ..PaneFrame::empty(PaneId(pane))
        }
    }

    #[test]
    fn the_focused_accessors_read_the_pane_focused_at_paint_time() {
        let hits = FrameHits {
            focused_pane: Some(PaneId(2)),
            panes: vec![pane_at(1, 0), pane_at(2, 20)],
            ..FrameHits::default()
        };
        assert_eq!(
            hits.focused_frame().map(|frame| frame.pane),
            Some(PaneId(2))
        );
        assert_eq!(hits.editor_rect(), Rect::new(20, 1, 10, 5));
        assert_eq!(hits.blame_rect(), Some(Rect::new(20, 2, 4, 1)));
    }

    #[test]
    fn a_frame_that_painted_no_focused_pane_claims_no_focused_geometry() {
        // Under another view no pane is painted, and a focused pane that was not
        // drawn (the frame holds only another one) answers nothing either.
        for hits in [
            FrameHits {
                focused_pane: Some(PaneId(1)),
                ..FrameHits::default()
            },
            FrameHits {
                focused_pane: Some(PaneId(3)),
                panes: vec![pane_at(1, 0)],
                ..FrameHits::default()
            },
        ] {
            assert!(hits.focused_frame().is_none());
            assert_eq!(hits.editor_rect(), Rect::default());
            assert_eq!(hits.blame_rect(), None);
            assert_eq!(hits.commit_badge_rect(), None);
            assert_eq!(hits.image_area(), None);
            assert_eq!(hits.find_rects().query, Rect::default());
        }
    }

    #[test]
    fn markdown_links_are_live_in_every_pane() {
        let hits = FrameHits {
            focused_pane: Some(PaneId(1)),
            panes: vec![pane_at(1, 0), pane_at(2, 20)],
            ..FrameHits::default()
        };
        let targets: Vec<_> = hits
            .markdown_link_hits()
            .map(|hit| hit.target.as_str())
            .collect();
        assert_eq!(targets, vec!["pane-1.md", "pane-2.md"]);
    }
}

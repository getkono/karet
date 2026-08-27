//! The diff tab's painter: unified and side-by-side columns.
//!
//! Split out of [`super`] because the pane painter sits at the repo's per-file
//! code-line ceiling, and this is the one arm large enough to stand alone.

use super::*;

#[allow(clippy::too_many_arguments)] // diff model, layout mode, scroll offsets and the track sink are independent
pub(in crate::ui) fn draw_diff(
    f: &mut Frame,
    ctx: &PaneCtx,
    area: Rect,
    file: &render::FileView,
    view: ViewMode,
    scroll: &mut u16,
    column: &mut u16,
    hits: &mut ScrollHits,
) -> Vec<crate::app::SelectRegion> {
    let theme = ctx.theme;
    match view {
        ViewMode::Unified => {
            let mut lines = render::unified_lines(file, theme);
            render::pad_diff_lines(&mut lines, area.width);
            hits.record_both(
                draw_scrollable_lines(f, theme, area, lines, scroll, column),
                ScrollSurface::TabRows,
                ScrollSurface::TabColumns,
            );
            select::unified(f, theme, ctx.selection, area, file, *scroll, *column)
        },
        ViewMode::SideBySide => {
            // The panes scroll together, so the view carries one vertical track for
            // the pair — but each pane has its own content width, so the horizontal
            // track is split to match the panes above it.
            let (body, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
            let (mut left, mut right) = render::side_by_side_lines(file, theme);
            let height = left.len().max(right.len());
            let max = u16::try_from(height)
                .unwrap_or(u16::MAX)
                .saturating_sub(body.height);
            *scroll = (*scroll).min(max);
            let constraints = [
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Min(0),
            ];
            let panes = Layout::horizontal(constraints).split(body);
            let left_width = left.iter().map(line_width).max().unwrap_or_default();
            let right_width = right.iter().map(line_width).max().unwrap_or_default();
            let content_width = left_width.max(right_width);
            let pane_width = panes[0].width.min(panes[2].width);
            let max_column = content_width.saturating_sub(usize::from(pane_width));
            *column = (*column).min(u16::try_from(max_column).unwrap_or(u16::MAX));
            render::pad_diff_lines(&mut left, panes[0].width);
            render::pad_diff_lines(&mut right, panes[2].width);
            f.render_widget(Paragraph::new(left).scroll((*scroll, *column)), panes[0]);
            f.render_widget(Block::new().borders(Borders::LEFT), panes[1]);
            f.render_widget(Paragraph::new(right).scroll((*scroll, *column)), panes[2]);
            let styles = ScrollbarStyles::from_theme(theme);
            if let Some(track) = tracks.vertical {
                let extent = ScrollExtent::new(height, usize::from(*scroll), body.height.into());
                f.render_widget(ScrollBar::vertical(extent, styles), track);
                hits.record_track(
                    Some(ScrollTrack::new(track, ScrollAxis::Vertical, extent)),
                    ScrollSurface::TabRows,
                );
            }
            if let Some(track) = tracks.horizontal {
                let halves = Layout::horizontal(constraints).split(track);
                let offset = usize::from(*column);
                for (half, width, pane) in [
                    (halves[0], left_width, panes[0]),
                    (halves[2], right_width, panes[2]),
                ] {
                    let extent = ScrollExtent::new(width, offset, pane.width.into());
                    f.render_widget(ScrollBar::horizontal(extent, styles), half);
                    // Both halves drive the one shared column offset, each at its own
                    // pane's scale — so a drag on either is measured against the text
                    // actually under it.
                    hits.record_track(
                        Some(ScrollTrack::new(half, ScrollAxis::Horizontal, extent)),
                        ScrollSurface::TabColumns,
                    );
                }
            }
            select::side_by_side(
                f,
                theme,
                ctx.selection,
                (panes[0], panes[2]),
                file,
                *scroll,
                *column,
            )
        },
    }
}

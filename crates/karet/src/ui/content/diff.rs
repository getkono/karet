//! Painting a backend-prepared diff: the diff tab's unified and side-by-side
//! columns, and the theme-keyed unified rows the commit and compare cards
//! reuse.
//!
//! Diffing, syntax highlighting, and intra-line pairing all happen in the
//! session backend (off the UI thread); this module only maps the theme onto
//! `karet-diff`'s `view`-feature painters and caches the unified rows on the
//! [`ChangedFile`] per theme snapshot.

use karet_core::TokenId;
use karet_diff::view::DiffPalette;
use karet_theme::Rgba;

use super::*;
use crate::tab::ChangedFile;

#[allow(clippy::too_many_arguments)] // diff model, layout mode, scroll offsets and the track sink are independent
pub(in crate::ui) fn draw_diff(
    f: &mut Frame,
    ctx: &PaneCtx,
    area: Rect,
    file: &ChangedFile,
    view: ViewMode,
    scroll: &mut u16,
    column: &mut u16,
    hits: &mut ScrollHits,
) -> Vec<crate::app::SelectRegion> {
    let theme = ctx.theme;
    match view {
        ViewMode::Unified => {
            let mut lines = unified_lines(file, theme);
            karet_diff::view::pad_diff_lines(&mut lines, area.width);
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
            let (mut left, mut right) = side_by_side_lines(file, theme);
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
            karet_diff::view::pad_diff_lines(&mut left, panes[0].width);
            karet_diff::view::pad_diff_lines(&mut right, panes[2].width);
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

/// Map `theme` onto the diff painter's color slots and run `paint` with it.
fn with_palette<R>(theme: &Theme, paint: impl FnOnce(&DiffPalette<'_>) -> R) -> R {
    let token_fg = |token: TokenId| theme.color(token).to_ratatui();
    let role = |role: karet_core::ThemeRole| theme.role(role).to_ratatui();
    let palette = DiffPalette {
        foreground: role(karet_core::ThemeRole::Foreground),
        added_bg: role(karet_core::ThemeRole::DiffAdded),
        added_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::DiffAdded)).to_ratatui(),
        removed_bg: role(karet_core::ThemeRole::DiffRemoved),
        removed_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::DiffRemoved)).to_ratatui(),
        plain_emphasis_bg: brighten(theme.role(karet_core::ThemeRole::Selection)).to_ratatui(),
        add_marker: role(karet_core::ThemeRole::DiagnosticHint),
        remove_marker: role(karet_core::ThemeRole::DiagnosticError),
        gutter: role(karet_core::ThemeRole::LineNumber),
        header: role(karet_core::ThemeRole::DiagnosticInfo),
        dim: role(karet_core::ThemeRole::LineNumberActive),
        token_fg: &token_fg,
    };
    paint(&palette)
}

/// The brighter variant of a base color used for intra-line change emphasis.
fn brighten(c: Rgba) -> Rgba {
    Rgba {
        r: c.r.saturating_add(0x24),
        g: c.g.saturating_add(0x24),
        b: c.b.saturating_add(0x2c),
        a: c.a,
    }
}

/// Build (or fetch from the theme-keyed cache) the unified-view lines for `file`.
pub(in crate::ui) fn unified_lines(file: &ChangedFile, theme: &Theme) -> Vec<Line<'static>> {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |(_, lines)| lines.clone())
}

/// Return the number of rows in the unified rendering without cloning those rows.
pub(in crate::ui) fn unified_line_count(file: &ChangedFile, theme: &Theme) -> usize {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or(0, |(_, lines)| lines.len())
}

/// Return the widest unified diff row in terminal columns without cloning rows.
pub(in crate::ui) fn unified_max_width(file: &ChangedFile, theme: &Theme) -> usize {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or(0, |(_, lines)| {
            lines.iter().map(crate::ui::line_width).max().unwrap_or(0)
        })
}

/// Clone only a requested window of the unified rendering.
pub(in crate::ui) fn unified_lines_window(
    file: &ChangedFile,
    theme: &Theme,
    start: usize,
    count: usize,
) -> Vec<Line<'static>> {
    ensure_unified(file, theme);
    file.unified_cache
        .borrow()
        .as_ref()
        .map_or_else(Vec::new, |(_, lines)| {
            lines.iter().skip(start).take(count).cloned().collect()
        })
}

/// Build the side-by-side lines for `file` as aligned `(old, new)` columns.
pub(in crate::ui) fn side_by_side_lines(
    file: &ChangedFile,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    with_palette(theme, |palette| {
        karet_diff::view::side_by_side_lines(&file.change.diff, palette)
    })
}

fn ensure_unified(file: &ChangedFile, theme: &Theme) {
    if file
        .unified_cache
        .borrow()
        .as_ref()
        .is_some_and(|(cached_theme, _)| cached_theme == theme)
    {
        return;
    }
    let lines = with_palette(theme, |palette| {
        karet_diff::view::unified_lines(&file.change.diff, palette)
    });
    *file.unified_cache.borrow_mut() = Some((theme.clone(), lines));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::test_changed_file;

    fn rendered_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn unified_lines_render_both_sides() {
        let fv = test_changed_file("notes.txt", "before\n", "after\n");
        let text = rendered_text(&unified_lines(&fv, &Theme::dark()));
        assert!(text.contains("before") && text.contains("after"));
    }

    #[test]
    fn unified_max_width_matches_the_rendered_rows() {
        let fv = test_changed_file("notes.txt", "short\n", "a much longer replacement\n");
        let theme = Theme::dark();
        let expected = unified_lines(&fv, &theme)
            .iter()
            .map(crate::ui::line_width)
            .max()
            .unwrap_or_default();
        assert_eq!(unified_max_width(&fv, &theme), expected);
    }

    #[test]
    fn unified_cache_returns_stable_rows() {
        let fv = test_changed_file("notes.txt", "a\n", "b\n");
        let theme = Theme::dark();
        let first = unified_lines(&fv, &theme);
        assert_eq!(unified_line_count(&fv, &theme), first.len());
        assert_eq!(unified_lines(&fv, &theme), first);
    }

    #[test]
    fn side_by_side_columns_stay_aligned() {
        let fv = test_changed_file("x.rs", "a\nb\nc\n", "a\nB\nc\n");
        let (left, right) = side_by_side_lines(&fv, &Theme::dark());
        assert_eq!(left.len(), right.len());
        assert!(!left.is_empty());
    }
}

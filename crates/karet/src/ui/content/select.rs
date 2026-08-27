//! Painting the pointer selection over a read-only surface's rows.
//!
//! The rows are already on screen by the time this runs: a surface renders
//! normally, then the selection is laid over the cells as a background, the same
//! post-render overlay the markdown preview uses for hover underlines and OSC 8
//! links. Nothing here owns selection state — it only draws what the app holds,
//! and reports back where the rows landed so the next click can hit-test them.

use karet_core::ThemeRole;
use karet_theme::Theme;
use karet_widgets::RowGeometry;
use karet_widgets::scroll::ScrollAxes;
use karet_widgets::scroll::reserve_tracks;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::SelectRegion;
use crate::app::SelectSurface;
use crate::app::SurfaceSelection;
use crate::app::select::SurfaceRow;
use crate::app::select::diff_row;
use crate::app::select::hex_row;
use crate::app::select::markdown_row;
use crate::render::FileView;

/// The rect `draw_scrollable_lines` paints rows into for `area`.
///
/// It reserves its own tracks, so the rect it paints into is narrower than the
/// area handed to it; asking [`reserve_tracks`] the same question is what keeps
/// the recorded region and the painted rows in agreement.
pub(super) fn scrollable_text_rect(area: Rect) -> Rect {
    reserve_tracks(area, ScrollAxes::BOTH).0
}

/// Record and paint the unified diff column's selectable rows.
pub(super) fn unified(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    area: Rect,
    file: &FileView,
    scroll: u16,
    column: u16,
) -> Vec<SelectRegion> {
    let region = SelectRegion {
        surface: SelectSurface::Unified,
        area: scrollable_text_rect(area),
        first_row: usize::from(scroll),
        hscroll: usize::from(column),
    };
    paint(f, theme, selection, &region, file);
    vec![region]
}

/// Record and paint both columns of the side-by-side diff.
///
/// The two panes are independent surfaces: a drag started in one never bleeds
/// into the other, which is what makes copying one side of a change possible.
pub(super) fn side_by_side(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    panes: (Rect, Rect),
    file: &FileView,
    scroll: u16,
    column: u16,
) -> Vec<SelectRegion> {
    [
        (SelectSurface::OldColumn, panes.0),
        (SelectSurface::NewColumn, panes.1),
    ]
    .into_iter()
    .map(|(surface, area)| {
        let region = SelectRegion {
            surface,
            area,
            first_row: usize::from(scroll),
            hscroll: usize::from(column),
        };
        paint(f, theme, selection, &region, file);
        region
    })
    .collect()
}

/// Record and paint the hex dump's selectable rows.
pub(super) fn hex(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    area: Rect,
    bytes: &[u8],
    scroll: usize,
) -> Vec<SelectRegion> {
    let region = SelectRegion {
        surface: SelectSurface::Hex,
        area,
        first_row: scroll,
        hscroll: 0,
    };
    paint_rows(f, theme, selection, &region, &|row| hex_row(bytes, row));
    vec![region]
}

/// Record and paint the markdown preview's selectable rows.
pub(in crate::ui) fn markdown(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    area: Rect,
    wrapped: &karet_markdown::WrappedDocument,
    scroll: u16,
) -> SelectRegion {
    let region = SelectRegion {
        surface: SelectSurface::MarkdownPreview,
        area,
        first_row: usize::from(scroll),
        hscroll: 0,
    };
    paint_rows(f, theme, selection, &region, &|row| {
        markdown_row(wrapped, row)
    });
    region
}

/// Lay the selection background over `region`'s visible rows.
fn paint(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    region: &SelectRegion,
    file: &FileView,
) {
    paint_rows(f, theme, selection, region, &|row| {
        diff_row(file, region.surface, row)
    });
}

/// Lay the selection background over `region`'s visible rows, asking `row_text`
/// for the content of each.
fn paint_rows(
    f: &mut Frame,
    theme: &Theme,
    selection: Option<SurfaceSelection>,
    region: &SelectRegion,
    row_text: &dyn Fn(usize) -> Option<SurfaceRow>,
) {
    let Some(active) = selection.filter(|active| active.surface == region.surface) else {
        return;
    };
    if active.selection.is_empty() {
        return;
    }
    let bg = theme.role(ThemeRole::Selection).to_ratatui();
    for offset in 0..region.area.height {
        let row = region.first_row.saturating_add(usize::from(offset));
        let Some(painted) = row_text(row) else {
            continue;
        };
        let Some(span) = active.selection.row_span(row, painted.text.len()) else {
            continue;
        };
        let geometry = RowGeometry::new(region.area, painted.content_x).hscroll(region.hscroll);
        karet_widgets::rowselect::paint_row(
            f.buffer_mut(),
            &geometry,
            region.area.y.saturating_add(offset),
            &painted.text,
            &span,
            bg,
        );
    }
}

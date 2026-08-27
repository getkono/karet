//! Pointer text selection on the read-only surfaces.
//!
//! The editor selects over a document model, because it is editable and the
//! selection has to survive edits. The read-only surfaces have no such model:
//! they paint rows they assemble themselves. They share this one instead — a
//! [`RowSelection`] over absolute row indices, plus the last-frame geometry
//! saying where those rows landed on screen.
//!
//! Row text is never retained between frames. A surface reports the copyable
//! text of a row on demand through [`App::surface_row`], which is what lets a
//! selection cover rows that have since scrolled out of view, and what keeps a
//! diff's line-number gutter and `+`/`-` marker out of the clipboard.

use karet_widgets::RowGeometry;
use karet_widgets::RowPos;
use karet_widgets::RowSelection;
use ratatui::layout::Rect;

use super::App;
use super::util::rect_contains;
use crate::render::FileView;
use crate::tab::TabKind;
use crate::tab::ViewMode;

/// A read-only surface whose rows can be selected with the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectSurface {
    /// The diff tab's unified column.
    Unified,
    /// The old (left) column of the side-by-side diff.
    OldColumn,
    /// The new (right) column of the side-by-side diff.
    NewColumn,
    /// The hex dump's byte and ASCII columns.
    Hex,
}

/// Where a selectable surface painted its rows last frame.
///
/// Recorded per pane, the way every other hit-testable rect is, so a click can
/// be routed to a surface in a pane that does not hold the focus yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectRegion {
    /// Which surface these rows belong to.
    pub(crate) surface: SelectSurface,
    /// The rect the surface actually painted its rows into, scrollbar tracks
    /// already subtracted.
    pub(crate) area: Rect,
    /// The absolute row index painted at `area.y`.
    pub(crate) first_row: usize,
    /// Display columns of row text scrolled off the left edge.
    pub(crate) hscroll: usize,
}

/// A live pointer selection on one surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceSelection {
    /// The surface the drag began on; a drag never leaves it.
    pub(crate) surface: SelectSurface,
    /// The selected span, in absolute rows and byte offsets.
    pub(crate) selection: RowSelection,
}

/// The copyable text of one row, and how wide the chrome before it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceRow {
    /// The row's text, without the gutter or marker painted ahead of it.
    pub(crate) text: String,
    /// Columns of chrome between the region's left edge and that text.
    pub(crate) content_x: u16,
}

/// The copyable content of `surface`'s row `row` of `file`.
///
/// A row's gutter width is not constant across a file — a line number past four
/// digits widens it — so it travels with the text rather than being assumed.
/// The copyable content of hex row `row` of `bytes`.
pub(crate) fn hex_row(bytes: &[u8], row: usize) -> Option<SurfaceRow> {
    Some(SurfaceRow {
        text: karet_fileview::hex::row_text(bytes, row)?,
        content_x: karet_fileview::hex::OFFSET_WIDTH,
    })
}

pub(crate) fn diff_row(file: &FileView, surface: SelectSurface, row: usize) -> Option<SurfaceRow> {
    let content = match surface {
        SelectSurface::Unified => karet_diff::unified_row(&file.change.diff, row),
        SelectSurface::OldColumn => karet_diff::side_by_side_row(&file.change.diff, row).0,
        SelectSurface::NewColumn => karet_diff::side_by_side_row(&file.change.diff, row).1,
        SelectSurface::Hex => None,
    }?;
    Some(SurfaceRow {
        text: content.text,
        content_x: content.gutter_width,
    })
}

impl App {
    /// The copyable content of `surface`'s row `row` in the active tab.
    pub(crate) fn surface_row(&self, surface: SelectSurface, row: usize) -> Option<SurfaceRow> {
        let kind = &self.tabs.get(self.active)?.kind;
        if surface == SelectSurface::Hex {
            let TabKind::Hex { bytes, .. } = kind else {
                return None;
            };
            return hex_row(bytes, row);
        }
        let TabKind::Diff {
            file: Some(file),
            view,
            ..
        } = kind
        else {
            return None;
        };
        // A surface only answers in the view mode that paints it, so a stale
        // selection from the other mode cannot resolve against these rows.
        let painted = matches!(
            (view, surface),
            (ViewMode::Unified, SelectSurface::Unified)
                | (
                    ViewMode::SideBySide,
                    SelectSurface::OldColumn | SelectSurface::NewColumn
                )
        );
        painted.then(|| diff_row(file, surface, row)).flatten()
    }

    /// The selectable region under `(col, row)`, with the pane holding it.
    fn select_region_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<(karet_widgets::PaneId, SelectRegion)> {
        self.pane_frames.iter().find_map(|frame| {
            frame
                .select_regions
                .iter()
                .find(|region| rect_contains(region.area, (col, row)))
                .map(|region| (frame.pane, *region))
        })
    }

    /// The focused pane's recorded region for `surface`.
    pub(crate) fn select_region(&self, surface: SelectSurface) -> Option<SelectRegion> {
        let focused = self.focus_pane();
        self.pane_frames
            .iter()
            .find(|frame| frame.pane == focused)?
            .select_regions
            .iter()
            .find(|region| region.surface == surface)
            .copied()
    }

    /// Resolve a screen cell to a position in `region`'s surface.
    ///
    /// A cell above or below the painted rows clamps onto the nearest visible
    /// one, so a drag that leaves the surface keeps extending along its edge.
    fn surface_pos_at(&self, region: &SelectRegion, col: u16, row: u16) -> RowPos {
        let last = usize::from(region.area.height.saturating_sub(1));
        let offset = usize::from(row.saturating_sub(region.area.y)).min(last);
        let index = region.first_row.saturating_add(offset);
        let Some(painted) = self.surface_row(region.surface, index) else {
            return RowPos::new(index, 0);
        };
        let geometry = RowGeometry::new(region.area, painted.content_x).hscroll(region.hscroll);
        RowPos::new(index, geometry.byte_at(&painted.text, col))
    }

    /// Begin a pointer selection at `(col, row)`; whether one started there.
    pub(super) fn begin_surface_selection(&mut self, col: u16, row: u16) -> bool {
        let Some((pane, region)) = self.select_region_at(col, row) else {
            self.surface_selection = None;
            return false;
        };
        self.focus_pane_switch(pane);
        self.focus = super::Focus::Editor;
        let at = self.surface_pos_at(&region, col, row);
        self.surface_selection = Some(SurfaceSelection {
            surface: region.surface,
            selection: RowSelection::new(at),
        });
        self.surface_selecting = Some(region.surface);
        true
    }

    /// Extend the live selection to `(col, row)` while the button is held.
    pub(super) fn drag_surface_selection(&mut self, col: u16, row: u16) {
        let Some(surface) = self.surface_selecting else {
            return;
        };
        let Some(region) = self.select_region(surface) else {
            return;
        };
        let to = self.surface_pos_at(&region, col, row);
        if let Some(active) = self.surface_selection.as_mut() {
            active.selection.extend_to(to);
        }
    }

    /// The text of the live surface selection, if one covers anything.
    ///
    /// Rows are re-derived here rather than remembered, so a selection dragged
    /// past the viewport copies the rows that scrolled out of sight too.
    pub(super) fn surface_selection_text(&self) -> Option<String> {
        let active = self.surface_selection.as_ref()?;
        if active.selection.is_empty() {
            return None;
        }
        let (start, end) = active.selection.bounds();
        let rows: Vec<String> = (start.row..=end.row)
            .map(|row| {
                self.surface_row(active.surface, row)
                    .map(|painted| painted.text)
                    .unwrap_or_default()
            })
            .collect();
        Some(active.selection.text(start.row, &rows))
    }
}

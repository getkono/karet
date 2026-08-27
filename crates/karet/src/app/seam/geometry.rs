//! What the last frame put where, so a pointer can be resolved back to a thing.
//!
//! Rebuilt by the renderer every frame and read by the mouse handler on the next event,
//! the way every other last-frame hit map in the shell works.
//!
//! Spine rows are keyed on node identity rather than on a `(column, row)` pair, because
//! that is what the rest of this view is keyed on: a rename costs one node its place and
//! nothing else. It also means the renderer records what it painted instead of the mouse
//! re-deriving it, so the cascading spine and the narrow-terminal tree need no separate
//! coordinate translations to disagree about.

use ratatui::layout::Rect;

use crate::app::util::rect_contains;

/// Where the last frame painted everything clickable.
#[derive(Clone, Debug, Default)]
pub(crate) struct SeamHits {
    /// The whole view — a press outside it is not this view's business.
    pub(crate) area: Rect,
    /// The header row.
    pub(crate) header: Rect,
    /// The spine, placeholder included.
    pub(crate) spine: Rect,
    /// The facet pane; zero-height when there is no selection.
    pub(crate) facets: Rect,
    /// The query line.
    pub(crate) query: Rect,
    /// Breadcrumb crumbs, each with the narrow depth clicking it widens back to.
    pub(crate) crumbs: Vec<(Rect, usize)>,
    /// The configuration marker.
    pub(crate) config: Rect,
    /// Legend entries, by lens index.
    pub(crate) lenses: Vec<(Rect, usize)>,
    /// The widen affordance on the query line.
    pub(crate) widen: Rect,
    /// Every painted spine row, by node identity.
    pub(crate) rows: Vec<(Rect, String)>,
    /// Facet-pane edge rows, by index into the selection's edges.
    pub(crate) edges: Vec<(Rect, usize)>,
}

/// What the pointer is over.
///
/// One resolution serves the click handler, the hover highlight and the pointer-shape
/// hint, so the three can never disagree about what is clickable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SeamTarget {
    /// A breadcrumb crumb: widen back to this narrow depth.
    Crumb(usize),
    /// The configuration marker.
    Configuration,
    /// A legend entry, by lens index.
    Lens(usize),
    /// A spine row, by node identity.
    Row(String),
    /// Somewhere on the spine that is not a row.
    Spine,
    /// A facet-pane edge row, by index.
    Edge(usize),
    /// The facet pane, off any edge row.
    Facets,
    /// The query line.
    Query,
    /// The widen affordance.
    Widen,
}

impl SeamHits {
    /// Start a frame: forget last frame's geometry, keeping the allocations.
    pub(crate) fn reset(&mut self, area: Rect) {
        self.area = area;
        self.header = Rect::default();
        self.spine = Rect::default();
        self.facets = Rect::default();
        self.query = Rect::default();
        self.config = Rect::default();
        self.widen = Rect::default();
        self.crumbs.clear();
        self.lenses.clear();
        self.rows.clear();
        self.edges.clear();
    }

    /// What is at `(x, y)`, if anything.
    ///
    /// Small targets are tested before the regions they sit inside; everything else is
    /// disjoint by construction.
    #[must_use]
    pub(crate) fn at(&self, x: u16, y: u16) -> Option<SeamTarget> {
        let point = (x, y);
        if !rect_contains(self.area, point) {
            return None;
        }
        if rect_contains(self.widen, point) {
            return Some(SeamTarget::Widen);
        }
        if let Some((_, depth)) = self
            .crumbs
            .iter()
            .find(|(rect, _)| rect_contains(*rect, point))
        {
            return Some(SeamTarget::Crumb(*depth));
        }
        if let Some((_, lens)) = self
            .lenses
            .iter()
            .find(|(rect, _)| rect_contains(*rect, point))
        {
            return Some(SeamTarget::Lens(*lens));
        }
        if rect_contains(self.config, point) {
            return Some(SeamTarget::Configuration);
        }
        if let Some((_, id)) = self
            .rows
            .iter()
            .find(|(rect, _)| rect_contains(*rect, point))
        {
            return Some(SeamTarget::Row(id.clone()));
        }
        if rect_contains(self.spine, point) {
            return Some(SeamTarget::Spine);
        }
        if let Some((_, edge)) = self
            .edges
            .iter()
            .find(|(rect, _)| rect_contains(*rect, point))
        {
            return Some(SeamTarget::Edge(*edge));
        }
        if rect_contains(self.facets, point) {
            return Some(SeamTarget::Facets);
        }
        if rect_contains(self.query, point) {
            return Some(SeamTarget::Query);
        }
        None
    }
}

/// The rect a run of `width` cells occupies at `(x, y)`, clipped to `area`.
///
/// Nothing is clickable where nothing was painted: a header run pushed past the right
/// edge by a long package name yields a zero-width rect, which no point is ever inside.
#[must_use]
pub(crate) fn span_rect(area: Rect, x: u16, y: u16, width: usize) -> Rect {
    let right = area.x.saturating_add(area.width);
    if x >= right || width == 0 {
        return Rect::new(x.min(right), y, 0, 1);
    }
    let width = u16::try_from(width).unwrap_or(u16::MAX).min(right - x);
    Rect::new(x, y, width, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits() -> SeamHits {
        let mut hits = SeamHits::default();
        hits.reset(Rect::new(0, 0, 80, 20));
        hits.header = Rect::new(0, 0, 80, 1);
        hits.spine = Rect::new(0, 1, 80, 10);
        hits.facets = Rect::new(0, 11, 80, 8);
        hits.query = Rect::new(0, 19, 80, 1);
        hits.crumbs = vec![(Rect::new(0, 0, 6, 1), 0), (Rect::new(8, 0, 5, 1), 1)];
        hits.config = Rect::new(20, 0, 10, 1);
        hits.lenses = vec![(Rect::new(40, 0, 8, 1), 0), (Rect::new(48, 0, 8, 1), 1)];
        hits.rows = vec![(Rect::new(0, 1, 20, 1), "pkg".to_owned())];
        hits.edges = vec![(Rect::new(0, 17, 40, 1), 0)];
        hits.widen = Rect::new(60, 19, 12, 1);
        hits
    }

    #[test]
    fn a_point_outside_every_region_resolves_to_nothing() {
        assert_eq!(hits().at(200, 200), None);
    }

    #[test]
    fn the_widen_affordance_wins_over_the_query_line_it_sits_on() {
        assert_eq!(hits().at(62, 19), Some(SeamTarget::Widen));
        assert_eq!(hits().at(2, 19), Some(SeamTarget::Query));
    }

    #[test]
    fn a_crumb_resolves_to_the_depth_it_widens_back_to() {
        assert_eq!(hits().at(1, 0), Some(SeamTarget::Crumb(0)));
        assert_eq!(hits().at(9, 0), Some(SeamTarget::Crumb(1)));
    }

    #[test]
    fn each_region_resolves_to_itself() {
        let hits = hits();
        assert_eq!(hits.at(41, 0), Some(SeamTarget::Lens(0)));
        assert_eq!(hits.at(21, 0), Some(SeamTarget::Configuration));
        assert_eq!(hits.at(2, 1), Some(SeamTarget::Row("pkg".to_owned())));
        // On the spine but off any row: still the spine's business, not the editor's.
        assert_eq!(hits.at(2, 5), Some(SeamTarget::Spine));
        assert_eq!(hits.at(2, 17), Some(SeamTarget::Edge(0)));
        assert_eq!(hits.at(2, 12), Some(SeamTarget::Facets));
    }

    #[test]
    fn a_run_clipped_by_the_right_edge_claims_no_cells() {
        let area = Rect::new(0, 0, 10, 1);
        assert_eq!(span_rect(area, 2, 0, 4).width, 4);
        // Pushed off the edge by a long name: painted nowhere, so clickable nowhere.
        assert_eq!(span_rect(area, 12, 0, 4).width, 0);
        // And a run that only half fits claims only the half that was painted.
        assert_eq!(span_rect(area, 8, 0, 6).width, 2);
    }

    #[test]
    fn resetting_forgets_the_previous_frame() {
        let mut hits = hits();
        hits.reset(Rect::new(0, 0, 80, 20));
        assert!(hits.rows.is_empty());
        assert_eq!(hits.at(2, 1), None);
    }
}

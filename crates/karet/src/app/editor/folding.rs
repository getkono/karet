//! Whole-document fold commands (VS Code parity): fold or unfold everything,
//! one nesting level, a region and everything inside it, or every comment or
//! import block.
//!
//! Collapsing never leaves a caret hidden: a region that holds one either
//! lifts it onto its header (as toggling a fold does) or, for "fold level",
//! is skipped, as VS Code skips the region the cursor is in.

use karet_syntax::FoldKind;
use karet_syntax::FoldRegion;
use karet_syntax::FoldRegions;

use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

/// The region a cursor-scoped fold command acts on at `line`: the one headed
/// on that line, else the innermost one containing it.
pub(super) fn region_at(folds: &FoldRegions, line: u32) -> Option<FoldRegion> {
    let regions = folds.regions();
    regions
        .iter()
        .find(|r| r.start == line)
        .or_else(|| {
            regions
                .iter()
                .filter(|r| r.start <= line && line <= r.end)
                .min_by_key(|r| r.end - r.start)
        })
        .copied()
}

/// The nesting depth of each region of `regions` (1 for an outermost one),
/// in the same order. A region counts as inside every earlier region whose
/// lines reach its header.
fn depths(regions: &[FoldRegion]) -> Vec<u32> {
    // The last lines of the regions enclosing the current one, innermost last.
    let mut open: Vec<u32> = Vec::new();
    regions
        .iter()
        .map(|r| {
            while open.last().is_some_and(|&end| end < r.start) {
                open.pop();
            }
            open.push(r.end);
            u32::try_from(open.len()).unwrap_or(u32::MAX)
        })
        .collect()
}

/// Whether `inner` lies within `outer` (a region lies within itself).
fn nested_in(inner: &FoldRegion, outer: &FoldRegion) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

impl App {
    /// Collapse every fold region of the active code tab.
    pub(in crate::app) fn fold_all(&mut self) {
        self.collapse_where(|_, _| true);
    }

    /// Expand every collapsed fold of the active code tab.
    pub(in crate::app) fn unfold_all(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { folded, .. },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            folded.clear();
        }
    }

    /// Collapse every region at nesting depth `level` (1 is outermost),
    /// except those holding a caret, leaving every other region as it is.
    pub(in crate::app) fn fold_level(&mut self, level: u8) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let lines: Vec<u32> = tab
            .editor
            .cursors()
            .selections
            .iter()
            .map(|s| s.head.line)
            .collect();
        let level = u32::from(level);
        self.collapse_where(|region, depth| {
            depth == level
                && !lines
                    .iter()
                    .any(|l| (region.start..=region.end).contains(l))
        });
    }

    /// Collapse every comment block (`kind` [`FoldKind::Comment`]) or every
    /// import block ([`FoldKind::Imports`]).
    pub(in crate::app) fn fold_kind(&mut self, kind: FoldKind) {
        self.collapse_where(|region, _| region.kind == kind);
    }

    /// Collapse the region at the cursor (see [`region_at`]) and every region
    /// inside it.
    pub(in crate::app) fn fold_recursively(&mut self) {
        let Some(target) = self.region_at_cursor() else {
            return;
        };
        self.collapse_where(|region, _| nested_in(region, &target));
    }

    /// Expand the region at the cursor (see [`region_at`]) and every region
    /// inside it.
    pub(in crate::app) fn unfold_recursively(&mut self) {
        let Some(target) = self.region_at_cursor() else {
            return;
        };
        if let Some(Tab {
            kind: TabKind::Code { folds, folded, .. },
            ..
        }) = self.tabs.get_mut(self.active)
        {
            for region in folds.regions().iter().filter(|r| nested_in(r, &target)) {
                folded.remove(&region.start);
            }
        }
    }

    /// The region a cursor-scoped fold command acts on in the active code
    /// tab, at its primary caret's line.
    pub(super) fn region_at_cursor(&self) -> Option<FoldRegion> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { folds, .. } = &tab.kind else {
            return None;
        };
        region_at(folds, tab.editor.cursor().line)
    }

    /// Collapse every region of the active code tab that `pick` accepts,
    /// given the region and its nesting depth, then lift each caret a
    /// collapsed region hides onto that region's header, until none is
    /// hidden. Outer regions are lifted out of first, and a header that is
    /// itself hidden (a region starting on the line a collapsed one ends
    /// on, like an `else` block) is lifted out of again, so a caret lands
    /// on the outermost header that stays visible.
    fn collapse_where(&mut self, pick: impl Fn(&FoldRegion, u32) -> bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &mut tab.kind
        else {
            return;
        };
        let regions = folds.regions();
        for (region, depth) in regions.iter().zip(depths(regions)) {
            if pick(region, depth) {
                folded.insert(region.start);
            }
        }
        // Each lift moves a caret strictly up, so this ends.
        while let Some(region) = regions.iter().find(|r| {
            folded.contains(&r.start)
                && tab.editor.cursors().selections.iter().any(|s| {
                    let line = s.head.line;
                    r.start < line && line <= r.end
                })
        }) {
            tab.editor.lift_carets_onto_header(
                buffer,
                karet_editor::Fold {
                    start: region.start,
                    end: region.end,
                    collapsed: true,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions(spans: &[(u32, u32)]) -> FoldRegions {
        FoldRegions::from_sorted_regions(
            spans.iter().map(|&(s, e)| FoldRegion::new(s, e)).collect(),
        )
    }

    #[test]
    fn depths_count_the_regions_reaching_each_header() {
        let folds = regions(&[(0, 9), (1, 4), (2, 3), (5, 8), (10, 12)]);
        assert_eq!(depths(folds.regions()), [1, 2, 3, 2, 1]);
        // A region starting on the line another ends on is still inside it.
        let touching = regions(&[(0, 3), (3, 5)]);
        assert_eq!(depths(touching.regions()), [1, 2]);
        assert!(depths(&[]).is_empty());
    }

    #[test]
    fn region_at_prefers_a_header_then_the_innermost_container() {
        let folds = regions(&[(0, 9), (2, 6), (3, 4)]);
        let start = |line| region_at(&folds, line).map(|r| r.start);
        assert_eq!(start(2), Some(2), "headed on the line");
        assert_eq!(start(5), Some(2), "innermost containing the line");
        assert_eq!(start(4), Some(3));
        assert_eq!(start(8), Some(0));
        assert_eq!(start(10), None);
    }

    #[test]
    fn nested_in_is_inclusive_and_rejects_overlap() {
        let outer = FoldRegion::new(2, 8);
        assert!(nested_in(&outer, &outer));
        assert!(nested_in(&FoldRegion::new(3, 8), &outer));
        assert!(!nested_in(&FoldRegion::new(1, 4), &outer));
        assert!(!nested_in(&FoldRegion::new(5, 9), &outer));
    }
}

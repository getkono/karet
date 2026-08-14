//! Per-lens subtree counts.
//!
//! Rollups are what make top-down navigation possible. Without them a collapsed module
//! is opaque — you would have to expand every branch to discover which ones hold
//! anything worth reading. With them, a module row can say "47 api, 12 substitution"
//! before you descend, so the reader chooses where to look instead of hunting.
//!
//! Counts are aggregated over a node's *entire* subtree and are always relative to the
//! active configuration: a node excluded by the configuration contributes nothing.

use crate::model::LENSES;
use crate::model::Lens;

/// Per-lens facet counts over a node's whole subtree, including the node itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Rollups([u32; LENSES.len()]);

impl Rollups {
    /// Rollups with every count at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The count for one lens.
    #[must_use]
    pub fn get(&self, lens: Lens) -> u32 {
        self.0[lens.index()]
    }

    /// Add `count` to one lens, saturating rather than wrapping.
    pub fn add(&mut self, lens: Lens, count: u32) {
        let slot = &mut self.0[lens.index()];
        *slot = slot.saturating_add(count);
    }

    /// Fold another node's rollups into these — the subtree aggregation step.
    pub fn merge(&mut self, other: Self) {
        for lens in LENSES {
            self.add(lens, other.get(lens));
        }
    }

    /// The sum across all five lenses.
    ///
    /// A node carrying both an `api` and a `hazard` facet counts twice here, which is
    /// intended: this answers "how much seam is under me", not "how many nodes".
    #[must_use]
    pub fn total(&self) -> u32 {
        self.0.iter().copied().fold(0u32, u32::saturating_add)
    }

    /// Whether every lens counts zero — nothing worth descending into.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|count| *count == 0)
    }

    /// The counts as a per-lens array, in [`LENSES`] order.
    #[must_use]
    pub fn counts(&self) -> [u32; LENSES.len()] {
        self.0
    }

    /// Every lens with a non-zero count, in display order.
    pub fn present(&self) -> impl Iterator<Item = (Lens, u32)> + '_ {
        LENSES
            .into_iter()
            .map(|lens| (lens, self.get(lens)))
            .filter(|(_, count)| *count > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_rollup_is_empty() {
        let rollups = Rollups::new();
        assert!(rollups.is_empty());
        assert_eq!(rollups.total(), 0);
        assert_eq!(rollups.present().count(), 0);
        for lens in LENSES {
            assert_eq!(rollups.get(lens), 0);
        }
    }

    #[test]
    fn adding_accumulates_per_lens() {
        let mut rollups = Rollups::new();
        rollups.add(Lens::Api, 3);
        rollups.add(Lens::Api, 4);
        rollups.add(Lens::Hazard, 1);
        assert_eq!(rollups.get(Lens::Api), 7);
        assert_eq!(rollups.get(Lens::Hazard), 1);
        assert_eq!(rollups.get(Lens::Variation), 0);
        assert_eq!(rollups.total(), 8);
        assert!(!rollups.is_empty());
    }

    #[test]
    fn merging_folds_a_subtree_in() {
        let mut parent = Rollups::new();
        parent.add(Lens::Api, 1);
        let mut child = Rollups::new();
        child.add(Lens::Api, 2);
        child.add(Lens::Substitution, 5);
        parent.merge(child);
        assert_eq!(parent.get(Lens::Api), 3);
        assert_eq!(parent.get(Lens::Substitution), 5);
    }

    #[test]
    fn present_lists_only_non_zero_lenses_in_display_order() {
        let mut rollups = Rollups::new();
        rollups.add(Lens::Hazard, 2);
        rollups.add(Lens::Api, 1);
        let present: Vec<_> = rollups.present().collect();
        assert_eq!(present, [(Lens::Api, 1), (Lens::Hazard, 2)]);
    }

    #[test]
    fn counts_are_indexed_by_lens_position() {
        let mut rollups = Rollups::new();
        rollups.add(Lens::Boundary, 9);
        assert_eq!(rollups.counts()[Lens::Boundary.index()], 9);
    }

    #[test]
    fn saturating_arithmetic_keeps_absurd_input_total() {
        let mut rollups = Rollups::new();
        rollups.add(Lens::Api, u32::MAX);
        rollups.add(Lens::Api, 10);
        assert_eq!(rollups.get(Lens::Api), u32::MAX);
        rollups.add(Lens::Hazard, 10);
        // The sum saturates too rather than wrapping to a small number.
        assert_eq!(rollups.total(), u32::MAX);
    }
}

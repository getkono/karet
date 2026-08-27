//! What one file contributed to an index, in a form that outlives the process.
//!
//! Re-reading a package means parsing every file in it, and parsing is the expensive
//! part by a wide margin. A file that has not changed since it was last read has nothing
//! new to say, so what it said before is kept and replayed instead.
//!
//! The unit is one **file**, not one package, because that is the granularity at which
//! source actually changes: editing one file should cost one parse, not a package's
//! worth. It is also the granularity extraction already works at —
//! [`ExtractOutcome::added`](crate::extract::ExtractOutcome::added) hands back exactly
//! the nodes one file produced, in source order.
//!
//! # Why identity is a path here
//!
//! A [`SeamId`](crate::id::SeamId) is an index-local handle assigned in first-seen
//! order; it means nothing to a different index, let alone a different run. A
//! [`SeamPath`] names a node by where it sits in the hierarchy, so it survives being
//! written down and read back. Everything stored here is therefore addressed by path,
//! and ids are re-assigned at replay.
//!
//! # What is deliberately *not* stored
//!
//! - **Rollups and configuration membership.** Both are derived — from the subtree and
//!   from the active configuration respectively — and the configuration can differ
//!   between the run that wrote the cache and the run that reads it. Recomputing them is
//!   cheap and storing them would let a stale answer survive a configuration change.
//! - **Effective visibility.** It belongs to the semantic tier, which resolves re-export
//!   chains across the whole index. It is not a fact about one file, and it carries a
//!   `SeamId` that would not survive the trip.
//! - **Edges.** Nothing populates the edge store during structural indexing yet. Caching
//!   an empty thing would be an invitation to cache a wrong one later.
//! - **The grammar.** A [`LanguageId`](karet_treesitter::LanguageId) is an index into the
//!   parse host's registry, which shifts with the compiled-in grammar set. It is
//!   re-derived from the file's own path at replay, which is where it came from anyway.

use std::path::PathBuf;

use karet_core::Range;
use karet_core::Span;

use crate::id::SeamPath;
use crate::lang::Owner;
use crate::model::Facet;
use crate::model::NodeKind;
use crate::model::Visibility;

/// What a file looked like when it was read.
///
/// Modification time and length together, compared for equality. Not a content hash:
/// hashing means reading, and reading is most of what the cache exists to avoid. The
/// trade is deliberate and one-sided — a file touched without being changed is re-parsed
/// needlessly, which costs time; a changed file is never mistaken for an unchanged one,
/// because writing to it moves its mtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileStamp {
    /// Modification time, as nanoseconds since the Unix epoch.
    ///
    /// Stored as a plain integer rather than a `SystemTime` so the encoding does not
    /// depend on how a given serde format chooses to represent a timestamp.
    pub modified_nanos: u128,
    /// Length in bytes.
    pub len: u64,
}

impl FileStamp {
    /// Read the stamp from filesystem metadata, or `None` if the platform withholds it.
    ///
    /// A missing modification time is not treated as "unchanged" — without it there is
    /// nothing to compare, so the caller re-parses rather than trusting a blank.
    #[must_use]
    pub fn of(metadata: &std::fs::Metadata) -> Option<Self> {
        let modified = metadata.modified().ok()?;
        let since_epoch = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            modified_nanos: since_epoch,
            len: metadata.len(),
        })
    }
}

/// One node as a file produced it, before anything cross-file has been resolved.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CachedNode {
    /// The node's identity.
    pub path: SeamPath,
    /// Its parent's identity. Empty only for a node whose parent is the file's owner.
    pub parent: SeamPath,
    /// The universal kind.
    pub kind: NodeKind,
    /// The display name.
    pub name: String,
    /// A short signature or descriptor.
    pub detail: Option<String>,
    /// The node's full extent.
    pub range: Range,
    /// The same extent in bytes.
    pub span: Span,
    /// The range to reveal when navigating here.
    pub selection: Range,
    /// The declaration head — everything up to the body the construct opens.
    pub header: Range,
    /// The seam properties it carries, with semantic-tier reach stripped.
    pub facets: Vec<Facet>,
    /// The declared visibility.
    pub visibility: Option<Visibility>,
    /// Whether it came from a subtree that failed to parse cleanly.
    pub provisional: bool,
}

/// A module declaration whose body lives in another file, addressed by path.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CachedModule {
    /// The module node awaiting its contents.
    pub node: SeamPath,
    /// The declared module name.
    pub name: String,
    /// The inline modules enclosing the declaration, outermost first.
    pub inline_path: Vec<String>,
    /// The `#[path = "…"]` override, when one is written.
    pub path_attribute: Option<String>,
}

/// Everything one file said, keyed by what it said it about.
///
/// Replaying this into an index is equivalent to re-reading the file, provided the file
/// has not changed — which is exactly what [`FileStamp`] is compared to establish.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileContribution {
    /// The file this came from.
    pub file: PathBuf,
    /// What it looked like when it was read.
    pub stamp: FileStamp,
    /// The node this file's contents hang from.
    pub owner: SeamPath,
    /// How many module hops from a package's entry point this file sits.
    ///
    /// Replay order is `(depth, file)`, which puts a module's node in the index before
    /// the file that fills it: a declared module's file is always strictly deeper than
    /// the file declaring it. That makes replay independent of the order the parallel
    /// walk happened to finish in, and so makes id assignment reproducible.
    pub depth: u32,
    /// Whether this file is a crate entry point, which changes module path resolution.
    pub crate_root: bool,
    /// Every node the file produced, parents before children.
    pub nodes: Vec<CachedNode>,
    /// Module declarations whose bodies live elsewhere.
    pub external_modules: Vec<CachedModule>,
    /// Nodes whose semantic owner is named elsewhere, with the candidates to try.
    ///
    /// Kept unresolved, exactly as extraction left them: the owner routinely lives in
    /// another file, so resolution is a whole-package step that re-runs on every build
    /// whether the file was parsed or replayed.
    pub ownership: Vec<(SeamPath, Vec<Owner>)>,
    /// Modules whose body could not be found, with the paths that were tried.
    pub unresolved: Vec<(SeamPath, Vec<PathBuf>)>,
}

impl FileContribution {
    /// Whether this contribution still describes the file on disk.
    #[must_use]
    pub fn matches(&self, stamp: FileStamp) -> bool {
        self.stamp == stamp
    }
}

/// Drop the parts of a facet that are not a fact about one file.
///
/// Effective reach is resolved across the whole index and carries an index-local id, so
/// it is recomputed rather than carried.
#[must_use]
pub(crate) fn structural_facet(facet: &Facet) -> Facet {
    Facet {
        lens: facet.lens,
        subtype: facet.subtype,
        detail: facet.detail.clone(),
        sites: facet.sites.clone(),
        effective: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effective;
    use crate::model::FacetSubtype;
    use crate::model::Lens;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_stamp_reads_length_and_time_off_the_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {}")?;
        let stamp = FileStamp::of(&std::fs::metadata(&file)?);
        assert_eq!(stamp.map(|stamp| stamp.len), Some(12));
        Ok(())
    }

    #[test]
    fn a_rewritten_file_does_not_match_its_old_stamp() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {}")?;
        let before = FileStamp::of(&std::fs::metadata(&file)?);

        // A different length is enough on its own, which is why both are carried: a
        // filesystem with coarse timestamps can still tell these two apart.
        std::fs::write(&file, "fn main() { let x = 1; }")?;
        let after = FileStamp::of(&std::fs::metadata(&file)?);
        assert_ne!(before, after);
        Ok(())
    }

    #[test]
    fn semantic_reach_is_not_carried_across_the_cache() {
        // It is resolved over the whole index and points at an index-local id, so
        // storing it would let a handle from one run be read back in another.
        let facet = Facet::new(Lens::Api, FacetSubtype("pub"))
            .with_detail("something")
            .with_effective(Effective {
                reach: crate::model::Visibility::Public,
                via: Some(crate::id::SeamId(7)),
                public_path: Some("pkg::Thing".to_owned()),
            });
        let stored = structural_facet(&facet);
        assert_eq!(stored.effective, None);
        assert_eq!(stored.lens, Lens::Api);
        assert_eq!(stored.detail.as_deref(), Some("something"));
    }
}

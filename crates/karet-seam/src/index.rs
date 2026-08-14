//! The seam index: the containment arena, the file table, and the edge store.
//!
//! This is the product. The view is a renderer over it, and the query language and the
//! agent surface read the same structure the spine draws — so anything the UI can show,
//! a caller can ask for directly.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::edge::EdgeStore;
use crate::id::SeamId;
use crate::id::SeamInterner;
use crate::id::SeamPath;
use crate::model::ConfigMembership;
use crate::model::FileId;
use crate::model::Node;
use crate::rollup::Rollups;

/// A package's containment tree, its non-containment edges, and the files behind them.
#[derive(Debug, Default, Clone)]
pub struct SeamIndex {
    interner: SeamInterner,
    nodes: HashMap<SeamId, Node>,
    roots: Vec<SeamId>,
    files: Vec<PathBuf>,
    file_ids: HashMap<PathBuf, FileId>,
    edges: EdgeStore,
    truncated: Option<usize>,
}

impl SeamIndex {
    /// An index holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // --- identity -------------------------------------------------------------

    /// The id for `path`, assigning one if this is the first time it is seen.
    pub fn intern(&mut self, path: SeamPath) -> SeamId {
        self.interner.intern(path)
    }

    /// The id already assigned to `path`, without assigning a new one.
    #[must_use]
    pub fn resolve(&self, path: &SeamPath) -> Option<SeamId> {
        self.interner.lookup(path)
    }

    /// The path behind `id`.
    #[must_use]
    pub fn path(&self, id: SeamId) -> Option<&SeamPath> {
        self.interner.path(id)
    }

    // --- files ----------------------------------------------------------------

    /// The handle for `path`, registering it if it is new.
    pub fn intern_file(&mut self, path: &Path) -> FileId {
        if let Some(id) = self.file_ids.get(path) {
            return *id;
        }
        let id = FileId(u32::try_from(self.files.len()).unwrap_or(u32::MAX));
        self.files.push(path.to_path_buf());
        self.file_ids.insert(path.to_path_buf(), id);
        id
    }

    /// The path behind a file handle.
    #[must_use]
    pub fn file_path(&self, file: FileId) -> Option<&Path> {
        self.files.get(file.0 as usize).map(PathBuf::as_path)
    }

    /// Every indexed file, in registration order.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    // --- nodes ----------------------------------------------------------------

    /// Add a node, linking it to its parent and recording it as a root when it has none.
    ///
    /// A node whose parent is not in the index yet is still accepted — the extractor
    /// inserts in tree order, and a missing parent means the caller is splicing a
    /// subtree whose root attaches later.
    pub fn insert(&mut self, node: Node) {
        let id = node.id;
        let parent = node.parent;
        match parent {
            Some(parent) => {
                if let Some(entry) = self.nodes.get_mut(&parent)
                    && !entry.children.contains(&id)
                {
                    entry.children.push(id);
                }
            },
            None => {
                if !self.roots.contains(&id) {
                    self.roots.push(id);
                }
            },
        }
        self.nodes.insert(id, node);
    }

    /// The node behind `id`.
    #[must_use]
    pub fn node(&self, id: SeamId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Mutable access to the node behind `id`.
    pub fn node_mut(&mut self, id: SeamId) -> Option<&mut Node> {
        self.nodes.get_mut(&id)
    }

    /// Every node, in unspecified order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// How many nodes the index holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the index holds no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The tree roots — one per indexed package.
    #[must_use]
    pub fn roots(&self) -> &[SeamId] {
        &self.roots
    }

    /// A node's children, in source order.
    #[must_use]
    pub fn children(&self, id: SeamId) -> &[SeamId] {
        self.nodes.get(&id).map_or(&[][..], |node| &node.children)
    }

    /// `id` and everything beneath it, parents before children.
    #[must_use]
    pub fn subtree(&self, id: SeamId) -> Vec<SeamId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(current) = stack.pop() {
            if self.nodes.contains_key(&current) {
                out.push(current);
                // Reversed so the pop order matches source order.
                stack.extend(self.children(current).iter().rev().copied());
            }
        }
        out
    }

    /// A node's ancestors, nearest first.
    #[must_use]
    pub fn ancestors(&self, id: SeamId) -> Vec<SeamId> {
        let mut out = Vec::new();
        let mut current = self.nodes.get(&id).and_then(|node| node.parent);
        while let Some(parent) = current {
            out.push(parent);
            current = self.nodes.get(&parent).and_then(|node| node.parent);
        }
        out
    }

    // --- edges ----------------------------------------------------------------

    /// The non-containment edges.
    #[must_use]
    pub fn edges(&self) -> &EdgeStore {
        &self.edges
    }

    /// Mutable access to the edge store.
    pub fn edges_mut(&mut self) -> &mut EdgeStore {
        &mut self.edges
    }

    // --- truncation -----------------------------------------------------------

    /// Record that indexing stopped early after `scanned` files.
    ///
    /// A capped index that says nothing reads as a complete one, so this is surfaced in
    /// the header rather than swallowed.
    pub fn mark_truncated(&mut self, scanned: usize) {
        self.truncated = Some(scanned);
    }

    /// How many files were scanned before indexing was cut short, if it was.
    #[must_use]
    pub fn truncated_after(&self) -> Option<usize> {
        self.truncated
    }

    // --- rollups --------------------------------------------------------------

    /// Recompute every node's per-lens subtree counts.
    ///
    /// A node excluded by the active configuration contributes nothing. An
    /// *indeterminate* node does contribute: excluding it would assert an absence the
    /// index has not established, which is precisely the confusion the three-state
    /// model exists to prevent.
    pub fn recompute_rollups(&mut self) {
        let roots = self.roots.clone();
        for root in roots {
            self.recompute_subtree(root);
        }
    }

    /// Recompute rollups for `id`'s subtree, then refresh every ancestor.
    ///
    /// This is the incremental path: re-indexing one file changes one subtree, and only
    /// the spine above it needs to be revisited.
    pub fn recompute_rollups_from(&mut self, id: SeamId) {
        self.recompute_subtree(id);
        for ancestor in self.ancestors(id) {
            let merged = self.own_and_children_rollups(ancestor);
            if let Some(node) = self.nodes.get_mut(&ancestor) {
                node.rollups = merged;
            }
        }
    }

    /// Post-order recompute over one subtree.
    fn recompute_subtree(&mut self, id: SeamId) {
        // Children first, so a parent merges settled counts.
        for child in self.children(id).to_vec() {
            self.recompute_subtree(child);
        }
        let merged = self.own_and_children_rollups(id);
        if let Some(node) = self.nodes.get_mut(&id) {
            node.rollups = merged;
        }
    }

    /// One node's own facet counts merged with its children's settled rollups.
    fn own_and_children_rollups(&self, id: SeamId) -> Rollups {
        let mut rollups = Rollups::new();
        let Some(node) = self.nodes.get(&id) else {
            return rollups;
        };
        if node.membership != ConfigMembership::Inactive {
            for facet in &node.facets {
                let count = u32::try_from(facet.occurrences()).unwrap_or(u32::MAX);
                rollups.add(facet.lens, count);
            }
        }
        for child in &node.children {
            if let Some(child) = self.nodes.get(child) {
                rollups.merge(child.rollups);
            }
        }
        rollups
    }
}

#[cfg(test)]
mod tests {
    use karet_core::Range;
    use karet_core::Span;

    use super::*;
    use crate::model::Facet;
    use crate::model::FacetSubtype;
    use crate::model::Lens;
    use crate::model::NodeKind;
    use crate::model::SeamLocation;

    fn location() -> SeamLocation {
        SeamLocation {
            file: FileId(0),
            range: Range::default(),
            span: Span::default(),
            selection: Range::default(),
        }
    }

    /// Add a node at `path` with the given facets, wiring its parent by path.
    fn add(index: &mut SeamIndex, path: &str, facets: Vec<Facet>) -> SeamId {
        let parsed: SeamPath = path.parse().unwrap_or_default();
        let parent = parsed.parent().and_then(|p| index.resolve(&p));
        let id = index.intern(parsed.clone());
        index.insert(Node {
            id,
            kind: NodeKind::Module,
            name: parsed.leaf().unwrap_or_default().to_owned(),
            detail: None,
            location: location(),
            parent,
            children: Vec::new(),
            facets,
            visibility: None,
            rollups: Rollups::new(),
            membership: ConfigMembership::Active,
            provisional: false,
        });
        id
    }

    fn api() -> Vec<Facet> {
        vec![Facet::new(Lens::Api, FacetSubtype("pub"))]
    }

    #[test]
    fn an_empty_index_answers_without_panicking() {
        let index = SeamIndex::new();
        assert!(index.is_empty());
        assert_eq!(index.len(), 0);
        assert!(index.roots().is_empty());
        assert_eq!(index.node(SeamId(0)), None);
        assert!(index.children(SeamId(0)).is_empty());
        assert!(index.ancestors(SeamId(0)).is_empty());
        assert_eq!(index.truncated_after(), None);
    }

    #[test]
    fn inserting_links_parents_and_records_roots() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        let module = add(&mut index, "pkg::m", vec![]);
        let item = add(&mut index, "pkg::m::T", vec![]);

        assert_eq!(index.roots(), [pkg]);
        assert_eq!(index.children(pkg), [module]);
        assert_eq!(index.children(module), [item]);
        assert_eq!(index.ancestors(item), [module, pkg]);
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn subtree_lists_the_node_and_its_descendants_in_source_order() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        add(&mut index, "pkg::a", vec![]);
        add(&mut index, "pkg::a::x", vec![]);
        add(&mut index, "pkg::b", vec![]);

        let names: Vec<String> = index
            .subtree(pkg)
            .into_iter()
            .filter_map(|id| index.path(id).map(ToString::to_string))
            .collect();
        assert_eq!(names, ["pkg", "pkg::a", "pkg::a::x", "pkg::b"]);
    }

    #[test]
    fn rollups_aggregate_the_whole_subtree() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        let module = add(&mut index, "pkg::m", api());
        add(&mut index, "pkg::m::T", api());
        add(&mut index, "pkg::m::U", api());

        index.recompute_rollups();

        // The package sees all three, without anything being expanded.
        assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(3));
        assert_eq!(
            index.node(module).map(|n| n.rollups.get(Lens::Api)),
            Some(3)
        );
    }

    #[test]
    fn a_facet_with_sites_counts_every_occurrence() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        add(
            &mut index,
            "pkg::f",
            vec![
                Facet::new(Lens::Hazard, FacetSubtype("unsafe")).with_sites(vec![
                    Range::default(),
                    Range::default(),
                    Range::default(),
                ]),
            ],
        );
        index.recompute_rollups();
        assert_eq!(
            index.node(pkg).map(|n| n.rollups.get(Lens::Hazard)),
            Some(3)
        );
    }

    #[test]
    fn a_configuration_excluded_node_contributes_nothing() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        let gated = add(&mut index, "pkg::gated", api());
        if let Some(node) = index.node_mut(gated) {
            node.membership = ConfigMembership::Inactive;
        }
        index.recompute_rollups();
        assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(0));
    }

    #[test]
    fn an_indeterminate_node_still_contributes() {
        // Dropping it would assert an absence the index has not established.
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        let unknown = add(&mut index, "pkg::maybe", api());
        if let Some(node) = index.node_mut(unknown) {
            node.membership = ConfigMembership::Indeterminate;
        }
        index.recompute_rollups();
        assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));
    }

    #[test]
    fn incremental_recompute_refreshes_only_the_ancestor_spine() {
        let mut index = SeamIndex::new();
        let pkg = add(&mut index, "pkg", vec![]);
        let module = add(&mut index, "pkg::m", vec![]);
        let leaf = add(&mut index, "pkg::m::T", api());
        index.recompute_rollups();
        assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));

        // The leaf gains a second facet, as an edit would produce.
        if let Some(node) = index.node_mut(leaf) {
            node.facets
                .push(Facet::new(Lens::Hazard, FacetSubtype("unsafe")));
        }
        index.recompute_rollups_from(leaf);

        assert_eq!(
            index.node(leaf).map(|n| n.rollups.get(Lens::Hazard)),
            Some(1)
        );
        assert_eq!(
            index.node(module).map(|n| n.rollups.get(Lens::Hazard)),
            Some(1)
        );
        assert_eq!(
            index.node(pkg).map(|n| n.rollups.get(Lens::Hazard)),
            Some(1)
        );
        assert_eq!(index.node(pkg).map(|n| n.rollups.get(Lens::Api)), Some(1));
    }

    #[test]
    fn files_intern_once_and_resolve_back() {
        let mut index = SeamIndex::new();
        let a = index.intern_file(Path::new("src/lib.rs"));
        let b = index.intern_file(Path::new("src/model.rs"));
        assert_eq!(index.intern_file(Path::new("src/lib.rs")), a);
        assert_ne!(a, b);
        assert_eq!(index.file_path(a), Some(Path::new("src/lib.rs")));
        assert_eq!(index.file_path(FileId(99)), None);
        assert_eq!(index.files().len(), 2);
    }

    #[test]
    fn truncation_is_recorded_rather_than_swallowed() {
        let mut index = SeamIndex::new();
        index.mark_truncated(20_000);
        assert_eq!(index.truncated_after(), Some(20_000));
    }

    #[test]
    fn several_packages_each_become_a_root() {
        let mut index = SeamIndex::new();
        let a = add(&mut index, "alpha", vec![]);
        let b = add(&mut index, "beta", vec![]);
        assert_eq!(index.roots(), [a, b]);
    }
}

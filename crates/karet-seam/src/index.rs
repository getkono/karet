//! The seam index: the containment arena, the file table, and the edge store.
//!
//! This is the product. The view is a renderer over it, and the query language and the
//! agent surface read the same structure the spine draws — so anything the UI can show,
//! a caller can ask for directly.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use karet_treesitter::LanguageId;

use crate::edge::EdgeStore;
use crate::edge::Endpoint;
use crate::id::SeamId;
use crate::id::SeamInterner;
use crate::id::SeamPath;
use crate::model::ConfigMembership;
use crate::model::FileId;
use crate::model::Node;
use crate::model::SeamLocation;
use crate::rollup::Rollups;

/// A package's containment tree, its non-containment edges, and the files behind them.
#[derive(Debug, Default, Clone)]
pub struct SeamIndex {
    interner: SeamInterner,
    nodes: HashMap<SeamId, Node>,
    roots: Vec<SeamId>,
    files: Vec<PathBuf>,
    file_ids: HashMap<PathBuf, FileId>,
    attribution: HashMap<FileId, (SeamId, LanguageId)>,
    edges: EdgeStore,
    unresolved: Vec<(SeamId, Vec<PathBuf>)>,
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

    /// The handle already assigned to `path`, without assigning one.
    ///
    /// Re-indexing asks this rather than [`Self::intern_file`]: a path the index never
    /// held has nothing to rebuild, and registering it would inflate the file count the
    /// header reports for work that never happened.
    #[must_use]
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.file_ids.get(path).copied()
    }

    /// Record which node owns `file`, and which grammar parsed it.
    ///
    /// Re-indexing an edited file has to rebuild it under the same node and the same
    /// grammar it was first built under. Rediscovering either by inspecting the tree
    /// works only while every module node is located in its *declaring* file, which is a
    /// Rust accident: Python declares nothing, so its module nodes sit in the very file
    /// they own, and the inference silently lands one level too high.
    pub fn attribute_file(&mut self, file: FileId, owner: SeamId, language: LanguageId) {
        self.attribution.insert(file, (owner, language));
    }

    /// The node and grammar recorded for `file`, if it was indexed.
    #[must_use]
    pub fn file_attribution(&self, file: FileId) -> Option<(SeamId, LanguageId)> {
        self.attribution.get(&file).copied()
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

    // --- unresolved modules ---------------------------------------------------

    /// Record that a module's body could not be found, keeping what was tried.
    ///
    /// The node stays in the tree. Removing it would say the package has no such module,
    /// when what actually happened is that its text could not be located — a different
    /// answer, and the one the reader needs.
    pub fn mark_module_unresolved(&mut self, id: SeamId, candidates: Vec<PathBuf>) {
        self.unresolved.push((id, candidates));
    }

    /// Every module whose body could not be found, with the paths that were tried.
    #[must_use]
    pub fn unresolved_modules(&self) -> &[(SeamId, Vec<PathBuf>)] {
        &self.unresolved
    }

    // --- incremental ----------------------------------------------------------

    /// The node that owns `file` — the module a re-index of that file should rebuild under.
    ///
    /// That is the shallowest node located in the file, since everything else in it hangs
    /// from that one.
    #[must_use]
    pub fn owner_of_file(&self, file: FileId) -> Option<SeamId> {
        self.nodes
            .values()
            .filter(|node| node.location.file == file)
            .min_by_key(|node| self.ancestors(node.id).len())
            .and_then(|node| node.parent)
    }

    /// Remove every node located in `file` that sits under `root`, detaching them from
    /// their parents and dropping the edges they owned.
    pub fn remove_nodes_in_file(&mut self, file: FileId, root: SeamId) {
        let doomed: Vec<SeamId> = self
            .subtree(root)
            .into_iter()
            .filter(|id| {
                *id != root
                    && self
                        .nodes
                        .get(id)
                        .is_some_and(|node| node.location.file == file)
            })
            .collect();
        for id in &doomed {
            self.edges.remove_from(*id);
            self.nodes.remove(id);
        }
        let removed: std::collections::HashSet<SeamId> = doomed.into_iter().collect();
        for node in self.nodes.values_mut() {
            node.children.retain(|child| !removed.contains(child));
        }
        self.roots.retain(|id| !removed.contains(id));
        self.unresolved.retain(|(id, _)| !removed.contains(id));
    }

    // --- relocation -----------------------------------------------------------

    /// The segment `name` should take as a child of `parent`, disambiguated as needed.
    ///
    /// Same rule as extraction's: the first occurrence keeps its bare name, later ones
    /// take a 1-based ordinal. Asked of the tree rather than of a counter, because a node
    /// arriving here was written somewhere else entirely and there is no sibling walk to
    /// have counted it.
    fn free_child_segment(&self, parent: SeamId, name: &str) -> crate::id::SeamSegment {
        // Ordinal zero is the bare slot, not the first numbered one: a lone sibling
        // wears its name unadorned, and only the second occurrence starts counting.
        let taken: Vec<u32> = self
            .children(parent)
            .iter()
            .filter_map(|child| self.path(*child))
            .filter_map(|path| path.segments().last())
            .filter(|segment| segment.name == name)
            .map(|segment| segment.ordinal)
            .collect();
        if taken.is_empty() {
            return crate::id::SeamSegment::new(name);
        }
        let next = (2..).find(|n| !taken.contains(n)).unwrap_or(u32::MAX);
        crate::id::SeamSegment::numbered(name, next)
    }

    /// Move `id` and everything under it to sit beneath `parent`, optionally renaming it.
    ///
    /// Identity is position, so moving a node changes its path and therefore its id, and
    /// every descendant's with it. The remap that produced is returned: a caller holding
    /// ids from before the move — the regroup pass holds a whole queue of them — would
    /// otherwise be addressing nodes that no longer exist.
    ///
    /// Refuses a move that would not leave a tree: onto itself, into its own subtree, or
    /// onto a parent the index does not hold.
    pub(crate) fn relocate(
        &mut self,
        id: SeamId,
        parent: SeamId,
        rename: Option<(String, String)>,
    ) -> HashMap<SeamId, SeamId> {
        let empty = HashMap::new();
        if id == parent || !self.nodes.contains_key(&parent) {
            return empty;
        }
        let subtree = self.subtree(id);
        if subtree.is_empty() || subtree.contains(&parent) {
            return empty;
        }
        let (Some(old_root), Some(parent_path)) =
            (self.path(id).cloned(), self.path(parent).cloned())
        else {
            return empty;
        };
        let name = match (&rename, old_root.segments().last()) {
            (Some((_, segment)), _) => segment.clone(),
            (None, Some(segment)) => segment.name.clone(),
            (None, None) => return empty,
        };
        let new_root = parent_path.child(self.free_child_segment(parent, &name));
        if new_root == old_root {
            return empty;
        }
        // A path already occupied by a node outside this subtree is not a slot: interning
        // it would move this node on top of that one and silently lose it.
        if self
            .resolve(&new_root)
            .is_some_and(|held| self.nodes.contains_key(&held) && !subtree.contains(&held))
        {
            return empty;
        }

        // Every descendant keeps its own segments and swaps its prefix, so the subtree's
        // internal shape — and every ordinal inside it — survives the move untouched.
        let mut remap = HashMap::new();
        for old in &subtree {
            let Some(path) = self.path(*old).cloned() else {
                continue;
            };
            let mut segments = new_root.segments().to_vec();
            segments.extend_from_slice(path.segments().get(old_root.len()..).unwrap_or_default());
            remap.insert(*old, self.intern(SeamPath::new(segments)));
        }

        let old_parent = self.nodes.get(&id).and_then(|node| node.parent);
        for old in &subtree {
            let Some(mut node) = self.nodes.remove(old) else {
                continue;
            };
            node.id = remap.get(old).copied().unwrap_or(node.id);
            node.parent = if *old == id {
                Some(parent)
            } else {
                node.parent.map(|up| remap.get(&up).copied().unwrap_or(up))
            };
            for child in &mut node.children {
                *child = remap.get(child).copied().unwrap_or(*child);
            }
            if *old == id
                && let Some((display, _)) = &rename
            {
                node.name = display.clone();
            }
            self.nodes.insert(node.id, node);
        }

        if let Some(previous) = old_parent.and_then(|up| self.nodes.get_mut(&up)) {
            previous.children.retain(|child| *child != id);
        }
        self.roots.retain(|root| *root != id);
        let moved = remap.get(&id).copied().unwrap_or(id);
        if let Some(target) = self.nodes.get_mut(&parent)
            && !target.children.contains(&moved)
        {
            target.children.push(moved);
        }
        self.sort_children(parent);
        self.remap_references(&remap);
        remap
    }

    /// Detach `id` from the tree, dropping it and everything still under it.
    pub(crate) fn discard(&mut self, id: SeamId) {
        for doomed in self.subtree(id) {
            self.edges.remove_from(doomed);
            self.nodes.remove(&doomed);
        }
        if let Some(parent) = self.nodes.values_mut().find(|n| n.children.contains(&id)) {
            parent.children.retain(|child| *child != id);
        }
        self.roots.retain(|root| *root != id);
        self.unresolved.retain(|(held, _)| *held != id);
    }

    /// Put a node's children back into source order after one arrived from elsewhere.
    ///
    /// By file, then by byte offset. Files have no order between them, so the file
    /// handle's own registration order stands in — arbitrary, but stable, which is the
    /// property a spine needs.
    fn sort_children(&mut self, parent: SeamId) {
        let mut children = match self.nodes.get(&parent) {
            Some(node) => node.children.clone(),
            None => return,
        };
        children.sort_by_key(|child| {
            self.nodes
                .get(child)
                .map_or((u32::MAX, usize::MAX), |node| {
                    (node.location.file.0, node.location.span.start.0)
                })
        });
        if let Some(node) = self.nodes.get_mut(&parent) {
            node.children = children;
        }
    }

    /// Point everything that names a node by id at wherever it moved to.
    fn remap_references(&mut self, remap: &HashMap<SeamId, SeamId>) {
        if remap.is_empty() {
            return;
        }
        let swap = |id: &mut SeamId| *id = remap.get(id).copied().unwrap_or(*id);
        for node in self.nodes.values_mut() {
            for child in &mut node.children {
                swap(child);
            }
        }
        for root in &mut self.roots {
            swap(root);
        }
        for (owner, _) in self.attribution.values_mut() {
            swap(owner);
        }
        for (id, _) in &mut self.unresolved {
            swap(id);
        }
        self.edges.remap(remap);
    }

    // --- replay and merge -----------------------------------------------------

    /// Re-insert everything a file contributed, without re-reading it.
    ///
    /// This is the warm path: a file whose [`FileStamp`](crate::contribution::FileStamp)
    /// still matches the disk has nothing new to say, so what it said last time is put
    /// back instead of parsed again. The result is indistinguishable from having read the
    /// file — same nodes, same paths, same order — which is what lets a cached build and
    /// a cold one be compared for equality in a test.
    ///
    /// Returns the file's unresolved ownership hints, translated to live ids, for the
    /// package layer to resolve once every file is in. `None` when the node this file
    /// hangs from is not in the index, which means the caller replayed out of order or
    /// handed over a contribution belonging to a different tree; either way the honest
    /// response is to parse the file rather than to guess.
    pub fn replay(
        &mut self,
        contribution: &crate::contribution::FileContribution,
    ) -> Option<Vec<(SeamId, Vec<crate::lang::Owner>)>> {
        let owner = self.resolve(&contribution.owner)?;
        let file = self.intern_file(&contribution.file);
        // Re-derived rather than stored: a `LanguageId` indexes the parse host's registry,
        // which shifts with the compiled-in grammar set.
        if let Some(language) = karet_treesitter::language_id_from_path(&contribution.file) {
            self.attribute_file(file, owner, language);
        }

        // Paths are rebuilt as the list is walked: a node stores one segment and the
        // offset of its parent, and parents always come first, so each path is its
        // parent's plus that segment.
        let mut resolved: Vec<SeamPath> = Vec::with_capacity(contribution.nodes.len());
        for cached in &contribution.nodes {
            let parent_path = match cached.parent {
                Some(index) => resolved.get(index as usize).cloned().unwrap_or_default(),
                None => contribution.owner.clone(),
            };
            let path = parent_path.child(cached.segment.clone());
            let id = self.intern(path.clone());
            let parent = self.resolve(&parent_path);
            self.insert(Node {
                id,
                kind: cached.kind,
                name: cached.name.clone(),
                detail: cached.detail.clone(),
                location: SeamLocation {
                    file,
                    range: cached.range,
                    span: cached.span,
                    selection: cached.selection,
                    header: cached.header,
                },
                parent,
                children: Vec::new(),
                facets: cached.facets.clone(),
                visibility: cached.visibility,
                rollups: Rollups::new(),
                membership: ConfigMembership::Active,
                provisional: cached.provisional,
            });
            resolved.push(path);
        }

        for (index, candidates) in &contribution.unresolved {
            if let Some(id) = resolved
                .get(*index as usize)
                .and_then(|path| self.resolve(path))
            {
                self.mark_module_unresolved(id, candidates.clone());
            }
        }

        Some(
            contribution
                .ownership
                .iter()
                .filter_map(|(index, owners)| {
                    let id = resolved
                        .get(*index as usize)
                        .and_then(|path| self.resolve(path))?;
                    Some((id, owners.clone()))
                })
                .collect(),
        )
    }

    /// Absorb another index, re-assigning its ids into this one's numbering.
    ///
    /// Packages are indexed independently so they can be built in parallel, and this is
    /// how they become one tree afterwards. Identity is the path, so absorbing is a
    /// matter of re-interning: a node keeps the name it had and gets whatever handle this
    /// index has free. Callers give each package a distinct root segment, so two packages
    /// never contend for a path.
    pub fn merge(&mut self, other: Self) {
        let Self {
            interner,
            mut nodes,
            roots,
            files,
            attribution,
            edges,
            unresolved,
            truncated,
            ..
        } = other;

        // Roots first, then each subtree, so a parent is always remapped before the child
        // that names it. Anything unreachable follows in id order rather than being
        // dropped: a node with no path to a root is still a node the index knows about.
        let mut order: Vec<SeamId> = Vec::with_capacity(nodes.len());
        let mut placed: HashSet<SeamId> = HashSet::new();
        for root in &roots {
            let mut stack = vec![*root];
            while let Some(current) = stack.pop() {
                let Some(node) = nodes.get(&current) else {
                    continue;
                };
                if !placed.insert(current) {
                    continue;
                }
                order.push(current);
                stack.extend(node.children.iter().rev().copied());
            }
        }
        let mut orphans: Vec<SeamId> = nodes
            .keys()
            .copied()
            .filter(|id| !placed.contains(id))
            .collect();
        orphans.sort_unstable();
        order.extend(orphans);

        let mut remap: HashMap<SeamId, SeamId> = HashMap::with_capacity(order.len());
        let mut file_remap: HashMap<FileId, FileId> = HashMap::with_capacity(files.len());
        for (old, path) in files.iter().enumerate() {
            let Ok(old) = u32::try_from(old) else {
                continue;
            };
            file_remap.insert(FileId(old), self.intern_file(path));
        }

        for old in order {
            let Some(mut node) = nodes.remove(&old) else {
                continue;
            };
            let Some(path) = interner.path(old) else {
                continue;
            };
            let id = self.intern(path.clone());
            remap.insert(old, id);
            node.id = id;
            node.parent = node.parent.and_then(|parent| remap.get(&parent).copied());
            node.children = Vec::new();
            node.location.file = file_remap
                .get(&node.location.file)
                .copied()
                .unwrap_or(node.location.file);
            self.insert(node);
        }

        for (file, (owner, language)) in attribution {
            let (Some(file), Some(owner)) = (file_remap.get(&file), remap.get(&owner)) else {
                continue;
            };
            self.attribute_file(*file, *owner, language);
        }
        for (id, candidates) in unresolved {
            if let Some(id) = remap.get(&id) {
                self.mark_module_unresolved(*id, candidates);
            }
        }
        for mut edge in edges.into_all() {
            let Some(from) = remap.get(&edge.from).copied() else {
                continue;
            };
            edge.from = from;
            if let Endpoint::Resolved(target) = edge.to {
                let Some(target) = remap.get(&target).copied() else {
                    continue;
                };
                edge.to = Endpoint::Resolved(target);
            }
            self.edges.insert(edge);
        }
        if let Some(scanned) = truncated {
            let total = self.truncated.unwrap_or(0).max(scanned);
            self.truncated = Some(total);
        }
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
mod tests;

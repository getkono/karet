//! Indexing packages: crate roots, then every module reachable from them.
//!
//! The tree the view shows is semantic, not filesystem — `src/model.rs` appears as
//! `karet-core::model`, and there are no file rows at all. Building that means starting
//! at each crate root and *following* module declarations to the files holding them,
//! rather than sweeping a directory and hoping the shape matches. Python is the exception
//! that proves the rule: it declares nothing, so [`crate::modules::python`] walks the
//! filesystem for it, and both kinds of seed feed the same drain.
//!
//! [`index_package`] reads one package at a known place; [`index_workspace`] reads a
//! *repository*, which is usually several. They share everything below the seed, and the
//! narrow one is kept because "this directory is one package" stays a question worth
//! being able to ask precisely.
//!
//! Three failure modes are represented rather than hidden. A module whose file cannot be
//! found is still a node, recorded in [`SeamIndex::unresolved_modules`] with the paths
//! that were tried. A package with no entry point keeps its root and records what was
//! looked for, because "this crate has nothing in it" and "this crate is laid out in a
//! way we do not read yet" are different claims. And an index cut short by the file cap
//! marks itself truncated, so a partial tree can never be mistaken for a complete one.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use karet_treesitter::LanguageId;
use karet_treesitter::ParserPool;

use crate::discover::Discovered;
use crate::discover::DiscoveryOptions;
use crate::discover::PackageKind;
use crate::extract::ExtractError;
use crate::extract::extract_file;
use crate::id::SeamId;
use crate::id::SeamPath;
use crate::id::SeamSegment;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::SeamLocation;
use crate::modules::ModuleSource;
use crate::modules::resolve;
use crate::rollup::Rollups;

/// Where a Cargo package's entry points conventionally live.
const CRATE_ROOTS: [&str; 2] = ["src/lib.rs", "src/main.rs"];

/// Errors indexing a package.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PackageError {
    /// No `Cargo.toml` was found at the given root.
    #[error("no Cargo.toml at {0}")]
    NoManifest(PathBuf),
    /// The manifest exists but declares no package (a virtual workspace root).
    #[error("{0} declares a workspace, not a package")]
    VirtualManifest(PathBuf),
    /// No crate root could be found, so there is nothing to index.
    #[error("no crate root (src/lib.rs or src/main.rs) under {0}")]
    NoCrateRoot(PathBuf),
    /// Nothing indexable was found anywhere under the given directory.
    #[error("nothing to index under {0}: no Cargo or Python package was found")]
    NothingToIndex(PathBuf),
    /// Every candidate crate root failed to extract.
    #[error("the package could not be indexed: {0}")]
    Extract(#[from] ExtractError),
}

/// How much of a package to index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexOptions {
    /// Stop after this many files, marking the index truncated.
    pub max_files: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        // Far above any real package, so the cap is a runaway guard rather than a policy.
        Self { max_files: 20_000 }
    }
}

/// One file waiting to be indexed, and the module node it belongs to.
struct Pending {
    file: PathBuf,
    parent: SeamId,
    /// Fixed when the file is enqueued, not guessed when it is read: an index may hold
    /// Rust and Python at once, so there is no one grammar to fall back on.
    language: LanguageId,
    crate_root: bool,
}

/// Index the package rooted at `root`, following module declarations across files.
///
/// `root` must *be* a Cargo package. Use [`index_workspace`] to read a directory that
/// merely *contains* packages.
///
/// # Errors
/// [`PackageError::NoManifest`] when there is no `Cargo.toml`,
/// [`PackageError::VirtualManifest`] when it declares only a workspace, and
/// [`PackageError::NoCrateRoot`] when no entry point can be found.
pub fn index_package(root: &Path, options: IndexOptions) -> Result<SeamIndex, PackageError> {
    let manifest_path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|_| PackageError::NoManifest(manifest_path.clone()))?;
    let name = dependable_core::parse_package_name(&manifest)
        .ok_or_else(|| PackageError::VirtualManifest(manifest_path.clone()))?;

    let entry_points = crate_roots(root);
    if entry_points.is_empty() {
        return Err(PackageError::NoCrateRoot(root.to_path_buf()));
    }

    let package = Discovered {
        name,
        root: root.to_path_buf(),
        anchor: manifest_path,
        kind: PackageKind::Cargo,
    };
    let mut index = SeamIndex::new();
    let mut pool = ParserPool::new();
    let root_id = add_package_node(&mut index, &package);
    let mut queue = seed::seed(&mut index, &package, root_id, entry_points);

    let mut ownership = Vec::new();
    drain(
        &mut index,
        &mut pool,
        &mut queue,
        &mut Walk::new(),
        options,
        &mut ownership,
    );
    crate::regroup::apply(&mut index, ownership);
    index.recompute_rollups();
    Ok(index)
}

/// Index every package under `root`, as one tree with a root per package.
///
/// This is what reads a repository. A Cargo workspace, a root that is both a package and a
/// workspace, crates parked a level or two down, and Python projects beside them all
/// resolve to a single index whose roots are the packages — so the view's first column is
/// the package list and a query spans the whole of it.
///
/// # Errors
/// [`PackageError::NothingToIndex`] when discovery finds no package at all. A package that
/// is found but cannot be read is represented in the index rather than failing the whole
/// call, since one unreadable crate is no reason to answer nothing about the rest.
pub fn index_workspace(root: &Path, options: IndexOptions) -> Result<SeamIndex, PackageError> {
    let packages = crate::discover::discover(root, DiscoveryOptions::default());
    if packages.is_empty() {
        return Err(PackageError::NothingToIndex(root.to_path_buf()));
    }

    let mut index = SeamIndex::new();
    let mut pool = ParserPool::new();
    // Shared across packages: a file reachable from two roots is indexed once, and the cap
    // is a budget for the whole index rather than a per-package allowance.
    let mut walk = Walk::new();
    // One list for the whole workspace, resolved once at the end. Resolution never
    // crosses a package boundary, so pooling them costs nothing and keeps the ordering
    // the extraction produced.
    let mut ownership = Vec::new();

    for package in &packages {
        if walk.scanned >= options.max_files {
            index.mark_truncated(walk.scanned);
            break;
        }
        let root_id = add_package_node(&mut index, package);
        let entry_points = if package.kind == PackageKind::Cargo {
            let found = crate_roots(&package.root);
            if found.is_empty() {
                // The package keeps its root. Dropping it would say the workspace has no
                // such member; an empty root would say the member holds nothing. Neither
                // is what happened — its entry point is somewhere we do not look yet, and
                // the paths tried say exactly that.
                index.mark_module_unresolved(root_id, crate_root_candidates(&package.root));
            }
            found
        } else {
            Vec::new()
        };
        let mut queue = seed::seed(&mut index, package, root_id, entry_points);
        drain(
            &mut index,
            &mut pool,
            &mut queue,
            &mut walk,
            options,
            &mut ownership,
        );
    }

    crate::regroup::apply(&mut index, ownership);
    index.recompute_rollups();
    Ok(index)
}

/// What one walk has already done, shared across every package in an index.
struct Walk {
    seen: HashSet<PathBuf>,
    scanned: usize,
}

impl Walk {
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            scanned: 0,
        }
    }
}

/// Work the queue until it empties or the file cap stops it.
///
/// Ownership hints accumulate rather than being acted on: a Rust `impl` and the type it
/// implements routinely live in different files, and the file holding the type may not be
/// read until later. They are resolved once the queue is empty.
fn drain(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    queue: &mut Vec<Pending>,
    walk: &mut Walk,
    options: IndexOptions,
    ownership: &mut Vec<(SeamId, Vec<crate::lang::Owner>)>,
) {
    while let Some(pending) = queue.pop() {
        let canonical = pending
            .file
            .canonicalize()
            .unwrap_or_else(|_| pending.file.clone());
        // A `#[path]` cycle would otherwise walk forever.
        if !walk.seen.insert(canonical) {
            continue;
        }
        if walk.scanned >= options.max_files {
            index.mark_truncated(walk.scanned);
            break;
        }
        walk.scanned += 1;

        let Ok(text) = std::fs::read_to_string(&pending.file) else {
            // Unreadable or not UTF-8: skip the file, keep the module node.
            continue;
        };
        index_one_file(index, pool, queue, &pending, &text, ownership);
    }
}

/// Extract one file and enqueue the modules it declares.
fn index_one_file(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    queue: &mut Vec<Pending>,
    pending: &Pending,
    text: &str,
    ownership: &mut Vec<(SeamId, Vec<crate::lang::Owner>)>,
) {
    let file_id = index.intern_file(&pending.file);
    // Recorded before extraction, so a file that will not parse can still be re-indexed
    // under the node and grammar it belongs to once it does.
    index.attribute_file(file_id, pending.parent, pending.language);
    let Ok(outcome) = extract_file(index, pool, pending.parent, file_id, pending.language, text)
    else {
        return;
    };

    ownership.extend(outcome.ownership);

    // Unbranched by design: `SeamLanguage::external_module` defaults to `None`, so a
    // language whose modules never span files reports none and this loop is simply inert.
    for declaration in outcome.external_modules {
        match resolve(
            &pending.file,
            pending.crate_root,
            &declaration.inline_path,
            &declaration.name,
            declaration.path_attribute.as_deref(),
        ) {
            ModuleSource::File(file) => {
                queue.extend(seed::pending_for(file, declaration.id, false));
            },
            ModuleSource::Missing { candidates } => {
                // The module stays in the tree. Dropping it would claim the package has
                // no such module, when the truth is that its text could not be found.
                index.mark_module_unresolved(declaration.id, candidates);
            },
            ModuleSource::Inline => {},
        }
    }
}

/// Add the package's root node, which every declaration ultimately hangs from.
fn add_package_node(index: &mut SeamIndex, package: &Discovered) -> SeamId {
    let path = SeamPath::new(vec![unique_root_segment(index, &package.name)]);
    let id = index.intern(path);
    let file = index.intern_file(&package.anchor);
    index.insert(Node {
        id,
        kind: NodeKind::Package,
        name: package.name.clone(),
        detail: None,
        location: SeamLocation {
            file,
            range: karet_core::Range::default(),
            span: karet_core::Span::default(),
            selection: karet_core::Range::default(),
            header: karet_core::Range::default(),
        },
        parent: None,
        children: Vec::new(),
        facets: Vec::new(),
        visibility: None,
        rollups: Rollups::new(),
        membership: ConfigMembership::Active,
        provisional: false,
    });
    id
}

/// A root segment no existing root already claims.
///
/// Cargo forbids two members sharing a name, but a Cargo package and a Python package can
/// collide, and identity is the path — so without this the second would intern to the
/// first's id and silently overwrite it. The disambiguator is the path model's own
/// ordinal (`myapp#2`) rather than an invented suffix, so the result still round-trips
/// through `SeamPath::from_str`, which is how a node's identity is handed back to us.
fn unique_root_segment(index: &SeamIndex, name: &str) -> SeamSegment {
    let plain = SeamSegment::new(name);
    if index.resolve(&SeamPath::new(vec![plain.clone()])).is_none() {
        return plain;
    }
    (2..=u32::MAX)
        .map(|ordinal| SeamSegment::numbered(name, ordinal))
        .find(|candidate| {
            index
                .resolve(&SeamPath::new(vec![candidate.clone()]))
                .is_none()
        })
        .unwrap_or(plain)
}

/// The entry points a Cargo package conventionally has, whether or not they exist.
///
/// Recorded as evidence when none of them do, so "we looked here" is answerable.
fn crate_root_candidates(root: &Path) -> Vec<PathBuf> {
    CRATE_ROOTS
        .iter()
        .map(|relative| root.join(relative))
        .collect()
}

/// The entry points to start walking from.
///
/// Conventional locations only, for now — the manifest tier supplies the full declared
/// target set, including the ones a manifest relocates with an explicit `path`.
fn crate_roots(root: &Path) -> Vec<PathBuf> {
    crate_root_candidates(root)
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .collect()
}

/// Re-index a single file in place, replacing the subtree it previously contributed.
///
/// This is the incremental path an edit takes. Only the edited file's nodes are removed
/// and rebuilt, and only the ancestor spine above them has its rollups recomputed —
/// re-walking the package on every keystroke would make the view unusable.
///
/// # Errors
/// Propagates [`ExtractError`] when the file cannot be parsed or has no mapping.
pub fn reindex_file(
    index: &mut SeamIndex,
    pool: &mut ParserPool,
    file: &Path,
    text: &str,
) -> Result<(), ExtractError> {
    // Looked up rather than interned: a path the index never held has nothing to rebuild,
    // and registering it here would inflate the file count the header reports.
    let file_id = index.file_id(file).ok_or(ExtractError::NoMapping)?;
    let (parent, language) = match index.file_attribution(file_id) {
        Some(recorded) => recorded,
        // An index built before the file was attributed, or a node spliced in by hand.
        None => (
            index
                .owner_of_file(file_id)
                .ok_or(ExtractError::NoMapping)?,
            karet_treesitter::language_id_from_path(file).ok_or(ExtractError::NoGrammar)?,
        ),
    };
    // Removal is scoped to the *package*, not to the module that declares the file.
    // Regrouping moves nodes out from under their declaring module — a Rust `impl` ends
    // up beneath the type it implements, which may be anywhere in the package — and a
    // narrower sweep would leave those behind as duplicates of what is about to be built.
    let root = index.ancestors(parent).last().copied().unwrap_or(parent);
    index.remove_nodes_in_file(file_id, root);
    let outcome = extract_file(index, pool, parent, file_id, language, text)?;
    crate::regroup::apply(index, outcome.ownership);
    // From the package root for the same reason: the rebuilt nodes may not have landed
    // in the subtree the edit appeared to touch.
    index.recompute_rollups_from(root);
    Ok(())
}

mod seed;

#[cfg(test)]
mod tests;

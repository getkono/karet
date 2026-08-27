//! Building an index across cores, one package at a time to whoever is watching.
//!
//! Indexing a repository is thousands of independent parses, and the serial walk that
//! preceded this used one core and answered only when the last file was in. Two things
//! change here, and they are the same change seen from either end: the work fans out, and
//! the answer arrives in pieces.
//!
//! # Two levels of fan-out
//!
//! Packages are independent, so they run concurrently — that alone saturates a workspace
//! with more members than cores. It does nothing for a repository holding one large crate,
//! which is why the walk *inside* a package is parallel too.
//!
//! That inner walk cannot be a plain `par_iter`: Rust's file set is not knowable up front,
//! because a file is discovered by parsing the `mod` declaration that names it. So it is a
//! recursive [`rayon::scope`] — each file, once read, spawns the files it declares — which
//! is a work-stolen frontier that widens as it goes and needs no barrier between levels.
//! Ecosystems whose modules are files on disk seed a frontier that is already wide.
//!
//! # Determinism, which is not free here
//!
//! Ids are assigned in first-seen order, so a walk that finishes in a different order every
//! run would number the tree differently every run. Extraction therefore happens into an
//! isolated scratch index and is *replayed* afterwards in `(depth, path)` order — a total
//! order fixed by the module tree rather than by the scheduler. Packages merge in discovery
//! order for the same reason. A cold build, a warm build and the old serial walk all
//! produce the same numbering.
//!
//! # What deliberately changed
//!
//! The serial walk shared one "already seen" set across every package, so a file reachable
//! from two package roots was indexed under whichever package was reached first. Here each
//! package keeps its own. Sharing it would make the result depend on which thread won, and
//! — the deciding reason — it would stop a package's index from being a fact about that
//! package alone, which is exactly what the on-disk cache stores. A file genuinely
//! reachable from two packages now appears under both, which is also the truer answer.
//!
//! The file cap remains one budget for the whole index. Which package loses out when it
//! runs out is now scheduling-dependent; truncation is already an explicit "this is
//! partial" state rather than a promise about what survived.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use karet_treesitter::LanguageId;
use karet_treesitter::ParserPool;
use rayon::prelude::*;

use crate::contribution::CachedModule;
use crate::contribution::CachedNode;
use crate::contribution::FileContribution;
use crate::contribution::FileStamp;
use crate::contribution::structural_facet;
use crate::discover::Discovered;
use crate::discover::DiscoveryOptions;
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
use crate::package::IndexOptions;
use crate::package::PackageError;
use crate::rollup::Rollups;

thread_local! {
    /// One parser pool per worker thread.
    ///
    /// `ParserPool` hands out `&mut tree_sitter::Parser`, so it cannot be shared; giving
    /// each thread its own is both the only workable shape and the cheapest one, since a
    /// pool amortizes `set_language` across every file that thread reads.
    static POOL: std::cell::RefCell<ParserPool> = std::cell::RefCell::new(ParserPool::new());
}

/// One package, indexed on its own.
pub struct IndexedPackage {
    /// Its position in discovery order, which is the order packages merge in.
    pub order: usize,
    /// What was discovered.
    pub package: Discovered,
    /// The package's tree, rollups already computed.
    pub index: SeamIndex,
    /// What each file contributed, for a caller that wants to cache it.
    pub contributions: Vec<FileContribution>,
    /// How many of those files had to be parsed rather than replayed.
    pub parsed: usize,
}

/// What a caller wants to know, and be asked, while an index is built.
///
/// The engine owns the walk; the caller owns storage and presentation. This is the seam
/// between them — it is how a cache is consulted without the engine knowing what a cache
/// is, and how a package reaches a view before its neighbours are finished.
pub trait IndexObserver: Sync {
    /// A previously stored contribution for `file`, if the caller has one that still
    /// matches `stamp`.
    ///
    /// Returning `Some` means the file is not read and not parsed. Returning `None` — the
    /// default — is always safe and is what makes a cold build a special case of a warm
    /// one rather than a separate path.
    fn cached(&self, file: &Path, stamp: FileStamp) -> Option<FileContribution> {
        let _ = (file, stamp);
        None
    }

    /// One package is complete.
    ///
    /// Called from whichever worker finished it, so packages arrive in completion order,
    /// not discovery order. Taken by `&mut` so a caller can apply its own configuration
    /// before reading the tree out.
    fn package_indexed(&self, indexed: &mut IndexedPackage) {
        let _ = indexed;
    }

    /// Whether to stop. Checked per file, so a cancelled build stops promptly.
    fn cancelled(&self) -> bool {
        false
    }
}

/// An observer that wants nothing, for callers that only want the finished index.
pub struct Unobserved;

impl IndexObserver for Unobserved {}

/// One file waiting to be read, addressed the way a worker can carry it across threads.
struct FileUnit {
    file: PathBuf,
    /// The node its contents attach to, by path — an id would mean nothing in the scratch
    /// index this file is extracted into.
    owner: SeamPath,
    owner_kind: NodeKind,
    language: LanguageId,
    crate_root: bool,
    depth: u32,
}

/// Index every package under `root`, fanning out and reporting each package as it lands.
///
/// # Errors
/// [`PackageError::NothingToIndex`] when discovery finds no package at all. A package that
/// is found but cannot be read is represented in the index rather than failing the call.
pub fn index_workspace_with(
    root: &Path,
    options: IndexOptions,
    observer: &dyn IndexObserver,
) -> Result<SeamIndex, PackageError> {
    let packages = crate::discover::discover(root, DiscoveryOptions::default());
    if packages.is_empty() {
        return Err(PackageError::NothingToIndex(root.to_path_buf()));
    }

    // Serial, and it has to be: disambiguating a root name needs to know every name
    // already claimed, which is not a question a worker can answer about itself.
    let segments = root_segments(&packages);
    let budget = AtomicUsize::new(0);
    let finished: Mutex<Vec<IndexedPackage>> = Mutex::new(Vec::with_capacity(packages.len()));

    packages
        .into_par_iter()
        .zip(segments)
        .enumerate()
        .for_each(|(order, (package, segment))| {
            if observer.cancelled() {
                return;
            }
            let mut indexed = index_one(order, package, segment, options, observer, &budget);
            observer.package_indexed(&mut indexed);
            if let Ok(mut finished) = finished.lock() {
                finished.push(indexed);
            }
        });

    let mut finished = finished.into_inner().unwrap_or_default();
    // Discovery order, never completion order: the view lists packages in the order the
    // repository presents them, and ids must not depend on which core finished first.
    finished.sort_by_key(|indexed| indexed.order);

    let mut index = SeamIndex::new();
    for indexed in finished {
        index.merge(indexed.index);
    }
    Ok(index)
}

/// A distinct root segment per package, assigned in discovery order.
///
/// Cargo forbids two members sharing a name, but two ecosystems can collide — and identity
/// is the path, so without this the second package would intern onto the first and
/// silently overwrite it. The disambiguator is the path model's own ordinal (`myapp#2`),
/// so the result still round-trips through `SeamPath::from_str`.
fn root_segments(packages: &[Discovered]) -> Vec<SeamSegment> {
    let mut claimed: HashSet<SeamSegment> = HashSet::with_capacity(packages.len());
    packages
        .iter()
        .map(|package| {
            let plain = SeamSegment::new(&package.name);
            let chosen = if claimed.contains(&plain) {
                (2..=u32::MAX)
                    .map(|ordinal| SeamSegment::numbered(&package.name, ordinal))
                    .find(|candidate| !claimed.contains(candidate))
                    .unwrap_or_else(|| plain.clone())
            } else {
                plain
            };
            claimed.insert(chosen.clone());
            chosen
        })
        .collect()
}

/// Build one package's tree, on its own, in parallel.
fn index_one(
    order: usize,
    package: Discovered,
    segment: SeamSegment,
    options: IndexOptions,
    observer: &dyn IndexObserver,
    budget: &AtomicUsize,
) -> IndexedPackage {
    let mut index = SeamIndex::new();
    let root_id = add_package_node(&mut index, &package, segment);

    let entry_points = if package.kind == crate::discover::PackageKind::Cargo {
        let found = crate::package::crate_roots(&package.root);
        if found.is_empty() {
            // The package keeps its root. Dropping it would say the workspace has no such
            // member; an empty root would say the member holds nothing. Neither is what
            // happened, and the paths tried say exactly that.
            index.mark_module_unresolved(
                root_id,
                crate::package::crate_root_candidates(&package.root),
            );
        }
        found
    } else {
        Vec::new()
    };

    // Seeding creates module nodes for the ecosystems whose tree is on disk, so it runs
    // against the package's own index before anything is read.
    let seeds = crate::package::seed::seed(&mut index, &package, root_id, entry_points);
    let seeds: Vec<FileUnit> = seeds
        .into_iter()
        .filter_map(|pending| FileUnit::from_pending(&index, pending))
        .collect();

    let contributions = walk(seeds, options, observer, budget);
    let truncated = contributions.truncated;
    let mut contributions = contributions.done;
    let parsed = contributions.iter().filter(|c| c.parsed).count();

    // The order the walk finished in is the scheduler's business. Replay order is the
    // module tree's: a declared module's file is always deeper than the file declaring
    // it, so `(depth, path)` puts every parent in before the child that needs it.
    contributions.sort_by(|a, b| (a.depth, &a.file).cmp(&(b.depth, &b.file)));

    let mut ownership: Vec<(SeamId, Vec<crate::lang::Owner>)> = Vec::new();
    for contribution in &contributions {
        if let Some(hints) = index.replay(&contribution.contribution) {
            ownership.extend(hints);
        }
    }
    crate::regroup::apply(&mut index, ownership);
    if let Some(scanned) = truncated {
        index.mark_truncated(scanned);
    }
    index.recompute_rollups();

    IndexedPackage {
        order,
        package,
        index,
        contributions: contributions
            .into_iter()
            .map(|held| held.contribution)
            .collect(),
        parsed,
    }
}

/// A contribution plus whether producing it cost a parse.
struct Walked {
    contribution: FileContribution,
    parsed: bool,
    depth: u32,
    file: PathBuf,
}

/// Everything one package's walk produced.
struct WalkResult {
    done: Vec<Walked>,
    truncated: Option<usize>,
}

/// Read every file reachable from `seeds`, spawning children as they are discovered.
fn walk(
    seeds: Vec<FileUnit>,
    options: IndexOptions,
    observer: &dyn IndexObserver,
    budget: &AtomicUsize,
) -> WalkResult {
    let seen: Mutex<HashSet<PathBuf>> = Mutex::new(HashSet::new());
    let done: Mutex<Vec<Walked>> = Mutex::new(Vec::new());
    let truncated = AtomicBool::new(false);

    let state = WalkState {
        seen: &seen,
        done: &done,
        truncated: &truncated,
        options,
        observer,
        budget,
    };

    rayon::scope(|scope| {
        for unit in seeds {
            spawn(scope, unit, &state);
        }
    });

    WalkResult {
        done: done.into_inner().unwrap_or_default(),
        // The cap itself, not the counter: the counter is bumped by every task that goes
        // on to find the budget spent, so it overshoots. Truncation happens exactly when
        // the budget runs out, so the number of files actually read is the budget.
        truncated: truncated
            .load(Ordering::Relaxed)
            .then_some(options.max_files),
    }
}

/// What every task in one package's walk shares.
struct WalkState<'a> {
    seen: &'a Mutex<HashSet<PathBuf>>,
    done: &'a Mutex<Vec<Walked>>,
    truncated: &'a AtomicBool,
    options: IndexOptions,
    observer: &'a dyn IndexObserver,
    budget: &'a AtomicUsize,
}

/// Read one file and spawn the files it declares.
fn spawn<'scope>(scope: &rayon::Scope<'scope>, unit: FileUnit, state: &'scope WalkState<'scope>) {
    // A `#[path]` cycle, or two modules naming one file, would otherwise walk forever.
    let canonical = unit
        .file
        .canonicalize()
        .unwrap_or_else(|_| unit.file.clone());
    let Ok(mut seen) = state.seen.lock() else {
        return;
    };
    let fresh = seen.insert(canonical);
    drop(seen);
    if !fresh {
        return;
    }

    scope.spawn(move |scope| {
        if state.observer.cancelled() {
            return;
        }
        if state.budget.fetch_add(1, Ordering::Relaxed) >= state.options.max_files {
            state.truncated.store(true, Ordering::Relaxed);
            return;
        }
        let Some(walked) = read(&unit, state.observer) else {
            return;
        };

        for declaration in &walked.contribution.external_modules {
            let ModuleSource::File(file) = resolve(
                &unit.file,
                unit.crate_root,
                &declaration.inline_path,
                &declaration.name,
                declaration.path_attribute.as_deref(),
            ) else {
                // `Missing` is already recorded in the contribution, and `Inline` has no
                // file to read. Neither is a child to walk to.
                continue;
            };
            let Some(owner) = walked.contribution.path_of(declaration.node) else {
                continue;
            };
            let Some(child) = FileUnit::child(file, owner, unit.depth) else {
                continue;
            };
            spawn(scope, child, state);
        }

        if let Ok(mut done) = state.done.lock() {
            done.push(walked);
        }
    });
}

/// Replay one file from the caller's cache, or read and parse it.
fn read(unit: &FileUnit, observer: &dyn IndexObserver) -> Option<Walked> {
    let stamp = std::fs::metadata(&unit.file)
        .ok()
        .as_ref()
        .and_then(FileStamp::of)?;

    if let Some(cached) = observer.cached(&unit.file, stamp) {
        return Some(Walked {
            depth: unit.depth,
            file: unit.file.clone(),
            contribution: FileContribution {
                // The cache stores what a file said; where it sits in *this* walk is the
                // walk's own business, and a file can be reached at a different depth or
                // under a different owner than when it was stored.
                owner: unit.owner.clone(),
                depth: unit.depth,
                crate_root: unit.crate_root,
                ..cached
            },
            parsed: false,
        });
    }

    let text = std::fs::read_to_string(&unit.file).ok()?;
    let contribution = POOL.with(|pool| {
        let mut pool = pool.try_borrow_mut().ok()?;
        extract_isolated(unit, stamp, &text, &mut pool)
    })?;
    Some(Walked {
        depth: unit.depth,
        file: unit.file.clone(),
        contribution,
        parsed: true,
    })
}

/// Extract one file into a scratch index of its own, and record what it produced.
///
/// Isolation is what makes the walk parallel: nothing is shared, so nothing is locked. The
/// scratch index holds only a stand-in for the node this file hangs from, because that is
/// all `extract_file` reads of its parent — the path it builds children under, and the kind
/// it is.
fn extract_isolated(
    unit: &FileUnit,
    stamp: FileStamp,
    text: &str,
    pool: &mut ParserPool,
) -> Option<FileContribution> {
    let mut scratch = SeamIndex::new();
    let owner = scratch.intern(unit.owner.clone());
    scratch.insert(stand_in(owner, unit.owner_kind));
    let file = scratch.intern_file(&unit.file);

    let outcome = extract_file(&mut scratch, pool, owner, file, unit.language, text).ok()?;

    // Every node this file produced, by id, so the tree can be stored as offsets into
    // this list rather than as a full path per node.
    let position: HashMap<SeamId, u32> = outcome
        .added
        .iter()
        .enumerate()
        .filter_map(|(index, id)| Some((*id, u32::try_from(index).ok()?)))
        .collect();

    let nodes = outcome
        .added
        .iter()
        .filter_map(|id| Some((scratch.path(*id)?.clone(), scratch.node(*id)?)))
        .map(|(path, node)| CachedNode {
            segment: path
                .segments()
                .last()
                .cloned()
                .unwrap_or_else(|| SeamSegment::new(String::new())),
            // Absent means "the node this file hangs from", which is not in this list.
            parent: node
                .parent
                .and_then(|parent| position.get(&parent).copied()),
            kind: node.kind,
            name: node.name.clone(),
            detail: node.detail.clone(),
            range: node.location.range,
            span: node.location.span,
            selection: node.location.selection,
            header: node.location.header,
            facets: node.facets.iter().map(structural_facet).collect(),
            visibility: node.visibility,
            provisional: node.provisional,
        })
        .collect();

    let external_modules = outcome
        .external_modules
        .iter()
        .filter_map(|declaration| {
            Some(CachedModule {
                node: position.get(&declaration.id).copied()?,
                name: declaration.name.clone(),
                inline_path: declaration.inline_path.clone(),
                path_attribute: declaration.path_attribute.clone(),
            })
        })
        .collect();

    let ownership = outcome
        .ownership
        .iter()
        .filter_map(|(id, owners)| Some((position.get(id).copied()?, owners.clone())))
        .collect();

    // Resolved here rather than at replay: it depends only on this file's declarations
    // and where the file sits, both of which are already known.
    let unresolved = outcome
        .external_modules
        .iter()
        .filter_map(|declaration| {
            let ModuleSource::Missing { candidates } = resolve(
                &unit.file,
                unit.crate_root,
                &declaration.inline_path,
                &declaration.name,
                declaration.path_attribute.as_deref(),
            ) else {
                return None;
            };
            Some((position.get(&declaration.id).copied()?, candidates))
        })
        .collect();

    Some(FileContribution {
        file: unit.file.clone(),
        stamp,
        owner: unit.owner.clone(),
        depth: unit.depth,
        crate_root: unit.crate_root,
        nodes,
        external_modules,
        ownership,
        unresolved,
    })
}

/// A stand-in for the node a file hangs from, so extraction has a parent to read.
fn stand_in(id: SeamId, kind: NodeKind) -> Node {
    Node {
        id,
        kind,
        name: String::new(),
        detail: None,
        location: SeamLocation {
            file: crate::model::FileId(0),
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
    }
}

/// Add the package's root node, under the segment assigned to it.
fn add_package_node(index: &mut SeamIndex, package: &Discovered, segment: SeamSegment) -> SeamId {
    let id = index.intern(SeamPath::new(vec![segment]));
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

impl FileUnit {
    /// A seed entry, resolved against the index the seeding step just built.
    fn from_pending(index: &SeamIndex, pending: crate::package::Pending) -> Option<Self> {
        Some(Self {
            owner: index.path(pending.parent)?.clone(),
            owner_kind: index
                .node(pending.parent)
                .map_or(NodeKind::Module, |n| n.kind),
            file: pending.file,
            language: pending.language,
            crate_root: pending.crate_root,
            depth: 0,
        })
    }

    /// The entry for a module declared by a file already read.
    fn child(file: PathBuf, owner: SeamPath, depth: u32) -> Option<Self> {
        Some(Self {
            language: karet_treesitter::language_id_from_path(&file)?,
            file,
            owner,
            owner_kind: NodeKind::Module,
            crate_root: false,
            depth: depth.saturating_add(1),
        })
    }
}

#[cfg(test)]
mod tests;

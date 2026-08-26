//! Finding the packages under a directory.
//!
//! [`index_workspace`](crate::index_workspace) reads a *repository*, not a package, and
//! real repositories are not one package at a known place. They are Cargo workspaces whose
//! root manifest declares no package at all, roots that are both a package and a
//! workspace, crates parked under `rust/` or `services/` with nothing at the top, and
//! repositories where the Rust and the Python live side by side.
//!
//! So discovery is separate from indexing, and deliberately cheap: it reads manifests and
//! lists directories, and never parses a line of source. That is what lets a caller ask
//! "what is in this tree?" — a picker offering start points, say — without paying for an
//! index, and it is why these tests need no grammar compiled in.
//!
//! Two rules shape the result:
//!
//! - **Both ecosystems always run.** Cargo discovery does not suppress Python discovery.
//!   A repository with a Rust workspace and a Python service is one repository, and
//!   answering with only half of it would be a quieter kind of wrong than failing.
//! - **The order is stable and meaningful.** Outermost first, then declaration order, then
//!   sorted. The order packages are discovered in is the order the view lists them in its
//!   first column, so a reshuffle between runs is user-visible.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

mod glob;
mod python;
mod scan;

pub(crate) use scan::skipped;

/// Which ecosystem's rules apply to a discovered package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PackageKind {
    /// A Cargo package: walked by following `mod` declarations from its crate roots.
    Cargo,
    /// A Python package: walked over the filesystem, because Python modules *are* files.
    Python,
}

/// One package found under a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The name its root node takes.
    pub name: String,
    /// The directory its sources are walked from.
    pub root: PathBuf,
    /// The file the root node points at, so the row has somewhere to open.
    pub anchor: PathBuf,
    /// Which ecosystem's walk applies.
    pub kind: PackageKind,
}

/// How far discovery is allowed to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryOptions {
    /// How many directory levels below the root the scan may descend.
    pub max_depth: usize,
    /// Stop after this many packages.
    pub max_packages: usize,
    /// Stop after visiting this many directories.
    ///
    /// Depth alone does not bound the walk — a shallow tree can still be enormous — and
    /// discovery runs synchronously while a reader waits for a picker to open.
    pub max_directories: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        // Three levels reaches `rust/api/`, `services/web/`, and `packages/group/one/`,
        // which is as deep as a repository layout goes before it is a vendored tree.
        Self {
            max_depth: 3,
            max_packages: 512,
            // Generous enough that no real repository is cut short, low enough that a
            // pathological tree cannot stall the picker for longer than it takes to
            // notice.
            max_directories: 8192,
        }
    }
}

/// The packages under `root`, Cargo first, then Python.
///
/// Infallible by design: an empty result means nothing indexable is here, which is an
/// answer a caller can act on — offering a picker, say — rather than an error it must
/// first unwrap. [`index_workspace`](crate::index_workspace) is where emptiness becomes a
/// failure, because *there* it means the reader asked to read nothing.
#[must_use]
pub fn discover(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut found = cargo(root, options);
    found.extend(python::discover(root, options));

    let mut seen: HashSet<PathBuf> = HashSet::new();
    found.retain(|package| seen.insert(package.root.clone()));
    found.truncate(options.max_packages);
    found
}

/// Every Cargo package under `root`.
fn cargo(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    collect_cargo(root, options, 0, &mut out, &mut seen);
    if out.is_empty() {
        // No manifest at the top, or one declaring neither a package nor members. The
        // crates may still be down a level, which is what `rust/`-style layouts do.
        out = scan_cargo(root, options);
    }
    out
}

/// Read `root`'s manifest, taking the package it declares and the members it lists.
///
/// `depth` bounds workspace-within-workspace nesting; a cycle through a symlinked member
/// would otherwise recurse forever.
fn collect_cargo(
    root: &Path,
    options: DiscoveryOptions,
    depth: usize,
    out: &mut Vec<Discovered>,
    seen: &mut HashSet<PathBuf>,
) {
    let manifest_path = root.join("Cargo.toml");
    let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
        return;
    };

    // A root that is both a package and a workspace is ordinary, and the package comes
    // first: it is the thing the directory is named after.
    if let Some(name) = dependable_core::parse_package_name(&manifest)
        && seen.insert(canonical(root))
    {
        out.push(Discovered {
            name,
            root: root.to_path_buf(),
            anchor: manifest_path.clone(),
            kind: PackageKind::Cargo,
        });
    }

    let Some(workspace) = dependable_core::parse_workspace(&manifest) else {
        return;
    };
    let excluded: HashSet<PathBuf> = workspace
        .exclude
        .iter()
        .flat_map(|pattern| glob::expand(root, pattern))
        .map(|path| canonical(&path))
        .collect();

    // `default-members` is a subset of `members` in any valid manifest, but taking the
    // union costs nothing and cannot lose a member to a malformed one.
    let patterns = workspace
        .members
        .iter()
        .chain(workspace.default_members.iter());
    for pattern in patterns {
        for member in glob::expand(root, pattern) {
            if out.len() >= options.max_packages {
                return;
            }
            if excluded.contains(&canonical(&member)) || scan::skipped(&member) {
                continue;
            }
            take_member(&member, options, depth, out, seen);
        }
    }
}

/// Take one workspace member: a package, or a nested workspace to recurse into once.
fn take_member(
    member: &Path,
    options: DiscoveryOptions,
    depth: usize,
    out: &mut Vec<Discovered>,
    seen: &mut HashSet<PathBuf>,
) {
    let manifest_path = member.join("Cargo.toml");
    let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    if let Some(name) = dependable_core::parse_package_name(&manifest) {
        if seen.insert(canonical(member)) {
            out.push(Discovered {
                name,
                root: member.to_path_buf(),
                anchor: manifest_path,
                kind: PackageKind::Cargo,
            });
        }
    } else if depth == 0 && dependable_core::parse_workspace(&manifest).is_some() {
        collect_cargo(member, options, depth + 1, out, seen);
    }
}

/// Cargo packages parked below a root that declares none.
fn scan_cargo(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut out = Vec::new();
    scan::walk(root, options.max_depth, options.max_directories, |dir| {
        if out.len() >= options.max_packages {
            return scan::Visit::Prune;
        }
        let manifest_path = dir.join("Cargo.toml");
        let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
            return scan::Visit::Descend;
        };
        if let Some(name) = dependable_core::parse_package_name(&manifest) {
            out.push(Discovered {
                name,
                root: dir.to_path_buf(),
                anchor: manifest_path,
                kind: PackageKind::Cargo,
            });
            // Its own members are its business; descending would find its examples and
            // its vendored copies and call them packages of this repository.
            return scan::Visit::Prune;
        }
        scan::Visit::Descend
    });
    out
}

/// A path in comparable form, falling back to the path itself when it cannot be resolved.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests;

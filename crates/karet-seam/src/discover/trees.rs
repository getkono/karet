//! Finding Node, Swift, and Gradle packages.
//!
//! All three are the same shape: a manifest file marks a directory as a package, and its
//! sources live in a conventional subdirectory. Only the manifest names and the source
//! layouts differ, so one walk serves all three and each ecosystem contributes a table.
//!
//! **The name comes from the directory, not the manifest.** `package.json` carries a
//! *distribution* name, which may be scoped (`@acme/widgets`) and need not match anything
//! importable; `Package.swift` and `settings.gradle` carry names a build tool resolves.
//! The directory is what a reader sees and what an import path is actually made of, and
//! taking it needs no second parser — the same call Python's discovery makes, for the same
//! reason.
//!
//! **A source root is a directory, not a glob.** Swift puts each target under
//! `Sources/<Target>`, Gradle puts each source set under `src/<set>/<language>`, and Node
//! puts everything under `src` when it uses one at all. Where the convention is absent the
//! package directory itself stands in, which is what a flat repository looks like.

use std::path::Path;
use std::path::PathBuf;

use super::Discovered;
use super::DiscoveryOptions;
use super::PackageKind;
use super::scan;

/// One ecosystem's rules: what marks a package, and where its sources are.
struct Ecosystem {
    /// Files that mark a directory as a package, in precedence order.
    manifests: &'static [&'static str],
    /// The kind discovered packages take.
    kind: PackageKind,
    /// Source roots, resolved against the package directory.
    roots: fn(&Path) -> Vec<PathBuf>,
}

/// Every ecosystem this walk knows, in the order their packages are listed.
const ECOSYSTEMS: &[Ecosystem] = &[
    Ecosystem {
        manifests: &["package.json"],
        kind: PackageKind::Node,
        roots: node_roots,
    },
    Ecosystem {
        manifests: &["Package.swift"],
        kind: PackageKind::Swift,
        roots: swift_roots,
    },
    Ecosystem {
        manifests: &["build.gradle.kts", "build.gradle"],
        kind: PackageKind::Gradle,
        roots: gradle_roots,
    },
];

/// Every Node, Swift, and Gradle package under `root`.
pub(super) fn discover(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut out = Vec::new();
    for ecosystem in ECOSYSTEMS {
        scan::walk(root, options.max_depth, options.max_directories, |dir| {
            if out.len() >= options.max_packages {
                return scan::Visit::Prune;
            }
            let Some(anchor) = manifest(dir, ecosystem.manifests) else {
                return scan::Visit::Descend;
            };
            out.extend((ecosystem.roots)(dir).into_iter().map(|source| Discovered {
                name: name_of(&source, dir),
                root: source,
                anchor: anchor.clone(),
                kind: ecosystem.kind,
            }));
            // A Gradle build is routinely a tree of them, and a monorepo nests Node
            // packages, so descending stays on for everything but the package's own
            // source directories — which hold modules, not further packages.
            scan::Visit::Descend
        });
    }
    out
}

/// The manifest marking `dir` as a package, if it has one.
fn manifest(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// What to call a package rooted at `source` inside `package`.
///
/// The source root's own name where it has a meaningful one — a Swift target, a Gradle
/// module — and the package directory's where the root is just `src`.
fn name_of(source: &Path, package: &Path) -> String {
    let named = |path: &Path| -> Option<String> {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
    };
    match named(source) {
        Some(name) if !GENERIC_ROOTS.contains(&name.as_str()) => name,
        _ => named(package).unwrap_or_default(),
    }
}

/// Directory names that say where sources are rather than what they are called.
const GENERIC_ROOTS: &[&str] = &["src", "kotlin", "java", "main", "Sources", "lib"];

/// A Node package's sources: `src` when it has one, else the package itself.
fn node_roots(package: &Path) -> Vec<PathBuf> {
    let src = package.join("src");
    if src.is_dir() {
        return vec![src];
    }
    vec![package.to_path_buf()]
}

/// A Swift package's targets: each directory under `Sources`, else `Sources` itself.
///
/// A single-target package puts its files loose in `Sources`, and one with several gives
/// each a directory. Both are ordinary, so both are read.
fn swift_roots(package: &Path) -> Vec<PathBuf> {
    let sources = package.join("Sources");
    if !sources.is_dir() {
        return vec![package.to_path_buf()];
    }
    let targets = children(&sources);
    if targets.is_empty() {
        vec![sources]
    } else {
        targets
    }
}

/// A Gradle module's source sets: `src/<set>/kotlin` and `src/<set>/java`.
///
/// Every set, not just `main`: `test` and the multiplatform `commonMain`/`jvmMain` are
/// source the reader asked about, and which of them is "the" one is a build-tool question
/// this has no business answering.
fn gradle_roots(package: &Path) -> Vec<PathBuf> {
    let src = package.join("src");
    if !src.is_dir() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = children(&src)
        .into_iter()
        .flat_map(|set| [set.join("kotlin"), set.join("java")])
        .filter(|path| path.is_dir())
        .collect();
    out.sort();
    out
}

/// A directory's subdirectories, sorted, skipping the ones never worth walking.
fn children(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !scan::skipped(path))
        .collect();
    out.sort();
    out
}

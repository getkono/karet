//! Finding Node, Swift, Gradle, Maven and C/C++ packages.
//!
//! All of them are the same shape: a manifest file marks a directory as a package, and its
//! sources live in a conventional subdirectory. Only the manifest names and the source
//! layouts differ, so one walk serves them all and each ecosystem contributes a table.
//!
//! C and C++ have no manifest, so the build file stands in for one: `CMakeLists.txt`,
//! `meson.build` or a `Makefile`. A CMake project routinely puts one in every directory,
//! each `add_subdirectory`'d from the one above, so the outermost build file claims the
//! tree and the ones below it are not packages of their own.
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
    /// Whether a package of this kind may contain further packages of the same kind.
    nests: bool,
}

/// Every ecosystem this walk knows, in the order their packages are listed.
const ECOSYSTEMS: &[Ecosystem] = &[
    Ecosystem {
        manifests: &["package.json"],
        kind: PackageKind::Node,
        roots: node_roots,
        nests: true,
    },
    Ecosystem {
        manifests: &["Package.swift"],
        kind: PackageKind::Swift,
        roots: swift_roots,
        nests: true,
    },
    Ecosystem {
        manifests: &["build.gradle.kts", "build.gradle"],
        kind: PackageKind::Gradle,
        roots: gradle_roots,
        nests: true,
    },
    // A parent `pom.xml` lists its modules in subdirectories, so a walk that keeps
    // descending finds each of them as the package it is.
    Ecosystem {
        manifests: &["pom.xml"],
        kind: PackageKind::Maven,
        roots: maven_roots,
        nests: true,
    },
    Ecosystem {
        manifests: &["CMakeLists.txt", "meson.build", "Makefile", "GNUmakefile"],
        kind: PackageKind::CFamily,
        roots: c_family_roots,
        nests: false,
    },
];

/// Every Node, Swift, Gradle, Maven and C/C++ package under `root`.
pub(super) fn discover(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut out = Vec::new();
    for ecosystem in ECOSYSTEMS {
        scan::walk(root, options.max_depth, options.max_directories, |dir| {
            if out.len() >= options.max_packages {
                return scan::Visit::Prune;
            }
            // A vendored copy carries its own build file and is still not this repository's.
            if dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| ecosystem.kind.vendored_directories().contains(&name))
            {
                return scan::Visit::Prune;
            }
            let Some(anchor) = manifest(dir, ecosystem.manifests) else {
                return scan::Visit::Descend;
            };
            let roots = (ecosystem.roots)(dir);
            let found = !roots.is_empty();
            out.extend(roots.into_iter().map(|source| Discovered {
                name: name_of(&source, dir),
                root: source,
                anchor: anchor.clone(),
                kind: ecosystem.kind,
            }));
            // A Gradle build is routinely a tree of them, and a monorepo nests Node
            // packages, so descending stays on for everything but the package's own
            // source directories — which hold modules, not further packages. A C build
            // file's subdirectories are its own, so there the outermost one that really
            // is a C project wins.
            if ecosystem.nests || !found {
                scan::Visit::Descend
            } else {
                scan::Visit::Prune
            }
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

/// A Maven module's source sets: `src/<set>/java`.
fn maven_roots(package: &Path) -> Vec<PathBuf> {
    let src = package.join("src");
    let mut out: Vec<PathBuf> = children(&src)
        .into_iter()
        .map(|set| set.join("java"))
        .filter(|path| path.is_dir())
        .collect();
    out.sort();
    out
}

/// A C or C++ project's sources: the project directory itself.
///
/// One root rather than `src` and `include` separately, because a header and its
/// definition are one module's two halves and splitting them would make two packages of
/// one library. A bare `Makefile` is the build file of every kind of project, so it marks
/// a C project only when C or C++ sources sit beside it or in its `src`.
fn c_family_roots(package: &Path) -> Vec<PathBuf> {
    let declared = ["CMakeLists.txt", "meson.build"]
        .iter()
        .any(|name| package.join(name).is_file());
    if declared || has_c_sources(package) || has_c_sources(&package.join("src")) {
        vec![package.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Whether `directory` directly holds a C or C++ source or header.
fn has_c_sources(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| PackageKind::CFamily.extensions().contains(&extension))
    })
}

/// A directory's subdirectories, sorted, skipping the ones never worth walking.
fn children(directory: &Path) -> Vec<PathBuf> {
    scan::subdirs(directory, scan::skipped)
}

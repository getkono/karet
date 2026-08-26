//! Finding Python projects, and the importable packages inside them.
//!
//! Two decisions here are worth stating, because both could plausibly have gone the other
//! way and neither is recoverable from the code alone.
//!
//! **A project is marked by a project file, never by loose `.py`.** `pyproject.toml`,
//! `setup.py`, and `setup.cfg` each say "this directory is a distribution". A directory
//! that merely contains Python does not: build scripts, notebooks, and one-off tools live
//! all over a repository, and rooting the view at them would fill the package column with
//! things nobody would call a package.
//!
//! **The root's name comes from the filesystem, not the manifest.** `[project] name` is a
//! *distribution* name — `my-app` — and the importable package is `my_app`. Nothing in the
//! manifest states that mapping; setuptools infers it, and the inference is configurable.
//! Since the seam path is an *import* path, the directory holding `__init__.py` is the
//! only honest source for its first segment. It also means no TOML parser is needed here.

use std::path::Path;
use std::path::PathBuf;

use super::Discovered;
use super::DiscoveryOptions;
use super::PackageKind;
use super::scan;

/// Files that mark a directory as a Python distribution.
const PROJECT_FILES: &[&str] = &["pyproject.toml", "setup.py", "setup.cfg"];

/// Directory names that sit beside a package without being one.
///
/// Only consulted for the flat layout, where a package directory and a support directory
/// are siblings and look alike. The `src/` layout exists precisely to make this
/// unnecessary, so it is not applied there.
const NOT_PACKAGES: &[&str] = &[
    "tests",
    "test",
    "docs",
    "doc",
    "examples",
    "example",
    "scripts",
    "benchmarks",
    "migrations",
];

/// Every Python package under `root`.
pub(super) fn discover(root: &Path, options: DiscoveryOptions) -> Vec<Discovered> {
    let mut out = Vec::new();
    scan::walk(root, options.max_depth, |dir| {
        if out.len() >= options.max_packages {
            return scan::Visit::Prune;
        }
        let Some(anchor) = project_file(dir) else {
            return scan::Visit::Descend;
        };
        out.extend(importable_roots(dir).into_iter().map(|package| {
            Discovered {
                name: package
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_owned(),
                root: package,
                anchor: anchor.clone(),
                kind: PackageKind::Python,
            }
        }));
        // A project's own subdirectories are its modules, not further projects.
        scan::Visit::Prune
    });
    out
}

/// The project file marking `dir` as a distribution, if it has one.
fn project_file(dir: &Path) -> Option<PathBuf> {
    PROJECT_FILES
        .iter()
        .map(|name| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The importable packages a project directory exposes, in precedence order.
///
/// The `src/` layout wins outright when it is present: a project that has one has said
/// where its packages are, and looking anywhere else would second-guess it.
#[must_use]
pub(crate) fn importable_roots(project: &Path) -> Vec<PathBuf> {
    let src = project.join("src");
    if src.is_dir() {
        let packages = package_dirs(&src, false);
        if !packages.is_empty() {
            return packages;
        }
    }
    let packages = package_dirs(project, true);
    if !packages.is_empty() {
        return packages;
    }
    // A flat project with no package directory at all: the modules sit loose beside the
    // manifest, and the project directory is what holds them.
    if holds_modules(project) {
        vec![project.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Subdirectories of `base` that are importable packages, sorted.
fn package_dirs(base: &Path, filter_support: bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !scan::skipped(path))
        .filter(|path| !filter_support || !is_support(path))
        .filter(|path| path.join("__init__.py").is_file())
        .collect();
    out.sort();
    out
}

/// Whether a directory name is one that conventionally sits beside a package.
fn is_support(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| NOT_PACKAGES.contains(&name))
}

/// Whether a directory directly holds at least one importable module.
fn holds_modules(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.path().extension().is_some_and(|ext| ext == "py") && entry.path().is_file()
    })
}

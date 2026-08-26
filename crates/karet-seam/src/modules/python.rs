//! Walking a Python package's modules over the filesystem.
//!
//! Rust's module tree is *declared* — `mod net;` says where to look next — so
//! [`resolve`](super::resolve) follows declarations and the filesystem only answers the
//! questions the source asks. Python declares nothing. Its modules *are* files and its
//! packages *are* directories, so the walk has to be the other way round, and
//! `SeamLanguage::external_module` is simply the wrong hook for it.
//!
//! One rule carries almost all of it: **`__init__.py` is what makes a directory a
//! package**. A directory that has one is a module and is descended into; a directory that
//! does not is a root holding loose modules, and the walk stops there. That single test
//! keeps `tests/`, `docs/`, and a stray `.venv` out of a flat-layout project without a
//! list of names to maintain — and unlike a list, it cannot go out of date.

use std::path::Path;
use std::path::PathBuf;

/// How deep a package may nest before the walk gives up.
///
/// Far beyond any real import path; a runaway guard against a symlink cycle, not a policy.
const MAX_DEPTH: usize = 16;

/// One module found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyModule {
    /// The file holding its text.
    pub file: PathBuf,
    /// Its import path below the package root, outermost first.
    ///
    /// Empty for the package's own `__init__.py`, which is what attaches its contents to
    /// the package node itself rather than to a child module named `__init__`.
    pub segments: Vec<String>,
}

/// Every module in the package rooted at `directory`, parents strictly before children.
///
/// The ordering is a contract, not an accident: the caller creates a node per module and
/// each one needs its parent to exist already.
#[must_use]
pub fn walk(directory: &Path) -> Vec<PyModule> {
    let mut out = Vec::new();
    if directory.join("__init__.py").is_file() {
        out.push(PyModule {
            file: directory.join("__init__.py"),
            segments: Vec::new(),
        });
        descend(directory, &mut Vec::new(), 0, &mut out);
    } else {
        // A root with no `__init__.py` holds loose modules. Descending would sweep in the
        // tests and the docs and the virtual environment beside them.
        out.extend(modules_in(directory, &[]));
    }
    out
}

/// Collect the modules and packages inside a directory that is itself a package.
fn descend(directory: &Path, segments: &mut Vec<String>, depth: usize, out: &mut Vec<PyModule>) {
    if depth >= MAX_DEPTH {
        return;
    }
    let shadowed = package_names(directory);
    out.extend(
        modules_in(directory, segments)
            .into_iter()
            // A directory beats a same-named module, as it does at import time: the
            // package shadows `bar.py` and `import bar` reaches `bar/__init__.py`.
            .filter(|module| {
                module
                    .segments
                    .last()
                    .is_none_or(|name| !shadowed.contains(name))
            }),
    );

    for child in subdirectories(directory) {
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_package = child.join("__init__.py").is_file();
        // A PEP 420 namespace package has no `__init__.py` and so contributes no node of
        // its own, but the modules beneath it are importable and must not be lost.
        if !is_package && !holds_modules(&child) {
            continue;
        }
        segments.push(name.to_owned());
        if is_package {
            out.push(PyModule {
                file: child.join("__init__.py"),
                segments: segments.clone(),
            });
        }
        descend(&child, segments, depth + 1, out);
        segments.pop();
    }
}

/// The `.py` files directly inside a directory, as modules under `segments`.
fn modules_in(directory: &Path, segments: &[String]) -> Vec<PyModule> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        // `.pyi` is deliberately absent. A stub declares the same API as the module beside
        // it, so indexing both would put two nodes at one identity.
        .filter(|path| path.extension().is_some_and(|ext| ext == "py"))
        .filter(|path| path.file_name().is_some_and(|name| name != "__init__.py"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let stem = file.file_stem()?.to_str()?.to_owned();
            let mut path = segments.to_vec();
            path.push(stem);
            Some(PyModule {
                file,
                segments: path,
            })
        })
        .collect()
}

/// The names of subdirectories that are packages, for shadow resolution.
fn package_names(directory: &Path) -> Vec<String> {
    subdirectories(directory)
        .into_iter()
        .filter(|path| path.join("__init__.py").is_file())
        .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
        .collect()
}

/// A directory's importable subdirectories, sorted by name.
fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !crate::discover::skipped(path))
        .collect();
    out.sort();
    out
}

/// Whether a directory holds an importable module anywhere beneath it.
fn holds_modules(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        (path.is_file() && path.extension().is_some_and(|ext| ext == "py"))
            || (path.is_dir() && path.join("__init__.py").is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Build a package tree from `(relative path, contents)` pairs.
    fn tree(files: &[&str]) -> Result<tempfile::TempDir, std::io::Error> {
        let dir = tempfile::tempdir()?;
        for relative in files {
            let path = dir.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, "")?;
        }
        Ok(dir)
    }

    /// The import paths the walk found, in the order it found them.
    fn paths(directory: &Path) -> Vec<String> {
        walk(directory)
            .into_iter()
            .map(|module| module.segments.join("."))
            .collect()
    }

    #[test]
    fn the_packages_own_init_carries_no_segments() -> TestResult {
        let dir = tree(&["__init__.py", "model.py"])?;
        let found = walk(dir.path());
        // An empty path is what attaches its contents to the package node itself, rather
        // than to a child module named `__init__`.
        assert_eq!(found.first().map(|m| m.segments.len()), Some(0));
        assert_eq!(
            found.first().map(|m| m.file.clone()),
            Some(dir.path().join("__init__.py"))
        );
        Ok(())
    }

    #[test]
    fn modules_appear_by_import_path() -> TestResult {
        let dir = tree(&[
            "__init__.py",
            "model.py",
            "net/__init__.py",
            "net/client.py",
        ])?;
        assert_eq!(paths(dir.path()), ["", "model", "net", "net.client"]);
        Ok(())
    }

    #[test]
    fn parents_are_always_emitted_before_their_children() -> TestResult {
        // A contract the caller depends on: it creates a node per module, and each needs
        // its parent to exist already.
        let dir = tree(&[
            "__init__.py",
            "a/__init__.py",
            "a/b/__init__.py",
            "a/b/c/__init__.py",
            "a/b/c/deep.py",
        ])?;
        let found = paths(dir.path());
        for (index, path) in found.iter().enumerate() {
            let Some((parent, _)) = path.rsplit_once('.') else {
                continue;
            };
            let at = found.iter().position(|p| p == parent);
            assert!(
                at.is_some_and(|at| at < index),
                "{path} precedes its parent"
            );
        }
        Ok(())
    }

    #[test]
    fn a_directory_shadows_a_module_of_the_same_name() -> TestResult {
        // What `import bar` actually reaches. Note this is the opposite of Rust, where
        // `net.rs` beats `net/mod.rs` — each language's own rule, not a shared one.
        let dir = tree(&["__init__.py", "bar.py", "bar/__init__.py", "bar/inner.py"])?;
        assert_eq!(paths(dir.path()), ["", "bar", "bar.inner"]);
        Ok(())
    }

    #[test]
    fn a_namespace_package_contributes_its_modules_but_no_node() -> TestResult {
        // PEP 420: no `__init__.py`, so nothing to attach a node to — but the modules
        // beneath it are importable and dropping them would deny they exist.
        let dir = tree(&["__init__.py", "space/thing.py"])?;
        assert_eq!(paths(dir.path()), ["", "space.thing"]);
        Ok(())
    }

    #[test]
    fn a_root_without_an_init_takes_its_loose_modules_and_stops() -> TestResult {
        let dir = tree(&["tool.py", "helper.py", "tests/test_tool.py", "docs/conf.py"])?;
        // No descent, so the tests and the docs never arrive.
        assert_eq!(paths(dir.path()), ["helper", "tool"]);
        Ok(())
    }

    #[test]
    fn a_stub_is_not_a_second_copy_of_its_module() -> TestResult {
        let dir = tree(&["__init__.py", "model.py", "model.pyi"])?;
        assert_eq!(paths(dir.path()), ["", "model"]);
        Ok(())
    }

    #[test]
    fn caches_and_environments_are_never_walked() -> TestResult {
        let dir = tree(&[
            "__init__.py",
            "real.py",
            "__pycache__/real.cpython-312.pyc",
            ".venv/lib/thing.py",
        ])?;
        assert_eq!(paths(dir.path()), ["", "real"]);
        Ok(())
    }

    #[test]
    fn a_dunder_main_is_a_module_like_any_other() -> TestResult {
        // A real entry point, and where a package's boundary seams live.
        let dir = tree(&["__init__.py", "__main__.py"])?;
        assert_eq!(paths(dir.path()), ["", "__main__"]);
        Ok(())
    }

    #[test]
    fn an_unreadable_directory_yields_nothing_rather_than_panicking() {
        assert!(walk(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn modules_are_listed_in_a_stable_sorted_order() -> TestResult {
        let dir = tree(&["__init__.py", "zeta.py", "alpha.py", "mid.py"])?;
        assert_eq!(paths(dir.path()), ["", "alpha", "mid", "zeta"]);
        Ok(())
    }
}

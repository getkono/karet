//! Walking a source tree whose modules are its files and directories.
//!
//! Rust declares its module tree — `mod net;` says where to look next — so
//! [`resolve`](super::resolve) follows declarations. Python, TypeScript, Swift and Kotlin
//! declare nothing of the kind: their files *are* modules and their directories *are*
//! namespaces, so the walk goes the other way and the filesystem is the source of truth.
//!
//! Python has its own walk because `__init__.py` carries a rule nothing else shares — it
//! is what makes a directory a package at all. Everywhere else a directory holding source
//! is a namespace and one holding none is not, which is all this needs to know.
//!
//! One convention is honoured where a language has it: a file named `index` (or whatever
//! the caller names) *is* its directory's module rather than a module beside it, exactly
//! as `__init__.py` and `mod.rs` are. A language without the convention passes none and
//! every file is its own module.

use std::path::Path;
use std::path::PathBuf;

use crate::discover::skipped;

/// How deep a source tree may nest before the walk gives up.
///
/// Far beyond any real namespace; a runaway guard against a symlink cycle, not a policy.
const MAX_DEPTH: usize = 16;

/// One module found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileModule {
    /// The file it points at: its own text, or — for a directory with no index file —
    /// the first file beneath it, so the row has somewhere to open.
    pub file: PathBuf,
    /// Its namespace path below the root, outermost first.
    ///
    /// Empty for the root's own index file, whose contents belong to the package node
    /// itself rather than to a child module named `index`.
    pub segments: Vec<String>,
    /// Whether [`Self::file`]'s contents *are* this module's contents.
    ///
    /// False for a directory standing in for a namespace: the file it points at has a
    /// module of its own, and extracting it twice would double every node in it.
    pub extract: bool,
}

/// Every module under `directory`, parents strictly before children.
///
/// The ordering is a contract, not an accident: the caller creates a node per module and
/// each one needs its parent to exist already.
#[must_use]
pub fn walk(directory: &Path, extensions: &[&str], index_names: &[&str]) -> Vec<FileModule> {
    let mut out = Vec::new();
    if let Some(index) = index_file(directory, extensions, index_names) {
        out.push(FileModule {
            file: index,
            segments: Vec::new(),
            extract: true,
        });
    }
    descend(directory, &[], 0, extensions, index_names, &mut out);
    out
}

/// Collect the modules inside one directory, then recurse into its subdirectories.
fn descend(
    directory: &Path,
    segments: &[String],
    depth: usize,
    extensions: &[&str],
    index_names: &[&str],
    out: &mut Vec<FileModule>,
) {
    if depth >= MAX_DEPTH {
        return;
    }
    let index = index_file(directory, extensions, index_names);
    for file in sources_in(directory, extensions) {
        if Some(&file) == index.as_ref() {
            continue;
        }
        let Some(stem) = stem(&file) else {
            continue;
        };
        let mut path = segments.to_vec();
        path.push(stem);
        out.push(FileModule {
            file,
            segments: path,
            extract: true,
        });
    }

    for child in subdirectories(directory) {
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A directory holding no source anywhere beneath it is not a namespace. Emitting
        // one would put an empty row in the spine for every `assets/` in the tree.
        let Some(anchor) = first_source(&child, extensions, depth) else {
            continue;
        };
        let mut path = segments.to_vec();
        path.push(name.to_owned());
        match index_file(&child, extensions, index_names) {
            Some(index) => out.push(FileModule {
                file: index,
                segments: path.clone(),
                extract: true,
            }),
            None => out.push(FileModule {
                file: anchor,
                segments: path.clone(),
                extract: false,
            }),
        }
        descend(&child, &path, depth + 1, extensions, index_names, out);
    }
}

/// The file that *is* this directory's module, by the caller's naming convention.
fn index_file(directory: &Path, extensions: &[&str], index_names: &[&str]) -> Option<PathBuf> {
    index_names.iter().find_map(|name| {
        extensions
            .iter()
            .map(|extension| directory.join(format!("{name}.{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

/// The source files directly in `directory`, sorted.
fn sources_in(directory: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && has_extension(path, extensions))
        .collect();
    out.sort();
    out
}

/// The subdirectories worth descending into, sorted.
fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !skipped(path))
        .collect();
    out.sort();
    out
}

/// The first source file at or beneath `directory`, if it holds any.
fn first_source(directory: &Path, extensions: &[&str], depth: usize) -> Option<PathBuf> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(file) = sources_in(directory, extensions).into_iter().next() {
        return Some(file);
    }
    subdirectories(directory)
        .into_iter()
        .find_map(|child| first_source(&child, extensions, depth + 1))
}

/// Whether a path carries one of the extensions, compared case-insensitively.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|found| {
            extensions
                .iter()
                .any(|wanted| found.eq_ignore_ascii_case(wanted))
        })
}

/// A file's name without its extension.
fn stem(file: &Path) -> Option<String> {
    file.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `files` under a fresh temporary directory.
    fn tree(files: &[&str]) -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().ok()?;
        for file in files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok()?;
            }
            std::fs::write(&path, "").ok()?;
        }
        Some(dir)
    }

    /// The walk's result as `segments:extract` strings, in order.
    fn walked(dir: &Path, extensions: &[&str], index_names: &[&str]) -> Vec<String> {
        walk(dir, extensions, index_names)
            .into_iter()
            .map(|module| {
                format!(
                    "{}:{}",
                    module.segments.join("/"),
                    if module.extract { "own" } else { "dir" }
                )
            })
            .collect()
    }

    #[test]
    fn every_source_file_becomes_a_module_and_every_directory_a_namespace() {
        let Some(dir) = tree(&["a.ts", "sub/b.ts", "sub/deep/c.ts"]) else {
            return;
        };
        assert_eq!(
            walked(dir.path(), &["ts"], &[]),
            [
                "a:own",
                "sub:dir",
                "sub/b:own",
                "sub/deep:dir",
                "sub/deep/c:own"
            ]
        );
    }

    #[test]
    fn an_index_file_is_its_directory_rather_than_a_module_beside_it() {
        let Some(dir) = tree(&["index.ts", "sub/index.ts", "sub/b.ts"]) else {
            return;
        };
        // The root's own index has no segments: its contents are the package's.
        assert_eq!(
            walked(dir.path(), &["ts"], &["index"]),
            [":own", "sub:own", "sub/b:own"]
        );
    }

    #[test]
    fn without_the_convention_an_index_file_is_an_ordinary_module() {
        let Some(dir) = tree(&["index.ts"]) else {
            return;
        };
        assert_eq!(walked(dir.path(), &["ts"], &[]), ["index:own"]);
    }

    #[test]
    fn a_directory_holding_no_source_is_not_a_namespace() {
        let Some(dir) = tree(&["a.ts", "assets/logo.png", "empty/nested/other.png"]) else {
            return;
        };
        assert_eq!(walked(dir.path(), &["ts"], &[]), ["a:own"]);
    }

    #[test]
    fn a_directory_whose_source_is_further_down_is_still_a_namespace() {
        let Some(dir) = tree(&["outer/inner/a.ts"]) else {
            return;
        };
        assert_eq!(
            walked(dir.path(), &["ts"], &[]),
            ["outer:dir", "outer/inner:dir", "outer/inner/a:own"]
        );
    }

    #[test]
    fn several_extensions_are_read_as_one_tree() {
        let Some(dir) = tree(&["a.ts", "b.tsx", "c.js", "d.md"]) else {
            return;
        };
        assert_eq!(
            walked(dir.path(), &["ts", "tsx", "js"], &[]),
            ["a:own", "b:own", "c:own"]
        );
    }

    #[test]
    fn build_output_and_dependency_caches_are_never_walked() {
        let Some(dir) = tree(&[
            "a.ts",
            "node_modules/dep/index.ts",
            "dist/a.js",
            ".git/x.ts",
        ]) else {
            return;
        };
        assert_eq!(walked(dir.path(), &["ts", "js"], &[]), ["a:own"]);
    }

    #[test]
    fn parents_always_come_before_their_children() {
        let Some(dir) = tree(&["z/deep/a.ts", "b.ts"]) else {
            return;
        };
        // The caller creates a node per module and each needs its parent to exist.
        let order = walked(dir.path(), &["ts"], &[]);
        let at = |needle: &str| order.iter().position(|entry| entry == needle);
        assert!(at("z:dir") < at("z/deep:dir"));
        assert!(at("z/deep:dir") < at("z/deep/a:own"));
    }

    #[test]
    fn a_missing_directory_walks_to_nothing_rather_than_failing() {
        assert!(walk(Path::new("/nope/absent"), &["ts"], &[]).is_empty());
    }
}

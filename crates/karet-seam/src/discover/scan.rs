//! A bounded, deterministic directory walk.
//!
//! Discovery reaches the filesystem in two places — finding packages a manifest never
//! declared, and finding Python projects — and both need the same three properties: a
//! depth bound so a pattern broader than its author intended cannot walk a whole disk, a
//! skip set so build output and virtual environments are never mistaken for source, and a
//! stable order, because `read_dir` order is not stable and the order packages are
//! discovered in is what the view shows as its first column.

use std::path::Path;
use std::path::PathBuf;

/// Directory names that never hold a package worth indexing.
///
/// Build output, dependency caches, and virtual environments all contain real source that
/// is nonetheless not *this* repository's. Indexing it would bury the packages the reader
/// asked about under thousands of nodes they did not.
const SKIPPED: &[&str] = &[
    "target",
    "node_modules",
    "__pycache__",
    "site-packages",
    "dist",
    "build",
    "vendor",
];

/// Whether a directory should never be descended into.
#[must_use]
pub(crate) fn skipped(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    // Hidden directories cover `.git`, `.venv`, `.tox`, `.mypy_cache` and the rest in one
    // rule, rather than a list that is always one entry out of date.
    name.starts_with('.')
        || name.ends_with(".egg-info")
        || SKIPPED.contains(&name)
        // Whatever a virtual environment is named, this file is what makes it one.
        || path.join("pyvenv.cfg").is_file()
}

/// What to do with a directory the walk has offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Visit {
    /// Look inside it too.
    Descend,
    /// Take it as it is and go no deeper.
    Prune,
}

/// Offer `root` and the directories beneath it to `visit`, breadth-first, in sorted order.
///
/// `max_depth` counts levels below `root`, so a depth of 0 offers only `root` itself.
/// Skipped directories are never offered at all.
pub(crate) fn walk(root: &Path, max_depth: usize, mut visit: impl FnMut(&Path) -> Visit) {
    if !root.is_dir() {
        return;
    }
    let mut level = match visit(root) {
        Visit::Descend => vec![root.to_path_buf()],
        Visit::Prune => return,
    };
    for _ in 0..max_depth {
        let mut next = Vec::new();
        for parent in &level {
            for child in directories(parent) {
                if skipped(&child) {
                    continue;
                }
                if visit(&child) == Visit::Descend {
                    next.push(child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
}

/// A directory's immediate subdirectories, sorted by name.
fn directories(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn tree(dirs: &[&str]) -> Result<tempfile::TempDir, std::io::Error> {
        let root = tempfile::tempdir()?;
        for dir in dirs {
            std::fs::create_dir_all(root.path().join(dir))?;
        }
        Ok(root)
    }

    /// Every directory the walk offers, relative to `root`, in the order offered.
    fn visited(root: &Path, max_depth: usize) -> Vec<String> {
        let mut seen = Vec::new();
        walk(root, max_depth, |path| {
            seen.push(
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            Visit::Descend
        });
        seen
    }

    #[test]
    fn the_walk_offers_the_root_first_then_sorted_children() -> TestResult {
        let root = tree(&["zeta", "alpha", "mid"])?;
        assert_eq!(visited(root.path(), 1), ["", "alpha", "mid", "zeta"]);
        Ok(())
    }

    #[test]
    fn the_depth_cap_stops_the_descent() -> TestResult {
        let root = tree(&["a/b/c/d"])?;
        assert_eq!(visited(root.path(), 0), [""]);
        assert_eq!(visited(root.path(), 2), ["", "a", "a/b"]);
        Ok(())
    }

    #[test]
    fn build_output_and_dependency_caches_are_never_offered() -> TestResult {
        let root = tree(&["target/debug", "node_modules/pkg", "src", "__pycache__"])?;
        assert_eq!(visited(root.path(), 2), ["", "src"]);
        Ok(())
    }

    #[test]
    fn hidden_directories_are_skipped_as_a_class() -> TestResult {
        let root = tree(&[".git/objects", ".venv/lib", ".mypy_cache", "src"])?;
        assert_eq!(visited(root.path(), 2), ["", "src"]);
        Ok(())
    }

    #[test]
    fn a_virtual_environment_is_skipped_whatever_it_is_named() -> TestResult {
        let root = tree(&["env/lib", "src"])?;
        std::fs::write(root.path().join("env").join("pyvenv.cfg"), "home = /usr")?;
        assert_eq!(visited(root.path(), 2), ["", "src"]);
        Ok(())
    }

    #[test]
    fn an_egg_info_directory_is_skipped() -> TestResult {
        let root = tree(&["thing.egg-info", "src"])?;
        assert_eq!(visited(root.path(), 2), ["", "src"]);
        Ok(())
    }

    #[test]
    fn pruning_takes_a_directory_without_descending_into_it() -> TestResult {
        let root = tree(&["a/deep", "b"])?;
        let mut seen = Vec::new();
        walk(root.path(), 3, |path| {
            let name = path
                .strip_prefix(root.path())
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            seen.push(name.clone());
            if name == "a" {
                Visit::Prune
            } else {
                Visit::Descend
            }
        });
        assert_eq!(seen, ["", "a", "b"]);
        Ok(())
    }

    #[test]
    fn pruning_the_root_ends_the_walk() -> TestResult {
        let root = tree(&["a"])?;
        let mut seen = 0usize;
        walk(root.path(), 3, |_| {
            seen += 1;
            Visit::Prune
        });
        assert_eq!(seen, 1);
        Ok(())
    }

    #[test]
    fn a_path_that_is_not_a_directory_offers_nothing() -> TestResult {
        let root = tempfile::tempdir()?;
        let file = root.path().join("notes.md");
        std::fs::write(&file, "x")?;
        let mut seen = 0usize;
        walk(&file, 3, |_| {
            seen += 1;
            Visit::Descend
        });
        assert_eq!(seen, 0);
        Ok(())
    }
}

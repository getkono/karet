//! The start points the Seam view can be opened on.
//!
//! Pure, so the ordering rules are testable without a terminal or a filesystem. The two
//! that matter:
//!
//! **The workspace root leads.** The picker and `Ctrl+K S` must not disagree about what
//! the obvious choice is — an Enter with nothing typed has to index the same thing the
//! chord does, or a mistyped query silently becomes a different view.
//!
//! **A start point is offered once.** The reader's context and the discovered packages
//! overlap constantly: the crate you have a file open in is also a workspace member. It
//! appears where the context put it, keeping the package name discovery knows.

use std::path::Path;
use std::path::PathBuf;

use karet_seam::Discovered;

/// Where a candidate came from, shown as the row's tail so the order reads rather than
/// having to be memorized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    WorkspaceRoot,
    Explorer,
    CurrentFile,
    Package,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::WorkspaceRoot => "workspace root",
            Self::Explorer => "explorer",
            Self::CurrentFile => "current file",
            Self::Package => "package",
        }
    }
}

/// Build the picker rows: `(label, root)` pairs, in offer order.
///
/// Both context paths are already directories — the caller knows whether it holds a file
/// or a directory, which is better information than this could recover by asking the
/// filesystem, and keeps the ordering rules testable without one.
#[must_use]
pub(crate) fn candidates(
    root: &Path,
    current_file_dir: Option<PathBuf>,
    explorer_dir: Option<PathBuf>,
    discovered: Vec<Discovered>,
) -> Vec<(String, PathBuf)> {
    // Indexed first, so a context row still gets the package name discovery found for it.
    let named: Vec<(PathBuf, String)> = discovered
        .iter()
        .map(|package| (package.root.clone(), package.name.clone()))
        .collect();

    let mut offered: Vec<PathBuf> = Vec::new();
    let mut rows: Vec<(String, PathBuf)> = Vec::new();
    let mut offer = |path: PathBuf, source: Source| {
        if offered.contains(&path) {
            return;
        }
        let name = named
            .iter()
            .find(|(candidate, _)| *candidate == path)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| directory_name(&path));
        rows.push((label(&name, &path, root, source), path.clone()));
        offered.push(path);
    };

    offer(root.to_path_buf(), Source::WorkspaceRoot);
    if let Some(path) = explorer_dir {
        offer(resolve(root, &path), Source::Explorer);
    }
    if let Some(path) = current_file_dir {
        offer(resolve(root, &path), Source::CurrentFile);
    }
    for (path, _) in named.clone() {
        offer(path, Source::Package);
    }
    rows
}

/// One row: the name, where it lives, and why it is on offer.
///
/// Both the name and the path are in the label so a fuzzy match finds either — a reader
/// types `karet-core` or `crates/karet` depending on which they hold in mind.
fn label(name: &str, path: &Path, root: &Path, source: Source) -> String {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() {
        format!("{name}  · {}", source.label())
    } else {
        format!("{name}  {relative}  · {}", source.label())
    }
}

/// Make a path absolute against the workspace root.
fn resolve(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

/// A directory's own name, for a path discovery had no package name for.
fn directory_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("/")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use karet_seam::PackageKind;

    use super::*;

    fn package(name: &str, root: &str) -> Discovered {
        Discovered {
            name: name.to_owned(),
            root: PathBuf::from(root),
            anchor: PathBuf::from(root).join("Cargo.toml"),
            kind: PackageKind::Cargo,
        }
    }

    fn roots(rows: &[(String, PathBuf)]) -> Vec<&Path> {
        rows.iter().map(|(_, path)| path.as_path()).collect()
    }

    #[test]
    fn the_workspace_root_is_always_offered_first() {
        // An Enter with nothing typed must index what `Ctrl+K S` would, or the two
        // disagree about the obvious choice.
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/crates/a/src")),
            Some(PathBuf::from("/repo/crates/b")),
            vec![package("a", "/repo/crates/a")],
        );
        assert_eq!(roots(&rows).first(), Some(&Path::new("/repo")));
        assert!(rows[0].0.contains("workspace root"));
    }

    #[test]
    fn a_start_point_reached_two_ways_is_offered_once() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("/repo/crates/a")),
            vec![
                package("a", "/repo/crates/a"),
                package("b", "/repo/crates/b"),
            ],
        );
        let a: Vec<_> = roots(&rows)
            .into_iter()
            .filter(|path| *path == Path::new("/repo/crates/a"))
            .collect();
        assert_eq!(a.len(), 1, "offered twice: {rows:?}");
    }

    #[test]
    fn a_context_row_still_carries_the_name_discovery_found() {
        // The reader thinks of it as `karet-core`, not as `crates/karet-core`, wherever
        // it happens to be listed.
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("/repo/crates/core")),
            vec![package("karet-core", "/repo/crates/core")],
        );
        let explorer = rows
            .iter()
            .find(|(label, _)| label.contains("explorer"))
            .map(|(label, _)| label.clone())
            .unwrap_or_default();
        assert!(explorer.starts_with("karet-core"), "got {explorer:?}");
    }

    #[test]
    fn the_directory_a_reader_is_in_is_offered() {
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/crates/a/src")),
            None,
            Vec::new(),
        );
        assert!(
            roots(&rows).contains(&Path::new("/repo/crates/a/src")),
            "got {rows:?}"
        );
    }

    #[test]
    fn a_relative_context_path_is_resolved_against_the_root() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            Some(PathBuf::from("crates/a")),
            Vec::new(),
        );
        assert!(
            roots(&rows).contains(&Path::new("/repo/crates/a")),
            "got {rows:?}"
        );
    }

    #[test]
    fn every_row_names_both_the_package_and_where_it_lives() {
        // Fuzzy matching has to find it either way — a reader types `karet-core` or
        // `crates/karet` depending on which they hold in mind.
        let rows = candidates(
            Path::new("/repo"),
            None,
            None,
            vec![package("karet-core", "/repo/crates/karet-core")],
        );
        let row = rows
            .iter()
            .find(|(_, path)| path == Path::new("/repo/crates/karet-core"))
            .map(|(label, _)| label.clone())
            .unwrap_or_default();
        assert!(row.contains("karet-core"), "got {row:?}");
        assert!(row.contains("crates/karet-core"), "got {row:?}");
        assert!(row.contains("package"), "got {row:?}");
    }

    #[test]
    fn the_root_row_omits_an_empty_relative_path() {
        let rows = candidates(Path::new("/repo"), None, None, Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].0.contains("  ·  "), "got {:?}", rows[0].0);
        assert!(rows[0].0.starts_with("repo  ·"), "got {:?}", rows[0].0);
    }

    #[test]
    fn a_directory_with_no_packages_still_offers_where_the_reader_is() {
        // Nothing discovered is a normal answer, not a reason to offer nothing.
        let rows = candidates(
            Path::new("/repo"),
            Some(PathBuf::from("/repo/notes")),
            None,
            Vec::new(),
        );
        assert!(roots(&rows).contains(&Path::new("/repo")));
        assert!(roots(&rows).contains(&Path::new("/repo/notes")));
    }

    #[test]
    fn discovered_packages_keep_the_order_they_were_found_in() {
        let rows = candidates(
            Path::new("/repo"),
            None,
            None,
            vec![
                package("alpha", "/repo/crates/alpha"),
                package("beta", "/repo/crates/beta"),
            ],
        );
        assert_eq!(
            roots(&rows),
            [
                Path::new("/repo"),
                Path::new("/repo/crates/alpha"),
                Path::new("/repo/crates/beta"),
            ]
        );
    }
}

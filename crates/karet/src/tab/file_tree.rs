//! The changed-file tree the commit-family views index their diffs with.
//!
//! A commit's changed files arrive as a flat `Vec<FileView>` of repo-relative
//! paths. Painted one row per path, a commit touching forty files under one crate
//! reads as forty near-identical strings whose distinguishing tail is truncated
//! away. [`changed_file_rows`] regroups them into a depth-annotated, foldable row
//! list so the shared prefix is stated once.
//!
//! **Folder compaction:** a directory whose only child is another directory merges
//! into a single `a/b/c` row, as the Explorer's tree does. A directory row's path
//! is therefore the *deepest* directory of its chain, and that is the key the
//! collapsed set uses — folding the row folds the whole chain.
//!
//! **Order:** children keep the order they arrive in, and a directory sits where
//! its first member did, so the tree reads in the same order as the diff cards
//! beside it. Git emits a commit's paths already sorted, so in practice that is
//! path order.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::ffi::OsString;

use super::*;

/// One row of a commit view's changed-file tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChangedFileRow {
    /// A directory grouping node.
    Dir {
        /// The deepest directory of a compacted chain — the collapsed set's key.
        path: PathBuf,
        /// What the row prints: the compacted `a/b/c` name, without its parents.
        label: String,
        /// Nesting depth, for indentation.
        depth: u16,
        /// Added lines across the whole subtree.
        added: usize,
        /// Removed lines across the whole subtree.
        removed: usize,
        /// Whether this directory's descendants are shown.
        expanded: bool,
    },
    /// A changed file.
    File {
        /// Index into the view's `files` slice.
        file: usize,
        /// The file name, without its directories.
        label: String,
        /// Nesting depth, for indentation.
        depth: u16,
    },
}

impl ChangedFileRow {
    /// The file this row indexes, if it is a file row.
    pub(crate) fn file(&self) -> Option<usize> {
        match self {
            Self::File { file, .. } => Some(*file),
            Self::Dir { .. } => None,
        }
    }

    /// The directory this row folds, if it is a directory row.
    pub(crate) fn dir(&self) -> Option<&Path> {
        match self {
            Self::Dir { path, .. } => Some(path.as_path()),
            Self::File { .. } => None,
        }
    }
}

/// A directory being assembled: its members in arrival order, plus its own stats.
#[derive(Default)]
struct Node {
    /// Child directories and files, in the order they were first seen.
    children: Vec<Child>,
    /// Added lines across the subtree.
    added: usize,
    /// Removed lines across the subtree.
    removed: usize,
}

/// One entry under a [`Node`].
enum Child {
    /// A subdirectory, by its index into the arena.
    Dir(usize),
    /// A changed file, by its index into the caller's `files` slice.
    File(usize),
}

/// The tree under construction: an arena of nodes plus each node's own path.
struct Arena {
    /// Node payloads, index 0 being the root.
    nodes: Vec<Node>,
    /// Each node's full path (the root's is empty).
    paths: Vec<PathBuf>,
    /// Child-directory lookup: `(parent, component name) -> node`.
    index: BTreeMap<(usize, OsString), usize>,
}

impl Arena {
    /// A fresh arena holding only the root.
    fn new() -> Self {
        Self {
            nodes: vec![Node::default()],
            paths: vec![PathBuf::new()],
            index: BTreeMap::new(),
        }
    }

    /// The child directory `name` of `parent`, created on first sight.
    fn child_dir(&mut self, parent: usize, name: &OsStr) -> usize {
        if let Some(node) = self.index.get(&(parent, name.to_os_string())) {
            return *node;
        }
        let node = self.nodes.len();
        self.nodes.push(Node::default());
        self.paths.push(self.paths[parent].join(name));
        self.index.insert((parent, name.to_os_string()), node);
        self.nodes[parent].children.push(Child::Dir(node));
        node
    }
}

/// Flatten `files` into the foldable tree rows the commit views paint.
///
/// `collapsed` holds the directories whose descendants are hidden, keyed by path
/// rather than by row index: a re-fetched file list must not let one directory
/// inherit another's fold state.
pub(crate) fn changed_file_rows(
    files: &[FileView],
    collapsed: &BTreeSet<PathBuf>,
) -> Vec<ChangedFileRow> {
    let mut arena = Arena::new();
    for (index, file) in files.iter().enumerate() {
        let (added, removed) = file.line_stats();
        let mut node = 0;
        arena.nodes[node].added += added;
        arena.nodes[node].removed += removed;
        let mut components = file.change.path.components().peekable();
        while let Some(component) = components.next() {
            // The last component is the file itself; everything before it a directory.
            if components.peek().is_none() {
                arena.nodes[node].children.push(Child::File(index));
                break;
            }
            node = arena.child_dir(node, component.as_os_str());
            arena.nodes[node].added += added;
            arena.nodes[node].removed += removed;
        }
    }
    let mut rows = Vec::with_capacity(files.len());
    push_rows(&arena, files, collapsed, 0, 0, &mut rows);
    rows
}

/// Append `node`'s children at `depth`, recursing into expanded directories.
fn push_rows(
    arena: &Arena,
    files: &[FileView],
    collapsed: &BTreeSet<PathBuf>,
    node: usize,
    depth: u16,
    rows: &mut Vec<ChangedFileRow>,
) {
    for child in &arena.nodes[node].children {
        match child {
            Child::Dir(dir) => {
                let (deepest, label) = compact(arena, *dir);
                let expanded = !collapsed.contains(&arena.paths[deepest]);
                rows.push(ChangedFileRow::Dir {
                    path: arena.paths[deepest].clone(),
                    label,
                    depth,
                    added: arena.nodes[deepest].added,
                    removed: arena.nodes[deepest].removed,
                    expanded,
                });
                if expanded {
                    push_rows(
                        arena,
                        files,
                        collapsed,
                        deepest,
                        depth.saturating_add(1),
                        rows,
                    );
                }
            },
            Child::File(file) => rows.push(ChangedFileRow::File {
                file: *file,
                label: files
                    .get(*file)
                    .and_then(|view| view.change.path.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                depth,
            }),
        }
    }
}

/// Walk a single-child directory chain to its end, returning the deepest node and
/// the joined `a/b/c` label the chain prints as.
fn compact(arena: &Arena, dir: usize) -> (usize, String) {
    let mut deepest = dir;
    let mut label = name_of(arena, dir);
    while let [Child::Dir(only)] = arena.nodes[deepest].children.as_slice() {
        deepest = *only;
        label.push('/');
        label.push_str(&name_of(arena, deepest));
    }
    (deepest, label)
}

/// A node's own path component, as it prints.
fn name_of(arena: &Arena, node: usize) -> String {
    arena.paths[node]
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_file_view;

    /// One added and one removed line per file, so subtree aggregates are checkable.
    fn file(path: &str) -> FileView {
        test_file_view(path, "a\nb\n", "a\nc\n")
    }

    /// Render the shape a caller sees: `D`/`F`, indented by depth.
    fn labels(rows: &[ChangedFileRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                ChangedFileRow::Dir { label, depth, .. } => {
                    format!("{}D {label}", "  ".repeat(usize::from(*depth)))
                },
                ChangedFileRow::File { label, depth, .. } => {
                    format!("{}F {label}", "  ".repeat(usize::from(*depth)))
                },
            })
            .collect()
    }

    #[test]
    fn groups_files_under_their_directories() {
        let files = [file("src/app/mouse.rs"), file("src/app/ui.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(labels(&rows), ["D src/app", "  F mouse.rs", "  F ui.rs"]);
    }

    /// A single-child chain collapses to one row so the shared prefix is stated once.
    #[test]
    fn compacts_single_child_directory_chains() {
        let files = [file("crates/karet/src/ui/commit.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(labels(&rows), ["D crates/karet/src/ui", "  F commit.rs"]);
    }

    /// A directory with two entries is a branch point: the chain stops there.
    #[test]
    fn stops_compacting_at_a_branch() {
        let files = [file("a/b/one.rs"), file("a/c/two.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(
            labels(&rows),
            ["D a", "  D b", "    F one.rs", "  D c", "    F two.rs"]
        );
    }

    /// A directory holding both a file and a subdirectory does not compact either.
    #[test]
    fn does_not_compact_past_a_file_sibling() {
        let files = [file("a/note.md"), file("a/b/one.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(
            labels(&rows),
            ["D a", "  F note.md", "  D b", "    F one.rs"]
        );
    }

    #[test]
    fn root_level_files_sit_at_depth_zero() {
        let files = [file("README.md"), file("src/lib.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(labels(&rows), ["F README.md", "D src", "  F lib.rs"]);
    }

    /// The collapsed set keys on the *deepest* directory of a compacted chain,
    /// which is the path the row reports and the click therefore folds.
    #[test]
    fn a_collapsed_directory_hides_its_descendants() {
        let files = [file("a/b/one.rs"), file("a/b/two.rs"), file("README.md")];
        let collapsed = BTreeSet::from([PathBuf::from("a/b")]);
        let rows = changed_file_rows(&files, &collapsed);
        assert_eq!(labels(&rows), ["D a/b", "F README.md"]);
        assert!(matches!(
            rows[0],
            ChangedFileRow::Dir {
                expanded: false,
                ..
            }
        ));
        // Keying the fold on a shallower link of the chain does nothing: the chain
        // is one row, and that row's identity is its deepest directory.
        let shallow = BTreeSet::from([PathBuf::from("a")]);
        assert_eq!(
            labels(&changed_file_rows(&files, &shallow)),
            ["D a/b", "  F one.rs", "  F two.rs", "F README.md"]
        );
    }

    /// Members of one directory that arrive apart still land in a single row, and
    /// the directory sits where its first member did.
    #[test]
    fn non_contiguous_members_share_one_directory_row() {
        let files = [file("a/one.rs"), file("b/two.rs"), file("a/three.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(
            labels(&rows),
            ["D a", "  F one.rs", "  F three.rs", "D b", "  F two.rs"]
        );
    }

    /// Directory stats are the subtree's, so a folded row still reports what it hides.
    #[test]
    fn directory_stats_aggregate_the_subtree() {
        let files = [file("a/b/one.rs"), file("a/two.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        let (added, removed) = files
            .iter()
            .map(FileView::line_stats)
            .fold((0, 0), |(a, r), (na, nr)| (a + na, r + nr));
        assert!(added > 0 && removed > 0, "the fixture must change lines");
        assert!(matches!(
            &rows[0],
            ChangedFileRow::Dir { path, added: a, removed: r, .. }
                if path == Path::new("a") && *a == added && *r == removed
        ));
    }

    #[test]
    fn an_empty_commit_has_no_rows() {
        assert!(changed_file_rows(&[], &BTreeSet::new()).is_empty());
    }

    #[test]
    fn row_accessors_name_what_the_row_addresses() {
        let files = [file("a/one.rs")];
        let rows = changed_file_rows(&files, &BTreeSet::new());
        assert_eq!(rows[0].dir(), Some(Path::new("a")));
        assert_eq!(rows[0].file(), None);
        assert_eq!(rows[1].file(), Some(0));
        assert_eq!(rows[1].dir(), None);
    }
}

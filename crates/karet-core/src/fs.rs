//! Neutral filesystem vocabulary for the client-server seam.
//!
//! A file tree renders directory listings; a backend produces them. Neither
//! should depend on the other, so the shape they agree on lives here — the same
//! inversion [`Decoration`](crate::Decoration) and [`Diagnostic`](crate::Diagnostic)
//! already use for producers and renderers.
//!
//! Deliberately minimal: what a tree row needs to draw itself, and nothing about
//! *how* the listing was obtained. A local walk and a listing that arrived over a
//! wire are the same value.

use std::path::PathBuf;

/// One immediate child of a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DirEntry {
    /// The child's full path.
    pub path: PathBuf,
    /// Whether it is a directory (following symlinks).
    pub is_dir: bool,
    /// Whether the path itself is a symbolic link.
    pub is_symlink: bool,
    /// Whether the ignore rules in force exclude it.
    ///
    /// Ignored entries are *listed and flagged*, never filtered out — a tree dims
    /// them the way VS Code does, so the user can still see what is there.
    pub ignored: bool,
}

impl DirEntry {
    /// The display label: the file name, or `?` for a path that has none.
    #[must_use]
    pub fn label(&self) -> &str {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("?")
    }

    /// The sort key placing directories first, then names case-insensitively.
    ///
    /// Exposed so a listing built anywhere — a local walk, a backend event —
    /// orders identically; a tree that sorted differently per source would jump
    /// as a client switched between them.
    #[must_use]
    pub fn sort_key(&self) -> (bool, String) {
        (!self.is_dir, self.label().to_lowercase())
    }
}

/// Order `entries` the way a file tree displays them: directories first, then
/// case-insensitive name.
pub fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by_key(DirEntry::sort_key);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            path: PathBuf::from(path),
            is_dir,
            is_symlink: false,
            ignored: false,
        }
    }

    #[test]
    fn the_label_is_the_file_name() {
        assert_eq!(entry("/a/b/main.rs", false).label(), "main.rs");
    }

    /// A root path has no file name; the label must still be renderable rather
    /// than empty, since it becomes a tree row either way.
    #[test]
    fn a_path_without_a_file_name_still_has_a_label() {
        assert_eq!(entry("/", true).label(), "?");
    }

    #[test]
    fn directories_sort_before_files_then_case_insensitively_by_name() {
        let mut entries = vec![
            entry("/w/Zebra.rs", false),
            entry("/w/alpha.rs", false),
            entry("/w/src", true),
            entry("/w/Assets", true),
        ];

        sort_entries(&mut entries);

        let labels: Vec<&str> = entries.iter().map(DirEntry::label).collect();
        assert_eq!(labels, ["Assets", "src", "alpha.rs", "Zebra.rs"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_directory_entry_round_trips_through_serde() -> Result<(), serde_json::Error> {
        let original = DirEntry {
            path: PathBuf::from("/w/target"),
            is_dir: true,
            is_symlink: false,
            ignored: true,
        };

        let restored: DirEntry = serde_json::from_str(&serde_json::to_string(&original)?)?;

        assert_eq!(restored, original);
        Ok(())
    }
}

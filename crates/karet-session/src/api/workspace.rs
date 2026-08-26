//! The workspace-filesystem half of the seam: reading and mutating the files a
//! session is rooted at.
//!
//! In local mode the presentation layer could read the workspace itself, and did.
//! That only works while the UI and the files share a machine. Routing every
//! workspace path through the backend makes the client's own disk irrelevant —
//! which is what a remote client needs, and costs a local client nothing but an
//! extra hop through an in-process channel.
//!
//! Note the scope: *workspace* files. A client's own theme, keymap and terminal
//! capabilities stay client-side, because they describe the machine doing the
//! rendering, not the one holding the code.

use std::path::PathBuf;

use karet_filetype::FileKind;

/// How a path should be opened, answering [`Command::ClassifyPath`].
///
/// Classification needs the leading bytes (magic-number recovery for a
/// mislabeled extension) and the total length (the size guard), both of which
/// live with the files. The `head` rides along so a client can build a hex view
/// or a placeholder without a second round trip.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PathClass {
    /// The renderer this path warrants.
    pub kind: FileKind,
    /// The file's total length in bytes.
    pub len: u64,
    /// The leading bytes classification was decided from.
    pub head: Vec<u8>,
}

/// A chunk of a workspace file's bytes, answering [`Command::ReadFileBytes`].
///
/// Media is read in chunks rather than whole so a large PDF cannot monopolize
/// the connection ahead of an interactive edit.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileChunk {
    /// The byte offset this chunk starts at.
    pub offset: u64,
    /// The bytes themselves.
    pub bytes: Vec<u8>,
    /// The file's total length, so a client can size its buffer up front.
    pub total_len: u64,
}

impl FileChunk {
    /// Whether this chunk reaches the end of the file.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.offset.saturating_add(self.bytes.len() as u64) >= self.total_len
    }
}

/// A filesystem mutation the explorer can request.
///
/// One enum rather than one command per verb: they share a permission model, an
/// error shape, and a "refresh what you were showing" follow-up, and a client
/// dispatching them writes one match instead of six.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum PathMutation {
    /// Create an empty file, failing if it exists.
    CreateFile {
        /// The file to create.
        path: PathBuf,
    },
    /// Create a directory and any missing parents.
    CreateDirectory {
        /// The directory to create.
        path: PathBuf,
    },
    /// Rename or move a path.
    Rename {
        /// The existing path.
        from: PathBuf,
        /// Its new path.
        to: PathBuf,
    },
    /// Copy a file, or a directory and everything under it.
    Copy {
        /// The path to copy.
        from: PathBuf,
        /// Where to copy it.
        to: PathBuf,
    },
    /// Delete a path, recursively for a directory.
    Delete {
        /// The path to remove.
        path: PathBuf,
    },
}

impl PathMutation {
    /// The path a client should reveal or re-select once the mutation lands.
    #[must_use]
    pub fn target(&self) -> &PathBuf {
        match self {
            Self::CreateFile { path } | Self::CreateDirectory { path } | Self::Delete { path } => {
                path
            },
            Self::Rename { to, .. } | Self::Copy { to, .. } => to,
        }
    }

    /// The directories whose listings this mutation invalidates.
    ///
    /// A rename or copy touches two: the source's parent loses an entry and the
    /// destination's gains one. Returning both keeps a tree from showing a file
    /// in two places at once.
    #[must_use]
    pub fn dirty_parents(&self) -> Vec<PathBuf> {
        let parent = |path: &PathBuf| path.parent().map(PathBuf::from);
        let mut dirs = Vec::new();
        match self {
            Self::CreateFile { path } | Self::CreateDirectory { path } | Self::Delete { path } => {
                dirs.extend(parent(path));
            },
            Self::Rename { from, to } | Self::Copy { from, to } => {
                dirs.extend(parent(from));
                dirs.extend(parent(to));
            },
        }
        dirs.dedup();
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_reaching_the_end_is_final() {
        let chunk = FileChunk {
            offset: 8,
            bytes: vec![0; 4],
            total_len: 12,
        };

        assert!(chunk.is_final());
    }

    #[test]
    fn a_chunk_short_of_the_end_is_not_final() {
        let chunk = FileChunk {
            offset: 0,
            bytes: vec![0; 4],
            total_len: 12,
        };

        assert!(!chunk.is_final());
    }

    /// An empty file yields one empty chunk, which must still terminate the read
    /// rather than leaving a client waiting for bytes that never come.
    #[test]
    fn an_empty_file_yields_a_final_chunk() {
        let chunk = FileChunk {
            offset: 0,
            bytes: Vec::new(),
            total_len: 0,
        };

        assert!(chunk.is_final());
    }

    #[test]
    fn a_rename_targets_its_destination_and_dirties_both_parents() {
        let mutation = PathMutation::Rename {
            from: PathBuf::from("/w/src/old.rs"),
            to: PathBuf::from("/w/lib/new.rs"),
        };

        assert_eq!(mutation.target(), &PathBuf::from("/w/lib/new.rs"));
        assert_eq!(
            mutation.dirty_parents(),
            [PathBuf::from("/w/src"), PathBuf::from("/w/lib")]
        );
    }

    /// A rename inside one directory must not list that directory twice, or a
    /// client would refresh it (and flicker) once per entry.
    #[test]
    fn a_rename_within_one_directory_dirties_it_once() {
        let mutation = PathMutation::Rename {
            from: PathBuf::from("/w/src/old.rs"),
            to: PathBuf::from("/w/src/new.rs"),
        };

        assert_eq!(mutation.dirty_parents(), [PathBuf::from("/w/src")]);
    }

    #[test]
    fn a_delete_targets_and_dirties_its_parent() {
        let mutation = PathMutation::Delete {
            path: PathBuf::from("/w/src/gone.rs"),
        };

        assert_eq!(mutation.target(), &PathBuf::from("/w/src/gone.rs"));
        assert_eq!(mutation.dirty_parents(), [PathBuf::from("/w/src")]);
    }
}

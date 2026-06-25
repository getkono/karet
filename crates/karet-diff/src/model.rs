//! The neutral diff data model: files, hunks, and lines.
//!
//! These types are produced by both [`crate::diff_text`] (diffing two texts) and
//! [`crate::parse`] (parsing a unified diff), and consumed by [`crate::align_hunk`],
//! [`crate::compute_highlights`] and [`crate::format_hunk_patch`]. They carry no
//! presentation — how a diff is displayed is left to the consumer.

/// Whether a [`DiffLine`] is unchanged context, an addition, or a removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// A line present and unchanged on both sides.
    Context,
    /// A line added on the new side.
    Add,
    /// A line removed from the old side.
    Remove,
}

/// One line within a [`Hunk`], tagged with its kind and 1-based line numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// Whether this line is context, an addition, or a removal.
    pub kind: LineKind,
    /// The 1-based line number on the old side, if present (`None` for additions).
    pub old_lineno: Option<u32>,
    /// The 1-based line number on the new side, if present (`None` for removals).
    pub new_lineno: Option<u32>,
    /// The line text, without its trailing terminator or `+`/`-`/space prefix.
    pub content: String,
}

/// A contiguous block of changes with surrounding context — a unified-diff hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The 1-based starting line on the old side (`0` when the old side is empty).
    pub old_start: u32,
    /// The number of old-side lines the hunk spans (context + removals).
    pub old_count: u32,
    /// The 1-based starting line on the new side (`0` when the new side is empty).
    pub new_start: u32,
    /// The number of new-side lines the hunk spans (context + additions).
    pub new_count: u32,
    /// The `@@ -a,b +c,d @@` header line (without any trailing scope text).
    pub header: String,
    /// Enclosing scope text from a git hunk header (e.g. `fn process`), always from
    /// the pre-image. `None` for engine-generated diffs (which have no scope text).
    pub scope: Option<String>,
    /// The new-side scope, set only when the enclosing scope line was itself changed
    /// elsewhere in this file; `None` means "same as [`scope`](Self::scope)".
    pub new_scope: Option<String>,
    /// The hunk's lines, in display order.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// The effective scope to show on the new (right) side: the new-side signature
    /// when known, otherwise the old [`scope`](Self::scope).
    #[must_use]
    pub fn right_scope(&self) -> Option<&str> {
        self.new_scope.as_deref().or(self.scope.as_deref())
    }

    /// The header line for the new (right) side: the same `@@ -a,b +c,d @@` range,
    /// but with the trailing scope suffix replaced by the new signature when known.
    /// Falls back to [`header`](Self::header) verbatim otherwise.
    #[must_use]
    pub fn right_header(&self) -> String {
        let Some(new_scope) = &self.new_scope else {
            return self.header.clone();
        };
        // Locate the byte just past the closing `@@`, mirroring scope extraction.
        let prefix_end = self
            .header
            .strip_prefix("@@ ")
            .and_then(|after| after.find(" @@").map(|c| "@@ ".len() + c + " @@".len()));
        match prefix_end {
            Some(end) => format!("{} {}", &self.header[..end], new_scope),
            None => self.header.clone(),
        }
    }
}

/// The change status of a whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// The file was added (the old side is empty).
    Added,
    /// The file was removed (the new side is empty).
    Removed,
    /// The file was modified in place.
    Modified,
    /// The file was renamed (and possibly also modified).
    Renamed {
        /// The similarity index (0–100) reported for the rename.
        similarity: u8,
    },
}

/// A single file's diff: its identity, status, and [`Hunk`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// The file path (the new path for renames).
    pub path: String,
    /// The previous path, set only for renames.
    pub old_path: Option<String>,
    /// The file's change status.
    pub status: FileStatus,
    /// Whether the file is binary (then [`hunks`](Self::hunks) is empty).
    pub is_binary: bool,
    /// The change hunks, in order.
    pub hunks: Vec<Hunk>,
}

/// A multi-file diff, as produced by [`crate::parse`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// The per-file diffs, in order.
    pub files: Vec<FileDiff>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_kinds_compare() {
        assert_ne!(LineKind::Add, LineKind::Remove);
        assert_eq!(LineKind::Context, LineKind::Context);
    }

    #[test]
    fn file_status_renamed_carries_similarity() {
        assert_eq!(
            FileStatus::Renamed { similarity: 90 },
            FileStatus::Renamed { similarity: 90 }
        );
        assert_ne!(FileStatus::Added, FileStatus::Modified);
    }
}

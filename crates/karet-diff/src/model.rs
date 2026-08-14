//! The neutral diff data model: files, hunks, and lines.
//!
//! These types are produced by both [`crate::diff_text`] (diffing two texts) and
//! [`crate::parse`] (parsing a unified diff), and consumed by [`crate::align_hunk`],
//! [`crate::compute_highlights`] and [`crate::format_hunk_patch`]. They carry no
//! presentation — how a diff is displayed is left to the consumer.

/// Whether a [`DiffLine`] is unchanged context, an addition, or a removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LineKind {
    /// A line present and unchanged on both sides.
    Context,
    /// A line added on the new side.
    Add,
    /// A line removed from the old side.
    Remove,
}

/// One line within a [`Hunk`], tagged with its kind and 1-based line numbers.
///
/// [`content`](Self::content) is always terminator-free so consumers can display
/// it directly; the terminator that *was* there is recorded separately by
/// [`crlf`](Self::crlf) and [`no_newline`](Self::no_newline), which together let
/// [`crate::format_hunk_patch`] reproduce the original bytes exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffLine {
    /// Whether this line is context, an addition, or a removal.
    pub kind: LineKind,
    /// The 1-based line number on the old side, if present (`None` for additions).
    pub old_lineno: Option<u32>,
    /// The 1-based line number on the new side, if present (`None` for removals).
    pub new_lineno: Option<u32>,
    /// The line text, without its trailing terminator or `+`/`-`/space prefix.
    pub content: String,
    /// Whether the line was terminated by `\r\n` rather than a bare `\n`.
    ///
    /// The `\r` is stripped from [`content`](Self::content) so a CRLF file renders
    /// without stray carriage returns, but it must be restored when rebuilding a
    /// patch — otherwise applying that patch silently rewrites the file's line
    /// endings.
    pub crlf: bool,
    /// Whether the file had no trailing newline after this line (git's
    /// `\ No newline at end of file` marker).
    pub no_newline: bool,
}

impl DiffLine {
    /// A line with a plain `\n` terminator and no end-of-file marker.
    ///
    /// The common case; set [`crlf`](Self::crlf) / [`no_newline`](Self::no_newline)
    /// afterwards for the exceptions.
    #[must_use]
    pub fn new(
        kind: LineKind,
        old_lineno: Option<u32>,
        new_lineno: Option<u32>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            old_lineno,
            new_lineno,
            content: content.into(),
            crlf: false,
            no_newline: false,
        }
    }

    /// The line's original terminator: `""`, `"\n"` or `"\r\n"`.
    #[must_use]
    pub fn terminator(&self) -> &'static str {
        match (self.no_newline, self.crlf) {
            (true, _) => "",
            (false, true) => "\r\n",
            (false, false) => "\n",
        }
    }
}

/// A contiguous block of changes with surrounding context — a unified-diff hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FileStatus {
    /// The file was added (the old side is empty).
    Added,
    /// The file was removed (the new side is empty).
    Removed,
    /// The file was modified in place.
    ///
    /// Also the status of a change that only altered the file's mode, in which
    /// case [`FileDiff::mode_changed`] is true and there are no hunks.
    Modified,
    /// The file was renamed (and possibly also modified).
    Renamed {
        /// The similarity index (0–100) reported for the rename.
        similarity: u8,
    },
    /// The file was copied from another path (git's `-C`), leaving the source in
    /// place.
    Copied {
        /// The similarity index (0–100) reported for the copy.
        similarity: u8,
    },
    /// The file was rewritten wholesale — git's "broken pair" (`-B`), reported as
    /// a `dissimilarity index` rather than a similarity one.
    Rewritten {
        /// The dissimilarity index (0–100) reported for the rewrite.
        dissimilarity: u8,
    },
    /// The file changed type, e.g. a regular file replaced by a symlink.
    ///
    /// git reports this as a deletion *and* an addition of the same path; both
    /// entries are kept, and both carry this status.
    TypeChanged,
    /// The file has unresolved merge conflicts, reported by git as a combined
    /// diff (`diff --cc`). [`hunks`](FileDiff::hunks) is empty: the combined
    /// multi-parent format is not decoded.
    Unmerged,
}

/// A single file's diff: its identity, status, modes, and [`Hunk`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileDiff {
    /// The file path (the new path for renames and copies).
    pub path: String,
    /// The previous path, set only for renames and copies.
    pub old_path: Option<String>,
    /// The file's change status.
    pub status: FileStatus,
    /// The old-side file mode (e.g. `0o100644`), when git reported one. `None`
    /// for an added file.
    pub old_mode: Option<u32>,
    /// The new-side file mode (e.g. `0o100755`), when git reported one. `None`
    /// for a deleted file.
    pub new_mode: Option<u32>,
    /// Whether the file is binary (then [`hunks`](Self::hunks) is empty).
    pub is_binary: bool,
    /// The change hunks, in order.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// Whether the file's mode changed (e.g. `chmod +x`).
    ///
    /// False when either side's mode is unknown, so an addition or deletion never
    /// counts as a mode change.
    #[must_use]
    pub fn mode_changed(&self) -> bool {
        match (self.old_mode, self.new_mode) {
            (Some(old), Some(new)) => old != new,
            _ => false,
        }
    }

    /// Whether the file has no hunks and no content difference to show — a pure
    /// rename, a mode-only change, or an empty file being added.
    ///
    /// Consumers that render hunks need this to distinguish "nothing changed
    /// inside the file" from "we failed to parse anything".
    #[must_use]
    pub fn is_content_unchanged(&self) -> bool {
        self.hunks.is_empty() && !self.is_binary
    }
}

/// A multi-file diff, as produced by [`crate::parse`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        // A copy and a rename of equal similarity are distinct changes.
        assert_ne!(
            FileStatus::Copied { similarity: 90 },
            FileStatus::Renamed { similarity: 90 }
        );
    }

    fn file_with_modes(old_mode: Option<u32>, new_mode: Option<u32>) -> FileDiff {
        FileDiff {
            path: "x".into(),
            old_path: None,
            status: FileStatus::Modified,
            old_mode,
            new_mode,
            is_binary: false,
            hunks: vec![],
        }
    }

    #[test]
    fn mode_changed_needs_both_sides() {
        assert!(file_with_modes(Some(0o100644), Some(0o100755)).mode_changed());
        assert!(!file_with_modes(Some(0o100644), Some(0o100644)).mode_changed());
        // An addition or deletion knows only one side, and is not a mode change.
        assert!(!file_with_modes(None, Some(0o100755)).mode_changed());
        assert!(!file_with_modes(Some(0o100644), None).mode_changed());
        assert!(!file_with_modes(None, None).mode_changed());
    }

    #[test]
    fn content_unchanged_excludes_binary() {
        assert!(file_with_modes(None, None).is_content_unchanged());
        let mut binary = file_with_modes(None, None);
        binary.is_binary = true;
        // A binary file also has no hunks, but its content very much did change.
        assert!(!binary.is_content_unchanged());
    }

    #[test]
    fn diff_line_terminator_reflects_flags() {
        let mut line = DiffLine::new(LineKind::Add, None, Some(1), "x");
        assert_eq!(line.terminator(), "\n");
        assert!(!line.crlf && !line.no_newline);

        line.crlf = true;
        assert_eq!(line.terminator(), "\r\n");

        // No trailing newline wins over the line-ending style either way.
        line.no_newline = true;
        assert_eq!(line.terminator(), "");
        line.crlf = false;
        assert_eq!(line.terminator(), "");
    }
}

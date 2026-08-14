//! Reconstructing unified-diff patches from the model, and per-hunk staging.

use std::collections::BTreeSet;

use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::model::Hunk;
use crate::model::LineKind;

/// The `diff --git` header, the rename/copy/mode metadata git would have written,
/// and the `---`/`+++` path lines for `file`.
fn file_header(file: &FileDiff) -> String {
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let new_path = &file.path;
    let mut out = format!("diff --git a/{old_path} b/{new_path}\n");
    out.push_str(&status_metadata(file, old_path, new_path));
    out.push_str(&mode_metadata(file));
    match file.status {
        FileStatus::Added => {
            out.push_str("--- /dev/null\n");
            out.push_str(&format!("+++ b/{new_path}\n"));
        },
        FileStatus::Removed => {
            out.push_str(&format!("--- a/{old_path}\n"));
            out.push_str("+++ /dev/null\n");
        },
        _ => {
            out.push_str(&format!("--- a/{old_path}\n"));
            out.push_str(&format!("+++ b/{new_path}\n"));
        },
    }
    out
}

/// The similarity and rename/copy lines implied by `file`'s status.
///
/// Without these, a rename patch reads as an edit to a file that does not exist
/// at the old path, and `git apply` rejects it.
fn status_metadata(file: &FileDiff, old_path: &str, new_path: &str) -> String {
    match file.status {
        FileStatus::Renamed { similarity } => format!(
            "similarity index {similarity}%\nrename from {old_path}\nrename to {new_path}\n"
        ),
        FileStatus::Copied { similarity } => {
            format!("similarity index {similarity}%\ncopy from {old_path}\ncopy to {new_path}\n")
        },
        FileStatus::Rewritten { dissimilarity } => {
            format!("dissimilarity index {dissimilarity}%\n")
        },
        _ => String::new(),
    }
}

/// The mode lines for `file`: `new file mode` / `deleted file mode` for a one-sided
/// change, or an `old mode`/`new mode` pair when the mode itself changed.
fn mode_metadata(file: &FileDiff) -> String {
    match (file.status.clone(), file.old_mode, file.new_mode) {
        (FileStatus::Added, _, Some(new)) => format!("new file mode {new:06o}\n"),
        (FileStatus::Removed, Some(old), _) => format!("deleted file mode {old:06o}\n"),
        _ if file.mode_changed() => {
            // `mode_changed` is only true when both sides are known.
            let old = file.old_mode.unwrap_or_default();
            let new = file.new_mode.unwrap_or_default();
            format!("old mode {old:06o}\nnew mode {new:06o}\n")
        },
        _ => String::new(),
    }
}

/// The hunk header line plus its prefixed content lines.
///
/// Each line is re-emitted with its original terminator, so a CRLF file stays CRLF
/// and a missing final newline stays missing — applying the result cannot silently
/// rewrite either.
fn hunk_body(hunk: &Hunk) -> String {
    let mut out = String::new();
    out.push_str(&hunk.header);
    out.push('\n');
    for line in &hunk.lines {
        let prefix = match line.kind {
            LineKind::Add => '+',
            LineKind::Remove => '-',
            LineKind::Context => ' ',
        };
        out.push(prefix);
        out.push_str(&line.content);
        out.push_str(line.terminator());
        if line.no_newline {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
    out
}

/// Reconstruct a valid unified-diff patch for a single `hunk` of `file`.
///
/// The output is suitable for piping to `git apply --cached`. Pass a hunk from
/// `file.hunks`; the `file` supplies the path/status header.
#[must_use]
pub fn format_hunk_patch(file: &FileDiff, hunk: &Hunk) -> String {
    let mut out = file_header(file);
    out.push_str(&hunk_body(hunk));
    out
}

/// Per-hunk staging state: which hunks (by index into a [`FileDiff`]) are selected.
///
/// Builds the combined patch for the staged hunks, for partial-commit workflows.
#[derive(Clone, Debug, Default)]
pub struct Staging {
    staged: BTreeSet<usize>,
}

impl Staging {
    /// Create an empty staging set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the hunk at `hunk_idx` as staged.
    pub fn stage(&mut self, hunk_idx: usize) {
        self.staged.insert(hunk_idx);
    }

    /// Unmark the hunk at `hunk_idx`.
    pub fn unstage(&mut self, hunk_idx: usize) {
        self.staged.remove(&hunk_idx);
    }

    /// Toggle whether the hunk at `hunk_idx` is staged.
    pub fn toggle(&mut self, hunk_idx: usize) {
        if !self.staged.insert(hunk_idx) {
            self.staged.remove(&hunk_idx);
        }
    }

    /// Whether the hunk at `hunk_idx` is staged.
    #[must_use]
    pub fn is_staged(&self, hunk_idx: usize) -> bool {
        self.staged.contains(&hunk_idx)
    }

    /// Whether no hunks are staged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// The combined unified-diff patch for the staged hunks of `file`.
    ///
    /// Emits one file header followed by the staged hunk bodies in order. Returns
    /// an empty string when nothing is staged.
    #[must_use]
    pub fn staged_patch(&self, file: &FileDiff) -> String {
        let mut out = String::new();
        for &idx in &self.staged {
            if let Some(hunk) = file.hunks.get(idx) {
                if out.is_empty() {
                    out.push_str(&file_header(file));
                }
                out.push_str(&hunk_body(hunk));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffOptions;
    use crate::diff_text;
    use crate::model::DiffLine;
    use crate::model::Hunk;

    #[test]
    fn modified_file_patch() {
        let file = diff_text(
            "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n",
            "fn main() {\n    println!(\"world\");\n    let x = 1;\n}\n",
            &DiffOptions {
                path_hint: Some("src/main.rs".into()),
                ..Default::default()
            },
        );
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.starts_with("diff --git a/src/main.rs b/src/main.rs\n"));
        assert!(patch.contains("--- a/src/main.rs\n"));
        assert!(patch.contains("+++ b/src/main.rs\n"));
        assert!(patch.contains("-    println!(\"hello\");\n"));
        assert!(patch.contains("+    println!(\"world\");\n"));
        assert!(patch.ends_with('\n'));
    }

    #[test]
    fn added_file_uses_dev_null_for_old() {
        let file = diff_text(
            "",
            "fn new_fn() {\n}\n",
            &DiffOptions {
                path_hint: Some("new.rs".into()),
                ..Default::default()
            },
        );
        assert_eq!(file.status, FileStatus::Added);
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.contains("--- /dev/null\n"));
        assert!(patch.contains("+++ b/new.rs\n"));
    }

    #[test]
    fn deleted_file_uses_dev_null_for_new() {
        let file = diff_text(
            "fn old_fn() {\n}\n",
            "",
            &DiffOptions {
                path_hint: Some("old.rs".into()),
                ..Default::default()
            },
        );
        assert_eq!(file.status, FileStatus::Removed);
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.contains("--- a/old.rs\n"));
        assert!(patch.contains("+++ /dev/null\n"));
    }

    /// A one-hunk `old();` → `new();` file, for exercising header emission.
    fn one_hunk_file(path: &str, old_path: Option<&str>, status: FileStatus) -> FileDiff {
        FileDiff {
            path: path.into(),
            old_path: old_path.map(Into::into),
            status,
            old_mode: None,
            new_mode: None,
            is_binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                header: "@@ -1,1 +1,1 @@".into(),
                scope: None,
                new_scope: None,
                lines: vec![
                    DiffLine::new(LineKind::Remove, Some(1), None, "old();"),
                    DiffLine::new(LineKind::Add, None, Some(1), "new();"),
                ],
            }],
        }
    }

    #[test]
    fn renamed_file_emits_rename_metadata() {
        let file = one_hunk_file(
            "bar.rs",
            Some("foo.rs"),
            FileStatus::Renamed { similarity: 90 },
        );
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.starts_with("diff --git a/foo.rs b/bar.rs\n"));
        // Without these the patch describes an edit to a file that is not there.
        assert!(patch.contains("similarity index 90%\n"));
        assert!(patch.contains("rename from foo.rs\n"));
        assert!(patch.contains("rename to bar.rs\n"));
        assert!(patch.contains("--- a/foo.rs\n"));
        assert!(patch.contains("+++ b/bar.rs\n"));
    }

    #[test]
    fn copied_file_emits_copy_metadata() {
        let file = one_hunk_file(
            "bar.rs",
            Some("foo.rs"),
            FileStatus::Copied { similarity: 82 },
        );
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.contains("similarity index 82%\n"));
        assert!(patch.contains("copy from foo.rs\n"));
        assert!(patch.contains("copy to bar.rs\n"));
        assert!(!patch.contains("rename"));
    }

    #[test]
    fn rewritten_file_emits_dissimilarity() {
        let file = one_hunk_file("x.rs", None, FileStatus::Rewritten { dissimilarity: 87 });
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.contains("dissimilarity index 87%\n"));
    }

    #[test]
    fn mode_lines_are_emitted_in_octal() {
        let mut changed = one_hunk_file("s.sh", None, FileStatus::Modified);
        changed.old_mode = Some(0o100644);
        changed.new_mode = Some(0o100755);
        let patch = format_hunk_patch(&changed, &changed.hunks[0]);
        assert!(patch.contains("old mode 100644\nnew mode 100755\n"));

        // An unchanged mode is not worth a line.
        let mut same = one_hunk_file("s.sh", None, FileStatus::Modified);
        same.old_mode = Some(0o100644);
        same.new_mode = Some(0o100644);
        assert!(!format_hunk_patch(&same, &same.hunks[0]).contains("mode"));

        let mut added = one_hunk_file("new.rs", None, FileStatus::Added);
        added.new_mode = Some(0o100644);
        assert!(format_hunk_patch(&added, &added.hunks[0]).contains("new file mode 100644\n"));

        let mut removed = one_hunk_file("old.rs", None, FileStatus::Removed);
        removed.old_mode = Some(0o120000);
        assert!(
            format_hunk_patch(&removed, &removed.hunks[0]).contains("deleted file mode 120000\n")
        );
    }

    #[test]
    fn crlf_and_missing_newline_survive_reconstruction() {
        let mut file = one_hunk_file("f.txt", None, FileStatus::Modified);
        file.hunks[0].lines[0].crlf = true;
        file.hunks[0].lines[1].no_newline = true;
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        // The `\r` belongs to the file, so applying this patch must not drop it.
        assert!(patch.contains("-old();\r\n"));
        assert!(patch.ends_with("+new();\n\\ No newline at end of file\n"));
    }

    /// Rebuild `file`'s single hunk into a patch, parse it back, and return the
    /// reparsed file — the fidelity check every shape below shares.
    fn round_trip(file: &FileDiff) -> Result<FileDiff, crate::DiffError> {
        let patch = format_hunk_patch(file, &file.hunks[0]);
        let mut reparsed = crate::parse(&patch)?;
        assert_eq!(reparsed.files.len(), 1);
        Ok(reparsed.files.remove(0))
    }

    #[test]
    fn engine_output_round_trips_through_patch_and_parse() -> Result<(), crate::DiffError> {
        let engine_diff = diff_text(
            "a\nb\nc\nd\n",
            "a\nB\nc\nd\n",
            &DiffOptions {
                path_hint: Some("file.txt".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(engine_diff.hunks.len(), 1);
        let rf = round_trip(&engine_diff)?;
        assert_eq!(rf.path, "file.txt");
        let eh = &engine_diff.hunks[0];
        let rh = &rf.hunks[0];
        assert_eq!(
            (eh.old_start, eh.old_count, eh.new_start, eh.new_count),
            (rh.old_start, rh.old_count, rh.new_start, rh.new_count)
        );
        assert_eq!(eh.lines, rh.lines);
        Ok(())
    }

    #[test]
    fn rename_round_trips_its_identity() -> Result<(), crate::DiffError> {
        let file = one_hunk_file(
            "bar.rs",
            Some("foo.rs"),
            FileStatus::Renamed { similarity: 90 },
        );
        let rf = round_trip(&file)?;
        assert_eq!(rf.path, "bar.rs");
        assert_eq!(rf.old_path.as_deref(), Some("foo.rs"));
        assert_eq!(rf.status, FileStatus::Renamed { similarity: 90 });
        Ok(())
    }

    #[test]
    fn copy_round_trips_its_identity() -> Result<(), crate::DiffError> {
        let file = one_hunk_file(
            "bar.rs",
            Some("foo.rs"),
            FileStatus::Copied { similarity: 82 },
        );
        let rf = round_trip(&file)?;
        assert_eq!(rf.old_path.as_deref(), Some("foo.rs"));
        assert_eq!(rf.status, FileStatus::Copied { similarity: 82 });
        Ok(())
    }

    #[test]
    fn mode_change_round_trips() -> Result<(), crate::DiffError> {
        let mut file = one_hunk_file("s.sh", None, FileStatus::Modified);
        file.old_mode = Some(0o100644);
        file.new_mode = Some(0o100755);
        let rf = round_trip(&file)?;
        assert_eq!((rf.old_mode, rf.new_mode), (Some(0o100644), Some(0o100755)));
        assert!(rf.mode_changed());
        Ok(())
    }

    #[test]
    fn line_terminators_round_trip() -> Result<(), crate::DiffError> {
        let mut file = one_hunk_file("f.txt", None, FileStatus::Modified);
        file.hunks[0].lines[0].crlf = true;
        file.hunks[0].lines[1].no_newline = true;
        let rf = round_trip(&file)?;
        assert_eq!(rf.hunks[0].lines, file.hunks[0].lines);
        Ok(())
    }

    #[test]
    fn staging_combines_only_selected_hunks() {
        let file = diff_text(
            "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\n",
            "A\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nM\n",
            &DiffOptions {
                path_hint: Some("x.txt".into()),
                context_lines: 1,
                ..Default::default()
            },
        );
        assert_eq!(file.hunks.len(), 2);

        let mut staging = Staging::new();
        assert!(staging.is_empty());
        staging.stage(0);
        assert!(staging.is_staged(0) && !staging.is_staged(1));

        let patch = staging.staged_patch(&file);
        assert!(patch.starts_with("diff --git a/x.txt b/x.txt\n"));
        assert!(patch.contains("+A\n"));
        assert!(!patch.contains("+M\n"));

        staging.toggle(1);
        let both = staging.staged_patch(&file);
        assert!(both.contains("+A\n") && both.contains("+M\n"));
    }
}

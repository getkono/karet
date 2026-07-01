//! Reconstructing unified-diff patches from the model, and per-hunk staging.

use std::collections::BTreeSet;

use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::model::Hunk;
use crate::model::LineKind;

/// The `diff --git` + `---`/`+++` header lines for `file`.
fn file_header(file: &FileDiff) -> String {
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let new_path = &file.path;
    let mut out = format!("diff --git a/{old_path} b/{new_path}\n");
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

/// The hunk header line plus its prefixed content lines.
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
        out.push('\n');
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

    #[test]
    fn renamed_file_uses_old_path() {
        let file = FileDiff {
            path: "bar.rs".into(),
            old_path: Some("foo.rs".into()),
            status: FileStatus::Renamed { similarity: 90 },
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
                    DiffLine {
                        kind: LineKind::Remove,
                        old_lineno: Some(1),
                        new_lineno: None,
                        content: "old();".into(),
                    },
                    DiffLine {
                        kind: LineKind::Add,
                        old_lineno: None,
                        new_lineno: Some(1),
                        content: "new();".into(),
                    },
                ],
            }],
        };
        let patch = format_hunk_patch(&file, &file.hunks[0]);
        assert!(patch.starts_with("diff --git a/foo.rs b/bar.rs\n"));
        assert!(patch.contains("--- a/foo.rs\n"));
        assert!(patch.contains("+++ b/bar.rs\n"));
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

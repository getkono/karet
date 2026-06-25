//! Generating a [`FileDiff`] from two in-memory texts via line-level diffing.
//!
//! Uses `imara-diff` (Histogram algorithm) to find the changed line ranges, then
//! reconstructs unified-diff hunks with surrounding context — the same shape
//! [`crate::parse`] produces, so [`crate::align_hunk`], [`crate::compute_highlights`]
//! and [`crate::format_hunk_patch`] consume either interchangeably.

use std::path::Path;

use imara_diff::{Algorithm, Diff as ImaraDiff, InternedInput};

use crate::DiffError;
use crate::model::{DiffLine, FileDiff, FileStatus, Hunk, LineKind};

/// Options controlling how [`diff_text`] / [`diff_files`] build the diff.
#[derive(Clone, Debug)]
pub struct DiffOptions {
    /// Number of context lines around each change. Matches `git diff -U<n>`.
    pub context_lines: usize,
    /// Path label baked into the resulting [`FileDiff`]. Used downstream for
    /// language detection (extension → grammar). `None` defaults to `"<input>"`.
    pub path_hint: Option<String>,
    /// When `true` and the two paths supplied to [`diff_files`] differ, mark the
    /// result [`FileStatus::Renamed`] and populate `old_path`.
    pub detect_rename: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context_lines: 3,
            path_hint: None,
            detect_rename: false,
        }
    }
}

/// Compare two in-memory text buffers and produce a [`FileDiff`].
#[must_use]
pub fn diff_text(old: &str, new: &str, opts: &DiffOptions) -> FileDiff {
    let path = opts
        .path_hint
        .clone()
        .unwrap_or_else(|| "<input>".to_string());
    build_file_diff(old, new, path, None, opts.context_lines)
}

/// Read two files and diff their contents. The result's `path` reflects the new
/// file; `old_path` is populated when the paths differ and `detect_rename` is set.
///
/// # Errors
/// Returns [`DiffError::Io`] if either file cannot be read.
pub fn diff_files(old: &Path, new: &Path, opts: &DiffOptions) -> Result<FileDiff, DiffError> {
    let old_content = std::fs::read_to_string(old).map_err(|e| DiffError::Io(e.to_string()))?;
    let new_content = std::fs::read_to_string(new).map_err(|e| DiffError::Io(e.to_string()))?;
    let new_path = new.to_string_lossy().into_owned();
    let old_path = if opts.detect_rename && old != new {
        Some(old.to_string_lossy().into_owned())
    } else {
        None
    };
    Ok(build_file_diff(
        &old_content,
        &new_content,
        new_path,
        old_path,
        opts.context_lines,
    ))
}

fn build_file_diff(
    old: &str,
    new: &str,
    path: String,
    old_path: Option<String>,
    context: usize,
) -> FileDiff {
    let status = file_status(old, new, &old_path);
    let hunks = compute_hunks(old, new, context);
    FileDiff {
        path,
        old_path,
        status,
        is_binary: false,
        hunks,
    }
}

fn file_status(old: &str, new: &str, old_path: &Option<String>) -> FileStatus {
    if old_path.is_some() {
        return FileStatus::Renamed { similarity: 100 };
    }
    match (old.is_empty(), new.is_empty()) {
        (true, false) => FileStatus::Added,
        (false, true) => FileStatus::Removed,
        _ => FileStatus::Modified,
    }
}

/// Strip a single trailing line terminator (`\n` or `\r\n`). The last line of a
/// file may have none. (Plain `trim_end_matches('\n')` would leave a stray `\r`.)
fn line_content(line: &str) -> &str {
    match line.strip_suffix('\n') {
        Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
        None => line,
    }
}

/// Bounds-checked line lookup (returns `""` out of range; never panics).
fn at<'a>(lines: &[&'a str], idx: u32) -> &'a str {
    lines.get(idx as usize).copied().unwrap_or("")
}

fn compute_hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    // Tokenize with the same source `imara-diff` uses, so line indices align.
    let old_lines: Vec<&str> = imara_diff::sources::lines(old).collect();
    let new_lines: Vec<&str> = imara_diff::sources::lines(new).collect();

    let input = InternedInput::new(old, new);
    let mut diff = ImaraDiff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);

    let changes: Vec<imara_diff::Hunk> = diff.hunks().collect();
    if changes.is_empty() {
        return Vec::new();
    }

    let ctx = context as u32;
    let old_len = old_lines.len() as u32;
    let new_len = new_lines.len() as u32;

    // Group changes whose context windows touch or overlap (gap <= 2*ctx unchanged
    // lines) into a single display hunk, matching `git diff -U<ctx>`.
    let mut hunks = Vec::new();
    let mut group_start = 0usize;
    for i in 1..=changes.len() {
        let split = i == changes.len() || {
            let gap = changes[i]
                .before
                .start
                .saturating_sub(changes[i - 1].before.end);
            gap > 2 * ctx
        };
        if split {
            hunks.push(build_group_hunk(
                &changes[group_start..i],
                &old_lines,
                &new_lines,
                ctx,
                old_len,
                new_len,
            ));
            group_start = i;
        }
    }
    hunks
}

fn build_group_hunk(
    group: &[imara_diff::Hunk],
    old_lines: &[&str],
    new_lines: &[&str],
    ctx: u32,
    old_len: u32,
    new_len: u32,
) -> Hunk {
    // `group` is non-empty by construction.
    let first = group.first().map_or(0..0, |h| h.before.clone());
    let last_before = group.last().map_or(0..0, |h| h.before.clone());
    let first_after = group.first().map_or(0..0, |h| h.after.clone());
    let last_after = group.last().map_or(0..0, |h| h.after.clone());

    let o0 = first.start.saturating_sub(ctx);
    let n0 = first_after.start.saturating_sub(ctx);
    let o1 = (last_before.end + ctx).min(old_len);
    let n1 = (last_after.end + ctx).min(new_len);

    let old_count = o1 - o0;
    let new_count = n1 - n0;

    let mut lines = Vec::new();
    let mut oi = o0;
    let mut ni = n0;
    for h in group {
        // Leading / inter-change context: old [oi, start) mirrors new [ni, start).
        while oi < h.before.start {
            lines.push(DiffLine {
                kind: LineKind::Context,
                old_lineno: Some(oi + 1),
                new_lineno: Some(ni + 1),
                content: line_content(at(old_lines, oi)).to_string(),
            });
            oi += 1;
            ni += 1;
        }
        while oi < h.before.end {
            lines.push(DiffLine {
                kind: LineKind::Remove,
                old_lineno: Some(oi + 1),
                new_lineno: None,
                content: line_content(at(old_lines, oi)).to_string(),
            });
            oi += 1;
        }
        while ni < h.after.end {
            lines.push(DiffLine {
                kind: LineKind::Add,
                old_lineno: None,
                new_lineno: Some(ni + 1),
                content: line_content(at(new_lines, ni)).to_string(),
            });
            ni += 1;
        }
    }
    // Trailing context.
    while oi < o1 {
        lines.push(DiffLine {
            kind: LineKind::Context,
            old_lineno: Some(oi + 1),
            new_lineno: Some(ni + 1),
            content: line_content(at(old_lines, oi)).to_string(),
        });
        oi += 1;
        ni += 1;
    }

    // Unified-diff convention: a side with zero lines reports its start as the
    // 0-based position (e.g. `-0,0`); otherwise the 1-based first line.
    let old_start = if old_count == 0 { o0 } else { o0 + 1 };
    let new_start = if new_count == 0 { n0 } else { n0 + 1 };
    let header = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@");

    Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        header,
        scope: None,
        new_scope: None,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align_hunk;

    #[test]
    fn identical_inputs_yield_no_hunks() {
        let f = diff_text("a\nb\nc\n", "a\nb\nc\n", &DiffOptions::default());
        assert!(f.hunks.is_empty());
        assert_eq!(f.status, FileStatus::Modified);
    }

    #[test]
    fn empty_to_nonempty_is_added() {
        let f = diff_text(
            "",
            "fn new_fn() {\n    // body\n}\n",
            &DiffOptions::default(),
        );
        assert_eq!(f.status, FileStatus::Added);
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (0, 0, 1, 3)
        );
        assert_eq!(h.header, "@@ -0,0 +1,3 @@");
        assert!(h.lines.iter().all(|l| l.kind == LineKind::Add));
    }

    #[test]
    fn nonempty_to_empty_is_removed() {
        let f = diff_text("a\nb\nc\n", "", &DiffOptions::default());
        assert_eq!(f.status, FileStatus::Removed);
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 3, 0, 0)
        );
        assert!(h.lines.iter().all(|l| l.kind == LineKind::Remove));
    }

    #[test]
    fn one_line_modification_lineno_math() {
        let old = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n    let y = 2;\n}\n";
        let new = "fn main() {\n    println!(\"world\");\n    let x = 1;\n    let y = 2;\n}\n";
        let f = diff_text(old, new, &DiffOptions::default());
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(h.header, "@@ -1,5 +1,5 @@");
        let remove = h.lines.iter().find(|l| l.kind == LineKind::Remove);
        let add = h.lines.iter().find(|l| l.kind == LineKind::Add);
        assert_eq!(remove.and_then(|l| l.old_lineno), Some(2));
        assert_eq!(remove.and_then(|l| l.new_lineno), None);
        assert_eq!(add.and_then(|l| l.new_lineno), Some(2));
        assert_eq!(add.and_then(|l| l.old_lineno), None);
    }

    #[test]
    fn crlf_terminator_is_stripped() {
        let f = diff_text("a\r\nb\r\n", "a\r\nB\r\n", &DiffOptions::default());
        let h = &f.hunks[0];
        let add = h.lines.iter().find(|l| l.kind == LineKind::Add);
        assert_eq!(add.map(|l| l.content.as_str()), Some("B"));
    }

    #[test]
    fn align_hunk_consumes_engine_output() {
        let old = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let new = "fn a() {}\nfn B() {}\nfn c() {}\n";
        let f = diff_text(old, new, &DiffOptions::default());
        let rows = align_hunk(&f.hunks[0].lines);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[1].left.as_ref().map(|c| c.content.as_str()),
            Some("fn b() {}")
        );
        assert_eq!(
            rows[1].right.as_ref().map(|c| c.content.as_str()),
            Some("fn B() {}")
        );
    }

    #[test]
    fn changes_far_apart_produce_two_hunks() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "A\nb\nc\nd\ne\nf\ng\nH\n";
        let f = diff_text(
            old,
            new,
            &DiffOptions {
                context_lines: 1,
                ..Default::default()
            },
        );
        assert_eq!(f.hunks.len(), 2);
    }

    #[test]
    fn path_hint_propagates_else_defaults() {
        let opts = DiffOptions {
            path_hint: Some("src/main.rs".to_string()),
            ..Default::default()
        };
        let f = diff_text("a\n", "b\n", &opts);
        assert_eq!(f.path, "src/main.rs");
        assert!(f.old_path.is_none());

        let f = diff_text("a\n", "b\n", &DiffOptions::default());
        assert_eq!(f.path, "<input>");
    }
}

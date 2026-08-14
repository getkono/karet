//! Generating a [`FileDiff`] from two in-memory texts via line-level diffing.
//!
//! Uses `imara-diff` (Histogram algorithm) to find the changed line ranges, then
//! reconstructs unified-diff hunks with surrounding context — the same shape
//! [`crate::parse`] produces, so [`crate::align_hunk`], [`crate::compute_highlights`]
//! and [`crate::format_hunk_patch`] consume either interchangeably.

use std::path::Path;

use imara_diff::Algorithm;
use imara_diff::Diff as ImaraDiff;
use imara_diff::InternedInput;

use crate::DiffError;
use crate::model::DiffLine;
use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::model::Hunk;
use crate::model::LineKind;

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

/// How many leading bytes are inspected when deciding whether a file is binary.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Read two files and diff their contents. The result's `path` reflects the new
/// file; `old_path` is populated when the paths differ and `detect_rename` is set.
///
/// When either side is binary, the result carries
/// [`is_binary`](FileDiff::is_binary) and no hunks — the same shape
/// [`crate::parse`] produces for `Binary files … differ`.
///
/// # Errors
/// Returns [`DiffError::Io`] if either file cannot be read.
pub fn diff_files(old: &Path, new: &Path, opts: &DiffOptions) -> Result<FileDiff, DiffError> {
    let old_bytes = std::fs::read(old).map_err(|e| DiffError::Io(e.to_string()))?;
    let new_bytes = std::fs::read(new).map_err(|e| DiffError::Io(e.to_string()))?;
    let new_path = new.to_string_lossy().into_owned();
    let old_path = if opts.detect_rename && old != new {
        Some(old.to_string_lossy().into_owned())
    } else {
        None
    };

    // Binary content has no lines to diff. Report it as such rather than failing:
    // "this file is binary" is an answer, an i/o error is not.
    let (Ok(old_content), Ok(new_content)) = (
        std::str::from_utf8(&old_bytes),
        std::str::from_utf8(&new_bytes),
    ) else {
        return Ok(binary_file_diff(&old_bytes, &new_bytes, new_path, old_path));
    };
    if looks_binary(&old_bytes) || looks_binary(&new_bytes) {
        return Ok(binary_file_diff(&old_bytes, &new_bytes, new_path, old_path));
    }

    Ok(build_file_diff(
        old_content,
        new_content,
        new_path,
        old_path,
        opts.context_lines,
    ))
}

/// Whether `bytes` look like binary content: a NUL byte near the start is the
/// same signal git itself uses.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// A hunk-less [`FileDiff`] for binary content, with the status still derived from
/// which sides are empty.
fn binary_file_diff(old: &[u8], new: &[u8], path: String, old_path: Option<String>) -> FileDiff {
    let status = match (old.is_empty(), new.is_empty()) {
        _ if old_path.is_some() => FileStatus::Renamed { similarity: 100 },
        (true, false) => FileStatus::Added,
        (false, true) => FileStatus::Removed,
        _ => FileStatus::Modified,
    };
    FileDiff {
        path,
        old_path,
        status,
        old_mode: None,
        new_mode: None,
        is_binary: true,
        hunks: Vec::new(),
    }
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
        // Diffing two in-memory texts says nothing about how they are stored, so
        // neither mode is known. Callers that know better set them afterwards.
        old_mode: None,
        new_mode: None,
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

/// Split a raw source line into its terminator-free content and the two facts a
/// [`DiffLine`] records about how it ended: whether the terminator was `\r\n`, and
/// whether there was no terminator at all (the file's last line).
///
/// Keeping these separate from `content` means a consumer displays clean text
/// while [`crate::format_hunk_patch`] can still reproduce the original bytes.
fn line_parts(line: &str) -> (&str, bool, bool) {
    match line.strip_suffix('\n') {
        Some(rest) => match rest.strip_suffix('\r') {
            Some(rest) => (rest, true, false),
            None => (rest, false, false),
        },
        // An out-of-range lookup yields `""`, which is not a real unterminated line.
        None if line.is_empty() => (line, false, false),
        None => (line, false, true),
    }
}

/// Build a [`DiffLine`] from a raw source line, carrying its terminator facts.
fn diff_line(
    kind: LineKind,
    old_lineno: Option<u32>,
    new_lineno: Option<u32>,
    raw: &str,
) -> DiffLine {
    let (content, crlf, no_newline) = line_parts(raw);
    let mut line = DiffLine::new(kind, old_lineno, new_lineno, content);
    line.crlf = crlf;
    line.no_newline = no_newline;
    line
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
            let raw = at(old_lines, oi);
            lines.push(diff_line(
                LineKind::Context,
                Some(oi + 1),
                Some(ni + 1),
                raw,
            ));
            oi += 1;
            ni += 1;
        }
        while oi < h.before.end {
            let raw = at(old_lines, oi);
            lines.push(diff_line(LineKind::Remove, Some(oi + 1), None, raw));
            oi += 1;
        }
        while ni < h.after.end {
            let raw = at(new_lines, ni);
            lines.push(diff_line(LineKind::Add, None, Some(ni + 1), raw));
            ni += 1;
        }
    }
    // Trailing context.
    while oi < o1 {
        let raw = at(old_lines, oi);
        lines.push(diff_line(
            LineKind::Context,
            Some(oi + 1),
            Some(ni + 1),
            raw,
        ));
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
    use std::path::PathBuf;

    use super::*;
    use crate::align_hunk;

    /// Write `bytes` to `name` inside `dir` and return the path.
    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> Result<PathBuf, DiffError> {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).map_err(|e| DiffError::Io(e.to_string()))?;
        Ok(path)
    }

    /// A scratch directory, removed when the test ends.
    fn scratch() -> Result<tempfile::TempDir, DiffError> {
        tempfile::tempdir().map_err(|e| DiffError::Io(e.to_string()))
    }

    #[test]
    fn diff_files_reports_binary_instead_of_failing() -> Result<(), DiffError> {
        let dir = scratch()?;
        // A NUL byte is the same signal git uses to call a file binary.
        let old = write(&dir, "old.bin", b"\x00\x01binary")?;
        let new = write(&dir, "new.bin", b"\x00\x09other")?;

        let f = diff_files(&old, &new, &DiffOptions::default())?;
        assert!(f.is_binary);
        assert!(f.hunks.is_empty());
        assert_eq!(f.status, FileStatus::Modified);
        Ok(())
    }

    #[test]
    fn diff_files_reports_invalid_utf8_as_binary() -> Result<(), DiffError> {
        let dir = scratch()?;
        // Invalid UTF-8 with no NUL: `read_to_string` used to turn this into an
        // i/o error, which told the caller nothing useful.
        let old = write(&dir, "old.dat", b"caf\xe9 latte")?;
        let new = write(&dir, "new.dat", b"caf\xe9 mocha")?;

        let f = diff_files(&old, &new, &DiffOptions::default())?;
        assert!(f.is_binary);
        assert!(f.hunks.is_empty());
        Ok(())
    }

    #[test]
    fn diff_files_diffs_text_normally() -> Result<(), DiffError> {
        let dir = scratch()?;
        let old = write(&dir, "old.txt", b"a\nb\nc\n")?;
        let new = write(&dir, "new.txt", b"a\nB\nc\n")?;

        let f = diff_files(&old, &new, &DiffOptions::default())?;
        assert!(!f.is_binary);
        assert_eq!(f.hunks.len(), 1);
        Ok(())
    }

    #[test]
    fn diff_files_still_errors_when_a_file_is_missing() -> Result<(), DiffError> {
        let dir = scratch()?;
        let present = write(&dir, "there.txt", b"x\n")?;
        let missing = dir.path().join("nope.txt");
        assert!(matches!(
            diff_files(&missing, &present, &DiffOptions::default()),
            Err(DiffError::Io(_))
        ));
        Ok(())
    }

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

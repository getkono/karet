//! Parsing unified diff text (e.g. `git diff` output) into the [`Diff`] model.
//!
//! Handles the header shapes git emits — added / deleted / renamed / binary files,
//! quoted and `\NNN`-octal-escaped paths, "no newline at end of file" markers — and
//! recovers each hunk's enclosing scope (and, where derivable, the new-side scope)
//! from the `@@ … @@` headers.

use crate::DiffError;
use crate::align::align_hunk;
use crate::model::{Diff, DiffLine, FileDiff, FileStatus, Hunk, LineKind};

/// Parse unified diff text into a [`Diff`].
///
/// # Errors
/// Returns [`DiffError::Parse`] if a hunk header or range is malformed.
pub fn parse(raw: &str) -> Result<Diff, DiffError> {
    // Split on "diff --git " boundaries into per-file blocks.
    let mut files = Vec::new();
    let mut blocks: Vec<&str> = Vec::new();

    let mut start = 0;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let nl = find_byte(b'\n', &bytes[i..]).map_or(bytes.len(), |p| i + p);
        let line = &raw[i..nl];
        if line.starts_with("diff --git ") && i > start {
            blocks.push(&raw[start..i]);
            start = i;
        }
        i = nl + 1;
    }
    if start < raw.len() {
        blocks.push(&raw[start..]);
    }

    for block in blocks {
        if let Some(fd) = parse_file_block(block)? {
            files.push(fd);
        }
    }

    Ok(Diff { files })
}

fn find_byte(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

fn parse_file_block(block: &str) -> Result<Option<FileDiff>, DiffError> {
    let lines: Vec<&str> = block.lines().collect();
    let Some(first) = lines.first() else {
        return Ok(None);
    };
    let Some(rest) = first.strip_prefix("diff --git ") else {
        return Ok(None);
    };

    let (_header_old, header_new) = split_ab_paths(rest);

    let mut old_path: Option<String> = None;
    let mut new_path = header_new;
    let mut status = FileStatus::Modified;
    let mut is_binary = false;
    let mut similarity: Option<u8> = None;

    let mut hunk_start_idx = lines.len(); // default: no hunks

    let mut idx = 1;
    while idx < lines.len() {
        let line = lines[idx];

        if let Some(pct) = line.strip_prefix("similarity index ") {
            similarity = pct.trim_end_matches('%').parse::<u8>().ok();
        } else if let Some(stripped) = line.strip_prefix("rename from ") {
            old_path = Some(decode_path(stripped));
        } else if let Some(stripped) = line.strip_prefix("rename to ") {
            new_path = decode_path(stripped);
            status = FileStatus::Renamed {
                similarity: similarity.unwrap_or(100),
            };
        } else if line.starts_with("new file mode") {
            status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            status = FileStatus::Removed;
        } else if line.starts_with("Binary files") {
            is_binary = true;
        } else if line.starts_with("@@ ") {
            hunk_start_idx = idx;
            break;
        }
        // `--- `, `+++ ` and `index ` lines carry no information we don't already
        // have, so they fall through and are skipped.
        idx += 1;
    }

    let path = new_path;

    let hunks = if is_binary {
        Vec::new()
    } else {
        parse_hunks(&lines[hunk_start_idx.min(lines.len())..])?
    };

    let mut fd = FileDiff {
        path,
        old_path,
        status,
        is_binary,
        hunks,
    };
    populate_new_scopes(&mut fd);
    Ok(Some(fd))
}

/// Compute each hunk's `new_scope` (the new-side signature for the side-by-side
/// view). Git's `scope` always reflects the *old* signature; when the enclosing
/// scope line was itself modified in this diff (a `-old`/`+new` pair, often in an
/// earlier hunk), the new signature is recoverable from the diff content.
///
/// For each hunk with `scope = Some(s)`, find the nearest *preceding* removed line
/// whose trimmed content starts with `s`, and take its 1:1 replacement (the paired
/// added line) as `new_scope`. Prefix matching handles git truncating long headings
/// to ~80 chars; trimming handles git stripping leading whitespace.
fn populate_new_scopes(file: &mut FileDiff) {
    // Nothing to resolve when no hunk carries a scope heading (the common case, and
    // always true for engine-built diffs).
    if file.hunks.iter().all(|h| h.scope.is_none()) {
        return;
    }

    // Phase 1: collect every removed→added replacement across all hunks.
    let mut replacements: Vec<(u32, String, String)> = Vec::new();
    for hunk in &file.hunks {
        for row in align_hunk(&hunk.lines) {
            if let (Some(left), Some(right)) = (&row.left, &row.right)
                && left.kind == LineKind::Remove
                && right.kind == LineKind::Add
            {
                replacements.push((
                    left.lineno,
                    left.content.trim().to_string(),
                    right.content.trim().to_string(),
                ));
            }
        }
    }

    // Phase 2: assign new_scope from the nearest preceding matching replacement.
    for hunk in &mut file.hunks {
        let Some(scope) = hunk.scope.as_deref() else {
            continue;
        };
        let best = replacements
            .iter()
            .filter(|(lineno, removed, _)| *lineno < hunk.old_start && removed.starts_with(scope))
            .max_by_key(|(lineno, _, _)| *lineno);
        if let Some((_, _, added)) = best {
            hunk.new_scope = Some(added.clone());
        }
    }
}

/// Given `a/<old> b/<new>`, split into `(old, new)` stripping the `a/` / `b/`
/// prefixes. Handles bare, both-quoted, and `\NNN`-octal-escaped header shapes.
fn split_ab_paths(s: &str) -> (String, String) {
    if s.starts_with("\"a/")
        && let Some((old, new)) = split_quoted_ab(s)
    {
        return (old, new);
    }
    if let Some(pos) = find_b_split(s) {
        let old = s[2..pos].to_string(); // skip "a/"
        let new = s[pos + 3..].to_string(); // skip " b/"
        return (old, new);
    }
    if let Some(pos) = s.find(' ') {
        let old = s[2..pos].to_string();
        let new = s[pos + 3..].to_string();
        return (old, new);
    }
    (s.to_string(), s.to_string())
}

/// Parse a `"a/..." "b/..."` header. Returns `None` if the structure doesn't match.
fn split_quoted_ab(s: &str) -> Option<(String, String)> {
    let close_a = find_closing_quote(s, 1)?;
    let after = &s[close_a + 1..];
    let after_b = after.strip_prefix(" \"b/")?;
    let new_inner = after_b.strip_suffix('"')?;
    let old_inner = &s[3..close_a]; // skip `"a/`
    Some((decode_path(old_inner), decode_path(new_inner)))
}

/// Find the closing `"` matching the opening quote at `start`, honoring `\"`.
fn find_closing_quote(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Decode a git-emitted path: strip surrounding quotes, `\NNN` octal escapes, and
/// the `\\`, `\"`, `\t`, `\n`, `\r` short escapes back into bytes, then read as
/// UTF-8 (lossy if the bytes aren't valid UTF-8, so the filename is never lost).
fn decode_path(s: &str) -> String {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);

    if !inner.contains('\\') {
        return inner.to_string();
    }

    let bytes = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        // `\` followed by something — try octal (\NNN) first, then short escapes.
        if bytes.get(i + 1).is_some_and(is_octal_digit)
            && bytes.get(i + 2).is_some_and(is_octal_digit)
            && bytes.get(i + 3).is_some_and(is_octal_digit)
        {
            let n = (octal_value(bytes[i + 1]) << 6)
                | (octal_value(bytes[i + 2]) << 3)
                | octal_value(bytes[i + 3]);
            out.push(n);
            i += 4;
            continue;
        }
        match bytes.get(i + 1) {
            Some(&b'\\') => {
                out.push(b'\\');
                i += 2;
            }
            Some(&b'"') => {
                out.push(b'"');
                i += 2;
            }
            Some(&b't') => {
                out.push(b'\t');
                i += 2;
            }
            Some(&b'n') => {
                out.push(b'\n');
                i += 2;
            }
            Some(&b'r') => {
                out.push(b'\r');
                i += 2;
            }
            _ => {
                out.push(b'\\');
                i += 1;
            }
        }
    }

    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
}

fn is_octal_digit(b: &u8) -> bool {
    (b'0'..=b'7').contains(b)
}

fn octal_value(b: u8) -> u8 {
    b - b'0'
}

/// Locate the rightmost ` b/` boundary that leaves a non-empty `a/` segment.
fn find_b_split(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut pos = s.len();
    while pos > 0 {
        pos -= 1;
        if bytes.get(pos) == Some(&b' ')
            && bytes.get(pos + 1) == Some(&b'b')
            && bytes.get(pos + 2) == Some(&b'/')
            && pos >= 3
        {
            return Some(pos);
        }
    }
    None
}

fn parse_hunks(lines: &[&str]) -> Result<Vec<Hunk>, DiffError> {
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if !line.starts_with("@@ ") {
            i += 1;
            continue;
        }

        let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
        let header = line.to_string();
        let scope = extract_scope(&header);
        i += 1;

        let mut diff_lines = Vec::new();
        let mut old_lineno = old_start;
        let mut new_lineno = new_start;

        while i < lines.len() && !lines[i].starts_with("@@ ") {
            let l = lines[i];
            if l.starts_with("\\ ") {
                // "\ No newline at end of file" — skip.
                i += 1;
                continue;
            }
            let (kind, content) = if let Some(rest) = l.strip_prefix('+') {
                (LineKind::Add, rest.to_string())
            } else if let Some(rest) = l.strip_prefix('-') {
                (LineKind::Remove, rest.to_string())
            } else {
                let rest = l.strip_prefix(' ').unwrap_or(l);
                (LineKind::Context, rest.to_string())
            };

            let (old_ln, new_ln) = match kind {
                LineKind::Context => {
                    let pair = (Some(old_lineno), Some(new_lineno));
                    old_lineno += 1;
                    new_lineno += 1;
                    pair
                }
                LineKind::Remove => {
                    let pair = (Some(old_lineno), None);
                    old_lineno += 1;
                    pair
                }
                LineKind::Add => {
                    let pair = (None, Some(new_lineno));
                    new_lineno += 1;
                    pair
                }
            };

            diff_lines.push(DiffLine {
                kind,
                old_lineno: old_ln,
                new_lineno: new_ln,
                content,
            });
            i += 1;
        }

        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            header,
            scope,
            new_scope: None,
            lines: diff_lines,
        });
    }

    Ok(hunks)
}

/// Extract scope text from a header like `@@ -1,5 +1,5 @@ fn main() {` (everything
/// after the closing `@@ `). Returns `None` when there is no trailing text.
fn extract_scope(header: &str) -> Option<String> {
    let after_prefix = header.strip_prefix("@@ ")?;
    let close = after_prefix.find(" @@")?;
    let trimmed = after_prefix[close + 3..].trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse `@@ -old_start[,old_count] +new_start[,new_count] @@`.
fn parse_hunk_header(line: &str) -> Result<(u32, u32, u32, u32), DiffError> {
    let err = || DiffError::Parse(format!("invalid hunk header: {line}"));

    let inner = line.strip_prefix("@@ ").ok_or_else(err)?;
    let end = inner.find(" @@").ok_or_else(err)?;
    let ranges = &inner[..end];

    let mut parts = ranges.splitn(2, ' ');
    let old_part = parts.next().ok_or_else(err)?;
    let new_part = parts.next().ok_or_else(err)?;

    let (old_start, old_count) = parse_range(old_part.trim_start_matches('-'))?;
    let (new_start, new_count) = parse_range(new_part.trim_start_matches('+'))?;

    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(s: &str) -> Result<(u32, u32), DiffError> {
    let err = || DiffError::Parse(format!("invalid range: {s}"));
    if let Some((a, b)) = s.split_once(',') {
        let start = a.parse::<u32>().map_err(|_| err())?;
        let count = b.parse::<u32>().map_err(|_| err())?;
        Ok((start, count))
    } else {
        let start = s.parse::<u32>().map_err(|_| err())?;
        Ok((start, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_MODIFIED: &str = r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,5 @@
 fn main() {
-    println!("hello");
+    println!("world");
     let x = 1;
     let y = 2;
 }
"#;

    const ADDED_FILE: &str = r#"diff --git a/new.rs b/new.rs
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,3 @@
+fn new_fn() {
+    // body
+}
"#;

    const DELETED_FILE: &str = r#"diff --git a/old.rs b/old.rs
deleted file mode 100644
index 1111111..0000000
--- a/old.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn old_fn() {
-    // body
-}
"#;

    const RENAMED_FILE: &str = r#"diff --git a/foo.rs b/bar.rs
similarity index 90%
rename from foo.rs
rename to bar.rs
index abc..def 100644
--- a/foo.rs
+++ b/bar.rs
@@ -1,3 +1,3 @@
 fn func() {
-    old();
+    new();
 }
"#;

    const BINARY_FILE: &str = r#"diff --git a/image.png b/image.png
index abc..def 100644
Binary files a/image.png and b/image.png differ
"#;

    const MULTI_HUNK: &str = r#"diff --git a/lib.rs b/lib.rs
index aaa..bbb 100644
--- a/lib.rs
+++ b/lib.rs
@@ -1,4 +1,4 @@
 use std::io;
-use std::fs;
+use std::path;

 fn a() {}
@@ -10,4 +10,4 @@
 fn b() {
-    old_call();
+    new_call();
     let z = 0;
 }
"#;

    const NO_NEWLINE: &str = "diff --git a/file.txt b/file.txt\nindex aaa..bbb 100644\n--- a/file.txt\n+++ b/file.txt\n@@ -1,1 +1,1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";

    fn count(file: &FileDiff, kind: LineKind) -> usize {
        file.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .count()
    }

    #[test]
    fn parse_modified() -> Result<(), DiffError> {
        let diff = parse(SIMPLE_MODIFIED)?;
        assert_eq!(diff.files.len(), 1);
        let f = &diff.files[0];
        assert_eq!(f.path, "src/main.rs");
        assert_eq!(f.status, FileStatus::Modified);
        assert!(!f.is_binary);
        assert_eq!(f.hunks.len(), 1);
        let h = &f.hunks[0];
        assert_eq!(h.lines.len(), 6); // 1 ctx + 1 rem + 1 add + 3 ctx
        assert_eq!(h.lines[1].kind, LineKind::Remove);
        assert_eq!(h.lines[2].kind, LineKind::Add);
        Ok(())
    }

    #[test]
    fn parse_added() -> Result<(), DiffError> {
        let diff = parse(ADDED_FILE)?;
        assert_eq!(diff.files[0].status, FileStatus::Added);
        assert_eq!(count(&diff.files[0], LineKind::Add), 3);
        assert_eq!(count(&diff.files[0], LineKind::Remove), 0);
        Ok(())
    }

    #[test]
    fn parse_deleted() -> Result<(), DiffError> {
        let diff = parse(DELETED_FILE)?;
        assert_eq!(diff.files[0].status, FileStatus::Removed);
        assert_eq!(count(&diff.files[0], LineKind::Add), 0);
        assert_eq!(count(&diff.files[0], LineKind::Remove), 3);
        Ok(())
    }

    #[test]
    fn parse_renamed() -> Result<(), DiffError> {
        let diff = parse(RENAMED_FILE)?;
        let f = &diff.files[0];
        assert_eq!(f.path, "bar.rs");
        assert_eq!(f.old_path.as_deref(), Some("foo.rs"));
        assert!(matches!(f.status, FileStatus::Renamed { similarity: 90 }));
        assert_eq!(count(f, LineKind::Add), 1);
        assert_eq!(count(f, LineKind::Remove), 1);
        Ok(())
    }

    #[test]
    fn parse_binary() -> Result<(), DiffError> {
        let diff = parse(BINARY_FILE)?;
        assert!(diff.files[0].is_binary);
        assert!(diff.files[0].hunks.is_empty());
        Ok(())
    }

    #[test]
    fn parse_multi_hunk() -> Result<(), DiffError> {
        let diff = parse(MULTI_HUNK)?;
        let f = &diff.files[0];
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(count(f, LineKind::Add), 2);
        assert_eq!(count(f, LineKind::Remove), 2);
        Ok(())
    }

    #[test]
    fn parse_no_newline_at_eof() -> Result<(), DiffError> {
        let diff = parse(NO_NEWLINE)?;
        let lines = &diff.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, LineKind::Remove);
        assert_eq!(lines[1].kind, LineKind::Add);
        Ok(())
    }

    #[test]
    fn parse_empty_input() -> Result<(), DiffError> {
        assert!(parse("")?.files.is_empty());
        Ok(())
    }

    #[test]
    fn line_numbers_tracked() -> Result<(), DiffError> {
        let diff = parse(SIMPLE_MODIFIED)?;
        let lines = &diff.files[0].hunks[0].lines;
        assert_eq!(
            (lines[0].old_lineno, lines[0].new_lineno),
            (Some(1), Some(1))
        );
        assert_eq!((lines[1].old_lineno, lines[1].new_lineno), (Some(2), None));
        assert_eq!((lines[2].old_lineno, lines[2].new_lineno), (None, Some(2)));
        Ok(())
    }

    #[test]
    fn invalid_hunk_header_errors() {
        let raw = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ bogus @@\n-a\n+b\n";
        assert!(matches!(parse(raw), Err(DiffError::Parse(_))));
    }

    #[test]
    fn engine_output_round_trips_through_patch_and_parse() -> Result<(), DiffError> {
        use crate::{DiffOptions, diff_text, format_hunk_patch};
        let engine_diff = diff_text(
            "a\nb\nc\nd\n",
            "a\nB\nc\nd\n",
            &DiffOptions {
                path_hint: Some("file.txt".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(engine_diff.hunks.len(), 1);
        let patch = format_hunk_patch(&engine_diff, &engine_diff.hunks[0]);
        let reparsed = parse(&patch)?;
        assert_eq!(reparsed.files.len(), 1);
        let rf = &reparsed.files[0];
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

    // ── scope / new_scope ────────────────────────────────────────────────────

    #[test]
    fn scope_absent_then_present() -> Result<(), DiffError> {
        let diff = parse(MULTI_HUNK)?;
        assert!(diff.files[0].hunks[0].scope.is_none());

        let input = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1,3 +1,3 @@ fn process_event(event: &Event) {\n use std::io;\n-old();\n+new();\n";
        let diff = parse(input)?;
        assert_eq!(
            diff.files[0].hunks[0].scope.as_deref(),
            Some("fn process_event(event: &Event) {")
        );
        Ok(())
    }

    #[test]
    fn new_scope_from_earlier_hunk() -> Result<(), DiffError> {
        let raw = "diff --git a/src/daemon.rs b/src/daemon.rs\nindex aaa..bbb 100644\n--- a/src/daemon.rs\n+++ b/src/daemon.rs\n@@ -54,2 +54,2 @@\n-pub(crate) fn start_daemon() -> anyhow::Result<()> {\n+pub(crate) fn start_daemon() -> anyhow::Result<Option<std::process::Child>> {\n use std::os::unix::fs::OpenOptionsExt;\n@@ -76,2 +101,2 @@ pub(crate) fn start_daemon() -> anyhow::Result<()> {\n-    return Err(anyhow!(\"x\"));\n+    return Ok(None);\n";
        let diff = parse(raw)?;
        let hunks = &diff.files[0].hunks;
        assert_eq!(
            hunks[1].scope.as_deref(),
            Some("pub(crate) fn start_daemon() -> anyhow::Result<()> {")
        );
        assert_eq!(
            hunks[1].new_scope.as_deref(),
            Some("pub(crate) fn start_daemon() -> anyhow::Result<Option<std::process::Child>> {")
        );
        assert_eq!(hunks[0].new_scope, None);
        Ok(())
    }

    #[test]
    fn new_scope_none_when_signature_unchanged() -> Result<(), DiffError> {
        let raw = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -10,3 +10,3 @@ fn process() {\n     before();\n-    old();\n+    new();\n";
        let diff = parse(raw)?;
        let h = &diff.files[0].hunks[0];
        assert_eq!(h.scope.as_deref(), Some("fn process() {"));
        assert_eq!(h.new_scope, None);
        Ok(())
    }

    #[test]
    fn new_scope_matches_by_trimmed_content() -> Result<(), DiffError> {
        let raw = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -5,2 +5,2 @@\n-    fn inner(&self) -> u8 {\n+    fn inner(&self) -> u16 {\n ctx\n@@ -20,2 +20,2 @@ fn inner(&self) -> u8 {\n-        a\n+        b\n";
        let diff = parse(raw)?;
        let h = &diff.files[0].hunks[1];
        assert_eq!(h.scope.as_deref(), Some("fn inner(&self) -> u8 {"));
        assert_eq!(h.new_scope.as_deref(), Some("fn inner(&self) -> u16 {"));
        Ok(())
    }

    #[test]
    fn new_scope_prefix_match_on_truncated_heading() -> Result<(), DiffError> {
        let full_new = "pub fn long_name(a: A, b: B, c: C, d: D, e: E, f: F, g: G) -> NewRet {";
        let truncated = "pub fn long_name(a: A, b: B, c: C, d: D, e: E, f: F, g: G) -> OldR";
        let raw = format!(
            "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -3,2 +3,2 @@\n-pub fn long_name(a: A, b: B, c: C, d: D, e: E, f: F, g: G) -> OldRet {{\n+{full_new}\n ctx\n@@ -30,2 +30,2 @@ {truncated}\n-    x\n+    y\n"
        );
        let diff = parse(&raw)?;
        let h = &diff.files[0].hunks[1];
        assert_eq!(h.scope.as_deref(), Some(truncated));
        assert_eq!(h.new_scope.as_deref(), Some(full_new));
        Ok(())
    }

    #[test]
    fn new_scope_nearest_preceding_wins() -> Result<(), DiffError> {
        let raw = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -5,5 +5,5 @@\n-fn build() -> A {\n+fn build() -> A2 {\n ctx\n-fn build() -> B {\n+fn build() -> B2 {\n@@ -30,2 +30,2 @@ fn build()\n-    x\n+    y\n";
        let diff = parse(raw)?;
        assert_eq!(
            diff.files[0].hunks[1].new_scope.as_deref(),
            Some("fn build() -> B2 {")
        );
        Ok(())
    }

    #[test]
    fn new_scope_none_when_scope_line_deleted_without_replacement() -> Result<(), DiffError> {
        let raw = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -5,3 +5,1 @@\n-fn gone() -> X {\n-    body\n ctx\n@@ -30,2 +30,2 @@ fn gone() -> X {\n-    a\n+    b\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files[0].hunks[1].new_scope, None);
        Ok(())
    }

    // ── quoted / escaped paths ───────────────────────────────────────────────

    #[test]
    fn parse_quoted_path_with_space() -> Result<(), DiffError> {
        let raw = "diff --git \"a/foo bar.txt\" \"b/foo bar.txt\"\nindex aaa..bbb 100644\n--- \"a/foo bar.txt\"\n+++ \"b/foo bar.txt\"\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files[0].path, "foo bar.txt");
        Ok(())
    }

    #[test]
    fn parse_quoted_path_with_octal_escape() -> Result<(), DiffError> {
        // \357\274\232 = U+FF1A FULLWIDTH COLON
        let raw = "diff --git \"a/A1\\357\\274\\232 X.html\" \"b/A1\\357\\274\\232 X.html\"\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ \"b/A1\\357\\274\\232 X.html\"\n@@ -0,0 +1,1 @@\n+hi\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files[0].path, "A1\u{ff1a} X.html");
        assert_eq!(diff.files[0].status, FileStatus::Added);
        Ok(())
    }

    #[test]
    fn parse_quoted_renamed_path() -> Result<(), DiffError> {
        let raw = "diff --git \"a/A\\357\\274\\232.txt\" \"b/B\\357\\274\\232.txt\"\nsimilarity index 100%\nrename from \"A\\357\\274\\232.txt\"\nrename to \"B\\357\\274\\232.txt\"\n";
        let diff = parse(raw)?;
        let f = &diff.files[0];
        assert_eq!(f.path, "B\u{ff1a}.txt");
        assert_eq!(f.old_path.as_deref(), Some("A\u{ff1a}.txt"));
        assert!(matches!(f.status, FileStatus::Renamed { similarity: 100 }));
        Ok(())
    }

    #[test]
    fn decode_path_passthrough_and_escapes() {
        assert_eq!(decode_path("src/main.rs"), "src/main.rs");
        // \343\201\223 = U+3053
        assert_eq!(decode_path("\\343\\201\\223.txt"), "こ.txt");
        assert_eq!(decode_path("a\\\\b"), "a\\b");
        assert_eq!(decode_path("a\\\"b"), "a\"b");
        assert_eq!(decode_path("a\\tb"), "a\tb");
    }

    // ── right_header / right_scope (new-side display) ─────────────────────────

    fn hunk_with(header: &str, scope: Option<&str>, new_scope: Option<&str>) -> Hunk {
        Hunk {
            old_start: 6,
            old_count: 6,
            new_start: 6,
            new_count: 6,
            header: header.to_string(),
            scope: scope.map(str::to_string),
            new_scope: new_scope.map(str::to_string),
            lines: vec![],
        }
    }

    #[test]
    fn right_header_swaps_suffix_keeps_range() {
        let h = hunk_with("@@ -6,6 +6,6 @@ old_sig", Some("old_sig"), Some("new_sig"));
        assert_eq!(h.right_header(), "@@ -6,6 +6,6 @@ new_sig");
    }

    #[test]
    fn right_header_unchanged_without_new_scope() {
        let h = hunk_with("@@ -6,6 +6,6 @@ old_sig", Some("old_sig"), None);
        assert_eq!(h.right_header(), "@@ -6,6 +6,6 @@ old_sig");
        let h2 = hunk_with("@@ -6,6 +6,6 @@", None, None);
        assert_eq!(h2.right_header(), "@@ -6,6 +6,6 @@");
    }

    #[test]
    fn right_scope_prefers_new() {
        let h = hunk_with("@@ -6,6 +6,6 @@ old", Some("old"), Some("new"));
        assert_eq!(h.right_scope(), Some("new"));
        let h2 = hunk_with("@@ -6,6 +6,6 @@ old", Some("old"), None);
        assert_eq!(h2.right_scope(), Some("old"));
    }
}

//! Recovering the enclosing-scope text git puts after a hunk's `@@ … @@`.
//!
//! Git's scope suffix always reflects the *pre-image* signature. When that
//! signature was itself edited elsewhere in the same file, the post-image form is
//! recoverable from the diff content, which is what [`populate_new_scopes`] does.

use crate::align::align_hunk;
use crate::model::FileDiff;
use crate::model::LineKind;

/// Extract scope text from a header like `@@ -1,5 +1,5 @@ fn main() {` (everything
/// after the closing `@@ `). Returns `None` when there is no trailing text.
pub(super) fn extract_scope(header: &str) -> Option<String> {
    let after_prefix = header.strip_prefix("@@ ")?;
    let close = after_prefix.find(" @@")?;
    let trimmed = after_prefix[close + 3..].trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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
pub(super) fn populate_new_scopes(file: &mut FileDiff) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffError;
    use crate::parse::parse;

    #[test]
    fn extract_scope_reads_the_suffix() {
        assert_eq!(
            extract_scope("@@ -1,5 +1,5 @@ fn main() {").as_deref(),
            Some("fn main() {")
        );
        assert_eq!(extract_scope("@@ -1,5 +1,5 @@"), None);
        assert_eq!(extract_scope("@@ -1,5 +1,5 @@   "), None);
        assert_eq!(extract_scope("not a hunk header"), None);
    }

    #[test]
    fn scope_absent_then_present() -> Result<(), DiffError> {
        let bare = "diff --git a/lib.rs b/lib.rs\nindex aaa..bbb 100644\n--- a/lib.rs\n+++ b/lib.rs\n@@ -1,4 +1,4 @@\n use std::io;\n-use std::fs;\n+use std::path;\n";
        assert!(parse(bare)?.files[0].hunks[0].scope.is_none());

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
}

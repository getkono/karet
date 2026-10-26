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

/// The mode git assumes for a file whose mode nobody recorded: a regular,
/// non-executable file.
const DEFAULT_FILE_MODE: u32 = 0o100644;

/// The mode lines for `file`: `new file mode` / `deleted file mode` for a one-sided
/// change, or an `old mode`/`new mode` pair when the mode itself changed.
///
/// A one-sided change always gets its line, falling back to
/// [`DEFAULT_FILE_MODE`] when the mode is unknown: without it git reads the
/// `/dev/null` side as a path, so a deletion renames the file to `dev/null` and
/// a creation fails for want of a `dev/null` in the index.
fn mode_metadata(file: &FileDiff) -> String {
    match (file.status.clone(), file.old_mode, file.new_mode) {
        (FileStatus::Added, _, new) => {
            format!("new file mode {:06o}\n", new.unwrap_or(DEFAULT_FILE_MODE))
        },
        (FileStatus::Removed, old, _) => {
            format!(
                "deleted file mode {:06o}\n",
                old.unwrap_or(DEFAULT_FILE_MODE)
            )
        },
        _ if file.mode_changed() => {
            // `mode_changed` is only true when both sides are known.
            let old = file.old_mode.unwrap_or_default();
            let new = file.new_mode.unwrap_or_default();
            format!("old mode {old:06o}\nnew mode {new:06o}\n")
        },
        _ => String::new(),
    }
}

/// Whether `hunk` only removes lines, the last of them the file's unterminated
/// final line.
///
/// `git apply --unidiff-zero` places such a hunk by its new-side start alone,
/// trying the line *before* the deletion first, and accepts it there when that
/// line holds the same text with a terminator: removing `c` from `c\nc` then
/// leaves `c` rather than `c\n`.
fn removes_unterminated_end(hunk: &Hunk) -> bool {
    hunk.new_count == 0
        && hunk
            .lines
            .last()
            .is_some_and(|line| line.kind == LineKind::Remove && line.no_newline)
}

/// The new-side start a patch writes for `hunk` when it differs from the
/// hunk's own, or `None` when the hunk's header stands as it is.
///
/// `shift` is how many lines the hunks written before it in the same patch add
/// minus how many they remove. The start is counted in the text git patches
/// rather than in the full new file, because `git apply --unidiff-zero` places
/// a context-free hunk by that start: a hunk applied without the ones before it
/// lands where those hunks were never applied. A hunk that keeps or adds lines
/// starts at its first line there; one that only removes lines starts at the
/// line before them (`0` when nothing precedes them), except for the shape
/// [`removes_unterminated_end`] names, which starts at the removed block's own
/// first line so git's first guess is that line rather than its terminated
/// neighbour.
fn written_new_start(hunk: &Hunk, shift: i64) -> Option<u32> {
    let old_start = i64::from(hunk.old_start);
    // The first line the hunk touches, counted in the patched text.
    let first = if hunk.old_count == 0 {
        old_start + 1
    } else {
        old_start
    } + shift;
    let start = if hunk.new_count > 0 || (first > 1 && removes_unterminated_end(hunk)) {
        first
    } else {
        (first - 1).max(0)
    };
    let start = u32::try_from(start).unwrap_or(u32::MAX);
    (start != hunk.new_start).then_some(start)
}

/// How many lines `hunk` adds minus how many it removes.
fn line_shift(hunk: &Hunk) -> i64 {
    i64::from(hunk.new_count) - i64::from(hunk.old_count)
}

/// The `@@` line written for `hunk`, with `shift` as [`written_new_start`]
/// takes it.
///
/// This is the hunk's own header, unless [`written_new_start`] gives another
/// new-side start. Any section text after the closing `@@` is kept.
fn hunk_header(hunk: &Hunk, shift: i64) -> String {
    let Some(new_start) = written_new_start(hunk, shift) else {
        return hunk.header.clone();
    };
    let section = hunk
        .header
        .strip_prefix("@@ ")
        .and_then(|ranges| ranges.split_once(" @@"))
        .map_or("", |(_, section)| section);
    format!(
        "@@ -{},{} +{new_start},{} @@{section}",
        hunk.old_start, hunk.old_count, hunk.new_count
    )
}

/// The hunk header line plus its prefixed content lines.
///
/// Each line is re-emitted with its original terminator, so a CRLF file stays CRLF
/// and a missing final newline stays missing — applying the result cannot silently
/// rewrite either.
fn hunk_body(hunk: &Hunk, shift: i64) -> String {
    let mut out = String::new();
    out.push_str(&hunk_header(hunk, shift));
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
/// `file.hunks`; the `file` supplies the path/status header. An added or removed
/// file whose mode is unknown is written as a regular file (`100644`), so git
/// creates or deletes the path.
///
/// A zero-context hunk needs `git apply --unidiff-zero`, which places it by its
/// line numbers, so the new-side start is written as counted in the old text:
/// the patch holds no other hunk, and git applies it to a text the earlier
/// hunks never touched (`@@ -9,1 +9,1 @@` where the hunk says `+6,1` because
/// earlier hunks removed three lines). One that only removes lines ending at
/// the file's unterminated last line is written with its new-side start at the
/// removed line itself rather than the line before (`@@ -7,1 +7,0 @@` where the
/// hunk says `+6,0`), since git otherwise removes a preceding line that differs
/// from it only by its terminator. Parsing such a patch back yields the start
/// as written.
#[must_use]
pub fn format_hunk_patch(file: &FileDiff, hunk: &Hunk) -> String {
    let mut out = file_header(file);
    out.push_str(&hunk_body(hunk, 0));
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
    /// Emits one file header followed by the staged hunk bodies in order. A staged
    /// index with no matching hunk in `file` is silently skipped, so the result is
    /// an empty string when nothing is staged or no staged index is in range.
    ///
    /// Each hunk's new-side start is counted as [`format_hunk_patch`] counts
    /// it, against the staged hunks before it alone, since the unstaged ones
    /// are not applied.
    #[must_use]
    pub fn staged_patch(&self, file: &FileDiff) -> String {
        let mut out = String::new();
        let mut shift = 0;
        for &idx in &self.staged {
            if let Some(hunk) = file.hunks.get(idx) {
                if out.is_empty() {
                    out.push_str(&file_header(file));
                }
                out.push_str(&hunk_body(hunk, shift));
                shift += line_shift(hunk);
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
        // The engine knows no mode, so the default stands in: without the line
        // git reads `/dev/null` as a path.
        assert!(patch.starts_with("diff --git a/new.rs b/new.rs\nnew file mode 100644\n"));
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
        assert!(patch.starts_with("diff --git a/old.rs b/old.rs\ndeleted file mode 100644\n"));
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

    #[test]
    fn staging_unstage_toggle_off_and_out_of_range() {
        let file = one_hunk_file("f.txt", None, FileStatus::Modified);
        let mut staging = Staging::new();

        staging.stage(0);
        staging.unstage(0);
        assert!(!staging.is_staged(0) && staging.is_empty());
        // Unstaging something never staged is a no-op, not an error.
        staging.unstage(7);
        assert!(staging.is_empty());

        staging.toggle(0);
        assert!(staging.is_staged(0));
        staging.toggle(0);
        assert!(!staging.is_staged(0) && staging.is_empty());

        // An index past the file's hunks is remembered but contributes nothing:
        // with only it staged there is no patch at all, not a header-only one.
        staging.stage(5);
        assert!(!staging.is_empty());
        assert_eq!(staging.staged_patch(&file), "");
        staging.stage(0);
        assert_eq!(
            staging.staged_patch(&file),
            format_hunk_patch(&file, &file.hunks[0])
        );
    }

    /// A tiny deterministic generator (64-bit LCG), so a failing case reproduces
    /// from its seed alone.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        /// A value in `0..n`.
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// Line contents chosen to look like patch syntax once prefixed: a removed
    /// `-- x` reads `--- x`, an added `++ y` reads `+++ y`, and so on.
    const CONTENTS: [&str; 9] = ["a", "b", "c", "", " lead", "-- x", "++ y", "@@ z", "\\ w"];

    /// A text of up to 8 lines drawn from [`CONTENTS`], each ending `\n` or
    /// `\r\n`, and the last one sometimes unterminated.
    fn gen_lines(rng: &mut Lcg) -> Vec<String> {
        let count = rng.below(9);
        (0..count)
            .map(|_| CONTENTS[rng.below(CONTENTS.len() as u64) as usize].to_string())
            .collect()
    }

    fn render(rng: &mut Lcg, lines: &[String]) -> String {
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            out.push_str(line);
            let last = i + 1 == lines.len();
            // An unterminated empty last line is no line at all, so keep it.
            match rng.below(3) {
                0 if last && !line.is_empty() => {},
                1 => out.push_str("\r\n"),
                _ => out.push('\n'),
            }
        }
        out
    }

    /// `lines` with seeded deletions, replacements and insertions.
    fn mutate(rng: &mut Lcg, lines: &[String]) -> Vec<String> {
        let pick = |rng: &mut Lcg| CONTENTS[rng.below(CONTENTS.len() as u64) as usize].to_string();
        let mut out = Vec::new();
        for line in lines {
            match rng.below(10) {
                0 => {},
                1 => out.push(pick(rng)),
                2 => {
                    out.push(pick(rng));
                    out.push(line.clone());
                },
                _ => out.push(line.clone()),
            }
        }
        if rng.below(4) == 0 {
            out.push(pick(rng));
        }
        out
    }

    /// One seeded `(old, new, context)` case.
    fn gen_case(seed: u64) -> (String, String, usize) {
        let mut rng = Lcg(seed);
        let old_lines = gen_lines(&mut rng);
        let new_lines = mutate(&mut rng, &old_lines);
        let old = render(&mut rng, &old_lines);
        let new = render(&mut rng, &new_lines);
        (old, new, rng.below(4) as usize)
    }

    /// Apply `hunks` to `old` the way `patch` would, checking every context and
    /// removed line against the text it claims to replace. `None` on a mismatch.
    fn apply_hunks(old: &str, hunks: &[Hunk]) -> Option<String> {
        let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
        let mut out = String::new();
        let mut next = 0usize;
        for hunk in hunks {
            let start = if hunk.old_count == 0 {
                hunk.old_start
            } else {
                hunk.old_start.checked_sub(1)?
            } as usize;
            out.extend(old_lines.get(next..start)?.iter().copied());
            next = start;
            let (mut old_seen, mut new_seen) = (0, 0);
            for line in &hunk.lines {
                let raw = format!("{}{}", line.content, line.terminator());
                if line.kind != LineKind::Add {
                    (*old_lines.get(next)? == raw).then_some(())?;
                    next += 1;
                    old_seen += 1;
                }
                if line.kind != LineKind::Remove {
                    out.push_str(&raw);
                    new_seen += 1;
                }
            }
            (old_seen == hunk.old_count && new_seen == hunk.new_count).then_some(())?;
        }
        out.extend(old_lines.get(next..)?.iter().copied());
        Some(out)
    }

    /// Which shapes the cases in `seeds` reach, so a generator change that stops
    /// producing one fails loudly instead of quietly testing less:
    /// `[crlf, no_newline, added, removed, ctx 0, multi-hunk]`.
    fn shapes_reached(seeds: std::ops::Range<u64>) -> [bool; 6] {
        let mut seen = [false; 6];
        for seed in seeds {
            let (old, new, context) = gen_case(seed);
            let file = diff_text(&old, &new, &opts(context));
            let lines = || file.hunks.iter().flat_map(|h| &h.lines);
            seen[0] |= lines().any(|l| l.crlf);
            seen[1] |= lines().any(|l| l.no_newline);
            seen[2] |= file.status == FileStatus::Added;
            seen[3] |= file.status == FileStatus::Removed;
            seen[4] |= context == 0 && !file.hunks.is_empty();
            seen[5] |= file.hunks.len() > 1;
        }
        seen
    }

    fn opts(context: usize) -> DiffOptions {
        DiffOptions {
            path_hint: Some("f.txt".into()),
            context_lines: context,
            ..Default::default()
        }
    }

    /// `hunk` as a patch carries it, with `shift` as [`written_new_start`]
    /// takes it: itself, renumbered on the new side to the start that gives.
    fn as_written(hunk: &Hunk, shift: i64) -> Hunk {
        let mut written = hunk.clone();
        if let Some(new_start) = written_new_start(hunk, shift) {
            written.header = hunk_header(hunk, shift);
            written.new_start = new_start;
            let moved = i64::from(new_start) - i64::from(hunk.new_start);
            for line in &mut written.lines {
                line.new_lineno = line
                    .new_lineno
                    .and_then(|n| u32::try_from(i64::from(n) + moved).ok());
            }
        }
        written
    }

    /// Each of `hunks` as one patch carrying all of them writes it.
    fn all_written(hunks: &[Hunk]) -> Vec<Hunk> {
        let mut shift = 0;
        hunks
            .iter()
            .map(|hunk| {
                let written = as_written(hunk, shift);
                shift += line_shift(hunk);
                written
            })
            .collect()
    }

    #[test]
    fn seeded_hunks_apply_and_round_trip_through_parse() -> Result<(), crate::DiffError> {
        assert_eq!(shapes_reached(0..2_000), [true; 6]);
        for seed in 0..2_000 {
            let (old, new, context) = gen_case(seed);
            let file = diff_text(&old, &new, &opts(context));
            assert_eq!(
                apply_hunks(&old, &file.hunks).as_deref(),
                Some(new.as_str()),
                "seed {seed}: {old:?} -> {new:?} at ctx {context}"
            );
            for hunk in &file.hunks {
                let reparsed = crate::parse(&format_hunk_patch(&file, hunk))?;
                let hunks: Vec<&Hunk> = reparsed.files.iter().flat_map(|f| &f.hunks).collect();
                let expected = as_written(hunk, 0);
                assert_eq!(hunks, [&expected], "seed {seed}: single-hunk patch");
            }
            let written = all_written(&file.hunks);
            let mut staging = Staging::new();
            (0..file.hunks.len()).for_each(|i| staging.stage(i));
            let reparsed = crate::parse(&staging.staged_patch(&file))?;
            let hunks: Vec<Hunk> = reparsed.files.into_iter().flat_map(|f| f.hunks).collect();
            assert_eq!(hunks, written, "seed {seed}: combined patch");
        }
        Ok(())
    }

    /// Run `git` in `dir` with `stdin` piped in, returning stdout, or the exit
    /// status and stderr as an error.
    fn git(dir: &std::path::Path, args: &[&str], stdin: &str) -> Result<Vec<u8>, String> {
        use std::io::Write as _;
        use std::process::Command;
        use std::process::Stdio;

        let mut child = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut pipe) = child.stdin.take() {
            pipe.write_all(stdin.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "git {args:?}: {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    /// Whether `git` can be run here; the git-backed tests skip without it.
    fn has_git() -> bool {
        let found = std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok();
        if !found {
            eprintln!("skipped: no git on PATH");
        }
        found
    }

    /// A fresh repository whose index holds `f.txt` with `old`, or nothing at
    /// all when `old` is `None`.
    fn repo_with(old: Option<&str>) -> Result<tempfile::TempDir, String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        git(dir.path(), &["init", "-q"], "")?;
        if let Some(old) = old {
            std::fs::write(dir.path().join("f.txt"), old).map_err(|e| e.to_string())?;
            git(dir.path(), &["add", "f.txt"], "")?;
        }
        Ok(dir)
    }

    /// Apply `patch` to the index of `dir` with the flags
    /// `Repository::apply_index_patch` passes, and return what the index then
    /// holds for `f.txt` (`None`: no entry).
    fn apply_to_index(
        dir: &std::path::Path,
        patch: &str,
        zero_context: bool,
    ) -> Result<Option<String>, String> {
        let mut apply = vec!["apply", "--cached", "--whitespace=nowarn"];
        // git refuses a context-free hunk unless told it is one, since nothing
        // anchors it.
        if zero_context {
            apply.push("--unidiff-zero");
        }
        apply.push("-");
        git(dir, &apply, patch).map_err(|e| format!("{e}\n{patch}"))?;
        let listed = git(dir, &["ls-files", "--", "f.txt"], "")?;
        if listed.is_empty() {
            return Ok(None);
        }
        let staged = git(dir, &["show", ":f.txt"], "")?;
        Ok(Some(String::from_utf8_lossy(&staged).into_owned()))
    }

    /// What `format_hunk_patch` promises — "suitable for `git apply --cached`" —
    /// checked against git itself for modifications, creations and deletions.
    /// Skipped where no `git` is on `PATH`.
    #[test]
    fn git_accepts_every_generated_patch() -> Result<(), String> {
        if !has_git() {
            return Ok(());
        }
        // `[crlf, no_newline, ctx 0, multi-hunk, added, removed, renumbered]`
        // among the cases this runs.
        let mut seen = [false; 7];
        for seed in 0..200 {
            let (old, new, context) = gen_case(seed);
            // Each seed's change, plus the creation of its new text and the
            // deletion of its old one.
            let cases = [(old.as_str(), new.as_str()), ("", &new), (&old, "")];
            for (old, new) in cases {
                let file = diff_text(old, new, &opts(context));
                if file.hunks.is_empty() {
                    continue;
                }
                let lines = || file.hunks.iter().flat_map(|h| &h.lines);
                seen[0] |= lines().any(|l| l.crlf);
                seen[1] |= lines().any(|l| l.no_newline);
                seen[2] |= context == 0;
                seen[3] |= file.hunks.len() > 1;
                seen[4] |= file.status == FileStatus::Added;
                seen[5] |= file.status == FileStatus::Removed;
                seen[6] |= file.hunks.iter().any(removes_unterminated_end);

                // An added file has no old side in the index at all.
                let before = (file.status != FileStatus::Added).then_some(old);
                let repo = repo_with(before)?;
                let dir = repo.path();
                // Each hunk applied alone, and every hunk but the first staged
                // together, leave the old text with just those hunks applied.
                let mut partial = Staging::new();
                (1..file.hunks.len()).for_each(|i| partial.stage(i));
                let alone = file.hunks.iter().enumerate().map(|(i, hunk)| {
                    let patch = format_hunk_patch(&file, hunk);
                    (patch, std::slice::from_ref(hunk), format!("hunk {i} alone"))
                });
                let rest = (file.hunks.len() > 1).then(|| {
                    let patch = partial.staged_patch(&file);
                    (patch, &file.hunks[1..], "all but hunk 0".to_owned())
                });
                for (patch, hunks, what) in alone.chain(rest) {
                    let staged = apply_to_index(dir, &patch, context == 0)
                        .map_err(|e| format!("seed {seed}: {what}: {e}"))?;
                    let expected = match file.status {
                        FileStatus::Removed => None,
                        _ => apply_hunks(old, hunks),
                    };
                    assert_eq!(
                        staged, expected,
                        "seed {seed}: {what}: what the index holds\n{patch}"
                    );
                    // Put the old text back in the index for the next patch.
                    if before.is_some() {
                        git(dir, &["add", "f.txt"], "")?;
                    } else {
                        git(dir, &["rm", "-q", "--cached", "--", "f.txt"], "")?;
                    }
                }
                let mut staging = Staging::new();
                (0..file.hunks.len()).for_each(|i| staging.stage(i));
                let patch = staging.staged_patch(&file);
                let staged = apply_to_index(dir, &patch, context == 0)
                    .map_err(|e| format!("seed {seed}: {e}"))?;
                // A removal leaves no entry; anything else leaves the new text,
                // byte for byte.
                let expected = (file.status != FileStatus::Removed).then_some(new);
                assert_eq!(
                    staged.as_deref(),
                    expected,
                    "seed {seed}: what the index holds\n{patch}"
                );
            }
        }
        assert_eq!(seen, [true; 7], "the seeds reach every shape");
        Ok(())
    }

    /// Removing a file's unterminated last line after a line holding the same
    /// text, terminated, at zero context: with the header git itself writes
    /// (`+1,0`), `git apply --unidiff-zero` removes the terminated line
    /// instead, so the patch names the removed line's own position.
    #[test]
    fn git_removes_the_unterminated_line_not_its_twin() -> Result<(), String> {
        for (old, new) in [("c\nc", "c\n"), ("c\r\nc", "c\r\n"), ("c\nc\nc", "c\n")] {
            let file = diff_text(old, new, &opts(0));
            let patch = format_hunk_patch(&file, &file.hunks[0]);
            assert!(patch.contains(" +2,0 @@\n"), "{patch}");
            if !has_git() {
                continue;
            }
            let repo = repo_with(Some(old))?;
            let staged = apply_to_index(repo.path(), &patch, true)?;
            assert_eq!(staged.as_deref(), Some(new), "{old:?} -> {new:?}\n{patch}");
        }
        Ok(())
    }

    /// The same removal applied without the hunk before it, which removed a
    /// line: the patch counts its start in the text git patches, so the
    /// unterminated `c` goes and its twin keeps its newline.
    #[test]
    fn git_removes_the_unterminated_line_without_the_hunks_before_it() -> Result<(), String> {
        let (old, wanted) = ("x\nc\nc", "x\nc\n");
        let file = diff_text(old, "c\n", &opts(0));
        assert_eq!(file.hunks.len(), 2, "{:?}", file.hunks);
        // In the full new file the removal follows line 1; alone, line 2.
        assert_eq!(file.hunks[1].new_start, 1);
        let alone = format_hunk_patch(&file, &file.hunks[1]);
        let mut staging = Staging::new();
        staging.stage(1);
        assert_eq!(staging.staged_patch(&file), alone);
        assert!(alone.contains("@@ -3,1 +3,0 @@\n"), "{alone}");
        if !has_git() {
            return Ok(());
        }
        let repo = repo_with(Some(old))?;
        let staged = apply_to_index(repo.path(), &alone, true)?;
        assert_eq!(staged.as_deref(), Some(wanted), "{alone}");
        Ok(())
    }

    #[test]
    fn only_a_pure_removal_of_the_unterminated_end_is_renumbered() {
        let mut hunk = one_hunk_file("f.txt", None, FileStatus::Modified)
            .hunks
            .remove(0);
        hunk.lines[1].no_newline = true;
        // It also adds a line, so git places it by that line, which it names.
        assert_eq!(hunk_header(&hunk, 0), hunk.header);
        hunk.lines.truncate(1);
        hunk.lines[0].no_newline = true;
        hunk.new_count = 0;
        hunk.new_start = 0;
        hunk.header = "@@ -1,1 +0,0 @@".into();
        // Nothing precedes the removal, so git's guess is already the line.
        assert_eq!(hunk_header(&hunk, 0), hunk.header);
        hunk.old_start = 2;
        hunk.new_start = 1;
        hunk.header = "@@ -2,1 +1,0 @@ fn f".into();
        // The section text after the ranges survives the renumbering.
        assert_eq!(hunk_header(&hunk, 0), "@@ -2,1 +2,0 @@ fn f");
        // Earlier hunks in the same patch move it by what they add or remove;
        // once they removed everything before it, nothing precedes it.
        assert_eq!(hunk_header(&hunk, 3), "@@ -2,1 +5,0 @@ fn f");
        assert_eq!(hunk_header(&hunk, -1), "@@ -2,1 +0,0 @@ fn f");
        hunk.lines[0].no_newline = false;
        // A terminated line matches only its own text, so it is left alone.
        assert_eq!(hunk_header(&hunk, 0), hunk.header);
    }

    /// A file that still exists but went empty, or an empty file that gained
    /// lines, is a modification: with the status corrected to `Modified` (as a
    /// producer that knows the file's history does), git keeps the path in the
    /// index rather than deleting or re-creating it.
    #[test]
    fn git_keeps_an_emptied_or_filled_file_as_a_modification() -> Result<(), String> {
        if !has_git() {
            return Ok(());
        }
        for (old, new) in [("a\nb\n", ""), ("", "a\nb\n")] {
            let mut file = diff_text(old, new, &opts(3));
            file.status = FileStatus::Modified;
            let repo = repo_with(Some(old))?;
            let patch = format_hunk_patch(&file, &file.hunks[0]);
            assert!(!patch.contains("file mode"), "{patch}");
            let staged = apply_to_index(repo.path(), &patch, false)?;
            assert_eq!(staged.as_deref(), Some(new), "{old:?} -> {new:?}");
        }
        Ok(())
    }
}

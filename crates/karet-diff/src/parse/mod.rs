//! Parsing unified diff text (e.g. `git diff` output) into the [`Diff`] model.
//!
//! Handles every block shape git emits — added / deleted / renamed / copied /
//! rewritten / binary / mode-only / type-changed files, and combined (`--cc`)
//! blocks for unmerged paths — plus quoted and `\NNN`-octal-escaped paths, CRLF
//! terminators, and "no newline at end of file" markers. Each hunk's enclosing
//! scope (and, where derivable, its new-side scope) is recovered from the
//! `@@ … @@` headers.
//!
//! Nothing git reports is dropped: every block becomes a [`FileDiff`], even when
//! its contents cannot be represented (a combined diff) or there are none (a mode
//! change). Consumers can rely on the file list being complete.

mod file;
mod hunks;
mod path;
mod scope;

use crate::DiffError;
use crate::model::Diff;
use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::parse::file::is_block_start;
use crate::parse::file::parse_file_block;

/// The `S_IFMT` mask: the bits of a git file mode that encode the entry's *type*
/// (regular file, symlink, gitlink) rather than its permissions.
const FILE_TYPE_MASK: u32 = 0o170000;

/// Split a raw line into its content and whether it ended with `\r`.
///
/// Unlike [`str::lines`], which silently discards the `\r`, this keeps the fact
/// available: for a content line that `\r` is part of the file and must survive
/// into a rebuilt patch.
fn strip_cr(line: &str) -> (&str, bool) {
    match line.strip_suffix('\r') {
        Some(rest) => (rest, true),
        None => (line, false),
    }
}

/// Parse unified diff text into a [`Diff`].
///
/// # Errors
/// Returns [`DiffError::Parse`] if a hunk header or range is malformed.
pub fn parse(raw: &str) -> Result<Diff, DiffError> {
    let mut files = Vec::new();
    for block in split_blocks(raw) {
        if let Some(fd) = parse_file_block(&block)? {
            files.push(fd);
        }
    }
    mark_type_changes(&mut files);
    Ok(Diff { files })
}

/// Split the diff into per-file blocks, each a list of raw lines (terminators
/// removed, but any `\r` retained for [`strip_cr`] to interpret).
fn split_blocks(raw: &str) -> Vec<Vec<&str>> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in raw.split('\n') {
        if is_block_start(strip_cr(line).0) {
            blocks.push(Vec::new());
        }
        if let Some(current) = blocks.last_mut() {
            current.push(line);
        }
        // Anything before the first header is preamble (e.g. `git log` output) and
        // is dropped.
    }
    // `split` yields a trailing empty element when the text ends with a newline.
    if let Some(last) = blocks.last_mut()
        && last.last().is_some_and(|l| l.is_empty())
    {
        last.pop();
    }
    blocks
}

/// Relabel the deletion/addition pair git emits for a type change (e.g. a regular
/// file replaced by a symlink) as [`FileStatus::TypeChanged`].
///
/// Both entries are kept, mirroring git's output. Git orders its output by path,
/// so the pair is always adjacent — checking only neighbours keeps this linear and
/// cannot pair up unrelated files that merely share a name.
fn mark_type_changes(files: &mut [FileDiff]) {
    for i in 1..files.len() {
        let (before, after) = files.split_at_mut(i);
        let (Some(a), Some(b)) = (before.last_mut(), after.first_mut()) else {
            continue;
        };
        if a.path != b.path {
            continue;
        }
        // Whichever order they appear in, we need the deleted side's old mode and
        // the added side's new mode.
        let modes = match (&a.status, &b.status) {
            (FileStatus::Removed, FileStatus::Added) => (a.old_mode, b.new_mode),
            (FileStatus::Added, FileStatus::Removed) => (b.old_mode, a.new_mode),
            _ => continue,
        };
        let (Some(old), Some(new)) = modes else {
            continue;
        };
        // Only a change of *type* qualifies; a permission change is not one.
        if old & FILE_TYPE_MASK != new & FILE_TYPE_MASK {
            a.status = FileStatus::TypeChanged;
            b.status = FileStatus::TypeChanged;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LineKind;

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

    /// The two same-path blocks git emits when a file becomes a symlink.
    const TYPECHANGE: &str = "diff --git a/t.txt b/t.txt\ndeleted file mode 100644\nindex ce01362..0000000\n--- a/t.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-hello\ndiff --git a/t.txt b/t.txt\nnew file mode 120000\nindex 0000000..b443386\n--- /dev/null\n+++ b/t.txt\n@@ -0,0 +1 @@\n+src.txt\n\\ No newline at end of file\n";

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
    fn parse_renamed() -> Result<(), DiffError> {
        let raw = "diff --git a/foo.rs b/bar.rs\nsimilarity index 90%\nrename from foo.rs\nrename to bar.rs\nindex abc..def 100644\n--- a/foo.rs\n+++ b/bar.rs\n@@ -1,3 +1,3 @@\n fn func() {\n-    old();\n+    new();\n }\n";
        let diff = parse(raw)?;
        let f = &diff.files[0];
        assert_eq!(f.path, "bar.rs");
        assert_eq!(f.old_path.as_deref(), Some("foo.rs"));
        assert!(matches!(f.status, FileStatus::Renamed { similarity: 90 }));
        assert_eq!(count(f, LineKind::Add), 1);
        assert_eq!(count(f, LineKind::Remove), 1);
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
    fn typechange_pairs_are_relabelled() -> Result<(), DiffError> {
        let diff = parse(TYPECHANGE)?;
        // Both of git's entries survive, so neither side of the change is lost.
        assert_eq!(diff.files.len(), 2);
        assert!(diff.files.iter().all(|f| f.path == "t.txt"));
        assert!(
            diff.files
                .iter()
                .all(|f| f.status == FileStatus::TypeChanged)
        );
        assert_eq!(diff.files[0].old_mode, Some(0o100644));
        assert_eq!(diff.files[1].new_mode, Some(0o120000));
        // The symlink target has no trailing newline.
        assert!(diff.files[1].hunks[0].lines[0].no_newline);
        Ok(())
    }

    #[test]
    fn a_delete_and_add_of_the_same_type_is_not_a_typechange() -> Result<(), DiffError> {
        // Same path, same file type, differing only in permissions: not a type
        // change, and in practice not something git emits as a pair at all.
        let raw = "diff --git a/x b/x\ndeleted file mode 100644\nindex aaa..0000000\n--- a/x\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\ndiff --git a/x b/x\nnew file mode 100755\nindex 0000000..bbb\n--- /dev/null\n+++ b/x\n@@ -0,0 +1 @@\n+a\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files[0].status, FileStatus::Removed);
        assert_eq!(diff.files[1].status, FileStatus::Added);
        Ok(())
    }

    #[test]
    fn unrelated_neighbours_are_never_paired() -> Result<(), DiffError> {
        let raw = "diff --git a/a.txt b/a.txt\ndeleted file mode 100644\nindex aaa..0000000\n--- a/a.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\ndiff --git a/b.txt b/b.txt\nnew file mode 120000\nindex 0000000..bbb\n--- /dev/null\n+++ b/b.txt\n@@ -0,0 +1 @@\n+a.txt\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files[0].status, FileStatus::Removed);
        assert_eq!(diff.files[1].status, FileStatus::Added);
        Ok(())
    }

    #[test]
    fn multiple_files_each_get_their_own_block() -> Result<(), DiffError> {
        let raw = format!("{SIMPLE_MODIFIED}{MULTI_HUNK}");
        let diff = parse(&raw)?;
        assert_eq!(diff.files.len(), 2);
        assert_eq!(diff.files[0].path, "src/main.rs");
        assert_eq!(diff.files[1].path, "lib.rs");
        Ok(())
    }

    #[test]
    fn diff_headers_inside_content_do_not_split_a_block() -> Result<(), DiffError> {
        // Committing a patch file puts `diff --git` lines *in* the content, where
        // they always carry a `+`/`-`/space prefix.
        let raw = "diff --git a/p.patch b/p.patch\nindex aaa..bbb 100644\n--- a/p.patch\n+++ b/p.patch\n@@ -1,2 +1,2 @@\n-diff --git a/old b/old\n+diff --git a/new b/new\n diff --cc merged\n";
        let diff = parse(raw)?;
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].hunks[0].lines.len(), 3);
        Ok(())
    }

    #[test]
    fn crlf_diff_round_trips_its_line_endings() -> Result<(), DiffError> {
        let raw = "diff --git a/f.txt b/f.txt\r\nindex aaa..bbb 100644\r\n--- a/f.txt\r\n+++ b/f.txt\r\n@@ -1,2 +1,2 @@\r\n alpha\r\n-beta\r\n+GAMMA\r\n";
        let diff = parse(raw)?;
        let lines = &diff.files[0].hunks[0].lines;
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|l| l.crlf));
        assert_eq!(lines[1].content, "beta");
        Ok(())
    }

    /// Real `git diff --cached -M30% -C` output covering every block shape at
    /// once: an addition, a copy, a deletion, a binary file, a plain edit, a file
    /// with no trailing newline, a mode-only change, a type change (two blocks for
    /// one path) and a pure rename.
    const KITCHEN_SINK: &str = r#"diff --git a/added.txt b/added.txt
new file mode 100644
index 0000000..fa49b07
--- /dev/null
+++ b/added.txt
@@ -0,0 +1 @@
+new file
diff --git a/keep.txt b/copied.txt
similarity index 66%
copy from keep.txt
copy to copied.txt
index de98044..7be73ce 100644
--- a/keep.txt
+++ b/copied.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index b77b4eb..0000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-x
-y
diff --git a/img.bin b/img.bin
index 88768ef..885bd7b 100644
Binary files a/img.bin and b/img.bin differ
diff --git a/keep.txt b/keep.txt
index de98044..7be73ce 100644
--- a/keep.txt
+++ b/keep.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
diff --git a/nonl.txt b/nonl.txt
index 3d879e0..1f9360a 100644
--- a/nonl.txt
+++ b/nonl.txt
@@ -1 +1 @@
-no nl
\ No newline at end of file
+no nl CHANGED
\ No newline at end of file
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/tgt.txt b/tgt.txt
deleted file mode 100644
index 421174b..0000000
--- a/tgt.txt
+++ /dev/null
@@ -1 +0,0 @@
-sym target
diff --git a/tgt.txt b/tgt.txt
new file mode 120000
index 0000000..1764325
--- /dev/null
+++ b/tgt.txt
@@ -0,0 +1 @@
+keep.txt
\ No newline at end of file
diff --git a/from.txt b/to.txt
similarity index 100%
rename from from.txt
rename to to.txt
"#;

    #[test]
    fn every_block_of_a_real_diff_becomes_a_file() -> Result<(), DiffError> {
        let diff = parse(KITCHEN_SINK)?;
        // One entry per `diff --git` block, including both halves of the type
        // change. Nothing git reported may be dropped.
        let seen: Vec<(&str, &FileStatus)> = diff
            .files
            .iter()
            .map(|f| (f.path.as_str(), &f.status))
            .collect();
        assert_eq!(
            seen,
            [
                ("added.txt", &FileStatus::Added),
                ("copied.txt", &FileStatus::Copied { similarity: 66 }),
                ("gone.txt", &FileStatus::Removed),
                ("img.bin", &FileStatus::Modified),
                ("keep.txt", &FileStatus::Modified),
                ("nonl.txt", &FileStatus::Modified),
                ("run.sh", &FileStatus::Modified),
                ("tgt.txt", &FileStatus::TypeChanged),
                ("tgt.txt", &FileStatus::TypeChanged),
                ("to.txt", &FileStatus::Renamed { similarity: 100 }),
            ]
        );
        Ok(())
    }

    #[test]
    fn real_diff_carries_modes_paths_and_terminators() -> Result<(), DiffError> {
        let diff = parse(KITCHEN_SINK)?;
        let by =
            |p: &str| -> Vec<&FileDiff> { diff.files.iter().filter(|f| f.path == p).collect() };

        // A chmod with no content change: present, with both modes, and no hunks.
        let run = by("run.sh")[0];
        assert!(run.mode_changed());
        assert_eq!(
            (run.old_mode, run.new_mode),
            (Some(0o100644), Some(0o100755))
        );
        assert!(run.hunks.is_empty());

        // The copy keeps its source path; the rename keeps its own.
        assert_eq!(by("copied.txt")[0].old_path.as_deref(), Some("keep.txt"));
        assert_eq!(by("to.txt")[0].old_path.as_deref(), Some("from.txt"));

        // Binary content is flagged, not parsed as lines.
        assert!(by("img.bin")[0].is_binary);
        assert!(by("img.bin")[0].hunks.is_empty());

        // Both sides of nonl.txt lack a trailing newline.
        let nonl = &by("nonl.txt")[0].hunks[0].lines;
        assert!(nonl.iter().all(|l| l.no_newline));

        // The type change spans a regular file and a symlink.
        let tgt = by("tgt.txt");
        assert_eq!(tgt[0].old_mode, Some(0o100644));
        assert_eq!(tgt[1].new_mode, Some(0o120000));
        Ok(())
    }

    #[test]
    fn preamble_before_the_first_header_is_dropped() -> Result<(), DiffError> {
        let raw = format!("commit abc123\nAuthor: T <t@example.com>\n\n{SIMPLE_MODIFIED}");
        let diff = parse(&raw)?;
        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, "src/main.rs");
        Ok(())
    }
}

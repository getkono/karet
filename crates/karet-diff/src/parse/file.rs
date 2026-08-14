//! Parsing one file's block of a unified diff: its identity, status, modes, and
//! the point at which its hunks begin.

use crate::DiffError;
use crate::model::FileDiff;
use crate::model::FileStatus;
use crate::parse::hunks::parse_hunks;
use crate::parse::path::decode_path;
use crate::parse::path::split_ab_paths;
use crate::parse::scope::populate_new_scopes;
use crate::parse::strip_cr;

/// The header that opens a combined (multi-parent) diff, as produced by `--cc`.
const COMBINED_CC: &str = "diff --cc ";
/// The header that opens a combined diff produced by `-c`.
const COMBINED_DASH_C: &str = "diff --combined ";
/// The header that opens an ordinary two-sided diff.
const GIT_HEADER: &str = "diff --git ";

/// Whether `line` opens a new file block. Content lines always carry a `+`, `-`
/// or space prefix, so they can never be mistaken for one.
pub(super) fn is_block_start(line: &str) -> bool {
    line.starts_with(GIT_HEADER)
        || line.starts_with(COMBINED_CC)
        || line.starts_with(COMBINED_DASH_C)
}

/// The metadata accumulated while scanning a block's header lines.
#[derive(Default)]
struct Header {
    old_path: Option<String>,
    new_path: Option<String>,
    status: Option<FileStatus>,
    old_mode: Option<u32>,
    new_mode: Option<u32>,
    similarity: Option<u8>,
    dissimilarity: Option<u8>,
    is_binary: bool,
}

/// Parse a single `diff --git`/`diff --cc` block. Returns `None` when the block
/// does not open with a header we recognize.
pub(super) fn parse_file_block(lines: &[&str]) -> Result<Option<FileDiff>, DiffError> {
    let Some(first) = lines.first().map(|l| strip_cr(l).0) else {
        return Ok(None);
    };

    // A combined diff has one path and no two-sided content. Represent the file so
    // it is never silently lost, but do not decode the N-parent `@@@` format.
    if let Some(path) = first
        .strip_prefix(COMBINED_CC)
        .or_else(|| first.strip_prefix(COMBINED_DASH_C))
    {
        return Ok(Some(FileDiff {
            path: decode_path(path),
            old_path: None,
            status: FileStatus::Unmerged,
            old_mode: None,
            new_mode: None,
            is_binary: false,
            hunks: Vec::new(),
        }));
    }

    let Some(rest) = first.strip_prefix(GIT_HEADER) else {
        return Ok(None);
    };
    let (_header_old, header_new) = split_ab_paths(rest);

    let mut h = Header::default();
    let hunk_start_idx = scan_header(lines, &mut h);

    let status = h.status.unwrap_or(match h.dissimilarity {
        Some(dissimilarity) => FileStatus::Rewritten { dissimilarity },
        None => FileStatus::Modified,
    });
    let hunks = if h.is_binary {
        Vec::new()
    } else {
        parse_hunks(&lines[hunk_start_idx.min(lines.len())..])?
    };

    let mut fd = FileDiff {
        path: h.new_path.unwrap_or(header_new),
        old_path: h.old_path,
        status,
        old_mode: h.old_mode,
        new_mode: h.new_mode,
        is_binary: h.is_binary,
        hunks,
    };
    populate_new_scopes(&mut fd);
    Ok(Some(fd))
}

/// Read the block's header lines into `h`, returning the index at which hunks
/// begin (or `lines.len()` when the block has none).
fn scan_header(lines: &[&str], h: &mut Header) -> usize {
    for (idx, raw) in lines.iter().enumerate().skip(1) {
        let (line, _) = strip_cr(raw);

        if let Some(pct) = line.strip_prefix("similarity index ") {
            h.similarity = parse_percent(pct);
        } else if let Some(pct) = line.strip_prefix("dissimilarity index ") {
            h.dissimilarity = parse_percent(pct);
        } else if let Some(p) = line.strip_prefix("rename from ") {
            h.old_path = Some(decode_path(p));
        } else if let Some(p) = line.strip_prefix("rename to ") {
            h.new_path = Some(decode_path(p));
            h.status = Some(FileStatus::Renamed {
                similarity: h.similarity.unwrap_or(100),
            });
        } else if let Some(p) = line.strip_prefix("copy from ") {
            h.old_path = Some(decode_path(p));
        } else if let Some(p) = line.strip_prefix("copy to ") {
            h.new_path = Some(decode_path(p));
            h.status = Some(FileStatus::Copied {
                similarity: h.similarity.unwrap_or(100),
            });
        } else if let Some(m) = line.strip_prefix("new file mode ") {
            h.status = Some(FileStatus::Added);
            h.new_mode = parse_mode(m);
        } else if let Some(m) = line.strip_prefix("deleted file mode ") {
            h.status = Some(FileStatus::Removed);
            h.old_mode = parse_mode(m);
        } else if let Some(m) = line.strip_prefix("old mode ") {
            h.old_mode = parse_mode(m);
        } else if let Some(m) = line.strip_prefix("new mode ") {
            h.new_mode = parse_mode(m);
        } else if let Some(rest) = line.strip_prefix("index ") {
            record_index_mode(rest, h);
        } else if line.starts_with("Binary files") || line == "GIT binary patch" {
            // `Binary files … differ` is the default; `GIT binary patch` is what
            // `--binary` emits, followed by a base85 payload we deliberately skip.
            h.is_binary = true;
        } else if line.starts_with("@@ ") {
            return idx;
        }
        // `--- ` and `+++ ` carry no information we don't already have.
    }
    lines.len()
}

/// Read the trailing mode from an `index <old>..<new> <mode>` line.
///
/// Git omits that mode exactly when the mode changed (it wrote `old mode` /
/// `new mode` lines instead), so this only ever fills in an unchanged mode and
/// never overwrites one already recorded.
fn record_index_mode(rest: &str, h: &mut Header) {
    let Some(mode) = rest.split_whitespace().nth(1).and_then(parse_mode) else {
        return;
    };
    h.old_mode.get_or_insert(mode);
    h.new_mode.get_or_insert(mode);
}

/// Parse a `NN%` similarity/dissimilarity value.
fn parse_percent(s: &str) -> Option<u8> {
    s.trim().trim_end_matches('%').parse::<u8>().ok()
}

/// Parse an octal file mode such as `100644`.
fn parse_mode(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim(), 8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a single block, erroring if it does not yield a file.
    fn parse_block(block: &str) -> Result<FileDiff, DiffError> {
        let lines: Vec<&str> = block.split('\n').collect();
        parse_file_block(&lines)?.ok_or_else(|| DiffError::Parse("block not recognized".into()))
    }

    #[test]
    fn mode_only_change_keeps_the_file_and_its_modes() -> Result<(), DiffError> {
        let f = parse_block("diff --git a/s.sh b/s.sh\nold mode 100644\nnew mode 100755\n")?;
        assert_eq!(f.path, "s.sh");
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100755)));
        assert!(f.mode_changed());
        assert!(f.hunks.is_empty());
        Ok(())
    }

    #[test]
    fn mode_and_content_change_together() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/s.sh b/s.sh\nold mode 100644\nnew mode 100755\nindex 8b2fe54..4935e13\n--- a/s.sh\n+++ b/s.sh\n@@ -1 +1,2 @@\n echo hi\n+echo more\n",
        )?;
        assert!(f.mode_changed());
        assert_eq!(f.hunks.len(), 1);
        // The index line carries no mode here, so it must not clobber either side.
        assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100755)));
        Ok(())
    }

    #[test]
    fn unchanged_mode_comes_from_the_index_line() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/x.rs b/x.rs\nindex abc1234..def5678 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-a\n+b\n",
        )?;
        assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100644)));
        assert!(!f.mode_changed());
        Ok(())
    }

    #[test]
    fn added_and_deleted_know_only_one_side_mode() -> Result<(), DiffError> {
        let added = parse_block(
            "diff --git a/new.rs b/new.rs\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+hi\n",
        )?;
        assert_eq!(added.status, FileStatus::Added);
        assert_eq!((added.old_mode, added.new_mode), (None, Some(0o100644)));
        assert!(!added.mode_changed());

        let deleted = parse_block(
            "diff --git a/old.rs b/old.rs\ndeleted file mode 100755\nindex 1111111..0000000\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-hi\n",
        )?;
        assert_eq!(deleted.status, FileStatus::Removed);
        assert_eq!((deleted.old_mode, deleted.new_mode), (Some(0o100755), None));
        Ok(())
    }

    #[test]
    fn copy_is_distinguished_from_a_rename() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/src.txt b/copy.txt\nsimilarity index 100%\ncopy from src.txt\ncopy to copy.txt\n",
        )?;
        assert_eq!(f.path, "copy.txt");
        assert_eq!(f.old_path.as_deref(), Some("src.txt"));
        assert_eq!(f.status, FileStatus::Copied { similarity: 100 });
        Ok(())
    }

    #[test]
    fn copy_with_edits_keeps_its_similarity() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/a.txt b/b.txt\nsimilarity index 82%\ncopy from a.txt\ncopy to b.txt\nindex aaa..bbb 100644\n--- a/a.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-x\n+y\n",
        )?;
        assert_eq!(f.status, FileStatus::Copied { similarity: 82 });
        assert_eq!(f.hunks.len(), 1);
        Ok(())
    }

    #[test]
    fn git_binary_patch_is_binary_and_drops_its_payload() -> Result<(), DiffError> {
        // Without this, the base85 payload parses as context lines.
        let f = parse_block(
            "diff --git a/b.bin b/b.bin\nindex 2f68929..6c714bb 100644\nGIT binary patch\nliteral 9\nQcmZSJ%rD7EEn;E@01FEOF#rGn\n\nliteral 9\nQcmZQzOv=nlEUIJz01GJsi2wiq\n\n",
        )?;
        assert!(f.is_binary);
        assert!(f.hunks.is_empty());
        Ok(())
    }

    #[test]
    fn binary_files_differ_is_binary() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/b.bin b/b.bin\nindex 2f68929..6c714bb 100644\nBinary files a/b.bin and b/b.bin differ\n",
        )?;
        assert!(f.is_binary);
        Ok(())
    }

    #[test]
    fn combined_diff_yields_an_unmerged_file() -> Result<(), DiffError> {
        // The whole file used to vanish, because only `diff --git ` was recognized.
        for header in ["diff --cc src.txt", "diff --combined src.txt"] {
            let f = parse_block(&format!(
                "{header}\nindex e5cdc04,713f333..0000000\n--- a/src.txt\n+++ b/src.txt\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< HEAD\n +MAIN\n++=======\n+ SIDE\n++>>>>>>> side\n"
            ))?;
            assert_eq!(f.path, "src.txt");
            assert_eq!(f.status, FileStatus::Unmerged);
            assert!(f.hunks.is_empty(), "combined hunks are not decoded");
        }
        Ok(())
    }

    #[test]
    fn dissimilarity_index_marks_a_rewrite() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/x.txt b/x.txt\ndissimilarity index 87%\nindex aaa..bbb 100644\n--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-a\n+z\n",
        )?;
        assert_eq!(f.status, FileStatus::Rewritten { dissimilarity: 87 });
        Ok(())
    }

    #[test]
    fn empty_file_addition_has_no_hunks() -> Result<(), DiffError> {
        let f = parse_block(
            "diff --git a/empty.txt b/empty.txt\nnew file mode 100644\nindex 0000000..e69de29\n",
        )?;
        assert_eq!(f.status, FileStatus::Added);
        assert!(f.is_content_unchanged());
        Ok(())
    }

    #[test]
    fn unrecognized_blocks_are_skipped() -> Result<(), DiffError> {
        let lines = ["--- a/x", "+++ b/x"];
        assert!(parse_file_block(&lines)?.is_none());
        assert!(parse_file_block(&[])?.is_none());
        Ok(())
    }

    #[test]
    fn block_starts_are_recognized() {
        assert!(is_block_start("diff --git a/x b/x"));
        assert!(is_block_start("diff --cc x"));
        assert!(is_block_start("diff --combined x"));
        // Content lines always carry a prefix, so they never look like a header.
        assert!(!is_block_start("+diff --git a/x b/x"));
        assert!(!is_block_start(" diff --git a/x b/x"));
        assert!(!is_block_start("-diff --cc x"));
    }
}

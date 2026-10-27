//! End-to-end blame against a real repository built with the `git` CLI.
//!
//! The library itself is pure-Rust (gix); we only use the `git` binary to *create*
//! the fixture. The test skips silently when `git` isn't available.

use std::error::Error;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// The author date every fixture commit carries, as git reads it.
const AUTHOR_DATE: &str = "2026-03-16T12:34:56+02:00";

/// Whether the `git` binary is available to build fixtures with.
fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// Run a git subcommand in `dir` with a fixed identity (no global/system config).
fn git(dir: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test One")
        .env("GIT_AUTHOR_EMAIL", "one@example.com")
        .env("GIT_COMMITTER_NAME", "Test One")
        .env("GIT_COMMITTER_EMAIL", "one@example.com")
        .env("GIT_AUTHOR_DATE", AUTHOR_DATE)
        .env("GIT_COMMITTER_DATE", AUTHOR_DATE)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()?
        .success();
    if ok {
        Ok(())
    } else {
        Err(format!("git {args:?} failed").into())
    }
}

#[test]
fn blames_two_commits_into_ordered_groups() -> TestResult {
    // Skip when git isn't installed (keeps pure environments green).
    if Command::new("git").arg("--version").output().is_err() {
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let file = root.join("file.txt");

    git(root, &["init", "-q", "-b", "main"])?;
    std::fs::write(&file, "line one\nline two\nline three\n")?;
    git(root, &["add", "file.txt"])?;
    git(
        root,
        &["commit", "-q", "-m", "first", "-m", "why it was written"],
    )?;

    // Second commit changes line 2 and appends line 4; lines 1 & 3 stay from `first`.
    std::fs::write(&file, "line one\nCHANGED two\nline three\nline four\n")?;
    git(root, &["commit", "-q", "-am", "second"])?;

    let groups = blameline::blame_file(root, Path::new("file.txt"))?;

    // Alternating attribution → four single-line groups covering lines 1..=4 in order.
    let covered: Vec<(u32, u32)> = groups
        .iter()
        .map(|g| (g.lines.start, g.lines.end))
        .collect();
    assert_eq!(covered, vec![(1, 1), (2, 2), (3, 3), (4, 4)]);

    // Exactly two distinct commits are involved.
    let distinct: std::collections::HashSet<&str> =
        groups.iter().map(|g| g.commit_hash.as_str()).collect();
    assert_eq!(distinct.len(), 2);

    // Full messages (subject, blank line, body; trailing newline trimmed),
    // author, and the ISO-8601 author date in the author's own offset.
    assert_eq!(groups[0].message, "first\n\nwhy it was written");
    assert_eq!(groups[0].summary(), "first");
    assert_eq!(groups[1].message, "second");
    assert_eq!(groups[2].message, groups[0].message);
    assert_eq!(groups[0].author, "Test One");
    assert_eq!(groups[0].date, AUTHOR_DATE);
    assert_eq!(groups[0].author_time, 1_773_657_296);

    // Function-scoped blame on a non-source file falls back to whole-file blame.
    let scoped = blameline::blame_function(root, Path::new("file.txt"), "x\n", 1)?;
    assert_eq!(scoped, groups);

    // Called the way the karet app does — repo_root is the file's *parent* and the
    // file is passed as an absolute path — yields identical groups (no path doubling).
    let karet_style = blameline::blame_file(file.parent().unwrap_or(root), &file)?;
    assert_eq!(karet_style, groups);

    Ok(())
}

#[test]
fn uncommitted_file_reports_not_committed() -> TestResult {
    // Skip when git isn't installed (keeps pure environments green).
    if Command::new("git").arg("--version").output().is_err() {
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let root = dir.path();

    // A repo needs at least one commit so HEAD resolves; the file under test is a
    // *different*, staged-but-uncommitted file with no history in HEAD.
    git(root, &["init", "-q", "-b", "main"])?;
    std::fs::write(root.join("committed.txt"), "seed\n")?;
    git(root, &["add", "committed.txt"])?;
    git(root, &["commit", "-q", "-m", "seed"])?;

    std::fs::write(root.join("fresh.txt"), "brand new\n")?;
    git(root, &["add", "fresh.txt"])?;

    let result = blameline::blame_file(root, Path::new("fresh.txt"));
    assert!(
        matches!(result, Err(blameline::BlameError::NotCommitted(_))),
        "expected NotCommitted, got {result:?}"
    );
    // The error names the path relative to the worktree, as it was asked for.
    assert_eq!(
        result.err().map(|e| e.to_string()).as_deref(),
        Some("no committed history for fresh.txt yet")
    );

    Ok(())
}

/// A directory no repository contains, and a bare repository with no
/// worktree to blame in, both report `NotARepository` rather than a generic
/// git error. Falsified by `map_discover` collapsing every discovery failure
/// into `Git`, or by dropping the bare-repository `workdir` check.
#[test]
fn no_repository_and_a_bare_repository_report_not_a_repository() -> TestResult {
    if !have_git() {
        return Ok(());
    }

    let plain = tempfile::tempdir()?;
    // Discovery walks upwards, so this only holds where the scratch dir is
    // not itself inside some repository.
    let inside_a_repo = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(plain.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()?
        .status
        .success();
    if !inside_a_repo {
        let result = blameline::blame_file(plain.path(), Path::new("file.txt"));
        assert!(
            matches!(result, Err(blameline::BlameError::NotARepository)),
            "expected NotARepository, got {result:?}"
        );
    }

    let bare = tempfile::tempdir()?;
    git(bare.path(), &["init", "-q", "--bare", "-b", "main"])?;
    let result = blameline::blame_file(bare.path(), Path::new("file.txt"));
    assert!(
        matches!(result, Err(blameline::BlameError::NotARepository)),
        "expected NotARepository, got {result:?}"
    );
    Ok(())
}

/// A file that exists but lives outside the repository's worktree is refused
/// with a `Git` error naming it, instead of being handed to gix as a bogus
/// path. Falsified by blaming the unstripped absolute path.
#[test]
fn a_file_outside_the_worktree_is_refused() -> TestResult {
    if !have_git() {
        return Ok(());
    }

    let repo = tempfile::tempdir()?;
    git(repo.path(), &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.path().join("seed.txt"), "seed\n")?;
    git(repo.path(), &["add", "seed.txt"])?;
    git(repo.path(), &["commit", "-q", "-m", "seed"])?;

    let elsewhere = tempfile::tempdir()?;
    let outside = elsewhere.path().join("stray.txt");
    std::fs::write(&outside, "stray\n")?;

    match blameline::blame_file(repo.path(), &outside) {
        Err(blameline::BlameError::Git(message)) => {
            let canonical = std::fs::canonicalize(&outside)?;
            assert_eq!(
                message,
                format!("{} is outside the repository worktree", canonical.display())
            );
        },
        other => {
            return Err(format!("expected an outside-worktree Git error, got {other:?}").into());
        },
    }
    Ok(())
}

/// `blame_function` end to end: blame is clipped to the function enclosing
/// the line, so only the commits that touched that function appear, with
/// their groups cut at its boundaries. Falsified by `blame_function` ignoring
/// the tree-sitter range (it would return all five lines and both commits).
#[cfg(feature = "lang-rust")]
#[test]
fn blame_function_narrows_to_the_enclosing_function() -> TestResult {
    if !have_git() {
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let file = root.join("lib.rs");
    git(root, &["init", "-q", "-b", "main"])?;
    std::fs::write(&file, "fn a() {\n}\n\nfn b() {\n}\n")?;
    git(root, &["add", "lib.rs"])?;
    git(root, &["commit", "-q", "-m", "first"])?;
    // Only `b` changes: its body gains a line.
    let source = "fn a() {\n}\n\nfn b() {\n    1;\n}\n";
    std::fs::write(&file, source)?;
    git(root, &["commit", "-q", "-am", "second"])?;

    let whole = blameline::blame_file(root, Path::new("lib.rs"))?;
    assert_eq!(whole.len(), 3, "{whole:?}");

    // Line 5 (`1;`) is inside `b`, lines 4..=6.
    let scoped = blameline::blame_function(root, Path::new("lib.rs"), source, 5)?;
    let shape: Vec<(u32, u32, &str)> = scoped
        .iter()
        .map(|g| (g.lines.start, g.lines.end, g.summary()))
        .collect();
    assert_eq!(
        shape,
        vec![(4, 4, "first"), (5, 5, "second"), (6, 6, "first")]
    );

    // Inside `a`, which only `first` touched.
    let in_a = blameline::blame_function(root, Path::new("lib.rs"), source, 1)?;
    let shape: Vec<(u32, u32, &str)> = in_a
        .iter()
        .map(|g| (g.lines.start, g.lines.end, g.summary()))
        .collect();
    assert_eq!(shape, vec![(1, 2, "first")]);

    // Between the functions there is nothing to narrow to: whole-file blame.
    assert_eq!(
        blameline::blame_function(root, Path::new("lib.rs"), source, 3)?,
        whole
    );
    // Nor at line 0, which 1-based numbering does not have.
    assert_eq!(
        blameline::blame_function(root, Path::new("lib.rs"), source, 0)?,
        whole
    );
    Ok(())
}

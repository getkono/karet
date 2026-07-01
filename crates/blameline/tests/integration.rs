//! End-to-end blame against a real repository built with the `git` CLI.
//!
//! The library itself is pure-Rust (gix); we only use the `git` binary to *create*
//! the fixture. The test skips silently when `git` isn't available.

use std::error::Error;
use std::path::Path;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

/// Run a git subcommand in `dir` with a fixed identity (no global/system config).
fn git(dir: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test One")
        .env("GIT_AUTHOR_EMAIL", "one@example.com")
        .env("GIT_COMMITTER_NAME", "Test One")
        .env("GIT_COMMITTER_EMAIL", "one@example.com")
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
    git(root, &["commit", "-q", "-m", "first"])?;

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

    // Full messages + author are resolved.
    assert_eq!(groups[0].summary(), "first");
    assert_eq!(groups[1].summary(), "second");
    assert_eq!(groups[2].summary(), "first");
    assert_eq!(groups[0].author, "Test One");
    assert!(!groups[0].date.is_empty());

    // Function-scoped blame on a non-source file falls back to whole-file blame.
    let scoped = blameline::blame_function(root, Path::new("file.txt"), "x\n", 0)?;
    assert_eq!(scoped.len(), groups.len());

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

    Ok(())
}

//! Job-level tests for the VCS worker: each job is dispatched through [`run`]
//! against a scratch repository, and the events it answers with are asserted.
//!
//! Kept beside `vcs_worker.rs` rather than in it so that file stays under the
//! code-line ceiling.

use super::*;
use crate::cancellation::CancellationHub;

type Events = tokio::sync::mpsc::UnboundedReceiver<(Option<RequestId>, Event)>;

/// A missing or wrong answer is an `Err`, so a test fails instead of passing.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A scratch repository with `commits` commits of `a.txt` (the n-th writes
/// `"n\n"`). `Ok(None)` only when `git` is unavailable: any fixture failure past
/// that is an error, so a test cannot pass on a repository it never built.
fn repo_with_commits(
    commits: usize,
) -> Result<Option<(tempfile::TempDir, PathBuf)>, Box<dyn std::error::Error>> {
    let git_on_path = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !git_on_path {
        return Ok(None);
    }
    let dir = tempfile::tempdir()?;
    let root = std::fs::canonicalize(dir.path())?;
    git(&root, &["init", "-q"])?;
    for n in 1..=commits {
        std::fs::write(root.join("a.txt"), format!("{n}\n"))?;
        git(&root, &["add", "a.txt"])?;
        git(&root, &["commit", "-q", "-m", &format!("commit {n}")])?;
    }
    Ok(Some((dir, root)))
}

/// Run `git` in `root` with a fixed identity and no signing, failing on a
/// non-zero exit.
fn git(root: &Path, args: &[&str]) -> TestResult {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=karet test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed: {status}").into())
    }
}

/// Dispatch one job through the worker's `run` and collect what it emitted.
fn dispatch(root: Option<PathBuf>, job: VcsJob) -> Vec<(Option<RequestId>, Event)> {
    let (tx, mut rx): (_, Events) = tokio::sync::mpsc::unbounded_channel();
    run(&root, &tx, &mut BlameCache::new(), &mut None, job);
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

fn log_page(events: &[(Option<RequestId>, Event)]) -> Option<(usize, Vec<String>, bool)> {
    events.iter().find_map(|(_, event)| match event {
        Event::VcsLog {
            skip,
            commits,
            has_more,
            ..
        } => Some((
            *skip,
            commits
                .iter()
                .map(|commit| commit.summary.clone())
                .collect(),
            *has_more,
        )),
        _ => None,
    })
}

#[test]
fn log_pages_through_history_and_reports_whether_more_remains() -> TestResult {
    let Some((_dir, root)) = repo_with_commits(3)? else {
        // Skip: `git` is not on PATH.
        return Ok(());
    };
    let hub = CancellationHub::default();
    let first = dispatch(
        Some(root.clone()),
        VcsJob::Log {
            id: RequestId(1),
            skip: 0,
            limit: 2,
            cancel: hub.register(RequestId(1)),
        },
    );
    assert_eq!(first.first().map(|(id, _)| *id), Some(Some(RequestId(1))));
    assert_eq!(
        log_page(&first),
        Some((0, vec!["commit 3".to_owned(), "commit 2".to_owned()], true)),
        "newest first, one page, more beyond it"
    );
    let last = dispatch(
        Some(root.clone()),
        VcsJob::Log {
            id: RequestId(2),
            skip: 2,
            limit: 2,
            cancel: hub.register(RequestId(2)),
        },
    );
    assert_eq!(
        log_page(&last),
        Some((2, vec!["commit 1".to_owned()], false))
    );
    // A page that ends exactly at the root commit has nothing beyond it.
    let exact = dispatch(
        Some(root),
        VcsJob::Log {
            id: RequestId(3),
            skip: 1,
            limit: 2,
            cancel: hub.register(RequestId(3)),
        },
    );
    assert_eq!(
        log_page(&exact),
        Some((1, vec!["commit 2".to_owned(), "commit 1".to_owned()], false))
    );
    Ok(())
}

#[test]
fn a_log_without_a_repository_notifies_and_a_cancelled_one_stays_silent() {
    let hub = CancellationHub::default();
    let failed = dispatch(
        None,
        VcsJob::Log {
            id: RequestId(1),
            skip: 0,
            limit: 10,
            cancel: hub.register(RequestId(1)),
        },
    );
    assert!(
        matches!(
            failed.as_slice(),
            [(Some(RequestId(1)), Event::Notification { .. })]
        ),
        "{failed:?}"
    );

    let cancel = hub.register(RequestId(2));
    hub.cancel(RequestId(2));
    let cancelled = dispatch(
        None,
        VcsJob::Log {
            id: RequestId(2),
            skip: 0,
            limit: 10,
            cancel,
        },
    );
    assert!(
        cancelled.is_empty(),
        "a cancelled request owes no answer: {cancelled:?}"
    );
}

/// The prepared diff a `DiffWithRev` answered with.
fn diff_with_rev(
    path: &Path,
    rev: &str,
    live: Option<&str>,
) -> Option<Result<(bool, usize), String>> {
    let hub = CancellationHub::default();
    let events = dispatch(
        None,
        VcsJob::DiffWithRev {
            id: RequestId(1),
            path: path.to_path_buf(),
            rev: rev.to_owned(),
            live: live.map(str::to_owned),
            syntax: false,
            cancel: hub.register(RequestId(1)),
        },
    );
    events.into_iter().find_map(|(_, event)| match event {
        Event::DiffPrepared { result, .. } => {
            Some(result.map(|change| (change.diff.diff.is_binary, change.diff.diff.hunks.len())))
        },
        _ => None,
    })
}

#[test]
fn diff_with_rev_compares_the_revision_with_the_live_buffer_or_the_worktree() -> TestResult {
    let Some((_dir, root)) = repo_with_commits(1)? else {
        // Skip: `git` is not on PATH.
        return Ok(());
    };
    let path = root.join("a.txt");
    std::fs::write(&path, "changed\n")?;
    assert_eq!(
        diff_with_rev(&path, "HEAD", None),
        Some(Ok((false, 1))),
        "the worktree file differs from HEAD"
    );
    assert_eq!(
        diff_with_rev(&path, "HEAD", Some("1\n")),
        Some(Ok((false, 0))),
        "unsaved live text wins over the worktree file"
    );
    assert!(
        matches!(
            diff_with_rev(&root.join("new.txt"), "HEAD", Some("x\n")),
            Some(Err(_))
        ),
        "a file absent at the revision is an error, not an all-added diff"
    );
    std::fs::write(&path, [0xff, 0xfe, 0x00])?;
    assert_eq!(
        diff_with_rev(&path, "HEAD", None),
        Some(Ok((true, 0))),
        "a non-text current side marks the change binary"
    );
    Ok(())
}

#[test]
fn remote_facts_report_why_they_are_unavailable() -> TestResult {
    let outside = tempfile::tempdir()?;
    assert_eq!(
        remote_facts(&outside.path().join("a.txt")).err().as_deref(),
        Some("not in a git repository")
    );
    let Some((_dir, root)) = repo_with_commits(1)? else {
        // Skip: `git` is not on PATH.
        return Ok(());
    };
    assert_eq!(
        remote_facts(&root.join("a.txt")).err().as_deref(),
        Some("no origin remote configured")
    );
    Ok(())
}

#[test]
fn remote_facts_describe_a_tracked_and_an_untracked_file() -> TestResult {
    let Some((_dir, root)) = repo_with_commits(1)? else {
        // Skip: `git` is not on PATH.
        return Ok(());
    };
    git(
        &root,
        &["remote", "add", "origin", "https://example.com/o/r.git"],
    )?;
    std::fs::create_dir_all(root.join("sub"))?;
    std::fs::write(root.join("sub/new.txt"), "x\n")?;
    let tracked = remote_facts(&root.join("a.txt"));
    assert!(
        tracked
            .as_ref()
            .is_ok_and(|facts| facts.origin_url == "https://example.com/o/r.git"
                && facts.rel_path == Path::new("a.txt")
                && facts.tracked
                && facts.head.is_some()
                && facts.branch.is_some()),
        "{tracked:?}"
    );
    let untracked = remote_facts(&root.join("sub/new.txt"));
    assert!(
        untracked
            .as_ref()
            .is_ok_and(|facts| facts.rel_path == Path::new("sub/new.txt") && !facts.tracked),
        "{untracked:?}"
    );

    // The job answers through the shared stream, unless it was cancelled.
    let hub = CancellationHub::default();
    let answered = dispatch(
        None,
        VcsJob::RemoteFacts {
            id: RequestId(1),
            path: root.join("a.txt"),
            cancel: hub.register(RequestId(1)),
        },
    );
    assert!(
        matches!(
            answered.as_slice(),
            [(Some(RequestId(1)), Event::RemoteFacts { facts: Ok(_), .. })]
        ),
        "{answered:?}"
    );
    let cancel = hub.register(RequestId(2));
    hub.cancel(RequestId(2));
    let cancelled = dispatch(
        None,
        VcsJob::RemoteFacts {
            id: RequestId(2),
            path: root.join("a.txt"),
            cancel,
        },
    );
    assert!(cancelled.is_empty(), "{cancelled:?}");
    Ok(())
}

#[test]
fn prepare_changes_gives_up_once_cancelled() -> TestResult {
    let hub = CancellationHub::default();
    let cancel = hub.register(RequestId(1));
    assert_eq!(
        prepare_changes(Vec::new(), false, &cancel).map(|c| c.len()),
        Some(0)
    );
    hub.cancel(RequestId(1));
    let Some((_dir, root)) = repo_with_commits(1)? else {
        // Skip: `git` is not on PATH.
        return Ok(());
    };
    std::fs::write(root.join("a.txt"), "changed\n")?;
    let changes = repository(&Some(root)).and_then(|repo| {
        repo.changes(Selection::Unstaged, None)
            .map_err(|e| e.to_string())
    })?;
    assert_eq!(changes.len(), 1);
    assert!(
        prepare_changes(changes, false, &cancel).is_none(),
        "a cancelled request prepares nothing"
    );
    Ok(())
}

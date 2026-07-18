use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use crate::Repository;
use crate::Selection;
use crate::StatusKind;
use crate::VcsError;

static COUNTER: AtomicU32 = AtomicU32::new(0);
/// Serializes the tests that mutate the process-wide current directory.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Restores the working directory on drop, so a panic can't leak the change.
struct CwdGuard(PathBuf);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// A temp directory removed on drop.
struct TempRepo {
    path: PathBuf,
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Allocate a unique, not-yet-created temp directory path.
fn unique_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("karet-vcs-{}-{}", std::process::id(), n))
}

/// Run `git` in `dir`, surfacing any failure as a [`VcsError`].
fn git(dir: &Path, args: &[&str]) -> Result<(), VcsError> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| VcsError::Git(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(VcsError::Git(format!("git {args:?} exited with {status}")))
    }
}

/// Create an initialized repository in a fresh temp directory.
fn init_repo() -> Result<TempRepo, VcsError> {
    let path = unique_dir();
    std::fs::create_dir_all(&path).map_err(|e| VcsError::Git(e.to_string()))?;
    let repo = TempRepo { path };
    git(&repo.path, &["init", "-q"])?;
    git(&repo.path, &["config", "user.email", "test@example.com"])?;
    git(&repo.path, &["config", "user.name", "karet test"])?;
    git(&repo.path, &["config", "commit.gpgsign", "false"])?;
    Ok(repo)
}

/// Write `contents` to `rel` inside `dir`.
fn write(dir: &Path, rel: &str, contents: &[u8]) -> Result<(), VcsError> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| VcsError::Git(e.to_string()))?;
    }
    std::fs::write(&path, contents).map_err(|e| VcsError::Git(e.to_string()))
}

#[test]
fn discover_non_repo_then_repo() -> Result<(), VcsError> {
    let dir = unique_dir();
    std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
    let guard = TempRepo { path: dir.clone() };

    assert!(matches!(
        Repository::discover(&dir),
        Err(VcsError::NotARepository)
    ));

    git(&dir, &["init", "-q"])?;
    assert!(Repository::discover(&dir).is_ok());

    drop(guard);
    Ok(())
}

#[test]
fn untracked_file_is_unstaged() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "new.txt", b"hello\n")?;
    let r = Repository::discover(&repo.path)?;

    assert_eq!(r.default_selection()?, Some(Selection::Unstaged));

    let changes = r.changes(Selection::Unstaged, None)?;
    assert_eq!(changes.len(), 1);
    let fc = &changes[0];
    assert_eq!(fc.path, PathBuf::from("new.txt"));
    assert_eq!(fc.status, StatusKind::Untracked);
    assert!(!fc.is_binary);
    assert_eq!(fc.old, "");
    assert_eq!(fc.new, "hello\n");
    Ok(())
}

#[test]
fn staged_modification_has_old_and_new() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"one\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    write(&repo.path, "a.txt", b"two\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    let r = Repository::discover(&repo.path)?;

    assert_eq!(r.default_selection()?, Some(Selection::Staged));

    let changes = r.changes(Selection::Staged, None)?;
    assert_eq!(changes.len(), 1);
    let fc = &changes[0];
    assert_eq!(fc.path, PathBuf::from("a.txt"));
    assert_eq!(fc.status, StatusKind::Modified);
    assert_eq!(fc.old, "one\n");
    assert_eq!(fc.new, "two\n");
    Ok(())
}

#[test]
fn unstaged_modification_has_old_and_new() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"one\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    write(&repo.path, "a.txt", b"two\n")?; // modify the worktree, do not stage
    let r = Repository::discover(&repo.path)?;

    let changes = r.changes(Selection::Unstaged, None)?;
    assert_eq!(changes.len(), 1);
    let fc = &changes[0];
    assert_eq!(fc.path, PathBuf::from("a.txt"));
    assert_eq!(fc.status, StatusKind::Modified);
    assert_eq!(fc.old, "one\n");
    assert_eq!(fc.new, "two\n");
    Ok(())
}

#[test]
fn binary_file_is_marked_binary() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "bin.dat", b"abc\x00def")?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.changes(Selection::Unstaged, None)?;
    assert_eq!(changes.len(), 1);
    let fc = &changes[0];
    assert!(fc.is_binary);
    assert_eq!(fc.old, "");
    assert_eq!(fc.new, "");
    Ok(())
}

#[test]
fn clean_repo_has_no_default_selection() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"one\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    let r = Repository::discover(&repo.path)?;

    assert_eq!(r.default_selection()?, None);
    Ok(())
}

#[test]
fn pathspec_equal_to_repo_root_is_no_filter() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"hi\n")?;
    let r = Repository::discover(&repo.path)?;
    // Passing the repo root as the pathspec yields an empty relative pattern,
    // which must be treated as "no filter" rather than erroring.
    let changes = r.changes(Selection::Unstaged, Some(&repo.path))?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, PathBuf::from("a.txt"));
    Ok(())
}

#[test]
fn cwd_relative_pathspec_resolves_against_current_dir() -> Result<(), VcsError> {
    // Mirrors `karet ../chat`: a relative pathspec is relative to the process's
    // current directory, not the repo root. Pointing it at the repo root itself
    // must reduce to an empty (no-op) filter, so the change is still reported.
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"hi\n")?;
    let parent = repo
        .path
        .parent()
        .ok_or_else(|| VcsError::Git("temp repo has no parent".into()))?
        .to_path_buf();
    let name = repo
        .path
        .file_name()
        .ok_or_else(|| VcsError::Git("temp repo has no name".into()))?
        .to_owned();

    let _lock = CWD_LOCK.lock().map_err(|e| VcsError::Git(e.to_string()))?;
    let original = std::env::current_dir().map_err(|e| VcsError::Git(e.to_string()))?;
    let _restore = CwdGuard(original);
    std::env::set_current_dir(&parent).map_err(|e| VcsError::Git(e.to_string()))?;

    let r = Repository::discover(Path::new(&name))?;
    let changes = r.changes(Selection::Unstaged, Some(Path::new(&name)))?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, PathBuf::from("a.txt"));
    Ok(())
}

#[test]
fn untracked_files_in_new_dir_are_listed() -> Result<(), VcsError> {
    // A brand-new, wholly-untracked directory tree: every file inside it must
    // surface, not just the top-level ones.
    let repo = init_repo()?;
    write(&repo.path, "newdir/a.txt", b"aaa\n")?;
    write(&repo.path, "newdir/sub/b.txt", b"bbb\n")?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.changes(Selection::Unstaged, None)?;
    let paths: Vec<PathBuf> = changes.iter().map(|c| c.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            PathBuf::from("newdir/a.txt"),
            PathBuf::from("newdir/sub/b.txt"),
        ]
    );
    assert!(changes.iter().all(|c| c.status == StatusKind::Untracked));
    Ok(())
}

#[test]
fn staged_rename_is_detected_even_when_config_disables_renames() -> Result<(), VcsError> {
    let repo = init_repo()?;
    // Explicitly turn rename detection off in config; the SCM view forces it on.
    git(&repo.path, &["config", "diff.renames", "false"])?;
    write(&repo.path, "old.txt", b"one\ntwo\nthree\nfour\nfive\n")?;
    git(&repo.path, &["add", "old.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    // `git mv` stages the rename (updates the index).
    git(&repo.path, &["mv", "old.txt", "new.txt"])?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.changes(Selection::Staged, None)?;
    let fc = changes
        .iter()
        .find(|c| c.path.as_path() == Path::new("new.txt"))
        .ok_or_else(|| VcsError::Git("renamed file not reported".into()))?;
    assert_eq!(fc.status, StatusKind::Renamed);
    assert_eq!(fc.old_path.as_deref(), Some(Path::new("old.txt")));
    Ok(())
}

#[test]
fn staged_changes_during_a_merge_do_not_error() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"base\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "base"])?;
    git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
    write(&repo.path, "a.txt", b"feature\n")?;
    git(&repo.path, &["commit", "-q", "-am", "feature"])?;
    git(&repo.path, &["checkout", "-q", "-"])?;
    write(&repo.path, "a.txt", b"main\n")?;
    git(&repo.path, &["commit", "-q", "-am", "main"])?;
    // The merge conflicts, leaving unmerged (stage 1/2/3) entries in the index.
    let _ = git(&repo.path, &["merge", "--no-edit", "feature"]);
    let r = Repository::discover(&repo.path)?;

    // The staged (HEAD↔index) read must succeed over an unmerged index rather
    // than error — otherwise `Session::compute_vcs` swallows it into an empty
    // staged section, and status "breaks" during a merge. (It may legitimately
    // be empty; the point is that the `?` below does not propagate an error.)
    r.changes(Selection::Staged, None)?;
    // The conflict is still surfaced on the unstaged side.
    let unstaged = r.changes(Selection::Unstaged, None)?;
    assert!(unstaged.iter().any(|c| c.status == StatusKind::Conflicted));
    Ok(())
}

#[test]
fn commit_changes_diffs_against_first_parent() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"one\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    write(&repo.path, "a.txt", b"one\ntwo\n")?;
    write(&repo.path, "b.txt", b"new file\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "second"])?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.commit_changes("HEAD")?;
    assert_eq!(changes.len(), 2);
    let a = changes
        .iter()
        .find(|c| c.path.as_path() == Path::new("a.txt"))
        .ok_or_else(|| VcsError::Git("a.txt missing".into()))?;
    assert_eq!(a.status, StatusKind::Modified);
    assert_eq!(a.old, "one\n");
    assert_eq!(a.new, "one\ntwo\n");
    let b = changes
        .iter()
        .find(|c| c.path.as_path() == Path::new("b.txt"))
        .ok_or_else(|| VcsError::Git("b.txt missing".into()))?;
    assert_eq!(b.status, StatusKind::Added);
    assert_eq!(b.new, "new file\n");
    Ok(())
}

#[test]
fn commit_changes_on_root_are_all_additions() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"one\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "root"])?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.commit_changes("HEAD")?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].status, StatusKind::Added);
    assert_eq!(changes[0].old, "");
    assert_eq!(changes[0].new, "one\n");
    Ok(())
}

#[test]
fn commit_changes_detect_renames() -> Result<(), VcsError> {
    let repo = init_repo()?;
    git(&repo.path, &["config", "diff.renames", "false"])?;
    write(&repo.path, "old.txt", b"one\ntwo\nthree\nfour\nfive\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "init"])?;
    git(&repo.path, &["mv", "old.txt", "new.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "rename"])?;
    let r = Repository::discover(&repo.path)?;

    let changes = r.commit_changes("HEAD")?;
    let fc = changes
        .iter()
        .find(|c| c.path.as_path() == Path::new("new.txt"))
        .ok_or_else(|| VcsError::Git("renamed file not reported".into()))?;
    assert_eq!(fc.status, StatusKind::Renamed);
    assert_eq!(fc.old_path.as_deref(), Some(Path::new("old.txt")));
    Ok(())
}

#[test]
fn conflicted_file_is_reported() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"base\n")?;
    git(&repo.path, &["add", "a.txt"])?;
    git(&repo.path, &["commit", "-q", "-m", "base"])?;
    // Diverge the same line on a feature branch and on the original branch.
    git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
    write(&repo.path, "a.txt", b"feature\n")?;
    git(&repo.path, &["commit", "-q", "-am", "feature"])?;
    git(&repo.path, &["checkout", "-q", "-"])?;
    write(&repo.path, "a.txt", b"main\n")?;
    git(&repo.path, &["commit", "-q", "-am", "main"])?;
    // The merge conflicts; `git merge` exits non-zero, which is expected here.
    let _ = git(&repo.path, &["merge", "--no-edit", "feature"]);
    let r = Repository::discover(&repo.path)?;

    let changes = r.changes(Selection::Unstaged, None)?;
    let fc = changes
        .iter()
        .find(|c| c.path.as_path() == Path::new("a.txt"))
        .ok_or_else(|| VcsError::Git("conflicted file not reported".into()))?;
    assert_eq!(fc.status, StatusKind::Conflicted);
    Ok(())
}

/// A repo whose `main` forks into `feature`, with `main` also advancing afterwards.
/// Returns the repo so callers can diff `main` against `feature` two ways.
fn forked_repo() -> Result<TempRepo, VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "base.txt", b"base\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "base"])?;
    git(&repo.path, &["branch", "-M", "main"])?;
    // feature adds feature.txt on top of the fork point.
    git(&repo.path, &["checkout", "-q", "-b", "feature"])?;
    write(&repo.path, "feature.txt", b"feature\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "add feature"])?;
    // main advances independently, changing base.txt after the fork.
    git(&repo.path, &["checkout", "-q", "main"])?;
    write(&repo.path, "base.txt", b"base changed on main\n")?;
    git(&repo.path, &["commit", "-q", "-am", "advance main"])?;
    Ok(repo)
}

#[test]
fn range_changes_two_dot_diffs_the_two_tips() -> Result<(), VcsError> {
    let repo = forked_repo()?;
    let r = Repository::discover(&repo.path)?;
    // main..feature is the raw tree diff between the tips: feature adds feature.txt,
    // and (relative to main's newer tip) reverts main's change to base.txt.
    let changes = r.range_changes("main", "feature", false)?;
    let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
    assert!(paths.contains(&PathBuf::from("feature.txt")));
    assert!(
        paths.contains(&PathBuf::from("base.txt")),
        "two-dot reflects main's later change to base.txt"
    );
    Ok(())
}

#[test]
fn range_changes_three_dot_uses_the_merge_base() -> Result<(), VcsError> {
    let repo = forked_repo()?;
    let r = Repository::discover(&repo.path)?;
    // main...feature diffs from the fork point, so only feature's own change shows;
    // main's later change to base.txt is excluded.
    let changes = r.range_changes("main", "feature", true)?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, PathBuf::from("feature.txt"));
    assert_eq!(changes[0].status, StatusKind::Added);
    Ok(())
}

#[test]
fn merge_base_finds_the_fork_point() -> Result<(), VcsError> {
    let repo = forked_repo()?;
    let r = Repository::discover(&repo.path)?;
    let base = r
        .merge_base("main", "feature")?
        .ok_or_else(|| VcsError::Git("expected a merge base".into()))?;
    // The fork point is the "base" commit — the first-parent of feature's tip.
    let expected = r.commit_detail("feature~1")?.hash;
    assert_eq!(base, expected);
    Ok(())
}

#[test]
fn merge_base_is_none_for_unrelated_histories() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"a\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "first"])?;
    git(&repo.path, &["branch", "-M", "main"])?;
    // An orphan branch has no common ancestor with main.
    git(&repo.path, &["checkout", "-q", "--orphan", "other"])?;
    write(&repo.path, "b.txt", b"b\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "orphan"])?;
    let r = Repository::discover(&repo.path)?;
    assert!(r.merge_base("main", "other")?.is_none());
    // A three-dot range over unrelated histories is an error, not a silent success.
    assert!(r.range_changes("main", "other", true).is_err());
    Ok(())
}

#[test]
fn default_base_branch_skips_the_current_branch() -> Result<(), VcsError> {
    let repo = forked_repo()?; // leaves us on `main`
    let r = Repository::discover(&repo.path)?;
    // On main, main is excluded and no other candidate exists.
    assert_eq!(r.default_base_branch(), None);
    // On feature, main is the detected base.
    git(&repo.path, &["checkout", "-q", "feature"])?;
    let r = Repository::discover(&repo.path)?;
    assert_eq!(r.default_base_branch().as_deref(), Some("main"));
    Ok(())
}

#[test]
fn upstream_of_head_is_none_without_a_tracking_branch() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"a\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "first"])?;
    let r = Repository::discover(&repo.path)?;
    // A fresh local repo has no remote, so the branch has no upstream.
    assert!(r.upstream_of_head()?.is_none());
    Ok(())
}

#[test]
fn file_at_rev_reads_each_revision_and_reports_absence() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"v0\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "c0"])?;
    write(&repo.path, "a.txt", b"v1\n")?;
    git(&repo.path, &["commit", "-q", "-am", "c1"])?;
    let r = Repository::discover(&repo.path)?;

    let path = repo.path.join("a.txt");
    // HEAD holds the newest content; the previous commit holds the older one.
    assert_eq!(r.file_at_rev(&path, "HEAD")?.as_deref(), Some(&b"v1\n"[..]));
    assert_eq!(
        r.file_at_rev(&path, "HEAD~1")?.as_deref(),
        Some(&b"v0\n"[..])
    );
    // A path that does not exist at that revision is `None`, not an error.
    assert!(
        r.file_at_rev(&repo.path.join("missing.txt"), "HEAD")?
            .is_none()
    );
    Ok(())
}

#[test]
fn file_at_rev_returns_bytes_verbatim_for_binary_content() -> Result<(), VcsError> {
    let repo = init_repo()?;
    let bytes = b"\x00\x01\x02rust\xff\xfe";
    write(&repo.path, "bin.dat", bytes)?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "add binary"])?;
    let r = Repository::discover(&repo.path)?;
    assert_eq!(
        r.file_at_rev(&repo.path.join("bin.dat"), "HEAD")?
            .as_deref(),
        Some(&bytes[..])
    );
    Ok(())
}

#[test]
fn file_at_rev_of_a_bad_revision_errors() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"x\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "c0"])?;
    let r = Repository::discover(&repo.path)?;
    assert!(
        r.file_at_rev(&repo.path.join("a.txt"), "no-such-rev")
            .is_err()
    );
    Ok(())
}

#[test]
fn branches_lists_locals_and_flags_the_head() -> Result<(), VcsError> {
    let repo = forked_repo()?; // leaves us on `main`; also has `feature`
    let r = Repository::discover(&repo.path)?;
    let branches = r.branches()?;
    let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["feature", "main"], "sorted by name");
    let head: Vec<&str> = branches
        .iter()
        .filter(|b| b.is_head)
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(head, vec!["main"], "only the checked-out branch is head");
    Ok(())
}

#[test]
fn current_branch_tracks_the_checkout() -> Result<(), VcsError> {
    let repo = forked_repo()?;
    let r = Repository::discover(&repo.path)?;
    assert_eq!(r.current_branch()?.as_deref(), Some("main"));
    git(&repo.path, &["checkout", "-q", "feature"])?;
    let r = Repository::discover(&repo.path)?;
    assert_eq!(r.current_branch()?.as_deref(), Some("feature"));
    Ok(())
}

#[test]
fn unborn_branch_has_a_name_but_no_branch_refs() -> Result<(), VcsError> {
    // A fresh repository's HEAD symbolically points at an unborn branch: it has
    // a name, but no ref exists yet so the local branch list is empty.
    let repo = init_repo()?;
    git(&repo.path, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
    let r = Repository::discover(&repo.path)?;
    assert_eq!(r.current_branch()?.as_deref(), Some("main"));
    assert!(r.branches()?.is_empty());
    Ok(())
}

#[test]
fn detached_head_has_no_current_branch() -> Result<(), VcsError> {
    let repo = init_repo()?;
    write(&repo.path, "a.txt", b"a\n")?;
    git(&repo.path, &["add", "."])?;
    git(&repo.path, &["commit", "-q", "-m", "c0"])?;
    git(&repo.path, &["checkout", "-q", "--detach"])?;
    let r = Repository::discover(&repo.path)?;
    assert!(r.current_branch()?.is_none());
    Ok(())
}

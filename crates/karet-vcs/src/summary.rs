//! Compact repository health for workspace explorers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use imara_diff::Algorithm;
use imara_diff::Diff;
use imara_diff::InternedInput;

use crate::FileChange;
use crate::Repository;
use crate::Selection;
use crate::VcsError;

/// Compact synchronization and uncommitted-line counts for one repository.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepositorySummary {
    /// Commits present locally but not in the configured upstream.
    pub ahead: usize,
    /// Commits present in the configured upstream but not locally.
    pub behind: usize,
    /// Uncommitted lines added relative to `HEAD`.
    pub added: usize,
    /// Uncommitted lines removed relative to `HEAD`.
    pub removed: usize,
}

impl RepositorySummary {
    /// Whether the repository is both clean and synchronized with its upstream.
    #[must_use]
    pub fn is_clean(self) -> bool {
        self.ahead == 0 && self.behind == 0 && self.added == 0 && self.removed == 0
    }
}

impl Repository {
    /// Compute compact upstream divergence and working-tree line statistics.
    ///
    /// Staged and unstaged edits to the same path are composed before diffing, so
    /// the counts describe `HEAD` → working tree rather than double-counting the
    /// intermediate index state. Binary changes contribute no line counts.
    ///
    /// # Errors
    /// Returns [`VcsError`] when repository state or changes cannot be read.
    pub fn summary(&self) -> Result<RepositorySummary, VcsError> {
        let state = self.repository_state()?;
        let staged = self.changes(Selection::Staged, None)?;
        let unstaged = self.changes(Selection::Unstaged, None)?;
        let (added, removed) = combined_line_counts(staged, unstaged);
        Ok(RepositorySummary {
            ahead: state.ahead,
            behind: state.behind,
            added,
            removed,
        })
    }
}

fn combined_line_counts(staged: Vec<FileChange>, unstaged: Vec<FileChange>) -> (usize, usize) {
    let mut files: BTreeMap<PathBuf, (Option<FileChange>, Option<FileChange>)> = BTreeMap::new();
    for change in staged {
        let path = change.path.clone();
        files.entry(path).or_default().0 = Some(change);
    }
    for change in unstaged {
        let path = change.path.clone();
        files.entry(path).or_default().1 = Some(change);
    }
    files
        .into_values()
        .fold((0_usize, 0_usize), |(added, removed), pair| {
            let (old, new, binary) = match pair {
                (Some(staged), Some(unstaged)) => (
                    staged.old,
                    unstaged.new,
                    staged.is_binary || unstaged.is_binary,
                ),
                (Some(change), None) | (None, Some(change)) => {
                    (change.old, change.new, change.is_binary)
                },
                (None, None) => return (added, removed),
            };
            if binary {
                return (added, removed);
            }
            let input = InternedInput::new(old.as_str(), new.as_str());
            let diff = Diff::compute(Algorithm::Histogram, &input);
            (
                added.saturating_add(diff.count_additions() as usize),
                removed.saturating_add(diff.count_removals() as usize),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StatusKind;
    use crate::test_support;

    fn change(path: &str, old: &str, new: &str) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status: StatusKind::Modified,
            is_binary: false,
            old: old.to_string(),
            new: new.to_string(),
        }
    }

    #[test]
    fn clean_predicate_covers_sync_and_line_counts() {
        assert!(RepositorySummary::default().is_clean());
        assert!(
            !RepositorySummary {
                ahead: 1,
                ..RepositorySummary::default()
            }
            .is_clean()
        );
        assert!(
            !RepositorySummary {
                added: 1,
                ..RepositorySummary::default()
            }
            .is_clean()
        );
    }

    #[test]
    fn staged_and_unstaged_versions_compose_before_counting() {
        let staged = change("a.txt", "one\ntwo\n", "one\nchanged\n");
        let unstaged = change("a.txt", "one\nchanged\n", "one\nchanged\nthree\n");
        assert_eq!(combined_line_counts(vec![staged], vec![unstaged]), (2, 1));
    }

    #[test]
    fn independent_files_sum_and_binary_changes_do_not_count() {
        let added = change("a.txt", "", "one\ntwo\n");
        let removed = change("b.txt", "old\n", "");
        let mut binary = change("image.png", "", "");
        binary.is_binary = true;
        assert_eq!(
            combined_line_counts(vec![removed, binary], vec![added]),
            (2, 1)
        );
    }

    #[test]
    fn repository_summary_tracks_divergence_and_composed_worktree_edits() -> Result<(), VcsError> {
        let repo = test_support::init("summary")?;
        let remote = test_support::bare_remote("summary")?;
        test_support::commit(&repo, "one\ntwo\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        test_support::git(&repo.0, &["push", "-q", "-u", "origin", "main"])?;
        let vcs = Repository::discover(&repo.0)?;
        assert!(vcs.summary()?.is_clean());

        test_support::commit(&repo, "one\nchanged\n", "local")?;
        test_support::write(&repo.0, "file.txt", b"one\nchanged\nthree\n")?;
        let summary = vcs.summary()?;
        assert_eq!(summary.ahead, 1);
        assert_eq!(summary.behind, 0);
        assert_eq!((summary.added, summary.removed), (1, 0));
        Ok(())
    }

    #[test]
    fn repository_summary_reports_both_divergence_directions_and_all_worktree_layers()
    -> Result<(), VcsError> {
        let repo = test_support::init("summary-diverged")?;
        let remote = test_support::bare_remote("summary-diverged")?;
        test_support::commit(&repo, "initial\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        test_support::git(&remote.0, &["symbolic-ref", "HEAD", "refs/heads/main"])?;
        test_support::git(&repo.0, &["push", "-q", "-u", "origin", "main"])?;

        test_support::commit(&repo, "local\n", "local")?;
        let peer = test_support::init("summary-diverged-peer")?;
        std::fs::remove_dir_all(&peer.0).map_err(|error| VcsError::Git(error.to_string()))?;
        let parent = peer
            .0
            .parent()
            .ok_or_else(|| VcsError::Git("test repository has no parent".to_string()))?;
        test_support::git(
            parent,
            &[
                "clone",
                "-q",
                &remote.0.to_string_lossy(),
                &peer.0.to_string_lossy(),
            ],
        )?;
        test_support::git(&peer.0, &["config", "user.email", "test@example.com"])?;
        test_support::git(&peer.0, &["config", "user.name", "karet test"])?;
        test_support::write(&peer.0, "remote.txt", b"remote\n")?;
        test_support::git(&peer.0, &["add", "remote.txt"])?;
        test_support::git(&peer.0, &["commit", "-q", "-m", "remote"])?;
        test_support::git(&peer.0, &["push", "-q", "origin", "main"])?;
        test_support::git(&repo.0, &["fetch", "-q", "origin"])?;

        test_support::write(&repo.0, "staged.txt", b"one\n")?;
        test_support::git(&repo.0, &["add", "staged.txt"])?;
        test_support::write(&repo.0, "staged.txt", b"one\ntwo\n")?;
        test_support::write(&repo.0, "untracked.txt", b"three\nfour\n")?;
        std::fs::remove_file(repo.0.join("file.txt"))
            .map_err(|error| VcsError::Git(error.to_string()))?;

        assert_eq!(
            Repository::discover(&repo.0)?.summary()?,
            RepositorySummary {
                ahead: 1,
                behind: 1,
                added: 4,
                removed: 1,
            }
        );
        Ok(())
    }

    #[test]
    fn repository_summary_tolerates_a_gone_upstream() -> Result<(), VcsError> {
        let repo = test_support::init("summary-gone-upstream")?;
        let remote = test_support::bare_remote("summary-gone-upstream")?;
        test_support::commit(&repo, "initial\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        test_support::git(&repo.0, &["push", "-q", "-u", "origin", "main"])?;
        test_support::git(&repo.0, &["update-ref", "-d", "refs/remotes/origin/main"])?;
        test_support::write(&repo.0, "file.txt", b"initial\nlocal\n")?;

        assert_eq!(
            Repository::discover(&repo.0)?.summary()?,
            RepositorySummary {
                added: 1,
                ..RepositorySummary::default()
            }
        );
        Ok(())
    }
}

//! History-rewriting and ref operations: tags, cherry-pick, revert, rebase,
//! reset, and detached checkout.
//!
//! Reads (labels, resolution) stay in-process through `gix`; every mutation
//! goes through the hardened `git` subprocess layer (see [`crate::write`]) so
//! hooks, signing, and user configuration keep working. A revision argument
//! is always resolved to its full hash first — validation and
//! option-injection safety in one step.

use gix::bstr::ByteSlice;

use crate::Repository;
use crate::VcsError;
use crate::repo::to_git;

/// What a [`RefLabel`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum RefKind {
    /// A local branch.
    Local,
    /// A remote-tracking branch.
    Remote,
    /// A tag.
    Tag,
    /// The current `HEAD` (attached or not).
    Head,
}

/// One ref decorating a commit in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RefLabel {
    /// The short name (`main`, `origin/main`, `v1.2.0`).
    pub name: String,
    /// What the name is.
    pub kind: RefKind,
}

/// How [`Repository::reset`] moves `HEAD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ResetMode {
    /// Move `HEAD` only; index and worktree stay.
    Soft,
    /// Move `HEAD` and reset the index; the worktree stays.
    Mixed,
    /// Move everything, discarding local changes.
    Hard,
}

impl Repository {
    /// Resolve `rev` to its full commit hash (validation + injection safety
    /// for the subprocess layer in one step).
    ///
    /// # Errors
    /// [`VcsError::Git`] when the revision does not resolve to a commit.
    pub fn resolve_commit(&self, rev: &str) -> Result<String, VcsError> {
        let id = self
            .inner
            .rev_parse_single(rev.as_bytes().as_bstr())
            .map_err(to_git)?;
        let commit = id.object().map_err(to_git)?;
        let commit = commit.peel_to_commit().map_err(to_git)?;
        Ok(commit.id().to_hex().to_string())
    }

    /// Every ref per commit, for log-row decorations. `HEAD` reports on its
    /// target commit (named `HEAD` when detached, otherwise through its
    /// branch's label already being present).
    ///
    /// # Errors
    /// [`VcsError::Git`] when the ref database cannot be read.
    pub fn ref_labels(&self) -> Result<std::collections::HashMap<String, Vec<RefLabel>>, VcsError> {
        let mut labels: std::collections::HashMap<String, Vec<RefLabel>> =
            std::collections::HashMap::new();
        let platform = self.inner.references().map_err(to_git)?;
        let mut push = |target: gix::Id<'_>, name: String, kind: RefKind| {
            labels
                .entry(target.detach().to_hex().to_string())
                .or_default()
                .push(RefLabel { name, kind });
        };
        for reference in platform.all().map_err(to_git)?.flatten() {
            let name = reference.name().shorten().to_str_lossy().into_owned();
            let kind = match reference.name().category() {
                Some(gix::refs::Category::LocalBranch) => RefKind::Local,
                Some(gix::refs::Category::RemoteBranch) => RefKind::Remote,
                Some(gix::refs::Category::Tag) => RefKind::Tag,
                _ => continue,
            };
            // A tag object peels to the commit it annotates; branches are
            // already direct.
            let mut reference = reference;
            if let Ok(id) = reference.peel_to_id() {
                push(id, name, kind);
            }
        }
        // A detached HEAD has no branch label; name it explicitly.
        if self.inner.head_name().map_err(to_git)?.is_none()
            && let Ok(head) = self.inner.head_id()
        {
            push(head, "HEAD".to_owned(), RefKind::Head);
        }
        Ok(labels)
    }

    /// Create a tag at `rev` — lightweight, or annotated when `message` is
    /// given (annotated tags respect `tag.gpgSign` and hooks, which is why
    /// this is a subprocess write).
    ///
    /// # Errors
    /// [`VcsError::Git`] on an invalid name/revision or a failed write.
    pub fn tag_create(&self, name: &str, rev: &str, message: Option<&str>) -> Result<(), VcsError> {
        self.validate_tag_name(name)?;
        let hash = self.resolve_commit(rev)?;
        match message {
            Some(message) => {
                self.git_checked(["tag", "--annotate", name, hash.as_str(), "-m", message])?;
            },
            None => {
                self.git_checked(["tag", name, hash.as_str()])?;
            },
        }
        Ok(())
    }

    /// Delete the local tag `name`.
    ///
    /// # Errors
    /// [`VcsError::Git`] when the tag does not exist or cannot be deleted.
    pub fn tag_delete(&self, name: &str) -> Result<(), VcsError> {
        self.validate_tag_name(name)?;
        self.git_checked(["tag", "--delete", name])?;
        Ok(())
    }

    /// Cherry-pick `rev` onto `HEAD`. On conflicts the repository is left in
    /// its in-progress state for the continue/abort/skip flow.
    ///
    /// # Errors
    /// [`VcsError::Git`] on conflicts (operation left in progress) or when
    /// the revision does not resolve.
    pub fn cherry_pick(&self, rev: &str) -> Result<(), VcsError> {
        let hash = self.resolve_commit(rev)?;
        self.git_checked(["cherry-pick", hash.as_str()])?;
        Ok(())
    }

    /// Revert `rev` on top of `HEAD` (a new commit undoing it). Conflicts
    /// leave the in-progress state for the continue/abort/skip flow.
    ///
    /// # Errors
    /// [`VcsError::Git`] on conflicts or an unresolvable revision.
    pub fn revert(&self, rev: &str) -> Result<(), VcsError> {
        let hash = self.resolve_commit(rev)?;
        self.git_checked(["revert", "--no-edit", hash.as_str()])?;
        Ok(())
    }

    /// Rebase the current branch onto `rev`. Conflicts leave the in-progress
    /// state for the continue/abort/skip flow.
    ///
    /// # Errors
    /// [`VcsError::Git`] on conflicts or an unresolvable revision.
    pub fn rebase_onto(&self, rev: &str) -> Result<(), VcsError> {
        let hash = self.resolve_commit(rev)?;
        self.git_checked(["rebase", hash.as_str()])?;
        Ok(())
    }

    /// Reset the current branch to `rev` with `mode`.
    ///
    /// # Errors
    /// [`VcsError::Git`] on an unresolvable revision or a failed reset.
    pub fn reset(&self, mode: ResetMode, rev: &str) -> Result<(), VcsError> {
        let hash = self.resolve_commit(rev)?;
        let flag = match mode {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        };
        self.git_checked(["reset", flag, hash.as_str()])?;
        Ok(())
    }

    /// Check out `rev` directly, detaching `HEAD`.
    ///
    /// # Errors
    /// [`VcsError::Git`] on an unresolvable revision or local changes the
    /// checkout would overwrite.
    pub fn checkout_detached(&self, rev: &str) -> Result<(), VcsError> {
        let hash = self.resolve_commit(rev)?;
        self.git_checked(["switch", "--detach", hash.as_str()])?;
        Ok(())
    }

    fn validate_tag_name(&self, name: &str) -> Result<(), VcsError> {
        let full = format!("refs/tags/{name}");
        if name.is_empty()
            || name.starts_with('-')
            || !self
                .git_output(["check-ref-format", full.as_str()])?
                .status
                .success()
        {
            return Err(VcsError::Git(format!("invalid tag name: {name}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::commit;
    use crate::test_support::git;
    use crate::test_support::init;
    use crate::test_support::write;

    #[test]
    fn tags_create_annotate_label_and_delete() -> Result<(), VcsError> {
        let dir = init("ops-tags")?;
        let first = commit(&dir, "one", "first")?;
        commit(&dir, "two", "second")?;
        let repo = Repository::discover(&dir.0)?;
        repo.tag_create("v1.0.0", &first, None)?;
        repo.tag_create("v1.1.0", "HEAD", Some("release 1.1"))?;
        // Labels attach to the peeled commits, annotated tags included.
        let labels = repo.ref_labels()?;
        let first_labels = labels.get(&first).map(Vec::as_slice).unwrap_or_default();
        assert!(
            first_labels
                .iter()
                .any(|l| l.name == "v1.0.0" && l.kind == RefKind::Tag)
        );
        let head = repo.resolve_commit("HEAD")?;
        let head_labels = labels.get(&head).map(Vec::as_slice).unwrap_or_default();
        assert!(
            head_labels
                .iter()
                .any(|l| l.name == "v1.1.0" && l.kind == RefKind::Tag)
        );
        assert!(
            head_labels
                .iter()
                .any(|l| l.name == "main" && l.kind == RefKind::Local)
        );
        repo.tag_delete("v1.0.0")?;
        assert!(
            !repo
                .ref_labels()?
                .values()
                .flatten()
                .any(|l| l.name == "v1.0.0")
        );
        // Bad names refuse before any subprocess mutation.
        assert!(repo.tag_create("-oops", "HEAD", None).is_err());
        assert!(repo.tag_create("a b", "HEAD", None).is_err());
        Ok(())
    }

    #[test]
    fn hostile_revisions_never_reach_the_subprocess_as_options() {
        // Every write resolves its revision to a full hash in-process first, so
        // an argument that looks like a `git` option — or like shell syntax —
        // must fail to resolve rather than being handed to the subprocess.
        let Ok(dir) = init("ops-injection") else {
            return;
        };
        if commit(&dir, "one", "first").is_err() {
            return;
        }
        let Ok(repo) = Repository::discover(&dir.0) else {
            return;
        };
        let canary = dir.0.join("PWNED");
        for rev in [
            "--upload-pack=touch PWNED",
            "-c core.pager=touch PWNED",
            "--exec=touch PWNED",
            "--output=PWNED",
            "; touch PWNED",
            "$(touch PWNED)",
            "`touch PWNED`",
            "HEAD --hard",
            "--all",
            "-",
            "--",
            "HEAD\ntouch PWNED",
        ] {
            assert!(repo.resolve_commit(rev).is_err(), "resolved {rev:?}");
            assert!(repo.cherry_pick(rev).is_err(), "cherry-picked {rev:?}");
            assert!(repo.revert(rev).is_err(), "reverted {rev:?}");
            assert!(repo.rebase_onto(rev).is_err(), "rebased onto {rev:?}");
            assert!(
                repo.reset(ResetMode::Soft, rev).is_err(),
                "reset to {rev:?}"
            );
            assert!(repo.checkout_detached(rev).is_err(), "checked out {rev:?}");
        }
        assert!(
            !canary.exists(),
            "a hostile revision reached the shell and wrote {}",
            canary.display()
        );
    }

    #[test]
    fn hostile_tag_names_are_refused_before_any_write() {
        let Ok(dir) = init("ops-tag-injection") else {
            return;
        };
        if commit(&dir, "one", "first").is_err() {
            return;
        }
        let Ok(repo) = Repository::discover(&dir.0) else {
            return;
        };
        for name in [
            "--upload-pack=x",
            "-f",
            "--delete",
            "a b",
            "a..b",
            "a~1",
            "a^",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            ".lock",
            "",
            "a\nb",
            "/leading",
            "trailing/",
            "a//b",
            "a@{b",
        ] {
            assert!(
                repo.tag_create(name, "HEAD", None).is_err(),
                "created a tag named {name:?}"
            );
            assert!(
                repo.tag_delete(name).is_err(),
                "deleted a tag named {name:?}"
            );
        }
        // The guard is not simply refusing everything.
        assert!(repo.tag_create("v9.9.9", "HEAD", Some("real")).is_ok());
    }

    #[test]
    fn cherry_pick_applies_a_commit_from_another_branch() -> Result<(), VcsError> {
        let dir = init("ops-pick")?;
        commit(&dir, "base\n", "base")?;
        git(&dir.0, &["switch", "-q", "-c", "feature"])?;
        write(&dir.0, "feature.txt", b"feature work\n")?;
        git(&dir.0, &["add", "feature.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "feature commit"])?;
        let picked = git(&dir.0, &["rev-parse", "HEAD"])?;
        git(&dir.0, &["switch", "-q", "main"])?;
        let repo = Repository::discover(&dir.0)?;
        repo.cherry_pick(&picked)?;
        assert!(dir.0.join("feature.txt").exists());
        assert_eq!(
            git(&dir.0, &["log", "-1", "--format=%s"])?,
            "feature commit"
        );
        Ok(())
    }

    #[test]
    fn revert_undoes_a_commit_with_a_new_one() -> Result<(), VcsError> {
        let dir = init("ops-revert")?;
        commit(&dir, "good\n", "good")?;
        let bad = commit(&dir, "bad\n", "bad")?;
        let repo = Repository::discover(&dir.0)?;
        repo.revert(&bad)?;
        let content = std::fs::read_to_string(dir.0.join("file.txt"))
            .map_err(|e| VcsError::Git(e.to_string()))?;
        assert_eq!(content, "good\n");
        assert!(git(&dir.0, &["log", "-1", "--format=%s"])?.starts_with("Revert"));
        Ok(())
    }

    #[test]
    fn conflicting_cherry_pick_reports_and_leaves_the_operation_in_progress() -> Result<(), VcsError>
    {
        let dir = init("ops-conflict")?;
        commit(&dir, "base\n", "base")?;
        git(&dir.0, &["switch", "-q", "-c", "other"])?;
        commit(&dir, "theirs\n", "theirs")?;
        let theirs = git(&dir.0, &["rev-parse", "HEAD"])?;
        git(&dir.0, &["switch", "-q", "main"])?;
        commit(&dir, "ours\n", "ours")?;
        let repo = Repository::discover(&dir.0)?;
        assert!(repo.cherry_pick(&theirs).is_err());
        // The in-progress state is what the continue/abort/skip flow reads.
        assert!(dir.0.join(".git/CHERRY_PICK_HEAD").exists());
        repo.abort_operation()?;
        assert!(!dir.0.join(".git/CHERRY_PICK_HEAD").exists());
        Ok(())
    }

    #[test]
    fn rebase_replays_the_branch_onto_a_new_base() -> Result<(), VcsError> {
        let dir = init("ops-rebase")?;
        commit(&dir, "base\n", "base")?;
        git(&dir.0, &["switch", "-q", "-c", "feature"])?;
        write(&dir.0, "feature.txt", b"work\n")?;
        git(&dir.0, &["add", "feature.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "on feature"])?;
        git(&dir.0, &["switch", "-q", "main"])?;
        write(&dir.0, "main.txt", b"ahead\n")?;
        git(&dir.0, &["add", "main.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "main ahead"])?;
        let main_tip = git(&dir.0, &["rev-parse", "HEAD"])?;
        git(&dir.0, &["switch", "-q", "feature"])?;
        let repo = Repository::discover(&dir.0)?;
        repo.rebase_onto(&main_tip)?;
        // The rebased branch now descends from main's tip.
        assert_eq!(git(&dir.0, &["rev-parse", "HEAD~1"])?, main_tip);
        Ok(())
    }

    #[test]
    fn reset_modes_move_head_with_the_documented_side_effects() -> Result<(), VcsError> {
        let dir = init("ops-reset")?;
        let first = commit(&dir, "one\n", "first")?;
        commit(&dir, "two\n", "second")?;
        let repo = Repository::discover(&dir.0)?;
        // Soft: HEAD moves, the second commit's content stays staged.
        repo.reset(ResetMode::Soft, &first)?;
        assert_eq!(git(&dir.0, &["rev-parse", "HEAD"])?, first);
        assert!(!git(&dir.0, &["diff", "--cached", "--name-only"])?.is_empty());
        // Hard: everything returns to the target's state.
        repo.reset(ResetMode::Hard, &first)?;
        let content = std::fs::read_to_string(dir.0.join("file.txt"))
            .map_err(|e| VcsError::Git(e.to_string()))?;
        assert_eq!(content, "one\n");
        Ok(())
    }

    #[test]
    fn detached_checkout_names_head_in_the_labels() -> Result<(), VcsError> {
        let dir = init("ops-detach")?;
        let first = commit(&dir, "one\n", "first")?;
        commit(&dir, "two\n", "second")?;
        let repo = Repository::discover(&dir.0)?;
        repo.checkout_detached(&first)?;
        let labels = repo.ref_labels()?;
        let at_first = labels.get(&first).map(Vec::as_slice).unwrap_or_default();
        assert!(at_first.iter().any(|l| l.kind == RefKind::Head));
        Ok(())
    }

    #[test]
    fn interactive_rebase_reorders_squashes_and_drops() -> Result<(), VcsError> {
        let dir = init("ops-irebase")?;
        let base = commit(&dir, "base\n", "base")?;
        write(&dir.0, "a.txt", b"a\n")?;
        git(&dir.0, &["add", "a.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "add a"])?;
        let a = git(&dir.0, &["rev-parse", "HEAD"])?;
        write(&dir.0, "b.txt", b"b\n")?;
        git(&dir.0, &["add", "b.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "add b"])?;
        let b = git(&dir.0, &["rev-parse", "HEAD"])?;
        write(&dir.0, "c.txt", b"c\n")?;
        git(&dir.0, &["add", "c.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "add c"])?;
        let c = git(&dir.0, &["rev-parse", "HEAD"])?;
        let repo = Repository::discover(&dir.0)?;
        // Reorder c before a, squash b into it, drop nothing.
        repo.rebase_interactive(
            &base,
            &[
                RebaseStep {
                    action: RebaseAction::Pick,
                    rev: c.clone(),
                },
                RebaseStep {
                    action: RebaseAction::Fixup,
                    rev: b.clone(),
                },
                RebaseStep {
                    action: RebaseAction::Pick,
                    rev: a.clone(),
                },
            ],
        )?;
        let subjects = git(&dir.0, &["log", "--format=%s", &format!("{base}..HEAD")])?;
        assert_eq!(subjects.lines().collect::<Vec<_>>(), vec!["add a", "add c"]);
        assert!(dir.0.join("a.txt").exists());
        assert!(dir.0.join("b.txt").exists(), "fixup keeps b's content");
        assert!(dir.0.join("c.txt").exists());
        Ok(())
    }

    #[test]
    fn interactive_rebase_survives_a_shell_hostile_repository_path() {
        // The todo file is handed to git through `GIT_SEQUENCE_EDITOR`, which
        // git runs under `sh`. Single quotes must neutralize every metacharacter
        // a directory name can carry, and the canary proves none of them ran.
        for hostile in [
            "with space",
            "semi;colon",
            "dollar$(touch PWNED)paren",
            "back`touch PWNED`tick",
            "amp&&touch PWNED",
            "pipe|touch PWNED",
            "star*glob?q",
        ] {
            let Ok(dir) = init(hostile) else {
                continue;
            };
            let Ok(base) = commit(&dir, "base\n", "base") else {
                continue;
            };
            if write(&dir.0, "gone.txt", b"gone\n").is_err()
                || git(&dir.0, &["add", "gone.txt"]).is_err()
                || git(&dir.0, &["commit", "-q", "-m", "gone"]).is_err()
            {
                continue;
            }
            let Ok(gone) = git(&dir.0, &["rev-parse", "HEAD"]) else {
                continue;
            };
            let Ok(repo) = Repository::discover(&dir.0) else {
                continue;
            };
            assert!(
                repo.rebase_interactive(
                    &base,
                    &[RebaseStep {
                        action: RebaseAction::Drop,
                        rev: gone,
                    }],
                )
                .is_ok(),
                "rebase failed in {hostile:?}"
            );
            assert!(
                !dir.0.join("gone.txt").exists(),
                "the plan did not apply in {hostile:?}"
            );
            assert!(
                !dir.0.join("PWNED").exists(),
                "a path metacharacter reached the shell in {hostile:?}"
            );
        }
    }

    #[test]
    fn interactive_rebase_refuses_a_path_it_cannot_quote() {
        // A single quote inside the path cannot be escaped portably inside the
        // single-quoted editor command, so the operation must refuse rather
        // than build a command that means something else.
        let Ok(dir) = init("quote'inside") else {
            return;
        };
        let Ok(base) = commit(&dir, "base\n", "base") else {
            return;
        };
        if write(&dir.0, "gone.txt", b"gone\n").is_err()
            || git(&dir.0, &["add", "gone.txt"]).is_err()
            || git(&dir.0, &["commit", "-q", "-m", "gone"]).is_err()
        {
            return;
        }
        let Ok(gone) = git(&dir.0, &["rev-parse", "HEAD"]) else {
            return;
        };
        let Ok(repo) = Repository::discover(&dir.0) else {
            return;
        };
        let refused = repo.rebase_interactive(
            &base,
            &[RebaseStep {
                action: RebaseAction::Drop,
                rev: gone,
            }],
        );
        assert!(refused.is_err(), "a quoted path must be refused");
        assert!(
            dir.0.join("gone.txt").exists(),
            "the refusal must leave the history alone"
        );
    }

    #[test]
    fn interactive_rebase_drop_removes_a_commit() -> Result<(), VcsError> {
        let dir = init("ops-irebase-drop")?;
        let base = commit(&dir, "base\n", "base")?;
        write(&dir.0, "keep.txt", b"keep\n")?;
        git(&dir.0, &["add", "keep.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "keep"])?;
        let keep = git(&dir.0, &["rev-parse", "HEAD"])?;
        write(&dir.0, "gone.txt", b"gone\n")?;
        git(&dir.0, &["add", "gone.txt"])?;
        git(&dir.0, &["commit", "-q", "-m", "gone"])?;
        let gone = git(&dir.0, &["rev-parse", "HEAD"])?;
        let repo = Repository::discover(&dir.0)?;
        repo.rebase_interactive(
            &base,
            &[
                RebaseStep {
                    action: RebaseAction::Pick,
                    rev: keep,
                },
                RebaseStep {
                    action: RebaseAction::Drop,
                    rev: gone,
                },
            ],
        )?;
        assert!(dir.0.join("keep.txt").exists());
        assert!(!dir.0.join("gone.txt").exists());
        assert!(repo.rebase_interactive(&base, &[]).is_err());
        Ok(())
    }

    #[test]
    fn unresolvable_revisions_refuse_every_operation() -> Result<(), VcsError> {
        let dir = init("ops-badrev")?;
        commit(&dir, "one\n", "first")?;
        let repo = Repository::discover(&dir.0)?;
        assert!(repo.resolve_commit("not-a-rev").is_err());
        assert!(repo.cherry_pick("not-a-rev").is_err());
        assert!(repo.reset(ResetMode::Hard, "not-a-rev").is_err());
        assert!(repo.checkout_detached("not-a-rev").is_err());
        Ok(())
    }
}

/// One step of an interactive-rebase plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RebaseAction {
    /// Keep the commit as-is.
    Pick,
    /// Keep the commit, stopping to reword its message.
    Reword,
    /// Stop at the commit for amending.
    Edit,
    /// Meld into the previous commit, keeping both messages.
    Squash,
    /// Meld into the previous commit, discarding this message.
    Fixup,
    /// Remove the commit.
    Drop,
}

impl RebaseAction {
    /// The verb git's todo file spells this action with.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Pick => "pick",
            Self::Reword => "reword",
            Self::Edit => "edit",
            Self::Squash => "squash",
            Self::Fixup => "fixup",
            Self::Drop => "drop",
        }
    }
}

/// One planned step: the action and the commit it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RebaseStep {
    /// What to do.
    pub action: RebaseAction,
    /// The commit, any resolvable spelling.
    pub rev: String,
}

impl Repository {
    /// Run an interactive rebase onto `onto` with a pre-planned todo list —
    /// karet is the todo editor, so git never opens one. Steps apply oldest
    /// first, exactly as a hand-edited todo file reads. `Reword` and `Edit`
    /// stop the rebase (reword uses the original message; amend then
    /// continue); conflicts land in the continue/abort/skip flow.
    ///
    /// # Errors
    /// [`VcsError::Git`] on an unresolvable revision, an empty plan, a
    /// conflict (operation left in progress), or a failed write.
    pub fn rebase_interactive(&self, onto: &str, steps: &[RebaseStep]) -> Result<(), VcsError> {
        if steps.is_empty() {
            return Err(VcsError::Git("empty rebase plan".to_string()));
        }
        let onto = self.resolve_commit(onto)?;
        let mut todo = String::new();
        for step in steps {
            let hash = self.resolve_commit(&step.rev)?;
            todo.push_str(step.action.verb());
            todo.push(' ');
            todo.push_str(&hash);
            todo.push('\n');
        }
        let todo_path = self
            .inner
            .path()
            .join(format!("karet-rebase-todo-{}", std::process::id()));
        std::fs::write(&todo_path, todo).map_err(|error| VcsError::Git(error.to_string()))?;
        // `GIT_SEQUENCE_EDITOR` runs through sh; single quotes keep an odd
        // .git path one argument. A quote inside the path cannot be escaped
        // portably, so refuse rather than mis-execute.
        let quoted = todo_path.to_string_lossy();
        if quoted.contains('\'') {
            let _ = std::fs::remove_file(&todo_path);
            return Err(VcsError::Git(
                "repository path contains a quote; interactive rebase unavailable".to_string(),
            ));
        }
        let editor = format!("cp '{quoted}'");
        let result = self.git_checked_with_env(
            ["rebase", "--interactive", onto.as_str()],
            &[("GIT_SEQUENCE_EDITOR", editor.as_str())],
        );
        let _ = std::fs::remove_file(&todo_path);
        result.map(|_| ())
    }
}

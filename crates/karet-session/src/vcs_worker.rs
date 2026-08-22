//! Ordered background execution for repository and forge operations.

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;

use karet_core::BlameCommit;
use karet_vcs::Repository;
use karet_vcs::Selection;
use karet_vcs::SyncOutcome;
use karet_vcs::VcsError;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::DocumentId;
use crate::api::Event;
use crate::api::PullRequestSummary;
use crate::api::RangeSpec;
use crate::api::RepositorySnapshot;
use crate::api::RequestId;
use crate::api::VcsAction;
use crate::api::VcsOutcome;
use crate::cancellation::Cancellation;

mod blame;
mod conflict;
mod history;
mod prepare;

use blame::BlameCache;
use blame::blame;
#[cfg(test)]
use blame::map_attribution;
use conflict::run_merge_conflict;
use history::run_commit_detail;
use history::run_file_history;
use history::run_log;
use history::run_range_changes;
use prepare::Status;
use prepare::compute_status;
use prepare::prepare_changes;
use prepare::run_diff_with_rev;
use prepare::run_status;

/// A unit of work sent by the session actor to its serialized VCS worker.
pub(crate) enum VcsJob {
    /// Recompute the source-control status and emit it. A requested refresh
    /// (`id` set) always emits; a spontaneous one (from a filesystem event)
    /// emits only when the status changed, collapsing event bursts and
    /// absorbing the feedback from the session's own index writes.
    Status { id: Option<RequestId> },
    /// Prepare one status entry's displayable diff.
    PrepareChange {
        id: RequestId,
        path: PathBuf,
        staged: bool,
        syntax: bool,
        cancel: Cancellation,
    },
    /// Prepare an ad-hoc diff of two provided texts.
    PrepareTexts {
        id: RequestId,
        path: PathBuf,
        old: String,
        new: String,
        syntax: bool,
        cancel: Cancellation,
    },
    /// Diff one file at a revision against its current content.
    DiffWithRev {
        id: RequestId,
        path: PathBuf,
        rev: String,
        live: Option<String>,
        syntax: bool,
        cancel: Cancellation,
    },
    /// Load the current repository snapshot.
    Snapshot { id: RequestId, cancel: Cancellation },
    /// Compute compact status for one exact nested repository path.
    NestedRepositoryStatus {
        id: RequestId,
        path: PathBuf,
        cancel: Cancellation,
    },
    /// Run one repository action.
    Action { id: RequestId, action: VcsAction },
    /// Query open GitHub pull requests.
    PullRequests {
        id: RequestId,
        remote: String,
        page: u32,
        per_page: u8,
        cancel: Cancellation,
    },
    /// Attribute a current document buffer.
    Blame {
        id: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        text: String,
        line: u32,
        cancel: Cancellation,
    },
    /// Resolve repository/remote facts for one file (per-file discovery).
    RemoteFacts {
        id: RequestId,
        path: PathBuf,
        cancel: Cancellation,
    },
    /// Fetch a page of repository history.
    Log {
        id: RequestId,
        skip: usize,
        limit: usize,
        cancel: Cancellation,
    },
    /// Resolve a commit, then load its changed files progressively.
    CommitDetail {
        id: RequestId,
        rev: String,
        syntax: bool,
        cancel: Cancellation,
    },
    /// Compute a comparison between revisions.
    RangeChanges {
        id: RequestId,
        spec: RangeSpec,
        syntax: bool,
        cancel: Cancellation,
    },
    /// Fetch one file's history.
    FileHistory {
        id: RequestId,
        path: PathBuf,
        skip: usize,
        limit: usize,
        cancel: Cancellation,
    },
    /// Load the two committed sides of one unresolved conflict.
    MergeConflict {
        id: RequestId,
        path: PathBuf,
        cancel: Cancellation,
    },
}

/// Start the one-per-session ordered repository worker.
pub(crate) fn spawn(
    root: Option<PathBuf>,
    events: UnboundedSender<(Option<RequestId>, Event)>,
) -> mpsc::Sender<VcsJob> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut blame_cache = BlameCache::new();
        // The last emitted status, for collapsing spontaneous no-op refreshes.
        let mut last_status = None;
        while let Ok(job) = rx.recv() {
            run(&root, &events, &mut blame_cache, &mut last_status, job);
        }
    });
    tx
}

fn run(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    blame_cache: &mut BlameCache,
    last_status: &mut Option<Status>,
    job: VcsJob,
) {
    match job {
        VcsJob::Status { id } => run_status(root, events, last_status, id),
        VcsJob::PrepareChange {
            id,
            path,
            staged,
            syntax,
            cancel,
        } => {
            let result = repository(root).and_then(|repo| {
                let selection = if staged {
                    Selection::Staged
                } else {
                    Selection::Unstaged
                };
                let changes = repo
                    .changes(selection, Some(&path))
                    .map_err(|error| error.to_string())?;
                changes
                    .into_iter()
                    .find(|change| change.path == path)
                    .ok_or_else(|| {
                        let section = if staged { "staged" } else { "working-tree" };
                        format!("no {section} changes for {}", path.display())
                    })
            });
            let result =
                result.map(|change| Box::new(crate::diff_prepare::prepare_change(change, syntax)));
            emit_cancellable(
                events,
                id,
                &cancel,
                Event::ChangePrepared {
                    path,
                    staged,
                    result,
                },
            );
        },
        VcsJob::PrepareTexts {
            id,
            path,
            old,
            new,
            syntax,
            cancel,
        } => {
            let file = Box::new(crate::diff_prepare::prepare_texts(
                path.clone(),
                &old,
                &new,
                false,
                syntax,
            ));
            emit_cancellable(
                events,
                id,
                &cancel,
                Event::DiffPrepared {
                    path,
                    result: Ok(file),
                },
            );
        },
        VcsJob::DiffWithRev {
            id,
            path,
            rev,
            live,
            syntax,
            cancel,
        } => {
            let result = run_diff_with_rev(&path, &rev, live, syntax);
            emit_cancellable(events, id, &cancel, Event::DiffPrepared { path, result });
        },
        VcsJob::Snapshot { id, cancel } => {
            match repository(root).and_then(|repo| snapshot(&repo)) {
                Ok(snapshot) => emit_cancellable(
                    events,
                    id,
                    &cancel,
                    Event::RepositorySnapshot {
                        snapshot: Box::new(snapshot),
                    },
                ),
                Err(message) => notify_cancellable(events, id, &cancel, message),
            }
        },
        VcsJob::NestedRepositoryStatus { id, path, cancel } => {
            if cancel.is_cancelled() {
                return;
            }
            let result = nested_repository(root, &path)
                .and_then(|repository| repository.summary().map_err(|error| error.to_string()));
            match result {
                Ok(summary) => emit_cancellable(
                    events,
                    id,
                    &cancel,
                    Event::NestedRepositoryStatus { path, summary },
                ),
                Err(message) => {
                    notify_cancellable(
                        events,
                        id,
                        &cancel,
                        format!("repository status: {message}"),
                    );
                },
            }
        },
        VcsJob::Action { id, action } => {
            let result = repository(root).and_then(|repo| {
                let outcome = execute(&repo, &action)?;
                let snapshot = snapshot(&repo)?;
                let status = compute_status(&repo);
                Ok((outcome, snapshot, status))
            });
            match result {
                Ok((outcome, snapshot, (staged, working))) => {
                    *last_status = Some((staged.clone(), working.clone()));
                    emit(events, id, Event::VcsStatus { staged, working });
                    emit(
                        events,
                        id,
                        Event::RepositorySnapshot {
                            snapshot: Box::new(snapshot),
                        },
                    );
                    emit(
                        events,
                        id,
                        Event::VcsOperationFinished {
                            action,
                            outcome: Some(outcome),
                            error: None,
                        },
                    );
                },
                Err(error) => emit(
                    events,
                    id,
                    Event::VcsOperationFinished {
                        action,
                        outcome: None,
                        error: Some(error),
                    },
                ),
            }
        },
        VcsJob::PullRequests {
            id,
            remote,
            page,
            per_page,
            cancel,
        } => match pull_requests(root, &remote, page, per_page) {
            Ok((items, next_page)) => emit_cancellable(
                events,
                id,
                &cancel,
                Event::PullRequests {
                    remote,
                    items,
                    next_page,
                },
            ),
            Err(message) => notify_cancellable(events, id, &cancel, message),
        },
        VcsJob::Blame {
            id,
            doc,
            version,
            path,
            text,
            line,
            cancel,
        } => match blame(blame_cache, root, doc, version, &path, &text, line) {
            Ok(attribution) => emit_cancellable(
                events,
                id,
                &cancel,
                Event::BlameResult {
                    doc,
                    version,
                    line,
                    attribution,
                },
            ),
            Err(message) => {
                notify_cancellable(events, id, &cancel, format!("blame: {message}"));
            },
        },
        VcsJob::RemoteFacts { id, path, cancel } => {
            let facts = remote_facts(&path);
            emit_cancellable(events, id, &cancel, Event::RemoteFacts { path, facts });
        },
        VcsJob::Log {
            id,
            skip,
            limit,
            cancel,
        } => run_log(root, events, id, skip, limit, &cancel),
        VcsJob::CommitDetail {
            id,
            rev,
            syntax,
            cancel,
        } => {
            run_commit_detail(root, events, id, &rev, syntax, &cancel);
        },
        VcsJob::RangeChanges {
            id,
            spec,
            syntax,
            cancel,
        } => {
            run_range_changes(root, events, id, &spec, syntax, &cancel);
        },
        VcsJob::FileHistory {
            id,
            path,
            skip,
            limit,
            cancel,
        } => run_file_history(root, events, id, path, skip, limit, &cancel),
        VcsJob::MergeConflict { id, path, cancel } => {
            run_merge_conflict(root, events, id, path, &cancel);
        },
    }
}

fn nested_repository(root: &Option<PathBuf>, path: &Path) -> Result<Repository, String> {
    let root = root
        .as_deref()
        .ok_or_else(|| "workspace has no root".to_string())?;
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("workspace root cannot be resolved: {error}"))?;
    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("nested repository cannot be resolved: {error}"))?;
    if path == root || !path.starts_with(&root) {
        return Err("repository is not nested inside the workspace root".to_string());
    }
    if !path.join(".git").exists() {
        return Err("nested repository no longer exists".to_string());
    }
    let repository = Repository::discover(&path).map_err(|error| error.to_string())?;
    if repository.worktree_root().as_deref() != Some(path.as_path()) {
        return Err("path is not the exact root of a nested repository".to_string());
    }
    Ok(repository)
}

fn repository(root: &Option<PathBuf>) -> Result<Repository, String> {
    let root = root
        .as_ref()
        .ok_or_else(|| "no workspace repository is open".to_string())?;
    Repository::discover(root).map_err(|error| error.to_string())
}

fn snapshot(repo: &Repository) -> Result<RepositorySnapshot, String> {
    Ok(RepositorySnapshot {
        state: repo.repository_state().map_err(|error| error.to_string())?,
        branches: repo.branches().map_err(|error| error.to_string())?,
        remotes: repo.remotes().map_err(|error| error.to_string())?,
        remote_branches: repo.remote_branches().map_err(|error| error.to_string())?,
        stashes: repo.stashes().map_err(|error| error.to_string())?,
    })
}

fn execute(repo: &Repository, action: &VcsAction) -> Result<VcsOutcome, String> {
    let result = match action {
        VcsAction::CreateBranch(options) => {
            repo.create_branch(options).map(|()| VcsOutcome::Completed)
        },
        VcsAction::SwitchBranch(target) => {
            repo.switch_branch(target).map(|()| VcsOutcome::Completed)
        },
        VcsAction::RenameBranch { old, new } => {
            repo.rename_branch(old, new).map(|()| VcsOutcome::Completed)
        },
        VcsAction::DeleteBranch { name } => {
            repo.delete_branch(name).map(|()| VcsOutcome::Completed)
        },
        VcsAction::PublishBranch {
            remote,
            branch,
            set_upstream,
        } => repo
            .publish_branch(remote, branch, *set_upstream)
            .map(|()| VcsOutcome::Completed),
        VcsAction::DeleteRemoteBranch { remote, branch } => repo
            .delete_remote_branch(remote, branch)
            .map(|()| VcsOutcome::Completed),
        VcsAction::UndoCommit { allow_upstream } => {
            repo.undo_commit(*allow_upstream)
                .map(|outcome| VcsOutcome::CommitUndone {
                    commit: outcome.commit,
                    was_upstream: outcome.was_upstream,
                })
        },
        VcsAction::StashPush(options) => repo.stash_push(options).map(VcsOutcome::StashCreated),
        VcsAction::StashPreview { reference } => {
            repo.stash_preview(reference)
                .map(|patch| VcsOutcome::StashPreview {
                    reference: reference.clone(),
                    patch,
                })
        },
        VcsAction::StashApply { reference } => {
            repo.stash_apply(reference).map(|()| VcsOutcome::Completed)
        },
        VcsAction::StashPop { reference } => {
            repo.stash_pop(reference).map(|()| VcsOutcome::Completed)
        },
        VcsAction::StashDrop { reference } => {
            repo.stash_drop(reference).map(|()| VcsOutcome::Completed)
        },
        VcsAction::StashBranch { name, reference } => repo
            .stash_branch(name, reference)
            .map(|()| VcsOutcome::Completed),
        VcsAction::TagCreate { name, rev, message } => repo
            .tag_create(name, rev, message.as_deref())
            .map(|()| VcsOutcome::Completed),
        VcsAction::TagDelete { name } => repo.tag_delete(name).map(|()| VcsOutcome::Completed),
        VcsAction::CherryPick { rev } => repo.cherry_pick(rev).map(|()| VcsOutcome::Completed),
        VcsAction::Revert { rev } => repo.revert(rev).map(|()| VcsOutcome::Completed),
        VcsAction::Rebase { rev } => repo.rebase_onto(rev).map(|()| VcsOutcome::Completed),
        VcsAction::RebaseInteractive { onto, steps } => repo
            .rebase_interactive(onto, steps)
            .map(|()| VcsOutcome::Completed),
        VcsAction::Reset { mode, rev } => repo.reset(*mode, rev).map(|()| VcsOutcome::Completed),
        VcsAction::CheckoutDetached { rev } => {
            repo.checkout_detached(rev).map(|()| VcsOutcome::Completed)
        },
        VcsAction::Fetch { remote } => repo.fetch(remote).map(|()| VcsOutcome::Completed),
        VcsAction::Sync => repo.sync().map(|outcome| match outcome {
            SyncOutcome::Synced => VcsOutcome::Completed,
            SyncOutcome::NeedsPublish => VcsOutcome::NeedsPublish,
            SyncOutcome::PullRequestUpdated => VcsOutcome::PullRequestUpdated,
            _ => VcsOutcome::Completed,
        }),
        VcsAction::Continue => repo.continue_operation().map(|()| VcsOutcome::Completed),
        VcsAction::Abort => repo.abort_operation().map(|()| VcsOutcome::Completed),
        VcsAction::Skip => repo.skip_operation().map(|()| VcsOutcome::Completed),
        VcsAction::CheckoutPullRequest { remote, number } => repo
            .checkout_github_pull_request(remote, *number)
            .map(|branch| VcsOutcome::PullRequestCheckedOut { branch }),
    };
    result.map_err(|error| error.to_string())
}

#[cfg(feature = "github")]
fn pull_requests(
    root: &Option<PathBuf>,
    remote_name: &str,
    page: u32,
    per_page: u8,
) -> Result<(Vec<PullRequestSummary>, Option<u32>), String> {
    let repo = repository(root)?;
    let remote = repo
        .remotes()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|remote| remote.name == remote_name)
        .ok_or_else(|| format!("unknown remote: {remote_name}"))?;
    let url = remote
        .url
        .ok_or_else(|| format!("remote {remote_name} has no fetch URL"))?;
    let (owner, name) = karet_github::parse_remote(&url)
        .ok_or_else(|| format!("remote {remote_name} is not hosted on GitHub"))?;
    let response = karet_github::open_pull_requests(&owner, &name, page, per_page)
        .map_err(|error| error.to_string())?;
    let items = response
        .items
        .into_iter()
        .map(|item| PullRequestSummary {
            number: item.number,
            title: item.title,
            author: item.author,
            draft: item.draft,
            head_ref: item.head_ref,
            head_repo: item.head_repo,
            head_sha: item.head_sha,
            base_ref: item.base_ref,
            base_repo: item.base_repo,
            url: item.url,
        })
        .collect();
    Ok((items, response.next_page))
}

#[cfg(not(feature = "github"))]
fn pull_requests(
    _root: &Option<PathBuf>,
    _remote_name: &str,
    _page: u32,
    _per_page: u8,
) -> Result<(Vec<PullRequestSummary>, Option<u32>), String> {
    Err("GitHub integration is disabled in this build".to_string())
}

fn emit(events: &UnboundedSender<(Option<RequestId>, Event)>, id: RequestId, event: Event) {
    let _ = events.send((Some(id), event));
}

fn emit_cancellable(
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    cancel: &Cancellation,
    event: Event,
) {
    if !cancel.is_cancelled() {
        emit(events, id, event);
    }
}

fn notify(events: &UnboundedSender<(Option<RequestId>, Event)>, id: RequestId, message: String) {
    emit(
        events,
        id,
        Event::Notification {
            severity: karet_core::Severity::Error,
            kind: karet_core::NotificationKind::Vcs,
            message,
        },
    );
}

fn notify_cancellable(
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    cancel: &Cancellation,
    message: String,
) {
    if !cancel.is_cancelled() {
        notify(events, id, message);
    }
}

/// Gather the repository/remote facts for `path`, discovering the repository
/// from the file's own directory (a file may live in a nested repository). The
/// `Err` side is a user-facing reason, doubling as a menu disabled-note.
fn remote_facts(path: &Path) -> Result<crate::api::RemoteFacts, String> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let start = abs.parent().unwrap_or(&abs);
    let repo = karet_vcs::Repository::discover(start)
        .map_err(|_| "not in a git repository".to_string())?;
    let origin_url = repo
        .origin_url()
        .ok_or_else(|| "no origin remote configured".to_string())?;
    let rel_path = repo
        .path_in_worktree(&abs)
        .ok_or_else(|| "file is outside the repository worktree".to_string())?;
    // An unborn branch has no HEAD hash; file_at_rev then errors, reading as
    // untracked — both surface as accurate notes downstream.
    let head = repo.head_hash().ok().flatten();
    let branch = repo.current_branch().ok().flatten();
    let tracked = repo.file_at_rev(&abs, "HEAD").ok().flatten().is_some();
    Ok(crate::api::RemoteFacts {
        origin_url,
        head,
        branch,
        rel_path,
        tracked,
    })
}

/// Read `path`'s content at `rev` via per-file repository discovery.
fn file_at_rev(path: &Path, rev: &str) -> Result<Option<String>, String> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let start = abs.parent().unwrap_or(&abs);
    let repo = karet_vcs::Repository::discover(start)
        .map_err(|_| "not in a git repository".to_string())?;
    let bytes = repo
        .file_at_rev(&abs, rev)
        .map_err(|error| error.to_string())?;
    // Bytes that are not UTF-8 read as absent: the caller renders such files as
    // binary changes rather than text diffs.
    Ok(bytes.and_then(|bytes| String::from_utf8(bytes).ok()))
}

#[cfg(test)]
mod tests {
    use karet_core::BlameAttribution;

    use super::*;

    fn init_repository(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(path)?;
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other("git init failed").into());
        }
        Ok(())
    }

    fn commit(hash: &str) -> BlameAttribution {
        BlameAttribution::Commit(BlameCommit {
            hash: hash.to_string(),
            author: "Ada".to_string(),
            author_time: 1_773_619_200,
        })
    }

    #[test]
    fn current_buffer_mapping_keeps_exact_and_unique_moved_lines() {
        let groups = vec![blameline::BlameGroup {
            lines: blameline::LineRange { start: 1, end: 3 },
            commit_hash: "one".to_string(),
            message: "change".to_string(),
            author: "Ada".to_string(),
            date: "1773619200 +0000".to_string(),
        }];
        let mapped = map_attribution(&["a", "new", "c", "b"], &["a", "b", "c"], &groups);
        assert_eq!(mapped[0], commit("one"));
        assert_eq!(mapped[1], BlameAttribution::Uncommitted);
        assert_eq!(mapped[2], commit("one"));
        assert_eq!(mapped[3], commit("one"));
    }

    #[test]
    fn ambiguous_moved_lines_are_uncommitted() {
        let mapped = map_attribution(&["x"], &["a", "x", "x"], &[]);
        assert_eq!(mapped, vec![BlameAttribution::Uncommitted]);
    }

    #[test]
    fn cancellation_hub_signals_live_job_and_forgets_finished_job() {
        let hub = crate::cancellation::CancellationHub::default();
        let token = hub.register(RequestId(41));
        assert!(!token.is_cancelled());
        hub.cancel(RequestId(41));
        assert!(token.is_cancelled());
        drop(token);

        // Cancelling after completion is a harmless no-op and does not poison a
        // later request that happens to use a different id.
        hub.cancel(RequestId(41));
        let next = hub.register(RequestId(42));
        assert!(!next.is_cancelled());
    }

    #[test]
    fn nested_repository_must_be_an_exact_child_worktree() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        init_repository(workspace.path())?;
        let nested = workspace.path().join("nested");
        init_repository(&nested)?;
        let ordinary_child = workspace.path().join("ordinary");
        std::fs::create_dir_all(&ordinary_child)?;
        let outside = tempfile::tempdir()?;
        init_repository(outside.path())?;
        let root = Some(workspace.path().to_path_buf());

        assert_eq!(
            nested_repository(&root, &nested)?
                .worktree_root()
                .as_deref(),
            std::fs::canonicalize(&nested).ok().as_deref()
        );
        assert!(nested_repository(&root, workspace.path()).is_err());
        assert!(nested_repository(&root, &ordinary_child).is_err());
        assert!(nested_repository(&root, outside.path()).is_err());
        Ok(())
    }
}

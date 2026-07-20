//! Ordered background execution for repository and forge operations.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;

use karet_core::BlameAttribution;
use karet_core::BlameCommit;
use karet_core::BlameHunk;
use karet_core::BlameLineRange;
use karet_core::BlameMode;
use karet_vcs::Repository;
use karet_vcs::Selection;
use karet_vcs::SyncOutcome;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::DocumentId;
use crate::api::Event;
use crate::api::PullRequestSummary;
use crate::api::RepositorySnapshot;
use crate::api::RequestId;
use crate::api::VcsAction;
use crate::api::VcsOutcome;

/// A unit of work sent by the session actor to its serialized VCS worker.
pub(crate) enum VcsJob {
    /// Load the current repository snapshot.
    Snapshot { id: RequestId },
    /// Run one repository action.
    Action { id: RequestId, action: VcsAction },
    /// Query open GitHub pull requests.
    PullRequests {
        id: RequestId,
        remote: String,
        page: u32,
        per_page: u8,
    },
    /// Attribute a current document buffer.
    Blame {
        id: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        text: String,
        line: u32,
        mode: BlameMode,
    },
}

/// Start the one-per-session ordered repository worker.
pub(crate) fn spawn(
    root: Option<PathBuf>,
    events: UnboundedSender<(Option<RequestId>, Event)>,
) -> mpsc::Sender<VcsJob> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            run(&root, &events, job);
        }
    });
    tx
}

fn run(root: &Option<PathBuf>, events: &UnboundedSender<(Option<RequestId>, Event)>, job: VcsJob) {
    match job {
        VcsJob::Snapshot { id } => match repository(root).and_then(|repo| snapshot(&repo)) {
            Ok(snapshot) => emit(
                events,
                id,
                Event::RepositorySnapshot {
                    snapshot: Box::new(snapshot),
                },
            ),
            Err(message) => notify(events, id, message),
        },
        VcsJob::Action { id, action } => {
            let result = repository(root).and_then(|repo| {
                let outcome = execute(&repo, &action)?;
                let snapshot = snapshot(&repo)?;
                let staged = repo
                    .changes(Selection::Staged, None)
                    .map_err(|error| error.to_string())?;
                let working = repo
                    .changes(Selection::Unstaged, None)
                    .map_err(|error| error.to_string())?;
                Ok((outcome, snapshot, (staged, working)))
            });
            match result {
                Ok((outcome, snapshot, (staged, working))) => {
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
        } => match pull_requests(root, &remote, page, per_page) {
            Ok((items, next_page)) => emit(
                events,
                id,
                Event::PullRequests {
                    remote,
                    items,
                    next_page,
                },
            ),
            Err(message) => notify(events, id, message),
        },
        VcsJob::Blame {
            id,
            doc,
            version,
            path,
            text,
            line,
            mode,
        } => match blame(root, &path, &text, line, mode) {
            Ok((scope, hunks)) => emit(
                events,
                id,
                Event::BlameResult {
                    doc,
                    version,
                    line,
                    mode,
                    scope,
                    hunks,
                },
            ),
            Err(message) => notify(events, id, format!("blame: {message}")),
        },
    }
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

fn blame(
    root: &Option<PathBuf>,
    path: &Path,
    text: &str,
    line: u32,
    mode: BlameMode,
) -> Result<(BlameLineRange, Vec<BlameHunk>), String> {
    let root = root
        .as_ref()
        .ok_or_else(|| "no workspace repository is open".to_string())?;
    let repo = Repository::discover(root).map_err(|error| error.to_string())?;
    let head = repo
        .file_at_rev(path, "HEAD")
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "file has no committed history yet".to_string())?;
    let head = String::from_utf8(head).map_err(|_| "committed file is not UTF-8".to_string())?;
    let groups = blameline::blame_file(root, path).map_err(|error| error.to_string())?;
    let current_lines: Vec<&str> = text.lines().collect();
    let head_lines: Vec<&str> = head.lines().collect();
    let last = current_lines.len().saturating_sub(1) as u32;
    let cursor = line.min(last);
    let scope = match mode {
        BlameMode::Line => BlameLineRange {
            start: cursor,
            end: cursor,
        },
        BlameMode::Semantic => blameline::enclosing_function_range(text, path, cursor)
            .map(|range| BlameLineRange {
                start: range.start.saturating_sub(1).min(last),
                end: range.end.saturating_sub(1).min(last),
            })
            .unwrap_or(BlameLineRange {
                start: cursor,
                end: cursor,
            }),
        _ => BlameLineRange {
            start: cursor,
            end: cursor,
        },
    };
    let attribution = map_attribution(&current_lines, &head_lines, &groups);
    Ok((scope, group_scope(&attribution, scope)))
}

fn map_attribution(
    current: &[&str],
    head: &[&str],
    groups: &[blameline::BlameGroup],
) -> Vec<BlameAttribution> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, content) in head.iter().enumerate() {
        positions.entry(content).or_default().push(index);
    }
    let mut by_head = vec![BlameAttribution::Uncommitted; head.len()];
    for group in groups {
        let commit = BlameCommit {
            hash: group.commit_hash.clone(),
            author: group.author.clone(),
            date: group.date.clone(),
            message: group.message.clone(),
        };
        let start = group.lines.start.saturating_sub(1) as usize;
        let end = (group.lines.end as usize).min(by_head.len());
        for item in by_head.iter_mut().take(end).skip(start) {
            *item = BlameAttribution::Commit(commit.clone());
        }
    }
    current
        .iter()
        .enumerate()
        .map(|(index, content)| {
            if head.get(index) == Some(content) {
                return by_head
                    .get(index)
                    .cloned()
                    .unwrap_or(BlameAttribution::Uncommitted);
            }
            match positions.get(content).map(Vec::as_slice) {
                Some([unique]) => by_head
                    .get(*unique)
                    .cloned()
                    .unwrap_or(BlameAttribution::Uncommitted),
                _ => BlameAttribution::Uncommitted,
            }
        })
        .collect()
}

fn group_scope(attribution: &[BlameAttribution], scope: BlameLineRange) -> Vec<BlameHunk> {
    let mut hunks: Vec<BlameHunk> = Vec::new();
    for line in scope.start..=scope.end {
        let item = attribution
            .get(line as usize)
            .cloned()
            .unwrap_or(BlameAttribution::Uncommitted);
        if let Some(last) = hunks.last_mut()
            && last.attribution == item
        {
            last.lines.end = line;
        } else {
            hunks.push(BlameHunk {
                lines: BlameLineRange {
                    start: line,
                    end: line,
                },
                attribution: item,
            });
        }
    }
    hunks
}

fn emit(events: &UnboundedSender<(Option<RequestId>, Event)>, id: RequestId, event: Event) {
    let _ = events.send((Some(id), event));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str) -> BlameAttribution {
        BlameAttribution::Commit(BlameCommit {
            hash: hash.to_string(),
            author: "Ada".to_string(),
            date: "today".to_string(),
            message: "change".to_string(),
        })
    }

    #[test]
    fn current_buffer_mapping_keeps_exact_and_unique_moved_lines() {
        let groups = vec![blameline::BlameGroup {
            lines: blameline::LineRange { start: 1, end: 3 },
            commit_hash: "one".to_string(),
            message: "change".to_string(),
            author: "Ada".to_string(),
            date: "today".to_string(),
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
    fn consecutive_attribution_is_grouped() {
        let attribution = vec![commit("a"), commit("a"), commit("b")];
        let hunks = group_scope(&attribution, BlameLineRange { start: 0, end: 2 });
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].lines, BlameLineRange { start: 0, end: 1 });
    }
}

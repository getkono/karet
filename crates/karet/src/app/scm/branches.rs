//! Branch and stash management: the pickers, forms, and prompts that choose a
//! branch, a stash, or a remote to act on.
//!
//! Split from the parent module because these share one shape — read the
//! repository snapshot, refuse with a reason when it cannot answer, otherwise
//! open the overlay that collects the choice.

use super::*;

impl App {
    /// Open a combined local/remote branch picker.
    pub(in crate::app) fn open_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "branches: loading repository state",
            );
            return;
        };
        let mut items = Vec::new();
        for branch in &snapshot.branches {
            let head = if branch.is_head { "✓ " } else { "  " };
            items.push((
                format!("{head}{}", branch.name),
                karet_vcs::BranchTarget::Local(branch.name.clone()),
            ));
        }
        for branch in &snapshot.remote_branches {
            let local_name = branch.name.clone();
            items.push((
                format!("  {}/{}", branch.remote, branch.name),
                karet_vcs::BranchTarget::Remote {
                    remote: branch.remote.clone(),
                    branch: branch.name.clone(),
                    local_name,
                },
            ));
        }
        self.overlay = Some(Overlay::branches(items));
    }

    /// Open the full branch-creation form with every configured remote available.
    pub(in crate::app) fn open_create_branch_form(&mut self) {
        let remotes = self
            .scm
            .repository
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .remotes
                    .iter()
                    .map(|remote| remote.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.overlay = Some(Overlay::create_branch(remotes));
    }

    /// Query open pull requests for the upstream-aware primary remote.
    pub(in crate::app) fn open_pull_request_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "pull requests: loading repository state",
            );
            return;
        };
        let preferred = snapshot
            .state
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/').map(|(remote, _)| remote));
        let remote = preferred
            .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
            })
            .or_else(|| snapshot.remotes.first())
            .map(|remote| remote.name.clone());
        let Some(remote) = remote else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "pull requests: no remote is configured",
            );
            return;
        };
        self.pull_request_items.clear();
        self.pull_request_remote = Some(remote.clone());
        self.pending_pull_requests = self.send(SessionCommand::PullRequests {
            remote: remote.clone(),
            page: 1,
            per_page: 100,
        });
        // Only once the request is actually out: nothing would ever answer — and
        // so retire the card — for a command a closed backend never took.
        if self.pending_pull_requests.is_some() {
            self.notify_progress(
                NotificationKind::Vcs,
                Self::PULL_REQUESTS_TAG.to_string(),
                format!("loading open pull requests from {remote}"),
                None,
            );
        }
    }

    /// Open stash creation controls.
    pub(in crate::app) fn open_stash_form(&mut self) {
        self.overlay = Some(Overlay::stash_form());
    }

    /// Open actions for every current stash entry.
    pub(in crate::app) fn open_stash_manager(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        if snapshot.stashes.is_empty() {
            self.notify(Report::Refusal, NotificationKind::Vcs, "stashes: none");
            return;
        }
        self.overlay = Some(Overlay::stashes(&snapshot.stashes));
    }

    /// Publish the current branch to its upstream remote, `origin`, or first remote.
    pub(in crate::app) fn publish_current_branch(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let Some(branch) = snapshot.state.branch.clone() else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "publish: HEAD is detached",
            );
            return;
        };
        let preferred = snapshot
            .state
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/').map(|(remote, _)| remote));
        let remote = preferred
            .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
            })
            .or_else(|| snapshot.remotes.first())
            .map(|remote| remote.name.clone());
        let Some(remote) = remote else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "publish: no remote is configured",
            );
            return;
        };
        self.run_vcs_action(VcsAction::PublishBranch {
            remote,
            branch,
            set_upstream: true,
        });
    }

    /// Prompt for a replacement name for the current local branch.
    pub(in crate::app) fn prompt_rename_current_branch(&mut self) {
        let current = self
            .scm
            .repository
            .as_ref()
            .and_then(|snapshot| snapshot.state.branch.clone());
        let Some(old) = current else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "rename branch: HEAD is detached",
            );
            return;
        };
        self.overlay = Some(Overlay::text(
            format!("Rename {old}"),
            TextPurpose::RenameBranch { old },
        ));
    }

    /// Pick a non-current local branch for safe (`git branch -d`) deletion.
    pub(in crate::app) fn open_delete_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let items: Vec<String> = snapshot
            .branches
            .iter()
            .filter(|branch| !branch.is_head)
            .map(|branch| branch.name.clone())
            .collect();
        if items.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "delete branch: no eligible local branches",
            );
        } else {
            self.overlay = Some(Overlay::delete_local_branches(items));
        }
    }

    /// Pick a non-default remote branch, then require its exact name as confirmation.
    pub(in crate::app) fn open_delete_remote_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let items: Vec<(String, String)> = snapshot
            .remote_branches
            .iter()
            .filter(|branch| !branch.is_default)
            .map(|branch| (branch.remote.clone(), branch.name.clone()))
            .collect();
        if items.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "delete remote branch: no eligible branches",
            );
        } else {
            self.overlay = Some(Overlay::delete_remote_branches(items));
        }
    }
}

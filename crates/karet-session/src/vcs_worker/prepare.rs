//! The status and diff-preparation jobs of the VCS worker.
//!
//! Split from `vcs_worker.rs` to keep it under the file-size ceiling; these are
//! the jobs that reduce repository changes to summaries or fully prepared,
//! displayable diffs (see [`crate::diff_prepare`]).

use super::*;

pub(super) type Status = (
    Vec<crate::api::ChangeSummary>,
    Vec<crate::api::ChangeSummary>,
);

/// Compute the current `(staged, working)` summaries. A read failure yields
/// empty sets rather than erroring, matching the previous actor-side behavior.
pub(super) fn compute_status(repo: &Repository) -> Status {
    let summaries = |selection| {
        repo.changes(selection, None)
            .unwrap_or_default()
            .iter()
            .map(crate::diff_prepare::summarize)
            .collect::<Vec<_>>()
    };
    (summaries(Selection::Staged), summaries(Selection::Unstaged))
}

pub(super) fn run_status(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    last_status: &mut Option<Status>,
    id: Option<RequestId>,
) {
    let Ok(repo) = repository(root) else {
        return;
    };
    let status = compute_status(&repo);
    if id.is_none() && last_status.as_ref() == Some(&status) {
        return;
    }
    let (staged, working) = status.clone();
    *last_status = Some(status);
    let _ = events.send((id, Event::VcsStatus { staged, working }));
}

/// Diff `path` at `rev` against its current content — `live` when the client
/// holds unsaved edits, the worktree file otherwise. A missing or non-text
/// revision side is an error; a non-text current side marks the diff binary.
pub(super) fn run_diff_with_rev(
    path: &Path,
    rev: &str,
    live: Option<String>,
    syntax: bool,
) -> Result<Box<crate::api::PreparedChange>, String> {
    let old = file_at_rev(path, rev)?
        .ok_or_else(|| format!("file does not exist at {rev} (or is not text)"))?;
    let new = live.or_else(|| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });
    // A non-text current side marks the change binary (both texts then empty),
    // matching the `FileChange::is_binary` contract.
    let prepared = match new {
        Some(new) => {
            crate::diff_prepare::prepare_texts(path.to_path_buf(), &old, &new, false, syntax)
        },
        None => crate::diff_prepare::prepare_texts(path.to_path_buf(), "", "", true, syntax),
    };
    Ok(Box::new(prepared))
}

/// Prepare each change for display, checking for cancellation between files
/// (highlighting a large commit is the most expensive job this worker runs).
/// `None` when the request was cancelled part-way.
pub(super) fn prepare_changes(
    changes: Vec<karet_vcs::FileChange>,
    syntax: bool,
    cancel: &Cancellation,
) -> Option<Vec<crate::api::PreparedChange>> {
    let mut prepared = Vec::with_capacity(changes.len());
    for change in changes {
        if cancel.is_cancelled() {
            return None;
        }
        prepared.push(crate::diff_prepare::prepare_change(change, syntax));
    }
    Some(prepared)
}

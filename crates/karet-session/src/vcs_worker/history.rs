//! The history-reading jobs of the VCS worker: log pages, commit detail,
//! range comparisons, and per-file history.
//!
//! Split from `vcs_worker.rs` to keep it under the file-size ceiling.

use super::*;

pub(super) fn run_log(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    skip: usize,
    limit: usize,
    cancel: &Cancellation,
) {
    if cancel.is_cancelled() {
        return;
    }
    let result = repository(root).and_then(|repo| {
        let mut commits = repo
            .log(skip, limit.saturating_add(1))
            .map_err(|error| error.to_string())?;
        let has_more = commits.len() > limit;
        commits.truncate(limit);
        // Decorations ride the page: a mutation that lands a new tag or moves
        // a branch is followed by a log refresh anyway.
        let labels = repo.ref_labels().unwrap_or_default();
        Ok((commits, has_more, labels))
    });
    match result {
        Ok((commits, has_more, labels)) => emit_cancellable(
            events,
            id,
            cancel,
            Event::VcsLog {
                skip,
                commits,
                has_more,
                labels,
            },
        ),
        Err(message) => notify_cancellable(events, id, cancel, message),
    }
}

pub(super) fn run_commit_detail(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    rev: &str,
    syntax: bool,
    cancel: &Cancellation,
) {
    if cancel.is_cancelled() {
        return;
    }
    let Ok(repo) = repository(root) else {
        return;
    };
    let detail = match repo.commit_detail(rev) {
        Ok(detail) => detail,
        Err(error) => {
            notify_cancellable(events, id, cancel, error.to_string());
            return;
        },
    };
    emit_cancellable(
        events,
        id,
        cancel,
        Event::CommitDetailReady {
            detail: Box::new(detail.clone()),
        },
    );
    if cancel.is_cancelled() {
        return;
    }
    match repo.commit_changes(rev) {
        Ok(changes) => {
            let Some(changes) = prepare_changes(changes, syntax, cancel) else {
                return;
            };
            emit_cancellable(
                events,
                id,
                cancel,
                Event::CommitReady {
                    detail: Box::new(detail),
                    changes,
                },
            );
        },
        Err(error) => notify_cancellable(events, id, cancel, error.to_string()),
    }
}

pub(super) fn run_range_changes(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    spec: &RangeSpec,
    syntax: bool,
    cancel: &Cancellation,
) {
    if cancel.is_cancelled() {
        return;
    }
    let outcome = repository(root).and_then(|repo| {
        let (base_rev, head_rev, merge_base, base_label, head_label) = match spec {
            RangeSpec::Unpushed => {
                let upstream = repo
                    .upstream_of_head()
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| {
                        "no upstream branch is set for the current branch".to_string()
                    })?;
                (
                    upstream.clone(),
                    "HEAD".to_string(),
                    true,
                    upstream,
                    "HEAD".to_string(),
                )
            },
            RangeSpec::SinceBase { base } => {
                let base = base
                    .clone()
                    .or_else(|| repo.default_base_branch())
                    .ok_or_else(|| {
                        "could not determine a base branch; use a range like main...HEAD"
                            .to_string()
                    })?;
                (
                    base.clone(),
                    "HEAD".to_string(),
                    true,
                    base,
                    "HEAD".to_string(),
                )
            },
            RangeSpec::Between {
                base,
                head,
                merge_base,
            } => (
                base.clone(),
                head.clone(),
                *merge_base,
                base.clone(),
                head.clone(),
            ),
        };
        let changes = repo
            .range_changes(&base_rev, &head_rev, merge_base)
            .map_err(|error| error.to_string())?;
        Ok((base_label, head_label, merge_base, changes))
    });
    match outcome {
        Ok((base_label, head_label, merge_base, changes)) => {
            let Some(changes) = prepare_changes(changes, syntax, cancel) else {
                return;
            };
            emit_cancellable(
                events,
                id,
                cancel,
                Event::RangeReady {
                    base_label,
                    head_label,
                    merge_base,
                    changes,
                },
            );
        },
        Err(message) => notify_cancellable(events, id, cancel, message),
    }
}

pub(super) fn run_file_history(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    path: PathBuf,
    skip: usize,
    limit: usize,
    cancel: &Cancellation,
) {
    if cancel.is_cancelled() {
        return;
    }
    let result = repository(root).and_then(|repo| {
        let mut commits = repo
            .file_history(&path, skip, limit.saturating_add(1))
            .map_err(|error| error.to_string())?;
        let has_more = commits.len() > limit;
        commits.truncate(limit);
        Ok((commits, has_more))
    });
    match result {
        Ok((commits, has_more)) => emit_cancellable(
            events,
            id,
            cancel,
            Event::FileHistory {
                path,
                skip,
                commits,
                has_more,
            },
        ),
        Err(message) => notify_cancellable(events, id, cancel, message),
    }
}

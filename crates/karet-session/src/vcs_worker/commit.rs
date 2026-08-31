//! The commit job of the VCS worker.
//!
//! Split from `vcs_worker.rs` to keep it under the file-size ceiling, and
//! because a commit is the one job here that owns a live child process.

use super::*;

/// Commit, streaming the hooks' output, then republish the repository's state.
///
/// Output is batched rather than emitted per line: a hook that prints thousands
/// of lines would otherwise cost thousands of events, and the client collapses
/// them into one frame anyway.
pub(super) fn run_commit(
    root: &Option<PathBuf>,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    last_status: &mut Option<Status>,
    id: RequestId,
    message: &str,
    cancel: &Cancellation,
) {
    /// Lines to gather before sending a batch.
    const BATCH: usize = 32;
    /// Longest a line waits for its batch to fill.
    const LINGER: std::time::Duration = std::time::Duration::from_millis(50);

    let repo = match repository(root) {
        Ok(repo) => repo,
        Err(error) => return notify(events, id, error),
    };
    // Cancelling this job means stopping a process tree, not dropping an answer.
    // Registering the token's action is what carries the request across the
    // thread boundary: the actor calls it, this thread is blocked inside git.
    let stop = karet_vcs::CommitCancel::new();
    cancel.on_cancel({
        let stop = stop.clone();
        move || stop.cancel()
    });

    let mut batch: Vec<karet_vcs::CommitOutputLine> = Vec::new();
    let mut sent_at = std::time::Instant::now();
    let result = repo.commit_with_output(message, &stop, &mut |line| {
        batch.push(line);
        if batch.len() >= BATCH || sent_at.elapsed() >= LINGER {
            emit(
                events,
                id,
                Event::CommitOutput {
                    lines: std::mem::take(&mut batch),
                },
            );
            sent_at = std::time::Instant::now();
        }
    });
    if !batch.is_empty() {
        emit(events, id, Event::CommitOutput { lines: batch });
    }
    match result {
        Ok(karet_vcs::CommitOutcome::Created(oid)) => emit(events, id, Event::Committed { oid }),
        // No commit, but the working tree may still have moved: a formatter hook
        // that rewrote files before it was killed leaves those edits behind, and
        // a stale status panel would hide them.
        Ok(karet_vcs::CommitOutcome::Cancelled) => emit(events, id, Event::CommitCancelled),
        Err(error) => return notify(events, id, error.to_string()),
    }
    // The same republication `VcsJob::Action` does, including updating the cached
    // status: without it the next spontaneous refresh re-emits what just changed.
    if let Ok(snapshot) = snapshot(&repo) {
        emit(
            events,
            id,
            Event::RepositorySnapshot {
                snapshot: Box::new(snapshot),
            },
        );
    }
    let (staged, working) = compute_status(&repo);
    *last_status = Some((staged.clone(), working.clone()));
    emit(events, id, Event::VcsStatus { staged, working });
}

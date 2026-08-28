//! The workspace-search worker: a dedicated thread for the gitignore-aware walk.
//!
//! Searching a large tree is blocking, filesystem-bound work that must never run
//! on the actor (or a UI) thread. Results **stream** as the walk finds them, so a
//! huge repository fills the panel progressively instead of staying blank until
//! the last file is read, and the walk **stops** once the caller's cap is reached
//! rather than reading the rest of the tree for results nobody will see.
//!
//! Jobs are coalesced to the newest request — a fresh query supersedes an
//! unstarted stale one, matching how a user types — and answers ride the ordinary
//! [`Event`] stream tagged with the request id. A superseded job still gets its
//! terminal event, so no request id is ever left unanswered.

mod preview;

use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_search::Matcher;
use karet_search::SearchQuery;
use karet_search::WorkspaceSearch;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::api::SearchHit;
use crate::cancellation::Cancellation;

/// Flush a batch after this many files, so a long stretch of non-matching files
/// still advances the panel's scanned count.
const FILES_PER_BATCH: usize = 200;
/// Flush a batch once it holds this many matching files.
const HITS_PER_BATCH: usize = 64;

/// One unit of background search work.
pub(crate) enum SearchJob {
    /// Run the workspace search, answering with a run of [`Event::SearchProgress`]
    /// batches and exactly one [`Event::SearchFinished`].
    Search {
        /// Correlates the answering events.
        id: RequestId,
        /// Every workspace root, walked in order.
        roots: Vec<PathBuf>,
        /// The query and options.
        query: SearchQuery,
        /// Stop the walk once this many files have matched.
        file_limit: usize,
        /// Stop the walk once this many matches have been found in total.
        match_limit: usize,
        /// Cooperative cancellation, checked between files.
        cancel: Cancellation,
    },
    /// Replace across the workspace and answer with [`Event::SearchReplaced`].
    ReplaceAll {
        /// Correlates the answering event.
        id: RequestId,
        /// Every workspace root to rewrite in.
        roots: Vec<PathBuf>,
        /// The query selecting the text to replace.
        query: SearchQuery,
        /// The replacement text.
        replacement: String,
    },
}

impl SearchJob {
    /// The request this job answers.
    fn id(&self) -> RequestId {
        match self {
            Self::Search { id, .. } | Self::ReplaceAll { id, .. } => *id,
        }
    }
}

/// Start the worker; the session sends [`SearchJob`]s and answers arrive on the
/// shared event stream.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<SearchJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-search".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(
    jobs: &Receiver<SearchJob>,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) {
    while let Ok(mut job) = jobs.recv() {
        // Coalesce: only the newest queued search matters (a replace is never
        // skipped — it mutates files).
        while let Ok(next) = jobs.try_recv() {
            match (&job, &next) {
                (SearchJob::Search { .. }, SearchJob::Search { .. }) => {
                    // The dropped job still owes its request an answer; without
                    // this the client's in-flight id never resolves and the panel
                    // sits on "searching…" forever.
                    if superseded(job.id(), events).is_break() {
                        return;
                    }
                    job = next;
                },
                _ => {
                    if execute(job, events).is_break() {
                        return;
                    }
                    job = next;
                },
            }
        }
        if execute(job, events).is_break() {
            return;
        }
    }
}

/// Answer a search that a newer one displaced before it started.
fn superseded(
    id: RequestId,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    send(
        events,
        id,
        Event::SearchFinished {
            files_scanned: 0,
            matches_found: 0,
            truncated: false,
            cancelled: true,
            error: None,
        },
    )
}

/// Everything one search accumulates while walking.
struct ScanState {
    hits: Vec<SearchHit>,
    files_scanned: usize,
    files_since_flush: usize,
    files_matched: usize,
    matches_found: usize,
    truncated: bool,
    cancelled: bool,
}

fn execute(
    job: SearchJob,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    match job {
        SearchJob::Search {
            id,
            roots,
            query,
            file_limit,
            match_limit,
            cancel,
        } => search(id, &roots, &query, file_limit, match_limit, &cancel, events),
        SearchJob::ReplaceAll {
            id,
            roots,
            query,
            replacement,
        } => {
            let mut files_changed = 0;
            let mut replacements = 0;
            for root in &roots {
                let summary = WorkspaceSearch::new()
                    .replace(root, &query, &replacement)
                    .unwrap_or_default();
                files_changed += summary.files_changed;
                replacements += summary.replacements;
            }
            send(
                events,
                id,
                Event::SearchReplaced {
                    files_changed,
                    replacements,
                },
            )
        },
    }
}

/// Walk every root, streaming batches, and answer with one terminal event.
fn search(
    id: RequestId,
    roots: &[PathBuf],
    query: &SearchQuery,
    file_limit: usize,
    match_limit: usize,
    cancel: &Cancellation,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    // Compile once for the whole tree rather than per file, and surface a bad
    // pattern instead of letting it read as "no matches".
    let matcher = match Matcher::new(query) {
        Ok(matcher) => matcher,
        Err(error) => {
            return send(
                events,
                id,
                Event::SearchFinished {
                    files_scanned: 0,
                    matches_found: 0,
                    truncated: false,
                    cancelled: false,
                    error: Some(error.to_string()),
                },
            );
        },
    };
    let mut state = ScanState {
        hits: Vec::new(),
        files_scanned: 0,
        files_since_flush: 0,
        files_matched: 0,
        matches_found: 0,
        truncated: false,
        cancelled: false,
    };
    let mut disconnected = false;
    let mut walk_error = None;
    // One `ScanState` across every root, so the caps, the batching and
    // cancellation stay global to the search rather than resetting per root.
    for root in roots {
        let outcome =
            karet_search::walk_text_files(root, &query.includes, &query.excludes, |path, text| {
                // Checked on every visited file, matching or not, so cancelling
                // over a large non-matching tree does not wait out the walk.
                if cancel.is_cancelled() {
                    state.cancelled = true;
                    return ControlFlow::Break(());
                }
                state.files_scanned += 1;
                state.files_since_flush += 1;
                let matches = matcher.find(&text);
                if !matches.is_empty() {
                    // Build previews only up to the remaining budget. Checking the
                    // cap after converting a whole file would let one 10 MiB
                    // minified line allocate a preview `String` per match — the
                    // very case the match cap exists to bound. The *count* still
                    // reflects everything found, so the total stays honest.
                    let room = match_limit.saturating_sub(state.matches_found);
                    let kept: Vec<_> = matches
                        .iter()
                        .take(room)
                        .map(|m| preview::search_match(&text, m))
                        .collect();
                    state.matches_found += matches.len();
                    if !kept.is_empty() {
                        state.files_matched += 1;
                        state.hits.push(SearchHit {
                            path: path.to_path_buf(),
                            matches: kept,
                        });
                    }
                }
                if state.files_matched >= file_limit || state.matches_found >= match_limit {
                    state.truncated = true;
                    return ControlFlow::Break(());
                }
                if state.hits.len() >= HITS_PER_BATCH || state.files_since_flush >= FILES_PER_BATCH
                {
                    state.files_since_flush = 0;
                    if flush(&mut state, id, events).is_break() {
                        disconnected = true;
                        return ControlFlow::Break(());
                    }
                }
                ControlFlow::Continue(())
            });
        if let Err(error) = outcome {
            walk_error = Some(error.to_string());
            break;
        }
        if disconnected || state.cancelled || state.truncated {
            break;
        }
    }
    if disconnected {
        return ControlFlow::Break(());
    }
    if !state.hits.is_empty() {
        flush(&mut state, id, events)?;
    }
    send(
        events,
        id,
        Event::SearchFinished {
            files_scanned: state.files_scanned,
            matches_found: state.matches_found,
            truncated: state.truncated,
            cancelled: state.cancelled,
            error: walk_error,
        },
    )
}

/// Send the pending batch, leaving `state.hits` empty.
fn flush(
    state: &mut ScanState,
    id: RequestId,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    send(
        events,
        id,
        Event::SearchProgress {
            hits: std::mem::take(&mut state.hits),
            files_scanned: state.files_scanned,
            matches_found: state.matches_found,
        },
    )
}

fn send(
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    event: Event,
) -> ControlFlow<()> {
    if events.send((Some(id), event)).is_err() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

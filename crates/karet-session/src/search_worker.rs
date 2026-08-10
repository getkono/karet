//! The workspace-search worker: a dedicated thread for the gitignore-aware walk.
//!
//! Searching a large tree is blocking, filesystem-bound work that must never run
//! on the actor (or a UI) thread. Jobs are coalesced to the newest request —
//! a fresh query supersedes an unstarted stale one, matching how a user types —
//! and answers ride the ordinary [`Event`] stream tagged with the request id.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_search::SearchQuery;
use karet_search::WorkspaceSearch;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;

/// One unit of background search work.
pub(crate) enum SearchJob {
    /// Run the workspace search and answer with [`Event::SearchResults`].
    Search {
        /// Correlates the answering event.
        id: RequestId,
        /// The workspace root to walk.
        root: PathBuf,
        /// The query and options.
        query: SearchQuery,
        /// Keep at most this many file hits.
        limit: usize,
    },
    /// Replace across the workspace and answer with [`Event::SearchReplaced`].
    ReplaceAll {
        /// Correlates the answering event.
        id: RequestId,
        /// The workspace root to walk.
        root: PathBuf,
        /// The query selecting the text to replace.
        query: SearchQuery,
        /// The replacement text.
        replacement: String,
    },
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
                (SearchJob::Search { .. }, SearchJob::Search { .. }) => job = next,
                _ => {
                    execute(job, events);
                    job = next;
                },
            }
        }
        execute(job, events);
    }
}

fn execute(job: SearchJob, events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>) {
    match job {
        SearchJob::Search {
            id,
            root,
            query,
            limit,
        } => {
            let mut hits = Vec::new();
            let _ = WorkspaceSearch::new().run(&root, &query, |hit| {
                if hits.len() < limit {
                    hits.push(hit);
                }
            });
            let _ = events.send((Some(id), Event::SearchResults { hits }));
        },
        SearchJob::ReplaceAll {
            id,
            root,
            query,
            replacement,
        } => {
            let summary = WorkspaceSearch::new()
                .replace(&root, &query, &replacement)
                .unwrap_or_default();
            let _ = events.send((
                Some(id),
                Event::SearchReplaced {
                    files_changed: summary.files_changed,
                    replacements: summary.replacements,
                },
            ));
        },
    }
}

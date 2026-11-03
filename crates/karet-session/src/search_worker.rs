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
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_search::Matcher;
use karet_search::SearchQuery;
use karet_search::replace_in_workspace;

use crate::api::Event;
use crate::api::RequestId;
use crate::api::SearchHit;
use crate::cancellation::Cancellation;
use crate::event_tx::EventTx;
use crate::scan_walk::BatchedScan;
use crate::scan_walk::Walk;

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
pub(crate) fn spawn(events: EventTx) -> Sender<SearchJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-search".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(jobs: &Receiver<SearchJob>, events: &EventTx) {
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
fn superseded(id: RequestId, events: &EventTx) -> ControlFlow<()> {
    events.try_emit(
        Some(id),
        Event::SearchFinished {
            files_scanned: 0,
            matches_found: 0,
            truncated: false,
            cancelled: true,
            error: None,
        },
    )
}

/// One search's matcher, caps and running totals, visited by the shared walk.
struct SearchScan<'a> {
    matcher: &'a Matcher,
    file_limit: usize,
    match_limit: usize,
    files_matched: usize,
    matches_found: usize,
}

impl BatchedScan for SearchScan<'_> {
    type Hit = SearchHit;

    fn visit(&mut self, path: &Path, text: &str, hits: &mut Vec<SearchHit>) -> ControlFlow<()> {
        let matches = self.matcher.find(text);
        if !matches.is_empty() {
            // Build previews only up to the remaining budget. Checking the cap
            // after converting a whole file would let one 10 MiB minified line
            // allocate a preview `String` per match — the very case the match
            // cap exists to bound. The *count* still reflects everything found,
            // so the total stays honest.
            let room = self.match_limit.saturating_sub(self.matches_found);
            let kept: Vec<_> = matches
                .iter()
                .take(room)
                .map(|m| preview::search_match(text, m))
                .collect();
            self.matches_found += matches.len();
            if !kept.is_empty() {
                self.files_matched += 1;
                hits.push(SearchHit {
                    path: path.to_path_buf(),
                    matches: kept,
                });
            }
        }
        if self.files_matched >= self.file_limit || self.matches_found >= self.match_limit {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    fn progress(&self, hits: Vec<SearchHit>, files_scanned: usize) -> Option<Event> {
        Some(Event::SearchProgress {
            hits,
            files_scanned,
            matches_found: self.matches_found,
        })
    }
}

fn execute(job: SearchJob, events: &EventTx) -> ControlFlow<()> {
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
        } => events.try_emit(Some(id), replace_all(&roots, &query, &replacement)),
    }
}

/// Replace across every root and fold the per-root summaries into one answer.
///
/// An error (a bad pattern or glob) is the same for every root, so it stops the
/// loop and is reported rather than read as "0 replacements"; whatever earlier
/// roots already wrote is still counted.
fn replace_all(roots: &[PathBuf], query: &SearchQuery, replacement: &str) -> Event {
    let mut files_changed = 0;
    let mut replacements = 0;
    let mut failures = Vec::new();
    let mut error = None;
    for root in roots {
        match replace_in_workspace(root, query, replacement) {
            Ok(summary) => {
                files_changed += summary.files_changed;
                replacements += summary.replacements;
                failures.extend(summary.failures);
            },
            Err(e) => {
                error = Some(e.to_string());
                break;
            },
        }
    }
    Event::SearchReplaced {
        files_changed,
        replacements,
        failures,
        error,
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
    events: &EventTx,
) -> ControlFlow<()> {
    // Compile once for the whole tree rather than per file, and surface a bad
    // pattern instead of letting it read as "no matches".
    let matcher = match Matcher::new(query) {
        Ok(matcher) => matcher,
        Err(error) => {
            return events.try_emit(
                Some(id),
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
    let mut scan = SearchScan {
        matcher: &matcher,
        file_limit,
        match_limit,
        files_matched: 0,
        matches_found: 0,
    };
    let walk = Walk {
        roots,
        includes: &query.includes,
        excludes: &query.excludes,
        cancel,
    };
    let walked = crate::scan_walk::walk_batched(&walk, &mut scan, id, events, Vec::new())?;
    events.try_emit(
        Some(id),
        Event::SearchFinished {
            files_scanned: walked.files_scanned,
            matches_found: scan.matches_found,
            truncated: walked.truncated,
            cancelled: walked.cancelled,
            error: walked.error,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationHub;

    type Events = tokio::sync::mpsc::UnboundedReceiver<(Option<RequestId>, Event)>;

    fn query(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_owned(),
            case_sensitive: true,
            ..SearchQuery::default()
        }
    }

    fn search_job(
        hub: &CancellationHub,
        id: u64,
        root: &std::path::Path,
        pattern: &str,
    ) -> SearchJob {
        SearchJob::Search {
            id: RequestId(id),
            roots: vec![root.to_path_buf()],
            query: query(pattern),
            file_limit: 100,
            match_limit: 100,
            cancel: hub.register(RequestId(id)),
        }
    }

    /// Queue every job before the worker starts, so they coalesce exactly as a
    /// burst would, then run the worker to completion.
    fn run_all(jobs: Vec<SearchJob>) -> Events {
        let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
        for job in jobs {
            let _ = jobs_tx.send(job);
        }
        drop(jobs_tx);
        let (events_tx, events_rx) = crate::event_tx::EventTx::channel();
        run(&jobs_rx, &events_tx);
        events_rx
    }

    /// `(request, files_scanned, matches_found, truncated, cancelled)` per
    /// terminal search event, in emission order.
    fn finishes(events: &mut Events) -> Vec<(u64, usize, usize, bool, bool)> {
        let mut out = Vec::new();
        while let Ok((id, event)) = events.try_recv() {
            if let Event::SearchFinished {
                files_scanned,
                matches_found,
                truncated,
                cancelled,
                ..
            } = event
            {
                out.push((
                    id.map_or(0, |id| id.0),
                    files_scanned,
                    matches_found,
                    truncated,
                    cancelled,
                ));
            }
        }
        out
    }

    /// A missing or wrong answer is an `Err`, so a test fails instead of passing.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn tree(files: &[(&str, &str)]) -> std::io::Result<tempfile::TempDir> {
        let dir = tempfile::tempdir()?;
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body)?;
        }
        Ok(dir)
    }

    #[test]
    fn a_queued_search_is_superseded_by_a_newer_one_and_still_answered() -> TestResult {
        let dir = tree(&[("a.txt", "needle\n")])?;
        let hub = CancellationHub::default();
        let mut events = run_all(vec![
            search_job(&hub, 1, dir.path(), "needle"),
            search_job(&hub, 2, dir.path(), "needle"),
        ]);
        assert_eq!(
            finishes(&mut events),
            vec![(1, 0, 0, false, true), (2, 1, 1, false, false)],
            "the stale search is answered as cancelled; only the newest walks"
        );
        Ok(())
    }

    #[test]
    fn a_replace_is_never_coalesced_away() -> TestResult {
        let dir = tree(&[("a.txt", "old old\n")])?;
        let hub = CancellationHub::default();
        let mut events = run_all(vec![
            search_job(&hub, 1, dir.path(), "old"),
            SearchJob::ReplaceAll {
                id: RequestId(2),
                roots: vec![dir.path().to_path_buf()],
                query: query("old"),
                replacement: "new".to_owned(),
            },
            search_job(&hub, 3, dir.path(), "new"),
        ]);
        let mut order = Vec::new();
        while let Ok((id, event)) = events.try_recv() {
            match event {
                Event::SearchReplaced {
                    files_changed,
                    replacements,
                    ..
                } => order.push((id.map_or(0, |id| id.0), files_changed, replacements)),
                Event::SearchFinished {
                    matches_found,
                    cancelled,
                    ..
                } => order.push((
                    id.map_or(0, |id| id.0),
                    usize::from(cancelled),
                    matches_found,
                )),
                _ => {},
            }
        }
        // The first search ran before the replace (not superseded), the replace
        // rewrote both matches, and the last search sees the new text.
        assert_eq!(order, vec![(1, 0, 2), (2, 1, 2), (3, 0, 2)]);
        let text = std::fs::read_to_string(dir.path().join("a.txt"))?;
        assert_eq!(text, "new new\n");
        Ok(())
    }

    /// A replace that could not run reports why instead of answering
    /// "0 replacements", and it stops at the first root rather than repeating
    /// the same error for each.
    #[test]
    fn an_invalid_replace_pattern_is_reported_not_read_as_zero() -> TestResult {
        let dir = tree(&[("a.txt", "needle\n")])?;
        let bad = SearchQuery {
            regex: true,
            ..query("(")
        };
        let roots = vec![dir.path().to_path_buf(), dir.path().to_path_buf()];
        let Event::SearchReplaced {
            files_changed,
            failures,
            error,
            ..
        } = replace_all(&roots, &bad, "pin")
        else {
            return Err("not a SearchReplaced".into());
        };
        assert_eq!((files_changed, failures.len()), (0, 0));
        assert_eq!(error, Some("invalid search pattern".to_owned()));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt"))?,
            "needle\n"
        );
        Ok(())
    }

    /// Files that could not be written back are carried in the answer from every
    /// root, beside the counts of the files that were.
    #[test]
    fn replace_failures_from_every_root_reach_the_answer() -> TestResult {
        let (one, two) = (
            tree(&[("a.txt", "needle\n")])?,
            tree(&[("b.txt", "needle\n")])?,
        );
        let locked = two.path().join("b.txt");
        let mut perms = std::fs::metadata(&locked)?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms)?;
        if std::fs::OpenOptions::new()
            .write(true)
            .open(&locked)
            .is_ok()
        {
            // A privileged user writes through the read-only bit; nothing to observe.
            return Ok(());
        }
        let roots = vec![one.path().to_path_buf(), two.path().to_path_buf()];
        let Event::SearchReplaced {
            files_changed,
            replacements,
            failures,
            error,
        } = replace_all(&roots, &query("needle"), "pin")
        else {
            return Err("not a SearchReplaced".into());
        };
        assert_eq!((files_changed, replacements, error), (1, 1, None));
        let failed: Vec<PathBuf> = failures.into_iter().map(|f| f.path).collect();
        assert_eq!(failed, vec![locked]);
        Ok(())
    }

    #[test]
    fn a_cancelled_search_finishes_as_cancelled_without_walking() -> TestResult {
        let dir = tree(&[("a.txt", "needle\n"), ("b.txt", "needle\n")])?;
        let hub = CancellationHub::default();
        let job = search_job(&hub, 1, dir.path(), "needle");
        hub.cancel(RequestId(1));
        let mut events = run_all(vec![job]);
        assert_eq!(finishes(&mut events), vec![(1, 0, 0, false, true)]);
        Ok(())
    }

    #[test]
    fn the_file_limit_truncates_the_walk() -> TestResult {
        let dir = tree(&[
            ("a.txt", "needle\n"),
            ("b.txt", "needle\n"),
            ("c.txt", "needle\n"),
        ])?;
        let hub = CancellationHub::default();
        let job = SearchJob::Search {
            id: RequestId(1),
            roots: vec![dir.path().to_path_buf()],
            query: query("needle"),
            file_limit: 2,
            match_limit: 100,
            cancel: hub.register(RequestId(1)),
        };
        let mut events = run_all(vec![job]);
        assert_eq!(finishes(&mut events), vec![(1, 2, 2, true, false)]);
        Ok(())
    }

    #[test]
    fn an_invalid_pattern_is_reported_not_read_as_no_matches() -> TestResult {
        let dir = tempfile::tempdir()?;
        let hub = CancellationHub::default();
        let job = SearchJob::Search {
            id: RequestId(1),
            roots: vec![dir.path().to_path_buf()],
            query: SearchQuery {
                regex: true,
                ..query("(")
            },
            file_limit: 10,
            match_limit: 10,
            cancel: hub.register(RequestId(1)),
        };
        let mut events = run_all(vec![job]);
        let error =
            std::iter::from_fn(|| events.try_recv().ok()).find_map(|(_, event)| match event {
                Event::SearchFinished { error, .. } => error,
                _ => None,
            });
        assert!(error.is_some(), "a bad regex must surface as an error");
        Ok(())
    }
}

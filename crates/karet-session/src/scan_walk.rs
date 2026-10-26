//! The streamed, batched workspace walk shared by the search, spelling and
//! codetag workers ([`crate::search_worker`], [`crate::spell_scan`],
//! [`crate::todo_scan`]).
//!
//! Each worker walks every root with [`karet_search::walk_text_files`] and
//! differs only in what it finds in a file and how it reports a batch. The walk
//! itself is one shape: check cancellation on every visited file, count it,
//! stop once the scan's cap is reached, and flush a progress batch every
//! [`FILES_PER_BATCH`] files or [`HITS_PER_BATCH`] hits, so a long walk over
//! clean files still advances the panel and a dirty tree fills it steadily. One
//! set of counters spans every root, so the caps, the batching and
//! cancellation stay global to the scan rather than resetting per root.

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;

use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::cancellation::Cancellation;

/// Flush a batch after this many files, so a long stretch of files with nothing
/// to report still advances the panel's scanned count.
pub(crate) const FILES_PER_BATCH: usize = 200;
/// Flush a batch once it holds this many hits.
pub(crate) const HITS_PER_BATCH: usize = 64;

/// The shared event stream every worker answers on.
pub(crate) type Events = tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>;

/// Send one event tagged with `id`, mapping a closed stream (the session is
/// gone) to [`ControlFlow::Break`].
pub(crate) fn send(events: &Events, id: RequestId, event: Event) -> ControlFlow<()> {
    if events.send((Some(id), event)).is_err() {
        ControlFlow::Break(())
    } else {
        ControlFlow::Continue(())
    }
}

/// The comparable form of `path`, for matching a walked file against the open
/// documents a scan must not re-read from disk.
///
/// Falls back to the path as given when it cannot be resolved (it was deleted
/// under us, or permissions bite): an unresolvable path is no worse a key than
/// the raw one, and both sides run through here.
pub(crate) fn resolve_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What one worker does with the files a [`walk_batched`] visits.
pub(crate) trait BatchedScan {
    /// One reported finding.
    type Hit;

    /// Examine one walked file, appending its findings to `hits`. Returns
    /// [`ControlFlow::Break`] once the scan's cap is reached, which ends the
    /// walk as truncated.
    fn visit(&mut self, path: &Path, text: &str, hits: &mut Vec<Self::Hit>) -> ControlFlow<()>;

    /// The progress event carrying one batch, or `None` to send nothing.
    fn progress(&self, hits: Vec<Self::Hit>, files_scanned: usize) -> Option<Event>;
}

/// How a [`walk_batched`] ended, for the worker's terminal event.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    /// Every file visited, reported or not.
    pub files_scanned: usize,
    /// The scan's cap stopped the walk.
    pub truncated: bool,
    /// Cancellation stopped the walk.
    pub cancelled: bool,
    /// The walk could not start (a bad include or exclude glob); no later root
    /// is walked, since every root would fail the same way.
    pub error: Option<String>,
}

/// Which files a [`walk_batched`] visits, and when it gives up.
pub(crate) struct Walk<'a> {
    /// The workspace roots, walked in order.
    pub roots: &'a [PathBuf],
    /// Include globs; empty walks everything.
    pub includes: &'a [String],
    /// Exclude globs.
    pub excludes: &'a [String],
    /// Cooperative cancellation, checked on every visited file.
    pub cancel: &'a Cancellation,
}

/// Walk every root in order, streaming batches of `scan`'s findings tagged with
/// `id`, and flush whatever is left at the end.
///
/// `hits` seeds the first batch (findings the caller made before walking).
/// Returns [`ControlFlow::Break`] when the event stream closed mid-walk; the
/// caller then owes no terminal event.
pub(crate) fn walk_batched<S: BatchedScan>(
    walk: &Walk<'_>,
    scan: &mut S,
    id: RequestId,
    events: &Events,
    mut hits: Vec<S::Hit>,
) -> ControlFlow<(), Walked> {
    let mut walked = Walked::default();
    let mut files_since_flush = 0;
    let mut disconnected = false;
    for root in walk.roots {
        let outcome =
            karet_search::walk_text_files(root, walk.includes, walk.excludes, |path, text| {
                // Checked on every visited file, matching or not, so cancelling over
                // a large tree with nothing to report does not wait out the walk.
                if walk.cancel.is_cancelled() {
                    walked.cancelled = true;
                    return ControlFlow::Break(());
                }
                walked.files_scanned += 1;
                files_since_flush += 1;
                if scan.visit(path, &text, &mut hits).is_break() {
                    walked.truncated = true;
                    return ControlFlow::Break(());
                }
                if hits.len() >= HITS_PER_BATCH || files_since_flush >= FILES_PER_BATCH {
                    files_since_flush = 0;
                    let batch = std::mem::take(&mut hits);
                    if let Some(event) = scan.progress(batch, walked.files_scanned)
                        && send(events, id, event).is_break()
                    {
                        disconnected = true;
                        return ControlFlow::Break(());
                    }
                }
                ControlFlow::Continue(())
            });
        if let Err(error) = outcome {
            walked.error = Some(error.to_string());
            break;
        }
        if disconnected || walked.cancelled || walked.truncated {
            break;
        }
    }
    if disconnected {
        return ControlFlow::Break(());
    }
    if !hits.is_empty()
        && let Some(event) = scan.progress(hits, walked.files_scanned)
    {
        send(events, id, event)?;
    }
    ControlFlow::Continue(walked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::TodoHit;
    use crate::cancellation::CancellationHub;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const ID: RequestId = RequestId(7);

    /// Reports every file whose text contains `hit`, stopping at `cap`.
    struct Probe {
        cap: usize,
        total: usize,
        report_empty: bool,
    }

    impl Probe {
        fn new(cap: usize, report_empty: bool) -> Self {
            Self {
                cap,
                total: 0,
                report_empty,
            }
        }
    }

    fn hit(path: &Path) -> TodoHit {
        TodoHit {
            path: path.to_path_buf(),
            line: 0,
            tag: "T".to_owned(),
            message: String::new(),
        }
    }

    impl BatchedScan for Probe {
        type Hit = TodoHit;

        fn visit(&mut self, path: &Path, text: &str, hits: &mut Vec<TodoHit>) -> ControlFlow<()> {
            if text.contains("hit") {
                hits.push(hit(path));
                self.total += 1;
            }
            if self.total >= self.cap {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        }

        fn progress(&self, hits: Vec<TodoHit>, files_scanned: usize) -> Option<Event> {
            (self.report_empty || !hits.is_empty()).then_some(Event::TodoScanProgress {
                hits,
                files_scanned,
            })
        }
    }

    /// Each progress batch as `(hit count, files_scanned)`.
    fn batches(
        events: &mut tokio_mpsc::UnboundedReceiver<(Option<RequestId>, Event)>,
    ) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        while let Ok((id, event)) = events.try_recv() {
            assert_eq!(id, Some(ID));
            if let Event::TodoScanProgress {
                hits,
                files_scanned,
            } = event
            {
                out.push((hits.len(), files_scanned));
            }
        }
        out
    }

    fn fill(root: &Path, count: usize, body: &str) -> std::io::Result<()> {
        for i in 0..count {
            std::fs::write(root.join(format!("f{i}.txt")), body)?;
        }
        Ok(())
    }

    fn walk_one(
        roots: &[PathBuf],
        excludes: &[String],
        probe: &mut Probe,
        seed: Vec<TodoHit>,
    ) -> (ControlFlow<(), Walked>, Vec<(usize, usize)>) {
        let hub = CancellationHub::default();
        let cancel = hub.register(ID);
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let walk = Walk {
            roots,
            includes: &[],
            excludes,
            cancel: &cancel,
        };
        let outcome = walk_batched(&walk, probe, ID, &tx, seed);
        (outcome, batches(&mut rx))
    }

    #[test]
    fn a_stretch_without_hits_still_flushes_every_files_per_batch() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), FILES_PER_BATCH + 1, "clean")?;
        let roots = [dir.path().to_path_buf()];
        let (outcome, sent) = walk_one(&roots, &[], &mut Probe::new(usize::MAX, true), Vec::new());
        assert_eq!(
            outcome,
            ControlFlow::Continue(Walked {
                files_scanned: FILES_PER_BATCH + 1,
                ..Walked::default()
            })
        );
        // One bare count at the batch boundary; nothing left to flush at the end.
        assert_eq!(sent, vec![(0, FILES_PER_BATCH)]);
        Ok(())
    }

    #[test]
    fn a_scan_that_declines_empty_batches_sends_none() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), FILES_PER_BATCH + 1, "clean")?;
        let roots = [dir.path().to_path_buf()];
        let (outcome, sent) = walk_one(&roots, &[], &mut Probe::new(usize::MAX, false), Vec::new());
        assert!(
            matches!(outcome, ControlFlow::Continue(Walked { files_scanned, .. }) if files_scanned == FILES_PER_BATCH + 1)
        );
        assert_eq!(sent, Vec::new());
        Ok(())
    }

    #[test]
    fn hits_flush_once_a_batch_fills_and_the_rest_at_the_end() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), HITS_PER_BATCH + 1, "hit")?;
        let roots = [dir.path().to_path_buf()];
        let (_, sent) = walk_one(&roots, &[], &mut Probe::new(usize::MAX, true), Vec::new());
        assert_eq!(
            sent,
            vec![(HITS_PER_BATCH, HITS_PER_BATCH), (1, HITS_PER_BATCH + 1)]
        );
        Ok(())
    }

    #[test]
    fn reaching_the_cap_truncates_and_still_reports_the_capping_hit() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), 3, "hit")?;
        let roots = [dir.path().to_path_buf(), dir.path().to_path_buf()];
        let (outcome, sent) = walk_one(&roots, &[], &mut Probe::new(2, true), Vec::new());
        assert_eq!(
            outcome,
            ControlFlow::Continue(Walked {
                files_scanned: 2,
                truncated: true,
                ..Walked::default()
            })
        );
        assert_eq!(sent, vec![(2, 2)]);
        Ok(())
    }

    #[test]
    fn seeded_hits_are_flushed_even_when_nothing_is_walked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let roots = [dir.path().to_path_buf()];
        let seed = vec![hit(dir.path())];
        let (outcome, sent) = walk_one(&roots, &[], &mut Probe::new(usize::MAX, false), seed);
        assert_eq!(outcome, ControlFlow::Continue(Walked::default()));
        assert_eq!(sent, vec![(1, 0)]);
        Ok(())
    }

    #[test]
    fn a_bad_glob_is_reported_and_no_root_is_walked() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), 2, "hit")?;
        let roots = [dir.path().to_path_buf(), dir.path().to_path_buf()];
        let (outcome, sent) = walk_one(
            &roots,
            &["[".to_owned()],
            &mut Probe::new(usize::MAX, true),
            Vec::new(),
        );
        let ControlFlow::Continue(walked) = outcome else {
            return Err("the stream is open, so the walk must answer".into());
        };
        assert!(walked.error.is_some());
        assert_eq!(walked.files_scanned, 0);
        assert_eq!(sent, Vec::new());
        Ok(())
    }

    #[test]
    fn a_cancelled_walk_stops_before_the_first_file() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), 2, "hit")?;
        let roots = [dir.path().to_path_buf(), dir.path().to_path_buf()];
        let hub = CancellationHub::default();
        let cancel = hub.register(ID);
        hub.cancel(ID);
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let walk = Walk {
            roots: &roots,
            includes: &[],
            excludes: &[],
            cancel: &cancel,
        };
        let outcome = walk_batched(
            &walk,
            &mut Probe::new(usize::MAX, true),
            ID,
            &tx,
            Vec::new(),
        );
        assert_eq!(
            outcome,
            ControlFlow::Continue(Walked {
                cancelled: true,
                ..Walked::default()
            })
        );
        assert_eq!(batches(&mut rx), Vec::new());
        Ok(())
    }

    #[test]
    fn a_closed_stream_breaks_instead_of_answering() -> TestResult {
        let dir = tempfile::tempdir()?;
        fill(dir.path(), 1, "hit")?;
        let roots = [dir.path().to_path_buf()];
        let hub = CancellationHub::default();
        let cancel = hub.register(ID);
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        drop(rx);
        let walk = Walk {
            roots: &roots,
            includes: &[],
            excludes: &[],
            cancel: &cancel,
        };
        let outcome = walk_batched(
            &walk,
            &mut Probe::new(usize::MAX, true),
            ID,
            &tx,
            Vec::new(),
        );
        assert_eq!(outcome, ControlFlow::Break(()));
        assert_eq!(
            send(
                &tx,
                ID,
                Event::TodoScanFinished {
                    files_scanned: 0,
                    truncated: false,
                    cancelled: false,
                }
            ),
            ControlFlow::Break(())
        );
        Ok(())
    }

    #[test]
    fn resolve_path_canonicalizes_and_falls_back_to_the_given_path() -> TestResult {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x")?;
        let dotted = dir.path().join(".").join("a.txt");
        assert_eq!(resolve_path(&dotted), std::fs::canonicalize(&file)?);
        let missing = dir.path().join("gone").join("b.txt");
        assert_eq!(resolve_path(&missing), missing);
        Ok(())
    }
}

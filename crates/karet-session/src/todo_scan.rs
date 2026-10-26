//! The workspace codetag-scan worker: a dedicated thread that finds `TODO:`-style
//! comments in every file, streaming batched results (see
//! [`crate::spell_scan`], whose shape this follows — one long cancellable walk
//! that must not starve the per-keystroke workers).
//!
//! Detection is [`karet_syntax::find_codetags`] over each file's own token
//! model, so the panel can never disagree with the editor's codetag tint. The
//! worker owns its parse host outright, like the other scanners.

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_syntax::LayeredHighlighter;
use karet_syntax::SemanticCommentConfig;
use karet_treesitter::LayeredParser;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::api::TodoHit;
use crate::cancellation::Cancellation;
use crate::scan_walk::BatchedScan;
use crate::scan_walk::Walk;
use crate::scan_walk::resolve_path;
use crate::scan_walk::send;

/// One workspace codetag scan request.
pub(crate) struct TodoScanJob {
    /// Correlates every answering event.
    pub id: RequestId,
    /// The workspace roots to walk, in order.
    pub roots: Vec<PathBuf>,
    /// The tag vocabulary (`editor.semanticComments.tags`).
    pub config: SemanticCommentConfig,
    /// Workspace-search excludes, honored like every other scan.
    pub excludes: Vec<String>,
    /// Live text for open documents, keyed by resolved path — their on-disk
    /// bytes may be stale.
    pub open: Vec<(PathBuf, String)>,
    /// Stop after this many hits and report `truncated`.
    pub limit: usize,
    /// Cooperative cancellation, checked between files.
    pub cancel: Cancellation,
}

/// Start the worker; the session sends [`TodoScanJob`]s and answers arrive on
/// the shared event stream.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<TodoScanJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-todo-scan".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(
    jobs: &Receiver<TodoScanJob>,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) {
    // The parse host outlives individual scans: a re-scan reuses every
    // compiled grammar.
    let mut host = ParseHost {
        parser: LayeredParser::new(),
        highlighter: LayeredHighlighter::new(),
    };
    while let Ok(job) = jobs.recv() {
        if scan(&job, &mut host, events).is_break() {
            return; // the session is gone
        }
    }
}

/// The reusable parse/highlight machinery one scan borrows.
struct ParseHost {
    parser: LayeredParser,
    highlighter: LayeredHighlighter,
}

/// One scan as the shared walk sees it.
struct TodoScan<'a> {
    job: &'a TodoScanJob,
    host: &'a mut ParseHost,
    /// The open documents' resolved paths, which the walk skips.
    open_paths: Vec<PathBuf>,
    total_hits: usize,
}

impl BatchedScan for TodoScan<'_> {
    type Hit = TodoHit;

    fn visit(&mut self, path: &Path, text: &str, hits: &mut Vec<TodoHit>) -> ControlFlow<()> {
        if !self
            .open_paths
            .iter()
            .any(|open| *open == resolve_path(path))
        {
            check_file(path, text, self.job, self.host, &mut self.total_hits, hits);
        }
        if self.total_hits >= self.job.limit {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    /// An empty batch sends nothing: unlike the other scans, this one never
    /// reports a bare scanned count.
    fn progress(&self, hits: Vec<TodoHit>, files_scanned: usize) -> Option<Event> {
        (!hits.is_empty()).then_some(Event::TodoScanProgress {
            hits,
            files_scanned,
        })
    }
}

/// Walk the workspace, streaming batches as it goes.
fn scan(
    job: &TodoScanJob,
    host: &mut ParseHost,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    // Open buffers first: their live text wins over the disk copy the walk
    // would read, and skipping them below keeps every path scanned once.
    let mut hits = Vec::new();
    let mut total_hits = 0;
    for (path, text) in &job.open {
        check_file(path, text, job, host, &mut total_hits, &mut hits);
    }
    let mut scan = TodoScan {
        job,
        host,
        open_paths: job
            .open
            .iter()
            .map(|(path, _)| resolve_path(path))
            .collect(),
        total_hits,
    };
    let walk = Walk {
        roots: &job.roots,
        includes: &[],
        excludes: &job.excludes,
        cancel: &job.cancel,
    };
    // A walk error is a bad exclude glob from the settings; the scan has never
    // reported one, and finishes with whatever the open documents held.
    let walked = crate::scan_walk::walk_batched(&walk, &mut scan, job.id, events, hits)?;
    send(
        events,
        job.id,
        Event::TodoScanFinished {
            files_scanned: walked.files_scanned,
            truncated: walked.truncated,
            cancelled: walked.cancelled,
        },
    )
}

/// Find the codetags of one file, appending them to `hits`.
fn check_file(
    path: &Path,
    text: &str,
    job: &TodoScanJob,
    host: &mut ParseHost,
    total_hits: &mut usize,
    hits: &mut Vec<TodoHit>,
) {
    // No compiled grammar means no comment spans — nothing to find. (Markdown
    // and plain prose have no comment vocabulary, matching the editor's tint.)
    let Some(tree) = karet_treesitter::language_id_from_path(path)
        .and_then(|lang| host.parser.parse(lang, text).ok())
    else {
        return;
    };
    let highlights = host.highlighter.highlight(&tree, text);
    let room = job.limit.saturating_sub(*total_hits);
    for hit in karet_syntax::find_codetags(text, &highlights, &job.config)
        .into_iter()
        .take(room)
    {
        *total_hits += 1;
        hits.push(TodoHit {
            path: path.to_path_buf(),
            line: hit.line,
            tag: hit.tag,
            message: hit.message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationHub;

    /// The events one scan produced, flattened for assertions.
    struct Scanned {
        hits: Vec<TodoHit>,
        files_scanned: usize,
        truncated: bool,
        cancelled: bool,
    }

    /// Whether this build compiled a grammar for `.rs` files; without one no
    /// comment spans exist and every scan finds nothing.
    fn rust_grammar() -> bool {
        karet_treesitter::language_id_from_path(Path::new("a.rs")).is_some()
    }

    fn scan_dir(root: &Path, hub: &CancellationHub, job: impl FnOnce(&mut TodoScanJob)) -> Scanned {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut request = TodoScanJob {
            id: RequestId(7),
            roots: vec![root.to_path_buf()],
            config: SemanticCommentConfig::default(),
            excludes: Vec::new(),
            open: Vec::new(),
            limit: 1000,
            cancel: hub.register(RequestId(7)),
        };
        job(&mut request);
        let mut host = ParseHost {
            parser: LayeredParser::new(),
            highlighter: LayeredHighlighter::new(),
        };
        let _ = scan(&request, &mut host, &tx);

        let mut scanned = Scanned {
            hits: Vec::new(),
            files_scanned: 0,
            truncated: false,
            cancelled: false,
        };
        while let Ok((id, event)) = rx.try_recv() {
            assert_eq!(
                id,
                Some(RequestId(7)),
                "every answer carries the request id"
            );
            match event {
                Event::TodoScanProgress { hits, .. } => scanned.hits.extend(hits),
                Event::TodoScanFinished {
                    files_scanned,
                    truncated,
                    cancelled,
                } => {
                    scanned.files_scanned = files_scanned;
                    scanned.truncated = truncated;
                    scanned.cancelled = cancelled;
                },
                _ => {},
            }
        }
        scanned
            .hits
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        scanned
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(root: &Path, rel: &str, body: &str) -> std::io::Result<()> {
        std::fs::write(root.join(rel), body)
    }

    #[test]
    fn scan_reports_each_codetag_with_its_line_and_message() -> TestResult {
        let dir = tempfile::tempdir()?;
        if !rust_grammar() {
            return Ok(()); // rust grammar not compiled in; nothing to test
        }
        write(
            dir.path(),
            "a.rs",
            "fn a() {}\n// TODO: tidy this\nfn b() {}\n",
        )?;
        let scanned = scan_dir(dir.path(), &CancellationHub::default(), |_| {});
        let hits: Vec<(u32, &str, &str)> = scanned
            .hits
            .iter()
            .map(|hit| (hit.line, hit.tag.as_str(), hit.message.as_str()))
            .collect();
        assert_eq!(hits, vec![(1, "TODO", "tidy this")]);
        assert_eq!(scanned.files_scanned, 1);
        assert!(!scanned.truncated && !scanned.cancelled);
        Ok(())
    }

    #[test]
    fn an_open_buffer_is_scanned_instead_of_its_stale_disk_copy() -> TestResult {
        let dir = tempfile::tempdir()?;
        if !rust_grammar() {
            return Ok(()); // rust grammar not compiled in; nothing to test
        }
        write(dir.path(), "a.rs", "// TODO: on disk\n")?;
        let path = dir.path().join("a.rs");
        let scanned = scan_dir(dir.path(), &CancellationHub::default(), |job| {
            job.open = vec![(
                path.clone(),
                "fn a() {}\n// FIXME: in the buffer\n".to_owned(),
            )];
        });
        let hits: Vec<(&Path, u32, &str)> = scanned
            .hits
            .iter()
            .map(|hit| (hit.path.as_path(), hit.line, hit.message.as_str()))
            .collect();
        assert_eq!(
            hits,
            vec![(path.as_path(), 1, "in the buffer")],
            "the live text wins and the disk copy is not scanned a second time"
        );
        Ok(())
    }

    #[test]
    fn the_hit_limit_truncates_the_scan() -> TestResult {
        let dir = tempfile::tempdir()?;
        if !rust_grammar() {
            return Ok(()); // rust grammar not compiled in; nothing to test
        }
        write(
            dir.path(),
            "a.rs",
            "// TODO: one\n// TODO: two\n// TODO: three\n",
        )?;
        let scanned = scan_dir(dir.path(), &CancellationHub::default(), |job| job.limit = 2);
        assert_eq!(scanned.hits.len(), 2, "never more hits than the limit");
        assert!(scanned.truncated);
        assert!(!scanned.cancelled);
        Ok(())
    }

    #[test]
    fn a_cancelled_scan_stops_before_walking() -> TestResult {
        let dir = tempfile::tempdir()?;
        write(dir.path(), "a.rs", "// TODO: one\n")?;
        write(dir.path(), "b.rs", "// TODO: two\n")?;
        let hub = CancellationHub::default();
        let scanned = scan_dir(dir.path(), &hub, |_| hub.cancel(RequestId(7)));
        assert!(scanned.cancelled);
        assert_eq!(scanned.files_scanned, 0);
        assert!(scanned.hits.is_empty());
        Ok(())
    }
}

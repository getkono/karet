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

/// Flush a partial batch after this many files (see `spell_scan`).
const FILES_PER_BATCH: usize = 200;
/// Flush a partial batch once it holds this many hits.
const HITS_PER_BATCH: usize = 64;

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

/// Everything one scan accumulates while walking.
struct ScanState {
    hits: Vec<TodoHit>,
    files_scanned: usize,
    files_since_flush: usize,
    total_hits: usize,
    truncated: bool,
    cancelled: bool,
}

/// Walk the workspace, streaming batches as it goes.
fn scan(
    job: &TodoScanJob,
    host: &mut ParseHost,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    let mut state = ScanState {
        hits: Vec::new(),
        files_scanned: 0,
        files_since_flush: 0,
        total_hits: 0,
        truncated: false,
        cancelled: false,
    };
    // Open buffers first: their live text wins over the disk copy the walk
    // would read, and skipping them below keeps every path scanned once.
    let open_paths: Vec<PathBuf> = job
        .open
        .iter()
        .map(|(path, _)| crate::spell_scan::resolve_path(path))
        .collect();
    for (path, text) in &job.open {
        check_file(path, text, job, host, &mut state);
    }
    let mut disconnected = false;
    for root in &job.roots {
        let _ = karet_search::walk_text_files(root, &[], &job.excludes, |path, text| {
            if job.cancel.is_cancelled() {
                state.cancelled = true;
                return ControlFlow::Break(());
            }
            state.files_scanned += 1;
            state.files_since_flush += 1;
            if !open_paths
                .iter()
                .any(|open| *open == crate::spell_scan::resolve_path(path))
            {
                check_file(path, &text, job, host, &mut state);
            }
            if state.total_hits >= job.limit {
                state.truncated = true;
                return ControlFlow::Break(());
            }
            if state.hits.len() >= HITS_PER_BATCH || state.files_since_flush >= FILES_PER_BATCH {
                state.files_since_flush = 0;
                if flush(&mut state, job.id, events).is_break() {
                    disconnected = true;
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        });
        if disconnected || state.cancelled || state.truncated {
            break;
        }
    }
    if disconnected {
        return ControlFlow::Break(());
    }
    flush(&mut state, job.id, events)?;
    send(
        events,
        job.id,
        Event::TodoScanFinished {
            files_scanned: state.files_scanned,
            truncated: state.truncated,
            cancelled: state.cancelled,
        },
    )
}

/// Find the codetags of one file.
fn check_file(
    path: &Path,
    text: &str,
    job: &TodoScanJob,
    host: &mut ParseHost,
    state: &mut ScanState,
) {
    // No compiled grammar means no comment spans — nothing to find. (Markdown
    // and plain prose have no comment vocabulary, matching the editor's tint.)
    let Some(tree) = karet_treesitter::language_id_from_path(path)
        .and_then(|lang| host.parser.parse(lang, text).ok())
    else {
        return;
    };
    let highlights = host.highlighter.highlight(&tree, text);
    let room = job.limit.saturating_sub(state.total_hits);
    for hit in karet_syntax::find_codetags(text, &highlights, &job.config)
        .into_iter()
        .take(room)
    {
        state.total_hits += 1;
        state.hits.push(TodoHit {
            path: path.to_path_buf(),
            line: hit.line,
            tag: hit.tag,
            message: hit.message,
        });
    }
}

/// Emit the accumulated partial batch, if any.
fn flush(
    state: &mut ScanState,
    id: RequestId,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    if state.hits.is_empty() {
        return ControlFlow::Continue(());
    }
    let hits = std::mem::take(&mut state.hits);
    send(
        events,
        id,
        Event::TodoScanProgress {
            hits,
            files_scanned: state.files_scanned,
        },
    )
}

/// Send one event, breaking when the session is gone.
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

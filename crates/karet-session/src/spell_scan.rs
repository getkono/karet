//! The workspace spelling-scan worker: a dedicated thread that spell-checks every
//! file in the workspace, not just the open ones.
//!
//! Deliberately separate from the per-document worker in [`crate::spell`]. That one
//! is a *deadline debounce* keyed by document, sized for one buffer per keystroke
//! burst; this one is a single long walk that must stream partial answers and stop
//! on request. Sharing the queue would let one scan starve every editor's squiggles.
//!
//! Like [`crate::highlight`], the worker owns its parse host outright: a scan needs a
//! token model for files nothing has opened, and a second `ParserPool` costs one
//! parser per language while removing every synchronization question.

use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;

use karet_core::NotificationKind;
use karet_core::Severity;
use karet_syntax::Highlights;
use karet_syntax::LayeredHighlighter;
use karet_treesitter::LayeredParser;
use spellbook::Dictionary;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::Event;
use crate::api::RequestId;
use crate::api::SpellingHit;
use crate::api::SpellingLanguage;
use crate::cancellation::Cancellation;
use crate::config::schema::Spellcheck;
use crate::spell::check::LineIndex;
use crate::spell::check::SpellInput;
use crate::spell::check::word_in_line;

/// Flush a partial batch after this many files, so a long walk over clean files
/// still reports progress rather than looking stalled.
const FILES_PER_BATCH: usize = 200;
/// Flush a partial batch once it holds this many hits, so a dirty workspace fills
/// the list steadily instead of in one late lump.
const HITS_PER_BATCH: usize = 64;

/// One workspace scan request.
pub(crate) struct SpellScanJob {
    /// Correlates every answering event.
    pub id: RequestId,
    /// The workspace root to walk.
    pub root: PathBuf,
    /// The dictionary to check against.
    pub spelling_language: SpellingLanguage,
    /// The resolved spell-check settings.
    pub settings: Spellcheck,
    /// Paths already answered from live buffers; the scan must not re-read their
    /// (possibly stale) on-disk text.
    pub open: HashSet<PathBuf>,
    /// Stop after this many hits and report `truncated`.
    pub limit: usize,
    /// Cooperative cancellation, checked between files.
    pub cancel: Cancellation,
}

/// Start the worker; the session sends [`SpellScanJob`]s and answers arrive on the
/// shared event stream.
pub(crate) fn spawn(
    events: tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> Sender<SpellScanJob> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-spell-scan".to_owned())
        .spawn(move || run(&jobs_rx, &events));
    jobs_tx
}

fn run(
    jobs: &Receiver<SpellScanJob>,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) {
    // The parse host and dictionary cache outlive individual scans: a re-scan of the
    // same workspace re-uses every compiled grammar and the loaded dictionary.
    let mut host = ScanHost {
        parse: ParseHost {
            parser: LayeredParser::new(),
            highlighter: LayeredHighlighter::new(),
        },
        dictionaries: HashMap::new(),
    };
    while let Ok(job) = jobs.recv() {
        if execute(job, &mut host, events).is_break() {
            return; // the session is gone
        }
    }
}

/// The reusable machinery one scan borrows. Split in two so a scan can hold the
/// loaded dictionary and the parse host at once — they are disjoint fields, and the
/// walk closure needs both.
struct ScanHost {
    parse: ParseHost,
    dictionaries: HashMap<SpellingLanguage, Result<Dictionary, String>>,
}

/// The parse/highlight side of [`ScanHost`].
struct ParseHost {
    parser: LayeredParser,
    highlighter: LayeredHighlighter,
}

/// Everything one scan accumulates while walking.
struct ScanState {
    hits: Vec<SpellingHit>,
    files_scanned: usize,
    files_since_flush: usize,
    total_hits: usize,
    truncated: bool,
    cancelled: bool,
}

/// Resolve the job's dictionary (cached for the worker's lifetime), then walk.
fn execute(
    job: SpellScanJob,
    host: &mut ScanHost,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    let ScanHost {
        parse,
        dictionaries,
    } = host;
    let dictionary = dictionaries
        .entry(job.spelling_language)
        .or_insert_with(|| crate::spell::load_dictionary(job.spelling_language));
    match dictionary {
        Ok(dictionary) => scan(&job, parse, dictionary, events),
        Err(error) => {
            // A missing dictionary is the user's environment, not a bug: say so once
            // and finish cleanly so the client leaves its "scanning" state.
            send(
                events,
                job.id,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::System,
                    message: error.clone(),
                },
            )?;
            send(
                events,
                job.id,
                Event::SpellingScanFinished {
                    files_scanned: 0,
                    truncated: false,
                    cancelled: false,
                },
            )
        },
    }
}

/// Walk the workspace with a resolved dictionary, streaming batches as it goes.
fn scan(
    job: &SpellScanJob,
    parse: &mut ParseHost,
    dictionary: &Dictionary,
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
    let mut disconnected = false;
    let _ = karet_search::walk_text_files(&job.root, &[], &[], |path, text| {
        if job.cancel.is_cancelled() {
            state.cancelled = true;
            return ControlFlow::Break(());
        }
        state.files_scanned += 1;
        state.files_since_flush += 1;
        if !job.open.contains(path) {
            check_file(path, &text, job, parse, dictionary, &mut state);
        }
        if state.total_hits >= job.limit {
            state.truncated = true;
            return ControlFlow::Break(());
        }
        if state.hits.len() >= HITS_PER_BATCH || state.files_since_flush >= FILES_PER_BATCH {
            state.files_since_flush = 0;
            let hits = std::mem::take(&mut state.hits);
            if send(
                events,
                job.id,
                Event::SpellingScanProgress {
                    hits,
                    files_scanned: state.files_scanned,
                },
            )
            .is_break()
            {
                disconnected = true;
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    if disconnected {
        return ControlFlow::Break(());
    }
    if !state.hits.is_empty() {
        send(
            events,
            job.id,
            Event::SpellingScanProgress {
                hits: std::mem::take(&mut state.hits),
                files_scanned: state.files_scanned,
            },
        )?;
    }
    send(
        events,
        job.id,
        Event::SpellingScanFinished {
            files_scanned: state.files_scanned,
            truncated: state.truncated,
            cancelled: state.cancelled,
        },
    )
}

/// Check one walked file, appending its misspellings to `state`.
fn check_file(
    path: &Path,
    text: &str,
    job: &SpellScanJob,
    parse: &mut ParseHost,
    dictionary: &Dictionary,
    state: &mut ScanState,
) {
    let language = crate::session::language_name_for_path(path);
    let language_selector = crate::session::language_selector_for_path(path);
    if !scope_can_match(language, &job.settings) {
        return;
    }
    // A file with no compiled grammar still checks as prose; a source file simply
    // has no tokens to classify and so contributes nothing.
    let highlights = karet_treesitter::language_id_from_path(path)
        .and_then(|lang| parse.parser.parse(lang, text).ok())
        .map_or_else(Highlights::default, |tree| {
            parse.highlighter.highlight(&tree, text)
        });
    let diagnostics = crate::spell::check::check(
        &SpellInput {
            text,
            language,
            language_selector,
            spelling_language: job.spelling_language,
            highlights: &highlights,
            // A scan reads committed files rather than a half-typed buffer, so the
            // "pause identifier linting while syntax is broken" guard has nothing
            // to suppress.
            syntax_error_lines: &[],
            settings: &job.settings,
            suggest: false,
        },
        dictionary,
    );
    if diagnostics.is_empty() {
        return;
    }
    let index = LineIndex::new(text);
    let room = job.limit.saturating_sub(state.total_hits);
    for diagnostic in diagnostics.into_iter().take(room) {
        state.hits.push(SpellingHit {
            path: path.to_path_buf(),
            range: diagnostic.range,
            word: word_in_line(index.line(diagnostic.range.start.line), diagnostic.range),
            line_text: index.line(diagnostic.range.start.line).trim().to_owned(),
        });
        state.total_hits += 1;
    }
}

/// Whether any scope this file could contribute is enabled — the cheap gate that
/// skips parsing a source file outright when comments, strings, and identifiers
/// are all off.
fn scope_can_match(language: Option<&'static str>, settings: &Spellcheck) -> bool {
    let prose = matches!(
        language.map(str::to_ascii_lowercase).as_deref(),
        Some("markdown" | "plain text" | "asciidoc" | "restructuredtext" | "tex")
    );
    if prose {
        return settings.documents;
    }
    settings.comments || settings.strings || settings.identifiers
}

/// Send one event, mapping a closed stream to [`ControlFlow::Break`].
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancellation::CancellationHub;

    const AFF: &str = "SET UTF-8\n";
    const DIC: &str = "4\nhello\nworld\nthe\nends\n";

    /// The events one scan produced, flattened for assertions.
    struct Scanned {
        hits: Vec<SpellingHit>,
        files_scanned: usize,
        truncated: bool,
        cancelled: bool,
        batches: usize,
    }

    fn scan_dir(root: &Path, job: impl FnOnce(&mut SpellScanJob)) -> Option<Scanned> {
        let dictionary = Dictionary::new(AFF, DIC).ok()?;
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let hub = CancellationHub::default();
        let mut request = SpellScanJob {
            id: RequestId(7),
            root: root.to_path_buf(),
            spelling_language: SpellingLanguage::EnglishUnitedStates,
            settings: Spellcheck {
                enabled: true,
                ..Spellcheck::default()
            },
            open: HashSet::new(),
            limit: 1000,
            cancel: hub.register(RequestId(7)),
        };
        job(&mut request);
        let mut parse = ParseHost {
            parser: LayeredParser::new(),
            highlighter: LayeredHighlighter::new(),
        };
        let _ = scan(&request, &mut parse, &dictionary, &tx);

        let mut scanned = Scanned {
            hits: Vec::new(),
            files_scanned: 0,
            truncated: false,
            cancelled: false,
            batches: 0,
        };
        while let Ok((id, event)) = rx.try_recv() {
            assert_eq!(
                id,
                Some(RequestId(7)),
                "every answer carries the request id"
            );
            match event {
                Event::SpellingScanProgress { hits, .. } => {
                    scanned.batches += 1;
                    scanned.hits.extend(hits);
                },
                Event::SpellingScanFinished {
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
        scanned.hits.sort_by(|a, b| {
            (&a.path, a.range.start.line, a.range.start.col).cmp(&(
                &b.path,
                b.range.start.line,
                b.range.start.col,
            ))
        });
        Some(scanned)
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, body);
    }

    #[test]
    fn scan_reports_prose_misspellings_with_their_line_as_context() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(dir.path(), "notes.md", "hello world\nthe wrld ends\n");
        let Some(scanned) = scan_dir(dir.path(), |_| {}) else {
            return;
        };

        assert_eq!(scanned.hits.len(), 1, "{:?}", scanned.hits);
        let hit = &scanned.hits[0];
        assert_eq!(hit.word, "wrld");
        assert_eq!(hit.path, dir.path().join("notes.md"));
        assert_eq!(hit.range.start, karet_core::LineCol::new(1, 4));
        assert_eq!(hit.line_text, "the wrld ends");
        assert_eq!(scanned.files_scanned, 1);
        assert!(!scanned.truncated);
        assert!(!scanned.cancelled);
    }

    #[test]
    fn scan_skips_paths_the_session_answered_from_live_buffers() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(dir.path(), "open.md", "the wrld ends\n");
        write(dir.path(), "closed.md", "the wrld ends\n");
        let open = dir.path().join("open.md");
        let Some(scanned) = scan_dir(dir.path(), |job| {
            job.open.insert(open.clone());
        }) else {
            return;
        };

        assert_eq!(
            scanned.hits.iter().map(|h| &h.path).collect::<Vec<_>>(),
            vec![&dir.path().join("closed.md")],
            "the open document's stale on-disk text must not be re-reported"
        );
        assert_eq!(
            scanned.files_scanned, 2,
            "a skipped file still counts as visited"
        );
    }

    #[test]
    fn scan_stops_at_the_limit_and_reports_truncation() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        for i in 0..5 {
            write(dir.path(), &format!("f{i}.md"), "wrld\n");
        }
        let Some(scanned) = scan_dir(dir.path(), |job| job.limit = 2) else {
            return;
        };

        assert_eq!(scanned.hits.len(), 2);
        assert!(scanned.truncated);
        assert!(scanned.files_scanned < 5, "the walk stopped early");
    }

    #[test]
    fn a_cancelled_scan_finishes_immediately_and_says_so() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(dir.path(), "notes.md", "the wrld ends\n");
        let hub = CancellationHub::default();
        let cancel = hub.register(RequestId(7));
        hub.cancel(RequestId(7));
        let Some(scanned) = scan_dir(dir.path(), |job| job.cancel = cancel) else {
            return;
        };

        assert!(scanned.cancelled);
        assert!(scanned.hits.is_empty());
        assert_eq!(scanned.files_scanned, 0);
    }

    #[test]
    fn source_scopes_are_off_by_default_so_only_prose_is_scanned() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        // `comments` is on by default, `strings`/`identifiers` are not.
        write(dir.path(), "a.rs", "// the wrld ends\nlet s = \"wrld\";\n");
        let Some(scanned) = scan_dir(dir.path(), |_| {}) else {
            return;
        };

        assert_eq!(
            scanned
                .hits
                .iter()
                .map(|h| h.word.as_str())
                .collect::<Vec<_>>(),
            vec!["wrld"],
            "the comment is checked, the string literal is not: {:?}",
            scanned.hits
        );
    }

    #[test]
    fn a_clean_workspace_still_reports_a_finish_event() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(dir.path(), "notes.md", "hello world\n");
        let Some(scanned) = scan_dir(dir.path(), |_| {}) else {
            return;
        };

        assert!(scanned.hits.is_empty());
        assert_eq!(scanned.batches, 0, "an empty batch is never sent");
        assert_eq!(scanned.files_scanned, 1);
    }

    fn settings(comments: bool, strings: bool, identifiers: bool, documents: bool) -> Spellcheck {
        Spellcheck {
            comments,
            strings,
            identifiers,
            documents,
            ..Spellcheck::default()
        }
    }

    #[test]
    fn source_files_are_skipped_when_every_source_scope_is_off() {
        assert!(!scope_can_match(
            Some("Rust"),
            &settings(false, false, false, true)
        ));
        assert!(scope_can_match(
            Some("Rust"),
            &settings(true, false, false, false)
        ));
        assert!(scope_can_match(
            Some("Rust"),
            &settings(false, true, false, false)
        ));
        assert!(scope_can_match(
            Some("Rust"),
            &settings(false, false, true, false)
        ));
    }

    #[test]
    fn prose_files_follow_the_documents_toggle_alone() {
        assert!(scope_can_match(
            Some("Markdown"),
            &settings(false, false, false, true)
        ));
        assert!(!scope_can_match(
            Some("Markdown"),
            &settings(true, true, true, false)
        ));
        // An unrecognized language is treated as source, not prose.
        assert!(!scope_can_match(None, &settings(false, false, false, true)));
    }
}

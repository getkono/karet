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
    /// The workspace's dictionary — the answer for every path an `.editorconfig`
    /// does not override.
    pub spelling_language: SpellingLanguage,
    /// The live settings. The whole snapshot rather than `spellcheck` alone,
    /// because resolving a path's dictionary goes through the same EditorConfig
    /// path an open document uses.
    pub settings: crate::config::Settings,
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
    /// Per directory, whether any `.editorconfig` above it selects a spelling
    /// locale. See [`ancestry_selects_locale`].
    locale_overrides: HashMap<PathBuf, bool>,
}

/// Resolve the job's dictionary (cached for the worker's lifetime), then walk.
fn execute(
    job: SpellScanJob,
    host: &mut ScanHost,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    // Load the workspace's own dictionary first: a missing one is the user's
    // environment, not a bug, and saying so once beats a silently empty scan.
    // Per-file EditorConfig locales are resolved lazily during the walk.
    let dictionary = host
        .dictionaries
        .entry(job.spelling_language)
        .or_insert_with(|| crate::spell::load_dictionary(job.spelling_language));
    if let Err(error) = dictionary {
        send(
            events,
            job.id,
            Event::Notification {
                severity: Severity::Warning,
                kind: NotificationKind::System,
                message: error.clone(),
            },
        )?;
        return send(
            events,
            job.id,
            Event::SpellingScanFinished {
                files_scanned: 0,
                truncated: false,
                cancelled: false,
            },
        );
    }
    scan(&job, host, events)
}

/// Walk the workspace, streaming batches as it goes.
fn scan(
    job: &SpellScanJob,
    host: &mut ScanHost,
    events: &tokio_mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) -> ControlFlow<()> {
    let mut state = ScanState {
        hits: Vec::new(),
        files_scanned: 0,
        files_since_flush: 0,
        total_hits: 0,
        truncated: false,
        cancelled: false,
        locale_overrides: HashMap::new(),
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
            check_file(path, &text, job, host, &mut state);
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
    host: &mut ScanHost,
    state: &mut ScanState,
) {
    let language = crate::session::language_name_for_path(path);
    let language_selector = crate::session::language_selector_for_path(path);
    if !crate::spell::scope::can_match(language, &job.settings.spellcheck) {
        return;
    }
    // A document resolves its dictionary through EditorConfig; so must a walked
    // file, or the panel checks against a locale the editor never uses — and a
    // path EditorConfig leaves without a supported locale is one the editor
    // never checks at all.
    let Some(spelling_language) = spelling_language_for(path, language, job, state) else {
        return;
    };
    let ScanHost {
        parse,
        dictionaries,
    } = host;
    let dictionary = dictionaries
        .entry(spelling_language)
        .or_insert_with(|| crate::spell::load_dictionary(spelling_language));
    let Ok(dictionary) = dictionary else {
        return; // an EditorConfig locale with no dictionary installed
    };
    // A file with no compiled grammar still checks as prose; a source file simply
    // has no tokens to classify and so contributes nothing.
    let tree = karet_treesitter::language_id_from_path(path)
        .and_then(|lang| parse.parser.parse(lang, text).ok());
    let (highlights, syntax_error_lines) = tree.as_ref().map_or_else(
        || (Highlights::default(), Vec::new()),
        |tree| (parse.highlighter.highlight(tree, text), tree.error_lines()),
    );
    let diagnostics = crate::spell::check::check(
        &SpellInput {
            text,
            language,
            language_selector,
            spelling_language,
            highlights: &highlights,
            // The editor pauses identifier linting while the file does not parse,
            // because a broken tree mislabels ordinary text as an identifier. A
            // file on disk is no less capable of not parsing than a half-typed
            // buffer, so the scan owes the same guard — without it the panel
            // reports identifiers the editor would never mark.
            syntax_error_lines: &syntax_error_lines,
            settings: &job.settings.spellcheck,
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

/// The dictionary this path should be checked against, resolved exactly as an
/// open document's is.
///
/// EditorConfig locale selection is opt-in and rare, so the ancestry is inspected
/// once per directory: when nothing above the file mentions `spelling_language`,
/// the workspace locale is the answer by construction and no per-file resolution
/// runs at all.
fn spelling_language_for(
    path: &Path,
    language: Option<&'static str>,
    job: &SpellScanJob,
    state: &mut ScanState,
) -> Option<SpellingLanguage> {
    let Some(dir) = path.parent() else {
        return Some(job.spelling_language);
    };
    let overridden = match state.locale_overrides.get(dir) {
        Some(&overridden) => overridden,
        None => {
            let overridden = ancestry_selects_locale(dir);
            state.locale_overrides.insert(dir.to_path_buf(), overridden);
            overridden
        },
    };
    if !overridden {
        return Some(job.spelling_language);
    }
    crate::editorconfig::resolve(path, language, &job.settings)
        .map_or(Some(job.spelling_language), |resolved| {
            resolved.spelling_language
        })
}

/// Whether any `.editorconfig` at or above `dir` mentions `spelling_language`.
///
/// Deliberately conservative: a false positive only costs a per-file resolution
/// that lands on the same answer, so this never needs to model EditorConfig's
/// `root` or glob semantics — it only decides whether asking is worth it.
fn ancestry_selects_locale(dir: &Path) -> bool {
    dir.ancestors().any(|ancestor| {
        std::fs::read_to_string(ancestor.join(".editorconfig"))
            .is_ok_and(|text| text.contains("spelling_language"))
    })
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
        let mut settings = crate::config::Settings::default();
        settings.spellcheck.enabled = true;
        let mut request = SpellScanJob {
            id: RequestId(7),
            root: root.to_path_buf(),
            spelling_language: SpellingLanguage::EnglishUnitedStates,
            settings,
            open: HashSet::new(),
            limit: 1000,
            cancel: hub.register(RequestId(7)),
        };
        job(&mut request);
        // Both supported locales resolve to the same fixture dictionary, so an
        // EditorConfig override changes the corpus rather than failing to load.
        let mut host = ScanHost {
            parse: ParseHost {
                parser: LayeredParser::new(),
                highlighter: LayeredHighlighter::new(),
            },
            dictionaries: [
                (SpellingLanguage::EnglishUnitedStates, Ok(dictionary)),
                (
                    SpellingLanguage::EnglishUnitedKingdom,
                    Dictionary::new(AFF, DIC).map_err(|error| error.to_string()),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let _ = scan(&request, &mut host, &tx);

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
    fn identifier_linting_pauses_on_a_file_that_does_not_parse() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        // Both files name the same identifier; only the second parses cleanly.
        write(dir.path(), "broken.rs", "fn wrld( {\n");
        write(dir.path(), "clean.rs", "fn wrld() {}\n");
        let Some(scanned) = scan_dir(dir.path(), |job| {
            job.settings.spellcheck.identifiers = true;
        }) else {
            return;
        };

        assert_eq!(
            scanned
                .hits
                .iter()
                .map(|hit| hit.path.clone())
                .collect::<Vec<_>>(),
            vec![dir.path().join("clean.rs")],
            "a broken tree mislabels text as identifiers; the editor pauses here \
             and so must the scan: {:?}",
            scanned.hits
        );
    }

    #[test]
    fn editorconfig_selects_the_dictionary_for_a_walked_file() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(
            dir.path(),
            ".editorconfig",
            "root = true\n[*.md]\nspelling_language = en-GB\n",
        );
        write(dir.path(), "notes.md", "the wrld ends\n");
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
            "{:?}",
            scanned.hits
        );
        assert_eq!(
            scanned.hits[0].path,
            dir.path().join("notes.md"),
            "the override resolves to a supported locale, so the file is checked"
        );
    }

    #[test]
    fn an_unsupported_editorconfig_locale_skips_the_file_as_the_editor_does() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        // `resolve` yields no spelling language for `de`, which clears an open
        // document's diagnostics outright — the panel must agree.
        write(
            dir.path(),
            ".editorconfig",
            "root = true\n[*.md]\nspelling_language = de\n",
        );
        write(dir.path(), "notes.md", "the wrld ends\n");
        let Some(scanned) = scan_dir(dir.path(), |_| {}) else {
            return;
        };

        assert!(scanned.hits.is_empty(), "{:?}", scanned.hits);
        assert_eq!(
            scanned.files_scanned, 1,
            "the file was visited and then skipped, not filtered out of the walk"
        );
    }

    #[test]
    fn a_workspace_without_the_property_never_pays_for_editorconfig_resolution() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        write(
            dir.path(),
            ".editorconfig",
            "root = true\n[*]\nindent_size = 2\n",
        );
        assert!(!ancestry_selects_locale(dir.path()));

        write(
            dir.path(),
            ".editorconfig",
            "root = true\n[*.md]\nspelling_language = en-GB\n",
        );
        assert!(ancestry_selects_locale(dir.path()));
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
}

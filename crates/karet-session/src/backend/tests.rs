use std::path::Path;
use std::path::PathBuf;

use super::*;
use crate::api::DocumentId;
use crate::api::Event;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Whether a `git` binary runs here: the one environment precondition the
/// repository tests skip on (fixture failures past it fail the test).
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Run `git args` in `dir`, failing when the fixture step fails.
fn git(dir: &Path, args: &[&str]) -> TestResult {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {args:?} failed: {status}").into())
    }
}

/// `git init` in `dir` with a committer identity.
fn init_repo(dir: &Path) -> TestResult {
    git(dir, &["init", "-q"])?;
    git(dir, &["config", "user.email", "test@example.com"])?;
    git(dir, &["config", "user.name", "karet test"])
}

/// Send `command`, then wait for the first event answering it that `pick`
/// accepts; an error when the send fails, the wait times out or the stream ends.
async fn answer<T>(
    backend: &LocalBackend,
    events: &mut EventRx,
    command: Command,
    mut pick: impl FnMut(Event) -> Option<T>,
) -> Result<T, Box<dyn std::error::Error>> {
    let id = backend.next_id();
    backend.send(id, command)?;
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id == Some(id)
                && let Some(found) = pick(event)
            {
                return Some(found);
            }
        }
        None
    })
    .await?;
    found.ok_or_else(|| "the event stream closed before the answer".into())
}

/// Open `path` and answer the `(doc, version)` it was opened as.
async fn open(
    backend: &LocalBackend,
    events: &mut EventRx,
    path: PathBuf,
) -> Result<(DocumentId, u64), Box<dyn std::error::Error>> {
    let command = Command::OpenDocument {
        path,
        language: None,
    };
    answer(backend, events, command, |event| match event {
        Event::Opened { doc, version } => Some((doc, version)),
        _ => None,
    })
    .await
}

#[test]
fn backend_error_displays() {
    assert_eq!(
        BackendError::Closed.to_string(),
        "the backend connection is closed"
    );
}

/// The event stream is single-consumer, and request ids never repeat: a
/// second `take_events` must not hand out a stream that silently splits the
/// answers, and two requests must never share an id.
#[tokio::test]
async fn local_backend_hands_out_its_events_once_and_ids_in_order() {
    let (backend, _snaps) = local(crate::session::SessionConfig::default());
    assert!(backend.take_events().is_some());
    assert!(backend.take_events().is_none());
    assert!(backend.take_events().is_none());

    let ids: Vec<u64> = (0..4).map(|_| backend.next_id().0).collect();
    assert!(ids.windows(2).all(|pair| pair[1] == pair[0] + 1), "{ids:?}");
}

#[tokio::test]
async fn local_backend_drives_open() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("hello.txt");
    std::fs::write(&path, "hello\n")?;

    let (session, mut events, _snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    // Startup producers may announce capability state first. Correlate the answer
    // instead of assuming this command owns the stream's first event.
    let opened = open(&backend, &mut events, path).await;
    assert!(
        opened.is_ok(),
        "local backend should drive the session to open the file: {opened:?}"
    );
    Ok(())
}

#[cfg(feature = "mdlint")]
#[tokio::test]
async fn opening_markdown_publishes_markdownlint_diagnostics() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("doc.md");
    // Trailing spaces (MD009) and a bare URL (MD034) under a proper title.
    std::fs::write(&path, "# Title\n\ntext \nhttps://example.com\n")?;

    let (session, mut events, _snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let id = backend.next_id();
    backend.send(
        id,
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let rules = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((_, event)) = events.recv().await {
            if let Event::DiagnosticsPublished { diagnostics, .. } = event
                && !diagnostics.is_empty()
            {
                return diagnostics
                    .iter()
                    .filter(|d| d.source.as_deref() == Some("markdownlint"))
                    .filter_map(|d| d.code.clone())
                    .collect::<Vec<_>>();
            }
        }
        Vec::new()
    })
    .await
    .unwrap_or_default();
    assert!(rules.contains(&"MD009".to_owned()), "found: {rules:?}");
    assert!(rules.contains(&"MD034".to_owned()), "found: {rules:?}");
    Ok(())
}

#[tokio::test]
async fn workspace_todo_scan_streams_codetag_hits() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("main.rs"),
        "// TODO: ship it\nfn main() {}\n",
    )?;

    let config = SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    };
    let (session, mut events, _snaps) = Session::new(config);
    let backend = local_session(session, None);
    let id = backend.next_id();
    backend.send(id, Command::ScanWorkspaceTodos { limit: 100 })?;
    let (mut tags, mut finished) = (Vec::new(), false);
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id != Some(id) {
                continue;
            }
            match event {
                Event::TodoScanProgress { hits, .. } => {
                    tags.extend(hits.into_iter().map(|h| (h.tag, h.message)));
                },
                Event::TodoScanFinished { .. } => {
                    finished = true;
                    break;
                },
                _ => {},
            }
        }
    })
    .await;
    assert!(finished, "the scan reports completion");
    assert!(
        tags.contains(&("TODO".to_owned(), "ship it".to_owned())),
        "found: {tags:?}"
    );
    Ok(())
}

#[tokio::test]
async fn local_backend_reports_an_exact_nested_repository_status() -> TestResult {
    if !git_available() {
        return Ok(()); // no `git` binary to build the fixture repository with
    }
    let dir = tempfile::tempdir()?;
    let nested = dir.path().join("nested");
    std::fs::create_dir_all(&nested)?;
    init_repo(&nested)?;
    std::fs::write(nested.join("file.txt"), "one\n")?;
    git(&nested, &["add", "file.txt"])?;
    git(&nested, &["commit", "-q", "-m", "initial"])?;
    std::fs::write(nested.join("file.txt"), "one\ntwo\n")?;

    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let command = Command::NestedRepositoryStatus {
        path: nested.clone(),
    };
    let (path, summary) = answer(&backend, &mut events, command, |event| match event {
        Event::NestedRepositoryStatus { path, summary } => Some((path, summary)),
        _ => None,
    })
    .await?;
    assert_eq!(path, nested);
    assert_eq!((summary.added, summary.removed), (1, 0));
    Ok(())
}

#[tokio::test]
async fn repository_actions_and_blame_run_off_actor() -> TestResult {
    use karet_core::BlameAttribution;
    use karet_vcs::CreateBranchOptions;

    use crate::api::VcsAction;

    if !git_available() {
        return Ok(()); // no `git` binary to build the fixture repository with
    }
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_path_buf();
    init_repo(&root)?;
    std::fs::write(root.join("code.rs"), "fn main() {}\n")?;
    git(&root, &["add", "code.rs"])?;
    git(&root, &["commit", "-q", "-m", "initial"])?;

    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.clone()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let (doc, version) = open(&backend, &mut events, root.join("code.rs")).await?;
    let blame = Command::Blame {
        doc,
        version,
        line: 0,
    };
    let blamed = answer(&backend, &mut events, blame, |event| match event {
        Event::BlameResult { attribution, .. } => Some(attribution),
        _ => None,
    })
    .await?;
    assert!(
        matches!(blamed, Some(BlameAttribution::Commit(_))),
        "{blamed:?}"
    );

    std::fs::write(root.join("untracked.rs"), "fn new_file() {}\n")?;
    let (untracked_doc, untracked_version) =
        open(&backend, &mut events, root.join("untracked.rs")).await?;
    let blame = Command::Blame {
        doc: untracked_doc,
        version: untracked_version,
        line: 0,
    };
    let unavailable = answer(&backend, &mut events, blame, Some).await?;
    assert!(
        matches!(
            unavailable,
            Event::BlameResult {
                attribution: None,
                ..
            }
        ),
        "{unavailable:?}"
    );

    let mut branch_options = CreateBranchOptions::default();
    branch_options.name = "feature".to_string();
    let create = Command::VcsAction {
        action: VcsAction::CreateBranch(branch_options),
    };
    let branch = answer(&backend, &mut events, create, |event| match event {
        Event::RepositorySnapshot { snapshot } => Some(snapshot.state.branch),
        _ => None,
    })
    .await?;
    assert_eq!(branch.as_deref(), Some("feature"));
    Ok(())
}

/// Drain snapshots until one satisfies `wanted`, or time out.
#[cfg(test)]
async fn await_snapshot(
    snaps: &mut crate::local::SnapshotRx,
    wanted: impl Fn(&crate::local::DocSnapshot) -> bool,
) -> bool {
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((_, snap)) = snaps.recv().await {
            if wanted(&snap) {
                return true;
            }
        }
        false
    })
    .await;
    found.unwrap_or(false)
}

#[tokio::test]
async fn injected_language_is_highlighted_through_the_worker() -> TestResult {
    use karet_core::TokenId;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("notes.md");
    // A markdown file whose fenced block is rust: only the injection machinery,
    // driven end to end through the worker, can colour `fn` as a keyword.
    std::fs::write(&path, "# T\n\n```rust\nfn main() {}\n```\n")?;

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let id = backend.next_id();
    assert!(
        backend
            .send(
                id,
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    // The open publishes immediately with no spans; the worker's answer follows.
    let highlighted = await_snapshot(&mut snaps, |snap| {
        snap.highlights
            .all()
            .iter()
            .any(|s| s.token == TokenId::KEYWORD)
    })
    .await;
    assert!(
        highlighted,
        "the embedded rust fence should eventually be highlighted"
    );
    Ok(())
}

#[tokio::test]
async fn syntax_error_lines_reach_the_snapshot_stream() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("broken.rs");
    // Line 1 (0-based) is broken; the completion gate reads these ranges.
    std::fs::write(&path, "fn ok() {}\nfn broken() { let x = ; }\n")?;

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    assert!(
        backend
            .send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    let flagged = await_snapshot(&mut snaps, |snap| {
        snap.syntax_error_lines
            .iter()
            .any(|&(start, end)| start <= 1 && 1 <= end)
    })
    .await;
    assert!(flagged, "the broken line should be flagged on the snapshot");
    Ok(())
}

#[tokio::test]
async fn markdown_table_lines_reach_the_snapshot_stream() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\n| a | b |\n|---|---|\n| 1 | 2 |\n")?;

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    assert!(
        backend
            .send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    let found = await_snapshot(&mut snaps, |snap| *snap.table_lines == [2..=4]).await;
    assert!(found, "the worker's table ranges should ride the snapshot");
    Ok(())
}

#[tokio::test]
async fn semantic_blocks_reach_the_snapshot_stream() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Top\n\n## Child\n\nbody\n")?;

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    assert!(
        backend
            .send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    let published = await_snapshot(&mut snaps, |snap| {
        snap.semantic_blocks.active_at(4).len() == 2
    })
    .await;
    assert!(
        published,
        "the Markdown H1/H2 chain should reach the UI snapshot"
    );
    Ok(())
}

#[tokio::test]
async fn document_symbols_fall_back_to_the_syntax_worker() -> TestResult {
    use karet_core::StandardToken;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("outline.rs");
    std::fs::write(&path, "mod café { pub struct Thé; }\n")?;

    let (session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let (doc, _) = open(&backend, &mut events, path).await?;
    assert!(
        await_snapshot(&mut snaps, |snapshot| snapshot
            .highlights
            .all()
            .iter()
            .any(|span| span.token == StandardToken::Keyword.id()))
        .await
    );

    let request = Command::DocumentSymbols { doc };
    let symbols = answer(&backend, &mut events, request, |event| match event {
        Event::Symbols { symbols, .. } => Some(symbols),
        _ => None,
    })
    .await?;
    assert_eq!(symbols[0].name, "café");
    assert_eq!(symbols[0].children[0].name, "Thé");
    Ok(())
}

#[tokio::test]
async fn todo_comments_are_marked_in_a_real_rust_buffer() -> TestResult {
    use karet_core::StandardToken;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "// TODO: fix bug here\n// context\nfn main() {}\n")?;

    // Default settings: `editor.semanticComments` is on.
    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    assert!(
        backend
            .send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    let mark = StandardToken::CommentMark.id();
    let marked = await_snapshot(&mut snaps, |snap| {
        snap.highlights.all().iter().any(|s| s.token == mark)
    })
    .await;
    assert!(
        marked,
        "the TODO comment block should be published as CommentMark"
    );
    Ok(())
}

#[tokio::test]
async fn disabling_semantic_comments_leaves_comments_plain() -> TestResult {
    use karet_core::StandardToken;
    use karet_core::TokenId;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("lib.rs");
    std::fs::write(&path, "// TODO: fix bug here\nfn main() {}\n")?;

    let mut config = SessionConfig::default();
    config.settings.editor.semantic_comments.enabled = false;
    let (session, _events, mut snaps) = Session::new(config);
    let backend = local_session(session, None);
    assert!(
        backend
            .send(
                backend.next_id(),
                Command::OpenDocument {
                    path,
                    language: None
                }
            )
            .is_ok()
    );

    // Wait for the worker's real answer: the snapshot that carries comment spans.
    let mark = StandardToken::CommentMark.id();
    let mut saw_mark = false;
    let highlighted = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((_, snap)) = snaps.recv().await {
            saw_mark |= snap.highlights.all().iter().any(|s| s.token == mark);
            if snap
                .highlights
                .all()
                .iter()
                .any(|s| s.token == TokenId::COMMENT)
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(highlighted, "the buffer should still be highlighted");
    assert!(
        !saw_mark,
        "with the setting off, no snapshot may carry CommentMark"
    );
    Ok(())
}

#[tokio::test]
async fn editing_republishes_highlights_for_the_new_text() -> TestResult {
    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::TextEdit;
    use karet_core::TokenId;
    use karet_text::EditCause;

    let dir = tempfile::tempdir()?;
    let path = dir.path().join("live.md");
    std::fs::write(&path, "text\n")?;

    let (session, mut events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let (doc, version) = open(&backend, &mut events, path).await?;

    // Type a rust code fence at the end of the buffer.
    let range = Range::new(LineCol::new(1, 0), LineCol::new(1, 0));
    let change = Change::new(
        version,
        vec![TextEdit {
            range,
            new_text: "\n```rust\nfn f() {}\n```\n".to_owned(),
        }],
    );
    let id = backend.next_id();
    assert!(backend.next_id() > id);
    backend.send(
        id,
        Command::ApplyChange {
            doc,
            change,
            cause: EditCause::Paste,
        },
    )?;

    // The fence did not exist a moment ago; the worker must discover the injection
    // and republish. This is the live-update contract.
    let highlighted = await_snapshot(&mut snaps, |snap| {
        snap.highlights
            .all()
            .iter()
            .any(|s| s.token == TokenId::KEYWORD)
    })
    .await;
    assert!(
        highlighted,
        "typing a code fence should light up the embedded language"
    );
    Ok(())
}

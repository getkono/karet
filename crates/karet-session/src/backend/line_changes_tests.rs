use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;
use karet_core::ThemeRole;

use super::*;
use crate::api::DocumentCommand;
use crate::api::DocumentId;
use crate::api::Event;
use crate::api::VcsCommand;
use crate::session::SessionConfig;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Send `command` and wait for the event answering it; an error when the send
/// fails, the wait times out or the stream ends first.
async fn answer(
    backend: &LocalBackend,
    events: &mut EventRx,
    command: impl Into<Command>,
) -> Result<Event, Box<dyn std::error::Error>> {
    let id = backend.next_id();
    backend.send(id, command.into())?;
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id == Some(id) {
                return Some(event);
            }
        }
        None
    })
    .await?;
    found.ok_or_else(|| "the event stream closed before the answer".into())
}

/// Each marker as `(first line, last line, role)`.
fn spans(markers: &[Decoration]) -> Vec<(u32, u32, Option<ThemeRole>)> {
    markers
        .iter()
        .map(|marker| (marker.range.start.line, marker.range.end.line, marker.role))
        .collect()
}

/// Whether a `git` binary runs here: the one environment precondition these
/// tests skip on (fixture failures past it fail the test).
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

/// A `LineChanges` answer as `(doc, version, marker spans)`.
type Answered = (DocumentId, u64, Vec<(u32, u32, Option<ThemeRole>)>);

/// Open `path`, failing unless the answer is `Opened`.
async fn opened(
    backend: &LocalBackend,
    events: &mut EventRx,
    path: PathBuf,
) -> Result<(DocumentId, u64), Box<dyn std::error::Error>> {
    let command = DocumentCommand::Open {
        path,
        language: None,
    };
    match answer(backend, events, command).await? {
        Event::Opened { doc, version } => Ok((doc, version)),
        event => Err(format!("expected Opened, got {event:?}").into()),
    }
}

/// Ask for `doc`'s line changes, answering `None` for a non-`LineChanges` event.
async fn line_changes(
    backend: &LocalBackend,
    events: &mut EventRx,
    doc: DocumentId,
) -> Result<Option<Answered>, Box<dyn std::error::Error>> {
    Ok(
        match answer(backend, events, VcsCommand::LineChanges { doc }).await? {
            Event::LineChanges {
                doc,
                version,
                markers,
            } => Some((doc, version, spans(&markers))),
            _ => None,
        },
    )
}

/// Open `path` in a session rooted at `root` and ask for its line changes,
/// answering `(opened doc, opened version, the LineChanges answer)`.
async fn line_changes_of(
    root: &Path,
    path: PathBuf,
) -> Result<(DocumentId, u64, Option<Answered>), Box<dyn std::error::Error>> {
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let (doc, version) = opened(&backend, &mut events, path).await?;
    let changed = line_changes(&backend, &mut events, doc).await?;
    Ok((doc, version, changed))
}

#[tokio::test]
async fn an_untracked_file_in_a_repository_has_no_markers() -> TestResult {
    if !git_available() {
        return Ok(()); // no `git` binary to build the fixture repository with
    }
    let dir = tempfile::tempdir()?;
    let tracked = dir.path().join("a.txt");
    let untracked = dir.path().join("new.txt");
    init_repo(dir.path())?;
    std::fs::write(&tracked, "one\n")?;
    git(dir.path(), &["add", "a.txt"])?;
    git(dir.path(), &["commit", "-q", "-m", "base"])?;
    std::fs::write(&untracked, "fresh\nlines\n")?;
    let (doc, version, changed) = line_changes_of(dir.path(), untracked).await?;
    assert_eq!(changed, Some((doc, version, Vec::new())));
    Ok(())
}

#[tokio::test]
async fn a_file_in_an_unborn_repository_has_no_markers() -> TestResult {
    if !git_available() {
        return Ok(()); // no `git` binary to build the fixture repository with
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.txt");
    init_repo(dir.path())?;
    std::fs::write(&path, "one\ntwo\n")?;
    let (doc, version, changed) = line_changes_of(dir.path(), path).await?;
    assert_eq!(changed, Some((doc, version, Vec::new())));
    Ok(())
}

#[tokio::test]
async fn line_changes_mark_the_buffer_against_head() -> TestResult {
    if !git_available() {
        return Ok(()); // no `git` binary to build the fixture repository with
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.txt");
    init_repo(dir.path())?;
    std::fs::write(&path, "one\ntwo\nthree\n")?;
    git(dir.path(), &["add", "a.txt"])?;
    git(dir.path(), &["commit", "-q", "-m", "base"])?;
    std::fs::write(&path, "one\nTWO\nthree\nfour\n")?;
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let (doc, version) = opened(&backend, &mut events, path).await?;

    let changed = line_changes(&backend, &mut events, doc).await?;
    assert_eq!(
        changed,
        Some((
            doc,
            version,
            vec![
                (1, 1, Some(ThemeRole::GutterModified)),
                (3, 3, Some(ThemeRole::GutterAdded)),
            ]
        ))
    );

    // A document closed before the request lands answers with nothing to mark.
    let stale = DocumentId(doc.0 + 1000);
    let unknown = answer(
        &backend,
        &mut events,
        VcsCommand::LineChanges { doc: stale },
    )
    .await?;
    assert!(
        matches!(
            &unknown,
            Event::LineChanges { doc, markers, .. } if *doc == stale && markers.is_empty()
        ),
        "{unknown:?}"
    );
    Ok(())
}

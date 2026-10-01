use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;
use karet_core::ThemeRole;

use super::*;
use crate::api::DocumentId;
use crate::api::Event;
use crate::session::SessionConfig;

/// Send `command` and wait for the event answering it.
async fn answer(backend: &LocalBackend, events: &mut EventRx, command: Command) -> Option<Event> {
    let id = backend.next_id();
    backend.send(id, command).ok()?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((event_id, event)) = events.recv().await {
            if event_id == Some(id) {
                return Some(event);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Each marker as `(first line, last line, role)`.
fn spans(markers: &[Decoration]) -> Vec<(u32, u32, Option<ThemeRole>)> {
    markers
        .iter()
        .map(|marker| (marker.range.start.line, marker.range.end.line, marker.role))
        .collect()
}

/// Run `git args` in `dir`, reporting whether it succeeded.
fn git(dir: &Path, args: &[&str]) -> bool {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .ok()
        .is_some_and(|status| status.success())
}

/// `git init` in `dir` with a committer identity; `false` when git is unavailable.
fn init_repo(dir: &Path) -> bool {
    git(dir, &["init", "-q"])
        && git(dir, &["config", "user.email", "test@example.com"])
        && git(dir, &["config", "user.name", "karet test"])
}

/// A `LineChanges` answer as `(doc, version, marker spans)`.
type Answered = (DocumentId, u64, Vec<(u32, u32, Option<ThemeRole>)>);

/// Open `path` in a session rooted at `root` and ask for its line changes,
/// answering `(opened doc, opened version, the LineChanges answer)`.
async fn line_changes_of(
    root: &Path,
    path: PathBuf,
) -> Option<(DocumentId, u64, Option<Answered>)> {
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let opened = answer(
        &backend,
        &mut events,
        Command::OpenDocument {
            path,
            language: None,
        },
    )
    .await;
    let Some(Event::Opened { doc, version }) = opened else {
        return None;
    };
    let changed = match answer(&backend, &mut events, Command::LineChanges { doc }).await {
        Some(Event::LineChanges {
            doc,
            version,
            markers,
        }) => Some((doc, version, spans(&markers))),
        _ => None,
    };
    Some((doc, version, changed))
}

#[tokio::test]
async fn an_untracked_file_in_a_repository_has_no_markers() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let tracked = dir.path().join("a.txt");
    let untracked = dir.path().join("new.txt");
    if !init_repo(dir.path())
        || std::fs::write(&tracked, "one\n").is_err()
        || !git(dir.path(), &["add", "a.txt"])
        || !git(dir.path(), &["commit", "-q", "-m", "base"])
        || std::fs::write(&untracked, "fresh\nlines\n").is_err()
    {
        return;
    }
    let Some((doc, version, changed)) = line_changes_of(dir.path(), untracked).await else {
        return;
    };
    assert_eq!(changed, Some((doc, version, Vec::new())));
}

#[tokio::test]
async fn a_file_in_an_unborn_repository_has_no_markers() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("a.txt");
    if !init_repo(dir.path()) || std::fs::write(&path, "one\ntwo\n").is_err() {
        return;
    }
    let Some((doc, version, changed)) = line_changes_of(dir.path(), path).await else {
        return;
    };
    assert_eq!(changed, Some((doc, version, Vec::new())));
}

#[tokio::test]
async fn line_changes_mark_the_buffer_against_head() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("a.txt");
    if !init_repo(dir.path())
        || std::fs::write(&path, "one\ntwo\nthree\n").is_err()
        || !git(dir.path(), &["add", "a.txt"])
        || !git(dir.path(), &["commit", "-q", "-m", "base"])
        || std::fs::write(&path, "one\nTWO\nthree\nfour\n").is_err()
    {
        return;
    }
    let (session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![dir.path().to_path_buf()],
        ..SessionConfig::default()
    });
    let backend = local_session(session, None);
    let opened = answer(
        &backend,
        &mut events,
        Command::OpenDocument {
            path,
            language: None,
        },
    )
    .await;
    let Some(Event::Opened { doc, version }) = opened else {
        return;
    };

    let changed = match answer(&backend, &mut events, Command::LineChanges { doc }).await {
        Some(Event::LineChanges {
            doc,
            version,
            markers,
        }) => Some((doc, version, spans(&markers))),
        _ => None,
    };
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
    let unknown = answer(&backend, &mut events, Command::LineChanges { doc: stale }).await;
    assert!(matches!(
        unknown,
        Some(Event::LineChanges { doc, markers, .. }) if doc == stale && markers.is_empty()
    ));
}

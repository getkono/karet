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

#[tokio::test]
async fn line_changes_mark_the_buffer_against_head() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .ok()
            .is_some_and(|status| status.success())
    };
    let path = dir.path().join("a.txt");
    if !git(&["init", "-q"])
        || !git(&["config", "user.email", "test@example.com"])
        || !git(&["config", "user.name", "karet test"])
        || std::fs::write(&path, "one\ntwo\nthree\n").is_err()
        || !git(&["add", "a.txt"])
        || !git(&["commit", "-q", "-m", "base"])
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

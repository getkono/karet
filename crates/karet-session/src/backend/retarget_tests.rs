use std::time::Duration;

use super::*;
use crate::api::DocumentId;
use crate::local::DocSnapshot;
use crate::local::SnapshotRx;
use crate::session::Session;
use crate::session::SessionConfig;

/// The first snapshot `wanted` accepts within ten seconds, with its document.
async fn next_snapshot(
    snaps: &mut SnapshotRx,
    wanted: impl Fn(&DocSnapshot) -> bool,
) -> Option<DocumentId> {
    let found = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some((doc, snap)) = snaps.recv().await {
            if wanted(&snap) {
                return Some(doc);
            }
        }
        None
    })
    .await;
    found.ok().flatten()
}

/// Retargeting to a path with no grammar gets no worker answer, so the session
/// itself must drop the table ranges the previous (Markdown) grammar produced.
#[tokio::test]
async fn retargeting_to_a_plaintext_path_clears_markdown_table_lines() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("notes.md");
    if std::fs::write(&path, "# Notes\n\n| a | b |\n|---|---|\n| 1 | 2 |\n").is_err() {
        return;
    }

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let open = Command::OpenDocument {
        path,
        language: None,
    };
    assert!(backend.send(backend.next_id(), open).is_ok());

    let doc = next_snapshot(&mut snaps, |snap| !snap.table_lines.is_empty()).await;
    assert!(
        doc.is_some(),
        "the Markdown file never published table ranges"
    );
    let Some(doc) = doc else {
        return;
    };

    let retarget = Command::RetargetDocument {
        doc,
        path: dir.path().join("notes.txt"),
    };
    assert!(backend.send(backend.next_id(), retarget).is_ok());
    let cleared = next_snapshot(&mut snaps, |snap| snap.table_lines.is_empty()).await;
    assert!(
        cleared.is_some(),
        "a path with no grammar should drop the stale table ranges"
    );
}

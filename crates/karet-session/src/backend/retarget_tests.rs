use std::time::Duration;

use super::*;
use crate::api::DocumentCommand;
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
async fn retargeting_to_a_plaintext_path_clears_markdown_table_lines()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\n| a | b |\n|---|---|\n| 1 | 2 |\n")?;

    let (session, _events, mut snaps) = Session::new(SessionConfig::default());
    let backend = local_session(session, None);
    let open = DocumentCommand::Open {
        path,
        language: None,
    };
    backend.send(backend.next_id(), open.into())?;

    let doc = next_snapshot(&mut snaps, |snap| !snap.table_lines.is_empty())
        .await
        .ok_or("the Markdown file never published table ranges")?;

    let retarget = DocumentCommand::Retarget {
        doc,
        path: dir.path().join("notes.txt"),
    };
    backend.send(backend.next_id(), retarget.into())?;
    let cleared = next_snapshot(&mut snaps, |snap| snap.table_lines.is_empty()).await;
    assert!(
        cleared.is_some(),
        "a path with no grammar should drop the stale table ranges"
    );
    Ok(())
}

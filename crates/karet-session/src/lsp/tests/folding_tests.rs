//! Folding ranges through the server task and the session: the primary
//! server is asked at every version it is sent, and the session's snapshots
//! carry its regions in place of the grammar's.

use karet_core::FoldKind;
use karet_core::FoldRegion;
use karet_text::TextBuffer;
use serde_json::Value;
use serde_json::json;

use super::*;

/// The double's answer to `textDocument/foldingRange`: a `region` over lines
/// 0..=1 and a `comment` block over 3..=4, out of order, beside an entry that
/// is no range.
pub(super) fn double_reply() -> Value {
    json!([
        {"startLine": 3, "endLine": 4, "kind": "comment"},
        {"startLine": 0, "endLine": 1, "kind": "region"},
        {"kind": "imports"},
    ])
}

/// The regions [`double_reply`] maps to.
fn served() -> [FoldRegion; 2] {
    [
        FoldRegion::new(0, 1).with_kind(FoldKind::Region),
        FoldRegion::new(3, 4).with_kind(FoldKind::Comment),
    ]
}

/// The next folding-range answer the server task relays.
async fn next_folds(
    updates: &mut mpsc::UnboundedReceiver<LspUpdate>,
) -> Option<(PathBuf, i32, Vec<FoldRegion>)> {
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .ok()??;
        if let LspUpdate::FoldingRanges {
            path,
            version,
            folds,
            ..
        } = update
        {
            return Some((path, version, folds.regions().to_vec()));
        }
    }
}

/// Opening a document asks for its ranges at the opened version, and an edit
/// asks again at the edited one, each answer mapped and normalized.
/// Falsified by asking only once per document, or by stamping the answer
/// with a version the server had not been sent.
#[tokio::test]
async fn the_primary_is_asked_at_every_version() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/folds.rs"));
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Folds,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let mut buffer = TextBuffer::from_text("fn a() {\n}\n");
    let text = buffer.text();
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, buffer.version(), || text);
    let opened = crate::lsp::version_i32(buffer.version());
    let (own, version, regions) = next_folds(&mut updates).await.ok_or("no folds")?;
    assert_eq!((own.as_path(), version), (path.as_path(), opened));
    assert_eq!(regions, served());

    let applied = buffer
        .apply_simple(&karet_core::Change::new(
            buffer.version(),
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(1, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "x\n".to_owned(),
            }],
        ))
        .ok()
        .ok_or("the edit did not apply")?;
    let _ = manager.document_changed(Some("rust"), &path, &buffer, Some(&applied));
    let (_, version, _) = next_folds(&mut updates).await.ok_or("not asked again")?;
    assert_eq!(version, crate::lsp::version_i32(buffer.version()));
    Ok(())
}

/// A server that does not offer folding ranges is never asked: the request
/// is refused before the wire, so no answer is relayed and the grammar's
/// folds stand. Falsified by fetching without consulting the capability.
#[tokio::test]
async fn a_server_without_folding_ranges_is_not_asked() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/unfolded.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn a() {}\n".into()
    });
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(400), observed.recv()).await
    {
        assert_ne!(message["method"], "textDocument/foldingRange");
    }
    Ok(())
}

/// Through the session: the snapshot's folds become the server's regions,
/// kinds and all, in place of the grammar's. Falsified by publishing the
/// grammar's folds alone (which never carry a `Region` kind).
#[tokio::test]
async fn a_snapshot_carries_the_server_folds() -> TestResult {
    let dir = tempfile::tempdir()?;
    let text = "fn a() {\n    b();\n}\n// one\n// two\n";
    let path = rust_file(&dir, "main.rs", text).ok_or("write failed")?;
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    session.set_lsp_connector(test_connector(
        Behavior::Folds,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let carried = async {
        while let Some((id, snap)) = snaps.recv().await {
            if id == doc && snap.folds.regions() == served() {
                return true;
            }
        }
        false
    };
    assert!(tokio::time::timeout(Duration::from_secs(10), carried).await?);
    Ok(())
}

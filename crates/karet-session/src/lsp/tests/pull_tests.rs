//! Pull diagnostics through the server task: every version the server is sent
//! is pulled, and the answer joins the diagnostic stream stamped with it.

use karet_core::Diagnostic;
use karet_text::TextBuffer;

use super::*;

/// The next diagnostic set the server task relays, whichever file it is for.
async fn next_set(
    updates: &mut mpsc::UnboundedReceiver<LspUpdate>,
) -> Option<(PathBuf, Option<i32>, Vec<Diagnostic>)> {
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .ok()??;
        if let LspUpdate::Diagnostics {
            path,
            version,
            diagnostics,
            ..
        } = update
        {
            return Some((path, version, diagnostics));
        }
    }
}

/// Opening a document pulls it; an edit pulls it again at the new version.
/// Each answer is stamped with the version pulled, carries the code's link
/// and related information, and is followed by the related document's set.
/// Falsified by pulling before the edit was flushed (the double would name
/// the older version), or by never re-asking after a change.
#[tokio::test]
async fn an_open_document_is_pulled_at_every_version_it_is_sent() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/pulled.rs"));
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Pulls,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let mut buffer = TextBuffer::from_text("fn main() {}\nlet x;\n");
    let text = buffer.text();
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, buffer.version(), || text);

    let opened = crate::lsp::version_i32(buffer.version());
    let (own, version, diagnostics) = next_set(&mut updates).await.ok_or("no pulled set")?;
    assert_eq!(own, path);
    assert_eq!(version, Some(opened));
    let diagnostic = diagnostics.first().ok_or("an empty pulled set")?;
    assert_eq!(diagnostic.message, format!("pulled at version {opened}"));
    assert_eq!(
        diagnostic.code_href.as_deref(),
        Some("https://example.com/E1")
    );
    let related = diagnostic.related.first().ok_or("no related information")?;
    assert_eq!(related.location.path, path);
    assert_eq!(related.message, "declared here");

    let (elsewhere, version, diagnostics) = next_set(&mut updates).await.ok_or("no related set")?;
    assert_eq!(elsewhere, path.with_file_name("elsewhere.rs"));
    assert_eq!(version, None);
    assert_eq!(diagnostics.len(), 1);

    let applied = buffer
        .apply_simple(&karet_core::Change::new(
            buffer.version(),
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "// edited\n".to_owned(),
            }],
        ))
        .ok()
        .ok_or("the edit did not apply")?;
    let _ = manager.document_changed(Some("rust"), &path, &buffer, Some(&applied));
    let edited = crate::lsp::version_i32(buffer.version());
    assert_ne!(edited, opened);
    let (own, version, diagnostics) = next_set(&mut updates).await.ok_or("no second pull")?;
    assert_eq!(own, path);
    assert_eq!(version, Some(edited));
    assert_eq!(
        diagnostics.first().map(|d| d.message.as_str()),
        Some(format!("pulled at version {edited}").as_str())
    );
    Ok(())
}

/// A server that never offered pull diagnostics is never asked for them.
#[tokio::test]
async fn a_server_without_pull_diagnostics_is_not_asked() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/unpulled.rs"));
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}\n".to_owned()
    });
    let mut opened = false;
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(500), observed_rx.recv()).await
    {
        opened |= message["method"] == "textDocument/didOpen";
        assert_ne!(message["method"], "textDocument/diagnostic");
    }
    assert!(opened, "the document never reached the server");
    Ok(())
}

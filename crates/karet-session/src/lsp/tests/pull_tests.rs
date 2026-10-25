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
///
/// On tokio's virtual clock, so the silence window proves absence: pulls run on
/// spawned tasks against an in-process server, and idle time advances only once
/// every one of them has run, so a pull already launched reaches the wire first.
#[tokio::test(start_paused = true)]
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

/// A server that registers pull diagnostics only after a document is open is
/// pulled for that document as soon as the registration arrives, with no
/// further command to wake the server task. Falsified by launching pulls only
/// when a command, flush, or answer happens to wake the task.
#[tokio::test]
async fn a_dynamic_registration_pulls_the_documents_already_open() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/registered.rs"));
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::RegistersPulls,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}\n".to_owned()
    });
    let (own, version, diagnostics) = next_set(&mut updates)
        .await
        .ok_or("the registration never led to a pull")?;
    assert_eq!(own, path);
    assert_eq!(version, Some(crate::lsp::version_i32(1)));
    assert_eq!(
        diagnostics.first().map(|d| d.message.as_str()),
        Some(format!("pulled at version {}", crate::lsp::version_i32(1)).as_str())
    );
    Ok(())
}

/// A document whose last change failed to send is not pulled: the server's
/// copy is behind, so an answer stamped with the sent version would describe
/// other text. The change that resyncs it is pulled at its version. Falsified
/// by dropping the `resync` skip in `Pulls::launch_ready`.
///
/// On tokio's virtual clock, for the reason
/// `a_server_without_pull_diagnostics_is_not_asked` gives: its silence window
/// closes only once every launched pull has had its turn.
#[tokio::test(start_paused = true)]
async fn a_document_awaiting_resync_is_not_pulled_until_a_change_resyncs_it() -> TestResult {
    use super::super::commands::OpenDocument;
    use super::super::commands::remember_document;
    use super::super::pull::Pulls;

    let path = crate::lsp::absolute_path(Path::new("/tmp/behind.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let connector = test_connector(
        Behavior::Pulls,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    );
    let spec = LspSpec::new("double", Vec::new(), vec!["rust".into()]);
    let client = Arc::new(connector(spec, PathBuf::from("/tmp")).await?);
    client.did_open(&path, "rust", 2, "fn main() {}\n").await?;

    // Version 2 was recorded, but its `didChange` failed to send.
    let mut documents = HashMap::new();
    documents.insert(
        path.clone(),
        OpenDocument {
            language: "rust".into(),
            version: 2,
            text: "fn main() {}\n".into(),
            lines_agree: true,
            resync: true,
        },
    );
    let (updates_tx, mut updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    pulls.launch_ready(&client, &documents, None, slot);
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(300), observed.recv()).await
    {
        assert_ne!(
            message["method"], "textDocument/diagnostic",
            "a document the server is behind on was pulled"
        );
    }

    // The next change goes out whole and resyncs the server.
    let mut change = ServerCmd::DidChange {
        path: path.clone(),
        version: 3,
        text: "fn main() {}\n// more\n".into(),
        delta: None,
    };
    remember_document(&mut documents, &mut change);
    client
        .did_change(&path, 3, "fn main() {}\n// more\n")
        .await?;
    pulls.launch_ready(&client, &documents, None, slot);
    let (own, version, _) = next_set(&mut updates).await.ok_or("never pulled")?;
    assert_eq!(own, path);
    assert_eq!(version, Some(3));
    pulls.shutdown().await;
    Ok(())
}

/// A server that publishes and answers pulls keeps both sets on screen: a
/// pull's answer replaces only the pulled layer, so it never erases what the
/// server published (rust-analyzer's `cargo check` results), across an edit
/// too. Falsified by keeping one layer per server, where the last set to
/// arrive wins.
#[tokio::test]
async fn pulled_and_pushed_sets_from_one_server_are_shown_together() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "both.rs", "fn main() {}\nlet x;\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let connector = test_connector(Behavior::PushesAndPulls, None, spawns);
    let (session, mut events) = session_with_connector(connector);
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    let opened = format!("pulled at version {}", crate::lsp::version_i32(version));
    both_shown(&mut events, &opened).await?;

    backend.send(
        backend.next_id(),
        Command::ApplyChange {
            doc,
            change: Change::new(
                version,
                vec![TextEdit {
                    range: Range {
                        start: LineCol::new(0, 0),
                        end: LineCol::new(0, 0),
                    },
                    new_text: "// edited\n".to_owned(),
                }],
            ),
            cause: EditCause::Type,
        },
    )?;
    let edited = format!("pulled at version {}", crate::lsp::version_i32(version + 1));
    both_shown(&mut events, &edited).await
}

/// Wait until one published set holds both the pulled diagnostic `pulled`
/// and the server's pushed `checked` one.
async fn both_shown(events: &mut EventRx, pulled: &str) -> TestResult {
    loop {
        let (_, event) = next_event(events)
            .await
            .ok_or("the pulled and pushed sets were never shown together")?;
        if let Event::DiagnosticsPublished { diagnostics, .. } = event
            && diagnostics.iter().any(|d| d.message == pulled)
            && diagnostics.iter().any(|d| d.message == "checked")
        {
            return Ok(());
        }
    }
}

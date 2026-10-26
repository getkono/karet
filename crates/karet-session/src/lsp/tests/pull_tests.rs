//! Pull diagnostics through the server task: every version the server is sent
//! is pulled, and the answer joins the diagnostic stream stamped with it.

use std::collections::HashSet;

use karet_core::Diagnostic;
use karet_text::TextBuffer;
use serde_json::Value;

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

/// The file name a diagnostic pull the double observed asked about, if
/// `message` is one.
fn pulled_uri(message: &Value) -> Option<String> {
    (message["method"] == "textDocument/diagnostic").then(|| {
        let uri = message["params"]["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default();
        uri.rsplit('/').next().unwrap_or_default().to_owned()
    })
}

/// Open `first.rs` and `second.rs` on a manager serving `behavior`, wait
/// until each has been pulled, then edit `first.rs`. Returns the manager (to
/// keep its server alive) and the observed stream positioned after the edit
/// reached the server.
async fn edit_one_of_two(
    behavior: Behavior,
) -> Result<(LspManager, mpsc::UnboundedReceiver<Value>), Box<dyn std::error::Error + Send + Sync>>
{
    let first = crate::lsp::absolute_path(Path::new("/tmp/first.rs"));
    let second = crate::lsp::absolute_path(Path::new("/tmp/second.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        behavior,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let mut buffer = TextBuffer::from_text("fn main() {}\n");
    for path in [&first, &second] {
        let text = buffer.text();
        let _ =
            manager.document_opened(Some("rust"), Some("rust"), path, buffer.version(), || text);
    }
    let mut unpulled = vec!["first.rs".to_owned(), "second.rs".to_owned()];
    while !unpulled.is_empty() {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("the server went away")?;
        if let Some(pulled) = pulled_uri(&message) {
            unpulled.retain(|uri| *uri != pulled);
        }
    }

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
    let _ = manager.document_changed(Some("rust"), &first, &buffer, Some(&applied));
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("the server went away")?;
        if message["method"] == "textDocument/didChange" {
            return Ok((manager, observed));
        }
    }
}

/// A server whose provider declares `interFileDependencies` has every open
/// document pulled again when one is edited: the untouched file's
/// diagnostics may depend on the edited one. Falsified by pulling only the
/// document whose version moved.
#[tokio::test]
async fn an_edit_re_pulls_every_open_document_across_inter_file_dependencies() -> TestResult {
    let (_manager, mut observed) = edit_one_of_two(Behavior::PullsAcrossFiles).await?;
    let mut unpulled = vec!["first.rs".to_owned(), "second.rs".to_owned()];
    while !unpulled.is_empty() {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("the server went away")?;
        if let Some(pulled) = pulled_uri(&message) {
            unpulled.retain(|uri| *uri != pulled);
        }
    }
    Ok(())
}

/// Without `interFileDependencies`, an edit pulls only the edited document.
/// Falsified by re-pulling every open document on any edit.
///
/// On tokio's virtual clock, for the reason
/// `a_server_without_pull_diagnostics_is_not_asked` gives.
#[tokio::test(start_paused = true)]
async fn an_edit_re_pulls_only_its_own_document_without_inter_file_dependencies() -> TestResult {
    let (_manager, mut observed) = edit_one_of_two(Behavior::Pulls).await?;
    let mut pulled = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(500), observed.recv()).await
    {
        pulled.extend(pulled_uri(&message));
    }
    assert_eq!(pulled, vec!["first.rs".to_owned()]);
    Ok(())
}

/// A client connected to a double serving `behavior`, and the stream of what
/// the double observed.
async fn connected(
    behavior: Behavior,
) -> Result<
    (Arc<karet_lsp::LspClient>, mpsc::UnboundedReceiver<Value>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let (observed_tx, observed) = mpsc::unbounded_channel();
    let connector = test_connector(behavior, Some(observed_tx), Arc::new(AtomicUsize::new(0)));
    let spec = LspSpec::new("double", Vec::new(), vec!["rust".into()]);
    Ok((
        Arc::new(connector(spec, PathBuf::from("/tmp")).await?),
        observed,
    ))
}

/// Open `name` under `/tmp` on `client` at version 1, and record it in
/// `documents` as the server task would. Returns its path.
async fn open_at_one(
    client: &karet_lsp::LspClient,
    documents: &mut HashMap<PathBuf, super::super::commands::OpenDocument>,
    name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = crate::lsp::absolute_path(&Path::new("/tmp").join(name));
    client.did_open(&path, "rust", 1, "fn main() {}\n").await?;
    documents.insert(
        path.clone(),
        super::super::commands::OpenDocument {
            language: "rust".into(),
            version: 1,
            text: "fn main() {}\n".into(),
            lines_agree: true,
            resync: false,
        },
    );
    Ok(path)
}

/// Wait until the double has been asked to pull every file in `names`.
async fn pulled_all(observed: &mut mpsc::UnboundedReceiver<Value>, names: &[&str]) -> Option<()> {
    let mut unpulled = names.to_vec();
    while !unpulled.is_empty() {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await
            .ok()
            .flatten()?;
        if let Some(pulled) = pulled_uri(&message) {
            unpulled.retain(|name| *name != pulled);
        }
    }
    Some(())
}

/// `workspace/diagnostic/refresh` wakes the pulls and marks what was asked
/// stale, so every open document is pulled again at the version it already
/// had. Falsified by not waking on the refresh, or by waking without marking
/// (nothing would be pulled: no version moved).
#[tokio::test]
async fn a_diagnostic_refresh_re_pulls_every_open_document() -> TestResult {
    use super::super::pull::Pulls;

    let (client, mut observed) = connected(Behavior::RefreshesPulls).await?;
    let mut documents = HashMap::new();
    let one = open_at_one(&client, &mut documents, "one.rs").await?;
    open_at_one(&client, &mut documents, "two.rs").await?;
    let (updates_tx, _updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs", "two.rs"])
        .await
        .ok_or("never pulled")?;

    client.did_save(&one, None).await?;
    tokio::time::timeout(Duration::from_secs(5), pulls.woken()).await?;
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs", "two.rs"])
        .await
        .ok_or("the refresh pulled nothing again")?;
    pulls.shutdown().await;
    Ok(())
}

/// A refresh that lands while an edit to an `interFileDependencies` document
/// is still buffered does not lose that edit: once it is flushed, every
/// other open document is pulled again, since its diagnostics were computed
/// against the text before the edit. Falsified by a refresh forgetting the
/// versions asked, which makes the flushed edit read as a first open.
#[tokio::test]
async fn an_edit_buffered_across_a_refresh_still_re_pulls_its_dependents() -> TestResult {
    use super::super::pull::Pulls;

    let (client, mut observed) = connected(Behavior::RefreshesPulls).await?;
    let mut documents = HashMap::new();
    let one = open_at_one(&client, &mut documents, "one.rs").await?;
    let two = open_at_one(&client, &mut documents, "two.rs").await?;
    let (updates_tx, _updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs", "two.rs"])
        .await
        .ok_or("never pulled")?;

    // `one.rs` is edited, but the edit is still buffered when the refresh
    // arrives: only `two.rs` can be pulled then.
    if let Some(document) = documents.get_mut(&one) {
        document.version = 2;
    }
    client.did_save(&two, None).await?;
    tokio::time::timeout(Duration::from_secs(5), pulls.woken()).await?;
    pulls.launch_ready(&client, &documents, Some(&one), slot);
    pulled_all(&mut observed, &["two.rs"])
        .await
        .ok_or("the refresh did not re-pull the document it could")?;

    client
        .did_change(&one, 2, "// edited\nfn main() {}\n")
        .await?;
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs", "two.rs"])
        .await
        .ok_or("the flushed edit did not re-pull its dependents")?;
    pulls.shutdown().await;
    Ok(())
}

/// Opening a document on an `interFileDependencies` server pulls only that
/// document: an open changes no other file's text (Decision 4). Falsified by
/// treating a document never pulled as edited, which would pull `one.rs`
/// again.
///
/// On tokio's virtual clock, for the reason
/// `a_server_without_pull_diagnostics_is_not_asked` gives.
#[tokio::test(start_paused = true)]
async fn a_first_open_does_not_re_pull_the_other_documents() -> TestResult {
    use super::super::pull::Pulls;

    let (client, mut observed) = connected(Behavior::PullsAcrossFiles).await?;
    let mut documents = HashMap::new();
    open_at_one(&client, &mut documents, "one.rs").await?;
    let (updates_tx, _updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs"])
        .await
        .ok_or("never pulled")?;

    open_at_one(&client, &mut documents, "two.rs").await?;
    pulls.launch_ready(&client, &documents, None, slot);
    let mut pulled = Vec::new();
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(500), observed.recv()).await
    {
        pulled.extend(pulled_uri(&message));
    }
    assert_eq!(pulled, vec!["two.rs".to_owned()]);
    pulls.shutdown().await;
    Ok(())
}

/// A refresh of something other than diagnostics (here inlay hints) does not
/// wake the pulls. Falsified by waking on every refresh, which would re-run
/// `launch_ready` for nothing. The refresh is seen arriving on a receiver of
/// the test's own, so the silence is not a refresh that never came.
///
/// On tokio's virtual clock, for the reason
/// `a_server_without_pull_diagnostics_is_not_asked` gives.
#[tokio::test(start_paused = true)]
async fn a_refresh_of_another_feature_does_not_wake_the_pulls() -> TestResult {
    use super::super::pull::Pulls;

    let (client, _observed) = connected(Behavior::RefreshesHints).await?;
    let documents = HashMap::new();
    let (updates_tx, _updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    // Subscribes the pulls to the connection's refreshes.
    pulls.launch_ready(&client, &documents, None, slot);
    let mut seen = client.refreshes();

    // The double asks for an inlay-hint refresh on every open.
    client
        .did_open(Path::new("/tmp/hinted.rs"), "rust", 1, "fn main() {}\n")
        .await?;
    let woke = tokio::time::timeout(Duration::from_millis(500), pulls.woken()).await;
    assert!(woke.is_err(), "an inlay-hint refresh woke the pulls");
    assert!(
        matches!(seen.try_recv(), Ok(karet_lsp::ServerRefresh::InlayHints)),
        "the double's refresh never arrived"
    );
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

/// Wait on `pulls` until the server covers `path` with pull diagnostics.
async fn registered(
    pulls: &mut super::super::pull::Pulls,
    client: &karet_lsp::LspClient,
    path: &Path,
) -> Option<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !client.supports_for(karet_core::ServerFeature::PullDiagnostics, path) {
            pulls.woken().await;
        }
    })
    .await
    .ok()
}

/// The next update the pulls relay that is not a diagnostic set's.
async fn next_clear(updates: &mut mpsc::UnboundedReceiver<LspUpdate>) -> Option<LspUpdate> {
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .ok()??;
        if !matches!(update, LspUpdate::Diagnostics { .. }) {
            return Some(update);
        }
    }
}

/// A `client/unregisterCapability` for `textDocument/diagnostic` clears the
/// slot's pulled layer -- for a document already closed too, whose related
/// reports may still be shown elsewhere -- and the withdrawn document is not
/// pulled again. Falsified by never checking for a withdrawal, or by
/// checking only the documents still open.
///
/// On tokio's virtual clock, for the reason
/// `a_server_without_pull_diagnostics_is_not_asked` gives.
#[tokio::test(start_paused = true)]
async fn withdrawing_pull_support_clears_the_pulled_layer() -> TestResult {
    use super::super::pull::Pulls;

    let (client, mut observed) = connected(Behavior::RegistersPulls).await?;
    let mut documents = HashMap::new();
    let one = open_at_one(&client, &mut documents, "one.rs").await?;
    let (updates_tx, mut updates) = mpsc::unbounded_channel();
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/tmp");
    let slot = (&updates_tx, &key, SlotToken::default());
    let mut pulls = Pulls::default();
    pulls.launch_ready(&client, &documents, None, slot);
    registered(&mut pulls, &client, &one)
        .await
        .ok_or("the double never registered pulls")?;
    pulls.launch_ready(&client, &documents, None, slot);
    pulled_all(&mut observed, &["one.rs"])
        .await
        .ok_or("never pulled")?;
    let (own, _, _) = next_set(&mut updates).await.ok_or("no pulled set")?;
    assert_eq!(own, one);

    // Closed before the withdrawal: its related report outlives it.
    client.did_save(&one, None).await?;
    documents.clear();
    tokio::time::timeout(Duration::from_secs(5), pulls.woken()).await?;
    pulls.launch_ready(&client, &documents, None, slot);
    match next_clear(&mut updates).await {
        Some(LspUpdate::PulledDiagnosticsWithdrawn { server, .. }) => assert_eq!(server, key),
        _ => return Err("the withdrawal cleared nothing".into()),
    }

    // Open again, it is not pulled: nothing covers it any more. Recorded
    // without a `didOpen`, which would have the double register pulls anew.
    documents.insert(
        one.clone(),
        super::super::commands::OpenDocument {
            language: "rust".into(),
            version: 2,
            text: "fn main() {}\n".into(),
            lines_agree: true,
            resync: false,
        },
    );
    pulls.launch_ready(&client, &documents, None, slot);
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(500), observed.recv()).await
    {
        assert_eq!(
            pulled_uri(&message),
            None,
            "a withdrawn document was pulled"
        );
    }
    assert!(
        updates.try_recv().is_err(),
        "the pulled layer was cleared or relayed again"
    );
    pulls.shutdown().await;
    Ok(())
}

/// End to end: once the server withdraws pull diagnostics, each open
/// document's pulled set leaves the screen -- the related document's, put
/// there by another document's pull, as well as the withdrawn document's
/// own. Falsified by leaving the pulled layer in place, or by clearing only
/// the documents whose own pull was withdrawn.
#[tokio::test]
async fn withdrawn_pull_diagnostics_leave_every_document() -> TestResult {
    let dir = tempfile::tempdir()?;
    let asking = rust_file(&dir, "asking.rs", "fn main() {}\nlet x;\n").ok_or("write failed")?;
    let elsewhere = rust_file(&dir, "elsewhere.rs", "fn other() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let connector = test_connector(Behavior::RegistersPulls, None, spawns);
    let (session, mut events) = session_with_connector(connector);
    let backend = local_session(session, None);
    for path in [asking, elsewhere] {
        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
    }

    let (mut docs, mut shown) = (Vec::new(), HashSet::new());
    while docs.len() < 2 || docs.iter().any(|doc| !shown.contains(doc)) {
        match next_event(&mut events)
            .await
            .ok_or("the pulled sets were never shown")?
        {
            (_, Event::Opened { doc, .. }) => docs.push(doc),
            (_, Event::DiagnosticsPublished { doc, diagnostics }) if !diagnostics.is_empty() => {
                shown.insert(doc);
            },
            _ => {},
        }
    }

    let doc = *docs.first().ok_or("nothing opened")?;
    let cause = crate::api::SaveCause::Manual;
    backend.send(backend.next_id(), Command::Save { doc, cause })?;
    let mut cleared = HashSet::new();
    while cleared.len() < docs.len() {
        let (_, event) = next_event(&mut events)
            .await
            .ok_or("a pulled set outlived the withdrawal")?;
        if let Event::DiagnosticsPublished { doc, diagnostics } = event {
            if diagnostics.is_empty() {
                cleared.insert(doc);
            } else {
                cleared.remove(&doc);
            }
        }
    }
    Ok(())
}

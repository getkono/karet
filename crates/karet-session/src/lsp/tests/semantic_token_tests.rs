//! Semantic tokens through the server task and the session: the owner is
//! asked at every version it is sent, a later version as a delta against the
//! last answer, only while semantic highlighting is on; the session lays the
//! answer over the snapshot's highlights in buffer bytes.

use karet_core::SemanticToken;
use karet_core::SemanticTokenModifiers as M;
use karet_core::SemanticTokenType as T;
use karet_core::TokenId;
use karet_text::TextBuffer;
use serde_json::Value;

use super::*;
use crate::config::schema::LspLanguage;

/// The next semantic-token answer the server task relays.
async fn next_tokens(
    updates: &mut mpsc::UnboundedReceiver<LspUpdate>,
) -> Option<(PathBuf, i32, Vec<SemanticToken>)> {
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .ok()??;
        if let LspUpdate::SemanticTokens {
            path,
            version,
            tokens,
            ..
        } = update
        {
            return Some((path, version, tokens));
        }
    }
}

/// Whether any semantic-token request reaches the double within a bound.
async fn asked_for_tokens(observed: &mut mpsc::UnboundedReceiver<Value>) -> bool {
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(400), observed.recv()).await
    {
        if message["method"]
            .as_str()
            .is_some_and(|method| method.starts_with("textDocument/semanticTokens"))
        {
            return true;
        }
    }
    false
}

fn token(col: u32, token: TokenId) -> SemanticToken {
    SemanticToken {
        start: LineCol::new(0, col),
        length: 2,
        token,
    }
}

/// Opening a document fetches its tokens whole; an edit fetches them again
/// as a delta against the result the first answer named, and the update
/// carries the edited array decoded. Falsified by asking before the edit is
/// flushed (the delta's result id would be stale), or by never asking for a
/// delta.
#[tokio::test]
async fn the_owner_is_asked_whole_then_by_delta_at_every_version() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/semantic.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::SemanticTokens,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let mut buffer = TextBuffer::from_text("ab!(cd);\n");
    let text = buffer.text();
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, buffer.version(), || text);

    let opened = crate::lsp::version_i32(buffer.version());
    let (own, version, tokens) = next_tokens(&mut updates).await.ok_or("no tokens")?;
    assert_eq!((own.as_path(), version), (path.as_path(), opened));
    assert_eq!(tokens, [token(2, TokenId::semantic(T::Macro, M::NONE))]);

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
    let edited = crate::lsp::version_i32(buffer.version());
    let (_, version, tokens) = next_tokens(&mut updates).await.ok_or("no second answer")?;
    assert_eq!(version, edited);
    assert_eq!(
        tokens,
        [token(2, TokenId::semantic(T::Variable, M::MUTABLE))]
    );

    let delta = loop {
        let message = tokio::time::timeout(Duration::from_secs(5), observed.recv())
            .await?
            .ok_or("the double went away")?;
        if message["method"] == "textDocument/semanticTokens/full/delta" {
            break message;
        }
    };
    assert_eq!(
        delta["params"]["previousResultId"],
        format!("v{opened}").as_str()
    );
    Ok(())
}

/// With semantic highlighting off nothing is asked; turning it on reaches
/// the running task with no further command, and it fetches the documents
/// already open. Falsified by reading the setting only at spawn.
#[tokio::test]
async fn the_setting_starts_and_stops_fetching_without_a_restart() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/toggled.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::SemanticTokens,
        Some(observed_tx),
        Arc::clone(&spawns),
    ));
    manager.set_semantic_highlighting(false);
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || "ab!();\n".into());
    assert!(
        !asked_for_tokens(&mut observed).await,
        "semantic tokens were asked for while switched off"
    );

    manager.set_semantic_highlighting(true);
    let (own, _, tokens) = next_tokens(&mut updates).await.ok_or("never fetched")?;
    assert_eq!(own, path);
    assert_eq!(tokens.len(), 1);
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "the server was restarted");
    Ok(())
}

/// `lsp.languages.<id>.semanticTokens` naming another provider takes the
/// tokens away from the primary server.
#[tokio::test]
async fn a_primary_that_does_not_own_semantic_tokens_is_not_asked() -> TestResult {
    let path = crate::lsp::absolute_path(Path::new("/tmp/unowned.rs"));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let mut settings = LspSettings::default();
    settings.languages.insert(
        "rust".to_owned(),
        LspLanguage {
            semantic_tokens: Some("someone-else".to_owned()),
            ..LspLanguage::default()
        },
    );
    let (mut manager, _updates) = LspManager::new(settings, None, None, None);
    manager.set_connector(test_connector(
        Behavior::SemanticTokens,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || "ab!();\n".into());
    assert!(!asked_for_tokens(&mut observed).await);
    Ok(())
}

/// Through the session: the snapshot's highlights carry the owner's token
/// at the bytes its UTF-16 columns name, past a character two units wide.
/// Falsified by laying the token at its UTF-16 offsets unconverted (bytes
/// 2..4, inside the emoji) or by publishing the syntax highlights alone.
#[tokio::test]
async fn a_snapshot_carries_the_overlay_in_buffer_bytes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "\u{1F600}ab = ab;\n").ok_or("write failed")?;
    let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
    session.set_lsp_connector(test_connector(
        Behavior::SemanticTokens,
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
    let mac = TokenId::semantic(T::Macro, M::NONE);
    let laid = async {
        while let Some((id, snap)) = snaps.recv().await {
            let carries = snap
                .highlights
                .all()
                .iter()
                .any(|s| s.token == mac && (s.span.start.0, s.span.end.0) == (4, 6));
            if id == doc && carries {
                return true;
            }
        }
        false
    };
    assert!(tokio::time::timeout(Duration::from_secs(10), laid).await?);
    Ok(())
}

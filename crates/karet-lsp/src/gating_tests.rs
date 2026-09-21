//! Refusing a request the server never agreed to answer.
//!
//! Split from `lib_tests`, which owns the wire harness and the per-request
//! round trips. These are about the gate in front of them: the whole point is
//! that a refused request produces *no traffic*, so they assert on silence.

use serde_json::json;

use super::TestResult;
use super::wire;
use crate::LineCol;
use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::Range;
use crate::ServerFeature;

#[tokio::test]
async fn an_unadvertised_capability_is_refused_without_issuing_a_request() -> TestResult {
    // The test issue #279 says could not exist: the old fakes advertised `{}`
    // and answered everything anyway, so karet's issuing requests no server had
    // agreed to was invisible. Here the server says only that it formats, and
    // the silence afterwards is the assertion.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server
            .handshake_with(json!({"documentFormattingProvider": true}))
            .await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut server = server_task.await?;

    let doc = Path::new("/w/a.rs");
    let pos = LineCol::new(0, 0);
    let range = Range::default();

    // Every one of these refuses, and none of them reaches the wire.
    assert!(matches!(
        client.hover(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/hover"
        })
    ));
    assert!(matches!(
        client.completion(doc, pos).await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.definition(doc, pos).await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.rename(doc, pos, "x").await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.inlay_hints(doc, range).await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.code_action(doc, range).await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.range_formatting(doc, range).await,
        Err(LspError::Unsupported { .. })
    ));

    // The one thing it *did* advertise is issued, and is the first message the
    // server sees after the handshake. If any refusal above had gone to the
    // wire, this would read that instead.
    let formatting = tokio::spawn(async move {
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/formatting");
        let id = request["id"].clone();
        server.respond(&id, json!([])).await;
    });
    assert!(client.formatting(doc).await?.is_empty());
    formatting.await?;
    Ok(())
}

#[tokio::test]
async fn capabilities_are_readable_and_reflect_the_handshake() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server
            .handshake_with(json!({
                "textDocumentSync": 2,
                "hoverProvider": true,
                "completionProvider": {"triggerCharacters": [".", ":"]},
            }))
            .await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let _server = server_task.await?;

    assert!(client.supports(ServerFeature::Hover));
    assert!(!client.supports(ServerFeature::Rename));

    let caps = client.capabilities();
    assert_eq!(caps.text_sync, karet_core::TextSyncKind::Incremental);
    assert!(caps.is_completion_trigger('.'));
    assert!(!caps.is_completion_trigger('x'));
    Ok(())
}

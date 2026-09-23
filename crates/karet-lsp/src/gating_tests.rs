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

#[tokio::test]
async fn a_dynamically_registered_capability_becomes_usable() -> TestResult {
    // The regression the capability gate would otherwise introduce. A server
    // that advertises nothing statically and registers afterwards -- what
    // haskell-language-server, eslint and some jdtls setups do -- used to have
    // its requests issued and answered. Gating on the handshake alone would
    // refuse them forever, silently, because a refusal is deliberately not a
    // failure anyone is told about.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(json!({})).await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut server = server_task.await?;

    let doc = Path::new("/w/a.hs");
    let pos = LineCol::new(0, 0);
    assert!(!client.supports(ServerFeature::Rename));
    assert!(matches!(
        client.rename(doc, pos, "x").await,
        Err(LspError::Unsupported { .. })
    ));

    // The server registers it after the handshake.
    server
        .send(&json!({
            "jsonrpc": "2.0", "id": "reg-1", "method": "client/registerCapability",
            "params": {"registrations": [
                {"id": "r1", "method": "textDocument/rename", "registerOptions": {}}
            ]}
        }))
        .await;
    let ack = server.recv().await;
    assert_eq!(ack["id"], json!("reg-1"));
    assert_eq!(ack["result"], json!(null));

    assert!(
        client.supports(ServerFeature::Rename),
        "a registered capability should be usable"
    );

    // And the request now reaches the wire.
    let rename = tokio::spawn(async move {
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/rename");
        let id = request["id"].clone();
        server.respond(&id, json!({"changes": {}})).await;
        server
    });
    client.rename(doc, pos, "x").await?;
    let mut server = rename.await?;

    // Withdrawing it closes the gate again.
    server
        .send(&json!({
            "jsonrpc": "2.0", "id": "unreg-1", "method": "client/unregisterCapability",
            "params": {"unregisterations": [{"id": "r1", "method": "textDocument/rename"}]}
        }))
        .await;
    let ack = server.recv().await;
    assert_eq!(ack["id"], json!("unreg-1"));
    assert!(!client.supports(ServerFeature::Rename));
    Ok(())
}

#[tokio::test]
async fn unregistering_one_of_two_registrations_keeps_the_feature() -> TestResult {
    // Two registrations can cover one method -- a server commonly registers
    // formatting per document selector. Withdrawing one must not disable a
    // feature the other still supplies.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(json!({})).await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut server = server_task.await?;

    for id in ["a", "b"] {
        server
            .send(&json!({
                "jsonrpc": "2.0", "id": format!("reg-{id}"),
                "method": "client/registerCapability",
                "params": {"registrations": [
                    {"id": id, "method": "textDocument/formatting"}
                ]}
            }))
            .await;
        let _ack = server.recv().await;
    }
    assert!(client.supports(ServerFeature::Formatting));

    server
        .send(&json!({
            "jsonrpc": "2.0", "id": "unreg", "method": "client/unregisterCapability",
            "params": {"unregisterations": [{"id": "a", "method": "textDocument/formatting"}]}
        }))
        .await;
    let _ack = server.recv().await;
    assert!(
        client.supports(ServerFeature::Formatting),
        "the surviving registration still provides formatting"
    );
    Ok(())
}

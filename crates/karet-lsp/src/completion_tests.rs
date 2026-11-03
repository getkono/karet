//! Completion over the wire: what the handshake promises, the context a
//! request carries, and resolving an item.

use karet_core::CompletionKind;
use karet_core::CompletionTrigger;
use karet_core::LineCol;
use serde_json::Value;
use serde_json::json;

use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::tests::FakeServer;
use crate::tests::TestResult;
use crate::tests::everything;
use crate::tests::wire;

/// A client connected to a fake server that advertised `capabilities`.
async fn connect(capabilities: Value) -> Result<(LspClient, FakeServer), LspError> {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(capabilities).await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let server = server_task
        .await
        .map_err(|error| LspError::Protocol(error.to_string()))?;
    Ok((client, server))
}

/// Falsified by a handshake that lets a server defer what an accept inserts,
/// or that never says it takes insert/replace edits, commit characters,
/// markdown documentation or a trigger context.
#[tokio::test]
async fn the_handshake_declares_what_completion_items_may_carry() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let completion = &params["capabilities"]["textDocument"]["completion"];
    let item = &completion["completionItem"];
    assert_eq!(
        item["resolveSupport"]["properties"],
        json!(["documentation", "detail", "additionalTextEdits"])
    );
    assert_eq!(item["insertReplaceSupport"], json!(true));
    assert_eq!(item["commitCharactersSupport"], json!(true));
    assert_eq!(item["documentationFormat"][0], json!("markdown"));
    assert_eq!(completion["contextSupport"], json!(true));
    Ok(())
}

/// Each trigger reaches the wire as the context LSP defines for it.
#[tokio::test]
async fn a_request_says_why_it_was_made() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let served = tokio::spawn(async move {
        let mut contexts = Vec::new();
        for _ in 0..3 {
            let request = server.recv().await;
            contexts.push(request["params"]["context"].clone());
            let id = request["id"].clone();
            server.respond(&id, json!([])).await;
        }
        contexts
    });
    let doc = Path::new("/w/a.rs");
    for trigger in [
        CompletionTrigger::Invoked,
        CompletionTrigger::Character('.'),
        CompletionTrigger::Incomplete,
    ] {
        client.completion(doc, LineCol::new(0, 1), trigger).await?;
    }
    let contexts = served.await?;
    assert_eq!(contexts[0], json!({"triggerKind": 1}));
    assert_eq!(
        contexts[1],
        json!({"triggerKind": 2, "triggerCharacter": "."})
    );
    assert_eq!(contexts[2], json!({"triggerKind": 3}));
    Ok(())
}

/// Falsified by resolving with anything but the item exactly as listed
/// (`data` included), or by losing the documentation and additional edits
/// the answer filled in.
#[tokio::test]
async fn an_item_resolves_with_its_own_data() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let served = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server
            .respond(
                &id,
                json!([{"label": "HashMap", "kind": 22, "data": {"n": 3}}]),
            )
            .await;
        let request = server.recv().await;
        assert_eq!(request["method"], "completionItem/resolve");
        let sent = request["params"].clone();
        let id = request["id"].clone();
        server
            .respond(
                &id,
                json!({
                    "label": "HashMap",
                    "documentation": {"kind": "markdown", "value": "A **map**."},
                    "additionalTextEdits": [{
                        "range": {"start": {"line": 0, "character": 0},
                                  "end": {"line": 0, "character": 0}},
                        "newText": "use std::collections::HashMap;\n"
                    }]
                }),
            )
            .await;
        sent
    });
    let list = client
        .completion(
            Path::new("/w/a.rs"),
            LineCol::new(1, 0),
            CompletionTrigger::Invoked,
        )
        .await?;
    let item = list.items.first().ok_or("one item listed")?;
    assert_eq!(item.kind, CompletionKind::Struct);
    let resolved = client
        .resolve_completion(item)
        .await?
        .ok_or("the item had data to resolve with")?;
    let sent = served.await?;
    assert_eq!(
        sent,
        json!({"label": "HashMap", "kind": 22, "data": {"n": 3}})
    );
    assert_eq!(
        resolved.documentation.map(|docs| docs.value),
        Some("A **map**.".to_owned())
    );
    assert_eq!(resolved.additional_edits.len(), 1);
    assert_eq!(resolved.payload, None);
    Ok(())
}

/// A server that does not resolve items is never asked to: its items carry
/// no payload, and resolving one is answered here with nothing.
#[tokio::test]
async fn nothing_resolves_at_a_server_that_does_not() -> TestResult {
    let mut capabilities = everything();
    capabilities["completionProvider"] = json!({});
    let (client, mut server) = connect(capabilities).await?;
    let served = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server
            .respond(&id, json!([{"label": "x", "data": 1}]))
            .await;
        server
    });
    let list = client
        .completion(
            Path::new("/w/a.rs"),
            LineCol::new(0, 0),
            CompletionTrigger::Invoked,
        )
        .await?;
    let _server = served.await?;
    let item = list.items.first().ok_or("one item listed")?;
    assert_eq!(item.payload, None);
    assert_eq!(client.resolve_completion(item).await?, None);
    Ok(())
}

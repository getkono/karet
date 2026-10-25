//! Refusing a request the server never agreed to answer.
//!
//! Split from `lib_tests`, which owns the wire harness and the per-request
//! round trips. These are about the gate in front of them: the whole point is
//! that a refused request produces *no traffic*, so they assert on silence.
//! The gate's open side lives here too for the requests no round trip in
//! `lib_tests` issues (`workspace/symbol`, `textDocument/signatureHelp`).

use serde_json::json;

use super::TestResult;
use super::wire;
use crate::Indentation;
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
        client
            .completion(doc, pos, karet_core::CompletionTrigger::Invoked)
            .await,
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
        client
            .range_formatting(doc, range, Indentation::default())
            .await,
        Err(LspError::Unsupported { .. })
    ));
    assert!(matches!(
        client.document_symbols(doc).await,
        Err(LspError::Unsupported {
            method: "textDocument/documentSymbol"
        })
    ));
    assert!(matches!(
        client.workspace_symbols("x").await,
        Err(LspError::Unsupported {
            method: "workspace/symbol"
        })
    ));
    assert!(matches!(
        client.implementations(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/implementation"
        })
    ));
    assert!(matches!(
        client.supertypes(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/prepareTypeHierarchy"
        })
    ));
    assert!(matches!(
        client
            .prepare_hierarchy(karet_core::HierarchyDirection::OutgoingCalls, doc, pos)
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/prepareCallHierarchy"
        })
    ));
    assert!(matches!(
        client.prepare_rename(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/prepareRename"
        })
    ));
    assert!(matches!(
        client.signature_help(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/signatureHelp"
        })
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
    assert!(
        client
            .formatting(doc, Indentation::default())
            .await?
            .is_empty()
    );
    formatting.await?;
    Ok(())
}

// The gate's open side, for the two requests no other test issues: once the
// server advertises them, they reach the wire with their params and the answer
// maps into the neutral model.

#[tokio::test]
async fn workspace_symbols_round_trip_once_advertised() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let request = server.recv().await;
        assert_eq!(request["method"], "workspace/symbol");
        assert_eq!(request["params"]["query"], "pars");
        server
            .respond(
                &request["id"],
                json!([{
                    "name": "parse", "kind": 12, "containerName": "codec",
                    "location": {"uri": "file:///w/src/codec.rs",
                                 "range": {"start": {"line": 4, "character": 7},
                                           "end": {"line": 4, "character": 12}}}
                }]),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let found = client.workspace_symbols("pars").await?;
    server_task.await?;
    let [symbol] = found.as_slice() else {
        return Err(format!("expected one symbol, got {found:?}").into());
    };
    assert_eq!(symbol.name, "parse");
    assert_eq!(symbol.kind, karet_core::SymbolKind::Function);
    assert_eq!(symbol.container_name.as_deref(), Some("codec"));
    assert_eq!(symbol.path, Path::new("/w/src/codec.rs"));
    let range = symbol
        .range
        .as_ref()
        .ok_or("the located form lost its range")?;
    assert_eq!(
        (range.start, range.end),
        (LineCol::new(4, 7), LineCol::new(4, 12))
    );
    Ok(())
}

#[tokio::test]
async fn signature_help_round_trips_once_advertised() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/signatureHelp");
        assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
        assert_eq!(
            request["params"]["position"],
            json!({"line": 3, "character": 9})
        );
        server
            .respond(
                &request["id"],
                json!({
                    "signatures": [{
                        "label": "fn f(a: i32, b: i32)",
                        // One simple label, one by UTF-16 offsets into `label`.
                        "parameters": [{"label": "a: i32"}, {"label": [13, 19]}]
                    }],
                    "activeSignature": 0,
                    "activeParameter": 1
                }),
            )
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let help = client
        .signature_help(Path::new("/w/a.rs"), LineCol::new(3, 9))
        .await?
        .ok_or("signature help came back empty")?;
    server_task.await?;
    assert_eq!((help.active_signature, help.active_parameter), (0, 1));
    let [signature] = help.signatures.as_slice() else {
        return Err(format!("expected one signature, got {:?}", help.signatures).into());
    };
    assert_eq!(signature.label, "fn f(a: i32, b: i32)");
    let labels: Vec<&str> = signature
        .parameters
        .iter()
        .map(|parameter| parameter.label.as_str())
        .collect();
    assert_eq!(labels, ["a: i32", "b: i32"]);
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

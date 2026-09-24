//! Dynamic registrations scoped by `documentSelector`.
//!
//! Split from `gating_tests`, which covers the gate as a whole. These cover
//! the per-document half: a registration turns a feature on only for the
//! documents its selector covers, and a request for any other document is
//! refused before it reaches the wire.

use serde_json::Value;
use serde_json::json;

use super::FakeServer;
use super::TestResult;
use super::wire;
use crate::Indentation;
use crate::LineCol;
use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::ServerFeature;

/// Connect to a server that advertises only `capabilities`.
async fn connect(
    capabilities: Value,
) -> Result<(LspClient, FakeServer), Box<dyn std::error::Error + Send + Sync>> {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(capabilities).await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    Ok((client, server_task.await?))
}

/// Send a server request and wait for the client's answer to it, skipping the
/// notifications (`didOpen`) the client sent before it.
async fn ask(server: &mut FakeServer, id: &str, method: &str, params: Value) {
    server
        .send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .await;
    loop {
        let message = server.recv().await;
        if message["id"] == json!(id) || message.is_null() {
            return;
        }
    }
}

async fn register(server: &mut FakeServer, id: &str, method: &str, options: Value) {
    let params = json!({"registrations": [
        {"id": id, "method": method, "registerOptions": options}
    ]});
    ask(
        server,
        &format!("reg-{id}"),
        "client/registerCapability",
        params,
    )
    .await;
}

async fn unregister(server: &mut FakeServer, id: &str, method: &str) {
    let params = json!({"unregisterations": [{"id": id, "method": method}]});
    ask(
        server,
        &format!("unreg-{id}"),
        "client/unregisterCapability",
        params,
    )
    .await;
}

#[tokio::test]
async fn a_selector_scopes_a_registration_to_the_documents_it_matches() -> TestResult {
    let (client, mut server) = connect(json!({"textDocumentSync": 1})).await?;
    let ts = Path::new("/w/a.ts");
    let js = Path::new("/w/a.js");
    client.did_open(ts, "typescript", 1, "").await?;
    client.did_open(js, "javascript", 1, "").await?;

    register(
        &mut server,
        "hover-ts",
        "textDocument/hover",
        json!({"documentSelector": [{"language": "typescript"}]}),
    )
    .await;
    assert!(client.supports_for(ServerFeature::Hover, ts));
    assert!(!client.supports_for(ServerFeature::Hover, js));
    assert!(
        client.supports(ServerFeature::Hover),
        "the connection-wide view still lists it"
    );

    // Refused for the uncovered document, with nothing on the wire: the hover
    // for the covered one is the next thing the server reads.
    assert!(matches!(
        client.hover(js, LineCol::new(0, 0)).await,
        Err(LspError::Unsupported {
            method: "textDocument/hover"
        })
    ));
    let answered = tokio::spawn(async move {
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/hover");
        assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.ts");
        server.respond(&request["id"], Value::Null).await;
    });
    assert_eq!(client.hover(ts, LineCol::new(0, 0)).await?, None);
    answered.await?;
    Ok(())
}

#[tokio::test]
async fn a_registration_without_a_selector_covers_every_document() -> TestResult {
    let (client, mut server) = connect(json!({})).await?;
    for (id, options) in [
        ("absent", json!({})),
        ("null", json!({"documentSelector": null})),
    ] {
        register(&mut server, id, "textDocument/rename", options).await;
        for doc in ["/w/a.rs", "/w/b.py", "/elsewhere/c.txt"] {
            assert!(client.supports_for(ServerFeature::Rename, Path::new(doc)));
        }
        unregister(&mut server, id, "textDocument/rename").await;
        assert!(!client.supports_for(ServerFeature::Rename, Path::new("/w/a.rs")));
    }
    Ok(())
}

#[tokio::test]
async fn two_registrations_cover_the_union_and_unregister_separately() -> TestResult {
    let (client, mut server) = connect(json!({})).await?;
    let ts = Path::new("/w/a.ts");
    let js = Path::new("/w/lib/a.js");
    let rs = Path::new("/w/a.rs");
    client.did_open(ts, "typescript", 1, "").await?;

    register(
        &mut server,
        "ts",
        "textDocument/formatting",
        json!({"documentSelector": [{"language": "typescript"}]}),
    )
    .await;
    register(
        &mut server,
        "js",
        "textDocument/formatting",
        json!({"documentSelector": [{"pattern": "**/*.js"}]}),
    )
    .await;
    assert!(client.supports_formatting(ts));
    assert!(client.supports_formatting(js));
    assert!(!client.supports_formatting(rs));

    unregister(&mut server, "ts", "textDocument/formatting").await;
    assert!(
        !client.supports_formatting(ts),
        "the withdrawn scope stayed"
    );
    assert!(client.supports_formatting(js), "the surviving scope went");
    assert!(matches!(
        client.formatting(ts, Indentation::default()).await,
        Err(LspError::Unsupported { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_closed_document_no_longer_matches_by_language() -> TestResult {
    let (client, mut server) = connect(json!({})).await?;
    let doc = Path::new("/w/a.go");
    client.did_open(doc, "go", 1, "").await?;
    register(
        &mut server,
        "go",
        "textDocument/definition",
        json!({"documentSelector": [{"language": "go"}]}),
    )
    .await;
    assert!(client.supports_for(ServerFeature::Definition, doc));
    client.did_close(doc).await?;
    assert!(!client.supports_for(ServerFeature::Definition, doc));
    Ok(())
}

#[tokio::test]
async fn unregistering_never_disables_an_advertised_feature() -> TestResult {
    // A server may advertise a feature and also register it, scoped; taking
    // the registration back must not take back what the handshake promised.
    let (client, mut server) = connect(json!({"hoverProvider": true})).await?;
    let doc = Path::new("/w/a.rs");
    register(
        &mut server,
        "extra",
        "textDocument/hover",
        json!({"documentSelector": [{"language": "markdown"}]}),
    )
    .await;
    unregister(&mut server, "extra", "textDocument/hover").await;
    assert!(client.supports(ServerFeature::Hover));
    assert!(client.supports_for(ServerFeature::Hover, doc));
    Ok(())
}

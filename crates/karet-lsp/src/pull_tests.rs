//! Pull diagnostics over the wire.

use std::path::PathBuf;
use std::time::Duration;

use karet_core::LineCol;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

use crate::CodeActionTrigger;
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

/// Everything, plus pull diagnostics.
fn pulling() -> Value {
    let mut capabilities = everything();
    capabilities["diagnosticProvider"] =
        json!({"interFileDependencies": true, "workspaceDiagnostics": false});
    capabilities
}

fn range(line: u32) -> Value {
    json!({"start": {"line": line, "character": 0}, "end": {"line": line, "character": 3}})
}

/// Answer the next request with `report`, handing the request back.
fn answer(mut server: FakeServer, report: Value) -> tokio::task::JoinHandle<(FakeServer, Value)> {
    tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server.respond(&id, report).await;
        (server, request)
    })
}

#[tokio::test]
async fn the_handshake_declares_pull_diagnostics_with_related_documents() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let declared = &params["capabilities"]["textDocument"]["diagnostic"];
    assert_eq!(declared["relatedDocumentSupport"], json!(true));
    // No `workspace/diagnostic/refresh` is answered, so none is invited.
    assert!(params["capabilities"]["workspace"]["diagnostics"].is_null());
    Ok(())
}

/// A full report maps like a publication, keeps its result id, carries the
/// related documents' full reports, and drops their unchanged ones.
#[tokio::test]
async fn a_full_report_maps_with_its_result_id_and_related_documents() -> TestResult {
    let (client, server) = connect(pulling()).await?;
    let serving = answer(
        server,
        json!({
            "kind": "full",
            "resultId": "r1",
            "items": [{
                "range": range(2),
                "severity": 2,
                "message": "unused",
                "codeDescription": {"href": "https://example.com/unused"},
                "code": "W1"
            }],
            "relatedDocuments": {
                "file:///w/b.rs": {"kind": "full", "items": [{"range": range(0), "message": "there"}]},
                "file:///w/c.rs": {"kind": "unchanged", "resultId": "c1"}
            }
        }),
    );
    let pulled = client
        .pull_diagnostics(Path::new("/w/a.rs"), Some("r0"))
        .await?;
    let (_server, request) = serving.await?;
    assert_eq!(request["method"], "textDocument/diagnostic");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(request["params"]["previousResultId"], "r0");

    let own = pulled.diagnostics.ok_or("a full report")?;
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].message, "unused");
    assert_eq!(own[0].range.start, LineCol::new(2, 0));
    assert_eq!(
        own[0].code_href.as_deref(),
        Some("https://example.com/unused")
    );
    assert_eq!(pulled.result_id.as_deref(), Some("r1"));
    assert_eq!(pulled.related.len(), 1);
    assert_eq!(pulled.related[0].path, PathBuf::from("/w/b.rs"));
    assert_eq!(pulled.related[0].version, None);
    assert_eq!(pulled.related[0].diagnostics[0].message, "there");
    Ok(())
}

/// An unchanged report says nothing new about the document: no set, and the
/// result id to keep using.
#[tokio::test]
async fn an_unchanged_report_carries_no_set() -> TestResult {
    let (client, server) = connect(pulling()).await?;
    let serving = answer(server, json!({"kind": "unchanged", "resultId": "r1"}));
    let pulled = client
        .pull_diagnostics(Path::new("/w/a.rs"), Some("r1"))
        .await?;
    serving.await?;
    assert_eq!(pulled.diagnostics, None);
    assert_eq!(pulled.result_id.as_deref(), Some("r1"));
    assert!(pulled.related.is_empty());
    Ok(())
}

/// A server that never offered pull diagnostics is not asked.
#[tokio::test]
async fn a_server_without_pull_diagnostics_is_not_asked() -> TestResult {
    let (client, _server) = connect(everything()).await?;
    let refused = tokio::time::timeout(
        Duration::from_secs(2),
        client.pull_diagnostics(Path::new("/w/a.rs"), None),
    )
    .await?;
    assert!(
        matches!(refused, Err(LspError::Unsupported { .. })),
        "{refused:?}"
    );
    Ok(())
}

/// A pulled set reaches the code-action context in its wire form, exactly as
/// a published one does.
#[tokio::test]
async fn a_pulled_set_feeds_the_code_action_context() -> TestResult {
    let (client, server) = connect(pulling()).await?;
    let wire_item = json!({"range": range(1), "message": "fix me", "data": {"fix": 1}});
    let serving = answer(server, json!({"kind": "full", "items": [wire_item]}));
    let doc = Path::new("/w/a.rs");
    client.pull_diagnostics(doc, None).await?;
    let (server, _) = serving.await?;

    let serving = answer(server, json!([]));
    let at = Range {
        start: LineCol::new(1, 1),
        end: LineCol::new(1, 1),
    };
    client
        .code_actions(doc, at, &[], CodeActionTrigger::Invoked)
        .await?;
    let (_server, request) = serving.await?;
    assert_eq!(request["method"], "textDocument/codeAction");
    let context = &request["params"]["context"]["diagnostics"];
    assert_eq!(context[0]["message"], "fix me");
    assert_eq!(context[0]["data"], json!({"fix": 1}));
    Ok(())
}

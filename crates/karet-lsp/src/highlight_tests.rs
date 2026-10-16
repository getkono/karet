//! `textDocument/documentHighlight` over the wire: the params it sends, how
//! each highlight's kind maps (an absent or unknown kind is text), and the
//! refusal a server that never advertised it gets instead of a request.

use karet_core::LineCol;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

use super::DocumentHighlight;
use super::HighlightKind;
use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::tests::FakeServer;
use crate::tests::TestResult;
use crate::tests::wire;

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

async fn answer(mut server: FakeServer, result: Value) -> Value {
    let request = server.recv().await;
    assert_eq!(request["method"], "textDocument/documentHighlight");
    server.respond(&request["id"], result).await;
    request
}

fn lsp_range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start},
           "end": {"line": line, "character": end}})
}

fn highlight(line: u32, start: u32, end: u32, kind: HighlightKind) -> DocumentHighlight {
    DocumentHighlight {
        range: Range {
            start: LineCol::new(line, start),
            end: LineCol::new(line, end),
        },
        kind,
    }
}

#[tokio::test]
async fn each_highlight_keeps_its_range_and_kind() -> TestResult {
    let (client, server) = connect(json!({"documentHighlightProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        json!([
            {"range": lsp_range(0, 4, 7), "kind": 3},
            {"range": lsp_range(1, 2, 5), "kind": 2},
            {"range": lsp_range(2, 0, 3), "kind": 1},
            {"range": lsp_range(3, 1, 4)},
            {"range": lsp_range(4, 1, 4), "kind": 9}
        ]),
    ));
    let highlights = client
        .document_highlights(Path::new("/w/a.rs"), LineCol::new(1, 3))
        .await?;
    let request = answered.await?;
    assert_eq!(
        request["params"]["position"],
        json!({"line": 1, "character": 3})
    );
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(
        highlights,
        vec![
            highlight(0, 4, 7, HighlightKind::Write),
            highlight(1, 2, 5, HighlightKind::Read),
            highlight(2, 0, 3, HighlightKind::Text),
            highlight(3, 1, 4, HighlightKind::Text),
            highlight(4, 1, 4, HighlightKind::Text),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_null_answer_is_no_highlights() -> TestResult {
    let (client, server) = connect(json!({"documentHighlightProvider": true})).await?;
    let answered = tokio::spawn(answer(server, Value::Null));
    let highlights = client
        .document_highlights(Path::new("/w/a.rs"), LineCol::new(0, 0))
        .await?;
    answered.await?;
    assert!(highlights.is_empty());
    Ok(())
}

#[tokio::test]
async fn unadvertised_document_highlight_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"definitionProvider": true})).await?;
    assert!(matches!(
        client
            .document_highlights(Path::new("/w/a.rs"), LineCol::new(0, 0))
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/documentHighlight"
        })
    ));
    Ok(())
}

//! `textDocument/selectionRange` over the wire: the params it sends, how the
//! linked answer flattens into one chain per position, and the refusal a
//! server that never advertised it gets instead of a request.

use karet_core::LineCol;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

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
    assert_eq!(request["method"], "textDocument/selectionRange");
    server.respond(&request["id"], result).await;
    request
}

fn lsp_range(line: u32, start: u32, end_line: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start},
           "end": {"line": end_line, "character": end}})
}

fn range(line: u32, start: u32, end_line: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(end_line, end),
    }
}

#[tokio::test]
async fn each_position_gets_its_chain_innermost_first() -> TestResult {
    let (client, server) = connect(json!({"selectionRangeProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        json!([
            {"range": lsp_range(0, 4, 0, 7),
             "parent": {"range": lsp_range(0, 3, 0, 12),
                        "parent": {"range": lsp_range(0, 0, 2, 1)}}},
            {"range": lsp_range(1, 2, 1, 5)}
        ]),
    ));
    let chains = client
        .selection_ranges(
            Path::new("/w/a.rs"),
            &[LineCol::new(0, 5), LineCol::new(1, 3)],
        )
        .await?;
    let request = answered.await?;
    assert_eq!(
        request["params"]["positions"],
        json!([{"line": 0, "character": 5}, {"line": 1, "character": 3}])
    );
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(
        chains,
        vec![
            vec![range(0, 4, 0, 7), range(0, 3, 0, 12), range(0, 0, 2, 1)],
            vec![range(1, 2, 1, 5)],
        ]
    );
    Ok(())
}

/// An out-of-spec parent that does not contain its child ends the chain there,
/// and a parent equal to its child is one step, not two.
#[tokio::test]
async fn a_chain_stops_at_a_parent_that_does_not_contain_it() -> TestResult {
    let (client, server) = connect(json!({"selectionRangeProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        json!([
            {"range": lsp_range(0, 4, 0, 7),
             "parent": {"range": lsp_range(0, 4, 0, 7),
                        "parent": {"range": lsp_range(0, 5, 0, 20),
                                   "parent": {"range": lsp_range(0, 0, 9, 0)}}}}
        ]),
    ));
    let chains = client
        .selection_ranges(Path::new("/w/a.rs"), &[LineCol::new(0, 5)])
        .await?;
    answered.await?;
    assert_eq!(chains, vec![vec![range(0, 4, 0, 7)]]);
    Ok(())
}

/// A null answer, or one shorter than the positions asked about, still yields
/// one chain per position.
#[tokio::test]
async fn missing_chains_come_back_empty() -> TestResult {
    let positions = [LineCol::new(0, 1), LineCol::new(3, 2)];
    let (client, server) = connect(json!({"selectionRangeProvider": true})).await?;
    let answered = tokio::spawn(answer(server, Value::Null));
    let chains = client
        .selection_ranges(Path::new("/w/a.rs"), &positions)
        .await?;
    answered.await?;
    assert_eq!(chains, vec![Vec::new(), Vec::new()]);

    let (client, server) = connect(json!({"selectionRangeProvider": true})).await?;
    let answered = tokio::spawn(answer(server, json!([{"range": lsp_range(0, 0, 0, 3)}])));
    let chains = client
        .selection_ranges(Path::new("/w/a.rs"), &positions)
        .await?;
    answered.await?;
    assert_eq!(chains, vec![vec![range(0, 0, 0, 3)], Vec::new()]);
    Ok(())
}

#[tokio::test]
async fn unadvertised_selection_range_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"definitionProvider": true})).await?;
    assert!(matches!(
        client
            .selection_ranges(Path::new("/w/a.rs"), &[LineCol::new(0, 0)])
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/selectionRange"
        })
    ));
    Ok(())
}

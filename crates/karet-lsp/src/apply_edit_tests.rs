use std::path::Path;

use karet_core::DocumentChange;
use karet_core::LineCol;
use serde_json::json;

use super::*;
use crate::LspClient;
use crate::tests::TestResult;
use crate::tests::wire;

/// A `workspace/applyEdit` creating one file, as a server sends it.
fn create_request(id: u64) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "workspace/applyEdit",
        "params": {"label": "Extract module", "edit": {"documentChanges": [
            {"kind": "create", "uri": "file:///tmp/ws/new.rs"}
        ]}}
    })
}

#[tokio::test]
async fn an_apply_edit_reaches_the_consumer_and_its_answer_reaches_the_server() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        let params = server.handshake().await;
        let workspace = &params["capabilities"]["workspace"];
        assert_eq!(workspace["applyEdit"], json!(true));
        assert_eq!(workspace["workspaceEdit"]["documentChanges"], json!(true));
        assert_eq!(
            workspace["workspaceEdit"]["resourceOperations"],
            json!(["create", "rename", "delete"])
        );
        server.send(&create_request(7)).await;
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let mut requests = client
        .apply_edit_requests()
        .ok_or("the stream is taken once")?;
    // Taking it a second time yields nothing: one answerer per request.
    assert!(client.apply_edit_requests().is_none());
    let request = requests.recv().await.ok_or("no edit arrived")?;
    assert_eq!(request.label.as_deref(), Some("Extract module"));
    assert!(matches!(
        request.edit.document_changes.as_slice(),
        [DocumentChange::Create { path, .. }] if path == Path::new("/tmp/ws/new.rs")
    ));
    request.respond(ApplyWorkspaceEditResult::failed("file exists", Some(0)));
    let reply = server_task.await?;
    assert_eq!(reply["id"], json!(7));
    assert_eq!(
        reply["result"],
        json!({"applied": false, "failureReason": "file exists", "failedChange": 0})
    );
    Ok(())
}

#[tokio::test]
async fn an_apply_edit_nobody_listens_for_is_refused_not_left_hanging() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server.send(&create_request(8)).await;
        server.recv().await
    });
    let _client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let reply = server_task.await?;
    assert_eq!(reply["id"], json!(8));
    assert_eq!(reply["result"]["applied"], json!(false));
    Ok(())
}

#[tokio::test]
async fn a_dropped_apply_edit_answers_not_applied() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server.send(&create_request(9)).await;
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let mut requests = client.apply_edit_requests().ok_or("stream")?;
    drop(requests.recv().await.ok_or("no edit arrived")?);
    let reply = server_task.await?;
    assert_eq!(reply["result"]["applied"], json!(false));
    assert!(reply["result"]["failureReason"].is_string());
    Ok(())
}

#[tokio::test]
async fn an_apply_edit_naming_a_non_file_is_refused_with_the_reason() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server
            .send(&json!({
                "jsonrpc": "2.0", "id": 10, "method": "workspace/applyEdit",
                "params": {"edit": {"changes": {"untitled:1": []}}}
            }))
            .await;
        let refused = server.recv().await;
        server
            .send(&json!({
                "jsonrpc": "2.0", "id": 11, "method": "workspace/applyEdit",
                "params": {"edit": 1}
            }))
            .await;
        (refused, server.recv().await)
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let mut requests = client.apply_edit_requests().ok_or("stream")?;
    let (refused, malformed) = server_task.await?;
    assert_eq!(refused["result"]["applied"], json!(false));
    assert!(
        refused["result"]["failureReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("untitled:1")),
        "{refused}"
    );
    assert_eq!(
        malformed["error"]["code"],
        json!(karet_jsonrpc::INVALID_PARAMS)
    );
    // Neither reached the consumer.
    assert!(requests.try_recv().is_err());
    Ok(())
}

#[tokio::test]
async fn prepare_rename_round_trips_and_is_gated() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        let params = server.handshake().await;
        let rename = &params["capabilities"]["textDocument"]["rename"];
        assert_eq!(rename["prepareSupport"], json!(true));
        assert_eq!(rename["prepareSupportDefaultBehavior"], json!(1));
        let request = server.recv().await;
        assert_eq!(request["method"], "textDocument/prepareRename");
        assert_eq!(
            request["params"]["position"],
            json!({"line": 1, "character": 2})
        );
        server
            .respond(&request["id"], json!({"defaultBehavior": true}))
            .await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    let answer = client
        .prepare_rename(Path::new("/tmp/ws/a.rs"), LineCol::new(1, 2))
        .await?;
    assert_eq!(answer, Some(karet_core::PrepareRename::DefaultBehavior));
    server_task.await?;

    // A server offering rename without `prepareProvider` is never asked.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(json!({"renameProvider": true})).await;
    });
    let client = LspClient::connect(read, write, Path::new("/tmp/ws")).await?;
    server_task.await?;
    let refused = client
        .prepare_rename(Path::new("/tmp/ws/a.rs"), LineCol::new(0, 0))
        .await;
    assert!(matches!(
        refused,
        Err(crate::LspError::Unsupported {
            method: "textDocument/prepareRename"
        })
    ));
    Ok(())
}

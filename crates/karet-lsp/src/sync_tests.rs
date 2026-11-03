//! Incremental document sync on the wire.
//!
//! Split from `lib_tests`, which owns the wire harness and pins full-text sync.
//! These pin what `did_change_incremental` sends to a server that asked for
//! ranges, and that it sends nothing to one that did not.

use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use serde_json::json;

use super::TestResult;
use super::wire;
use crate::LspClient;
use crate::LspError;
use crate::Path;

fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
    TextEdit {
        range: Range {
            start: LineCol::new(start.0, start.1),
            end: LineCol::new(end.0, end.1),
        },
        new_text: new_text.to_owned(),
    }
}

#[tokio::test]
async fn incremental_changes_travel_as_ordered_ranges() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        // The options-object form, as most servers send it.
        server
            .handshake_with(json!({"textDocumentSync": {"openClose": true, "change": 2}}))
            .await;
        let _open = server.recv().await;
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let doc = Path::new("/w/main.rs");
    client.did_open(doc, "rust", 1, "😀a\nb\n").await?;
    // Columns are UTF-16: the emoji is two code units, so `a` starts at 2.
    let edits = [edit((1, 0), (1, 1), "c"), edit((0, 2), (0, 3), "xy\nz")];
    client.did_change_incremental(doc, 2, &edits).await?;
    let change = server_task.await?;
    assert_eq!(change["method"], "textDocument/didChange");
    assert_eq!(change["params"]["textDocument"]["version"], json!(2));
    assert_eq!(
        change["params"]["contentChanges"],
        json!([
            {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1}},
             "text": "c"},
            {"range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 3}},
             "text": "xy\nz"},
        ])
    );
    Ok(())
}

#[tokio::test]
async fn a_full_sync_server_is_refused_incremental_changes_unsent() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake_with(json!({"textDocumentSync": 1})).await;
        let _open = server.recv().await;
        // The next thing on the wire is the close: the refused change never
        // left the client.
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let doc = Path::new("/w/main.rs");
    client.did_open(doc, "rust", 1, "a\n").await?;
    let refused = client
        .did_change_incremental(doc, 2, &[edit((0, 0), (0, 1), "b")])
        .await;
    assert!(matches!(refused, Err(LspError::Unsupported { .. })));
    client.did_close(doc).await?;
    let next = server_task.await?;
    assert_eq!(next["method"], "textDocument/didClose");
    Ok(())
}

//! Folding ranges over the wire: the handshake's declaration, kind mapping,
//! and the request itself.

use karet_core::FoldKind;
use karet_core::FoldRegion;
use serde_json::Value;
use serde_json::json;

use super::fold_region_from_lsp;
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

/// The client tells the server it folds whole lines only, knows the three
/// standard kinds, and honours a dynamic registration (whose options carry
/// nothing beyond a document selector).
#[tokio::test]
async fn the_handshake_declares_line_folding() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let folding = &params["capabilities"]["textDocument"]["foldingRange"];
    assert_eq!(folding["lineFoldingOnly"], json!(true));
    assert_eq!(folding["dynamicRegistration"], json!(true));
    assert_eq!(
        folding["foldingRangeKind"]["valueSet"],
        json!(["comment", "imports", "region"])
    );
    Ok(())
}

/// Each standard kind maps to its fold kind; an unknown kind or none is
/// code; an entry missing a line, or with one past `u32`, is no region.
/// Falsified by decoding through `lsp_types::FoldingRange`, whose closed
/// kind enum rejects `"custom"`.
#[test]
fn ranges_map_their_kinds_leniently() {
    let region =
        |kind: Value| fold_region_from_lsp(&json!({"startLine": 1, "endLine": 4, "kind": kind}));
    let kinds = [
        (json!("comment"), FoldKind::Comment),
        (json!("imports"), FoldKind::Imports),
        (json!("region"), FoldKind::Region),
        (json!("custom"), FoldKind::Code),
        (json!(null), FoldKind::Code),
    ];
    for (kind, expected) in kinds {
        assert_eq!(
            region(kind),
            Some(FoldRegion::new(1, 4).with_kind(expected))
        );
    }
    assert_eq!(fold_region_from_lsp(&json!({"startLine": 1})), None);
    assert_eq!(
        fold_region_from_lsp(&json!({"startLine": 0, "endLine": 5_000_000_000_u64})),
        None
    );
    assert_eq!(fold_region_from_lsp(&json!("not a range")), None);
}

/// The ranges are asked for the document and come back normalized: in
/// document order, the outermost per header line, an entry that is not a
/// range and one that hides nothing skipped.
#[tokio::test]
async fn ranges_are_asked_for_the_document_and_normalized() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listing = tokio::spawn(async move {
        let request = server.recv().await;
        let ranges = json!([
            {"startLine": 6, "endLine": 9, "kind": "region"},
            {"startLine": 0, "endLine": 2, "kind": "imports"},
            {"startLine": 6, "endLine": 7},
            {"no": "lines"},
            {"startLine": 4, "endLine": 4},
        ]);
        server.respond(&request["id"], ranges).await;
        request
    });
    let folds = client.folding_ranges(Path::new("/w/a.rs")).await?;
    let request = listing.await?;
    assert_eq!(request["method"], "textDocument/foldingRange");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(
        folds.regions(),
        [
            FoldRegion::new(0, 2).with_kind(FoldKind::Imports),
            FoldRegion::new(6, 9).with_kind(FoldKind::Region),
        ]
    );
    Ok(())
}

/// A `null` answer is no ranges, not an error.
#[tokio::test]
async fn a_null_answer_is_no_ranges() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listing = tokio::spawn(async move {
        let request = server.recv().await;
        server.respond(&request["id"], json!(null)).await;
    });
    let folds = client.folding_ranges(Path::new("/w/a.rs")).await?;
    listing.await?;
    assert!(folds.regions().is_empty());
    Ok(())
}

/// Without the provider the request is refused before anything is sent.
#[tokio::test]
async fn ranges_are_refused_without_the_capability() -> TestResult {
    let mut capabilities = everything();
    if let Some(caps) = capabilities.as_object_mut() {
        caps.remove("foldingRangeProvider");
    }
    let (client, _server) = connect(capabilities).await?;
    let refused = client.folding_ranges(Path::new("/w/a.rs")).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "textDocument/foldingRange"
            })
        ),
        "{refused:?}"
    );
    Ok(())
}

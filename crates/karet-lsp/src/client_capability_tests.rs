//! What the client says about itself at the handshake, and the one
//! server→client request that exists because of it.
//!
//! Split from `lib_tests`, which owns the wire harness. A capability the client
//! declares is a promise: `refreshSupport` is a promise to answer
//! `workspace/inlayHint/refresh` *and act on it*, and `dynamicRegistration` a
//! promise that a later `client/registerCapability` is honoured. These pin both
//! halves.

use serde_json::json;

use super::TestResult;
use super::wire;
use crate::LspClient;
use crate::Path;
use crate::ServerRefresh;

#[tokio::test]
async fn the_handshake_declares_inlay_hints_and_their_refresh() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let caps = &params["capabilities"];

    assert_eq!(
        caps["textDocument"]["inlayHint"]["dynamicRegistration"],
        json!(true)
    );
    assert_eq!(
        caps["workspace"]["inlayHint"]["refreshSupport"],
        json!(true)
    );
    Ok(())
}

#[tokio::test]
async fn dynamic_registration_is_declared_only_where_it_is_honoured() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let text = &params["capabilities"]["textDocument"];

    // Every gated request whose registration carries nothing but "on".
    for key in [
        "hover",
        "definition",
        "implementation",
        "typeHierarchy",
        "documentSymbol",
        "rename",
        "formatting",
        "rangeFormatting",
        "inlayHint",
    ] {
        assert_eq!(
            text[key]["dynamicRegistration"],
            json!(true),
            "{key} should declare dynamic registration"
        );
    }
    assert_eq!(
        params["capabilities"]["workspace"]["symbol"]["dynamicRegistration"],
        json!(true)
    );
    // Registrations for these carry trigger characters and action kinds the
    // handler drops, so inviting them would lose what a handshake says.
    for key in ["completion", "signatureHelp", "codeAction"] {
        assert!(
            text[key]["dynamicRegistration"].is_null(),
            "{key} must not invite a registration whose options are dropped"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_inlay_hint_refresh_is_answered_and_surfaced() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut server = server_task.await?;
    let mut refreshes = client.refreshes();

    server
        .send(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "workspace/inlayHint/refresh"
        }))
        .await;

    // Answered with `null`, not method-not-found: the server is told the
    // client heard it, rather than learning that karet cannot refresh.
    let answer = server.recv().await;
    assert_eq!(answer["id"], json!(7));
    assert_eq!(answer["result"], json!(null));
    assert!(answer.get("error").is_none(), "refresh refused: {answer}");

    let surfaced =
        tokio::time::timeout(std::time::Duration::from_secs(5), refreshes.recv()).await??;
    assert_eq!(surfaced, ServerRefresh::InlayHints);
    Ok(())
}

#[tokio::test]
async fn a_refresh_with_nobody_listening_is_still_answered() -> TestResult {
    // No subscriber is the normal state of a headless consumer that never
    // shows hints. The request must still be answered, or the server waits
    // on it forever.
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server
    });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut server = server_task.await?;

    server
        .send(&json!({
            "jsonrpc": "2.0", "id": "r", "method": "workspace/inlayHint/refresh"
        }))
        .await;
    let answer = server.recv().await;
    assert_eq!(answer["id"], json!("r"));
    assert_eq!(answer["result"], json!(null));
    Ok(())
}

//! Inlay hints over the wire: what the handshake promises about resolving
//! them, and the resolve itself.

use std::time::Duration;

use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::LineCol;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::tests::FakeServer;
use crate::tests::TestResult;
use crate::tests::everything;
use crate::tests::wire;

/// A client connected to a fake server that advertised `capabilities`, and
/// the `initialize` params the client sent it.
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

/// The client says which properties it lets a server defer, which is what a
/// server like rust-analyzer reads before leaving tooltips and locations out
/// of the first answer.
#[tokio::test]
async fn the_handshake_declares_which_hint_properties_resolve_on_demand() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    assert_eq!(
        params["capabilities"]["textDocument"]["inlayHint"]["resolveSupport"]["properties"],
        json!(["tooltip", "label.tooltip", "label.location"])
    );
    Ok(())
}

/// A listed hint keeps its `data`; resolving sends the hint back as it was
/// sent, and what comes back fills in the part's tooltip and location.
#[tokio::test]
async fn a_hint_resolves_with_its_own_data() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listed = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server
            .respond(
                &id,
                json!([{
                    "position": {"line": 3, "character": 8},
                    "label": [{"value": ": "}, {"value": "Vec"}],
                    "kind": 1,
                    "data": {"hint": 7}
                }]),
            )
            .await;
        server
    });
    let hints = client
        .inlay_hints(Path::new("/w/a.rs"), Range::default())
        .await?;
    let mut server = listed.await?;
    let hint = hints.first().ok_or("one hint")?.clone();
    assert_eq!(hint.location_for(1), None);
    assert!(hint.payload.is_some());

    let resolving = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let mut resolved = request["params"].clone();
        resolved["label"][1]["location"] = json!({
            "uri": "file:///w/vec.rs",
            "range": {"start": {"line": 9, "character": 11},
                      "end": {"line": 9, "character": 14}}
        });
        resolved["label"][1]["tooltip"] = json!("struct Vec<T>");
        // A server is not supposed to move a hint it resolves; if one does,
        // the caller's position still stands.
        resolved["position"] = json!({"line": 0, "character": 0});
        server.respond(&id, resolved).await;
        request
    });
    let resolved = client.resolve_inlay_hint(&hint).await?;
    let request = resolving.await?;
    assert_eq!(request["method"], "inlayHint/resolve");
    assert_eq!(request["params"]["data"], json!({"hint": 7}));
    assert_eq!(request["params"]["position"]["character"], json!(8));
    assert_eq!(resolved.position, LineCol::new(3, 8));
    let location = resolved.location_for(1).ok_or("the part has a location")?;
    assert_eq!(location.path, Path::new("/w/vec.rs"));
    assert_eq!(location.range.start, LineCol::new(9, 11));
    assert_eq!(
        resolved
            .tooltip_for(1)
            .map(|tooltip| tooltip.value.as_str()),
        Some("struct Vec<T>")
    );
    Ok(())
}

/// Nothing to resolve -- the server does not resolve hints, or the hint has
/// no payload -- comes back at once, with no request for a reply that would
/// never come.
#[tokio::test]
async fn nothing_to_resolve_comes_back_unchanged() -> TestResult {
    let mut capabilities = everything();
    capabilities["inlayHintProvider"] = json!(true);
    let (client, _server) = connect(capabilities).await?;
    let with_data = InlayHint {
        payload: Some(
            json!({"position": {"line": 0, "character": 0},
                             "label": "x", "data": 1})
            .to_string(),
        ),
        ..InlayHint::new(LineCol::default(), "x", InlayHintKind::Type)
    };
    let answer = tokio::time::timeout(
        Duration::from_secs(2),
        client.resolve_inlay_hint(&with_data),
    )
    .await??;
    assert_eq!(answer, with_data);

    let (client, _server) = connect(everything()).await?;
    let bare = InlayHint::new(LineCol::default(), "x", InlayHintKind::Type);
    let answer =
        tokio::time::timeout(Duration::from_secs(2), client.resolve_inlay_hint(&bare)).await??;
    assert_eq!(answer, bare);
    Ok(())
}

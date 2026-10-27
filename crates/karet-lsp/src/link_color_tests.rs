//! Document links and colours over the wire: listing and resolving links,
//! and listing colours.

use std::time::Duration;

use karet_core::DocumentLink;
use karet_core::LineCol;
use serde_json::Value;
use serde_json::json;

use super::document_link_from_lsp;
use super::rgba_from_lsp;
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

/// The capabilities `everything` advertises, without `key`.
fn without(key: &str) -> Value {
    let mut capabilities = everything();
    if let Some(caps) = capabilities.as_object_mut() {
        caps.remove(key);
    }
    capabilities
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

/// The client tells the server it shows links (with tooltips) and colours.
/// Links go without dynamic registration, whose `resolveProvider` the gate
/// would drop; colours carry nothing a registration could lose.
#[tokio::test]
async fn the_handshake_declares_links_and_colours() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let text_document = &params["capabilities"]["textDocument"];
    let links = &text_document["documentLink"];
    assert_eq!(links["tooltipSupport"], json!(true));
    assert!(links["dynamicRegistration"].is_null());
    assert_eq!(
        text_document["colorProvider"]["dynamicRegistration"],
        json!(true)
    );
    Ok(())
}

/// A link maps its range, target and tooltip, keeping its wire form for a
/// resolve; an empty target is no target; an entry with no range is no link;
/// a target no URI parser would accept still arrives.
#[test]
fn links_map_leniently() -> TestResult {
    let literal = json!({
        "range": range(3, 2, 9),
        "target": "file:///w/my notes.md",
        "tooltip": "Open the notes",
        "data": {"link": 1}
    });
    let link = document_link_from_lsp(literal.clone()).ok_or("a link with a range maps")?;
    assert_eq!(link.range.start, LineCol::new(3, 2));
    assert_eq!(link.range.end, LineCol::new(3, 9));
    assert_eq!(link.target.as_deref(), Some("file:///w/my notes.md"));
    assert_eq!(link.tooltip.as_deref(), Some("Open the notes"));
    let payload: Option<Value> = link
        .payload
        .as_deref()
        .and_then(|payload| serde_json::from_str(payload).ok());
    assert_eq!(payload, Some(literal));

    let empty = document_link_from_lsp(json!({"range": range(0, 0, 1), "target": ""}));
    assert_eq!(empty.and_then(|link| link.target), None);
    assert_eq!(document_link_from_lsp(json!({"target": "https://x"})), None);
    assert_eq!(document_link_from_lsp(json!(7)), None);
    Ok(())
}

/// Channels scale to bytes and round; out-of-range values clamp, and a
/// value that is not a number is 0 rather than a panic.
#[test]
fn colours_scale_round_and_clamp() {
    let colour = |red, green, blue, alpha| lsp_types::Color {
        red,
        green,
        blue,
        alpha,
    };
    assert_eq!(
        rgba_from_lsp(colour(1.0, 0.0, 0.5, 1.0)),
        [255, 0, 128, 255]
    );
    assert_eq!(
        rgba_from_lsp(colour(2.0, -1.0, f32::NAN, 0.2)),
        [255, 0, 0, 51]
    );
}

/// The list is asked for the document; a bad entry is skipped and the rest
/// kept, and a deferred link resolves by handing its own `data` back, the
/// answer keeping the link where it was.
#[tokio::test]
async fn links_list_and_resolve_with_their_own_data() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listing = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let links = json!([
            {"range": range(2, 0, 5), "data": {"link": 7}},
            {"no": "range"},
            {"range": range(4, 1, 3), "target": "https://example.com"},
        ]);
        server.respond(&id, links).await;
        (server, request)
    });
    let links = client.document_links(Path::new("/w/a.rs")).await?;
    let (mut server, request) = listing.await?;
    assert_eq!(request["method"], "textDocument/documentLink");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(links.len(), 2, "the entry without a range is skipped");
    let link = links.first().ok_or("one link")?.clone();
    assert!(!link.is_resolved());

    let resolving = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let mut resolved = request["params"].clone();
        resolved["target"] = json!("file:///w/b.rs");
        // A server is not supposed to move a link it resolves; if one does,
        // the caller's range still stands.
        resolved["range"]["start"]["line"] = json!(0);
        server.respond(&id, resolved).await;
        request
    });
    let resolved = client.resolve_document_link(&link).await?;
    let request = resolving.await?;
    assert_eq!(request["method"], "documentLink/resolve");
    assert_eq!(request["params"]["data"], json!({"link": 7}));
    assert_eq!(resolved.target.as_deref(), Some("file:///w/b.rs"));
    assert_eq!(resolved.range, link.range);
    Ok(())
}

/// A resolve answered with something that is not a link is a protocol
/// error, not a link with no range.
#[tokio::test]
async fn a_resolve_answered_with_no_link_is_an_error() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let deferred = DocumentLink {
        payload: Some(json!({"range": range(0, 0, 1), "data": 1}).to_string()),
        ..DocumentLink::default()
    };
    let resolving = tokio::spawn(async move {
        let request = server.recv().await;
        server.respond(&request["id"], json!(null)).await;
    });
    let answer = client.resolve_document_link(&deferred).await;
    resolving.await?;
    assert!(matches!(answer, Err(LspError::Protocol(_))), "{answer:?}");
    Ok(())
}

/// Nothing to resolve -- the link already has its target, it has no
/// payload, or the server does not resolve links -- comes back at once, with
/// no request for a reply that would never come.
#[tokio::test]
async fn nothing_to_resolve_comes_back_unchanged() -> TestResult {
    let mut capabilities = everything();
    capabilities["documentLinkProvider"] = json!({});
    let (client, _server) = connect(capabilities).await?;
    let deferred = DocumentLink {
        payload: Some(json!({"range": range(0, 0, 1), "data": 1}).to_string()),
        ..DocumentLink::default()
    };
    let answer = tokio::time::timeout(
        Duration::from_secs(2),
        client.resolve_document_link(&deferred),
    )
    .await??;
    assert_eq!(answer, deferred);

    let (client, _server) = connect(everything()).await?;
    for link in [
        DocumentLink::default(),
        DocumentLink {
            target: Some("https://example.com".into()),
            ..deferred
        },
    ] {
        let answer =
            tokio::time::timeout(Duration::from_secs(2), client.resolve_document_link(&link))
                .await??;
        assert_eq!(answer, link);
    }
    Ok(())
}

/// Colours are asked for the document and arrive as bytes over their spans.
#[tokio::test]
async fn colours_list_as_bytes() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listing = tokio::spawn(async move {
        let request = server.recv().await;
        let colours = json!([{
            "range": range(1, 7, 14),
            "color": {"red": 1.0, "green": 0.0, "blue": 0.0, "alpha": 1.0}
        }]);
        server.respond(&request["id"], colours).await;
        request
    });
    let colours = client.document_colors(Path::new("/w/a.css")).await?;
    let request = listing.await?;
    assert_eq!(request["method"], "textDocument/documentColor");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.css");
    let colour = colours.first().ok_or("one colour")?;
    assert_eq!(colour.rgba, [255, 0, 0, 255]);
    assert_eq!(colour.range.start, LineCol::new(1, 7));
    Ok(())
}

/// Without the provider, each listing is refused before anything is sent.
#[tokio::test]
async fn listings_are_refused_without_the_capability() -> TestResult {
    let (client, _server) = connect(without("documentLinkProvider")).await?;
    let refused = client.document_links(Path::new("/w/a.rs")).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "textDocument/documentLink"
            })
        ),
        "{refused:?}"
    );
    let (client, _server) = connect(without("colorProvider")).await?;
    let refused = client.document_colors(Path::new("/w/a.css")).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "textDocument/documentColor"
            })
        ),
        "{refused:?}"
    );
    Ok(())
}

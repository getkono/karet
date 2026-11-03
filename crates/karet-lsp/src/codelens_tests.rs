//! Code lenses over the wire: listing, resolving, and running one.

use std::time::Duration;

use karet_core::CodeLens;
use karet_core::CommandId;
use karet_core::LineCol;
use serde_json::Value;
use serde_json::json;

use super::code_lens_from_lsp;
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

/// The client tells the server it shows lenses: some only compute them for a
/// client that says so.
#[tokio::test]
async fn the_handshake_declares_code_lenses() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let declared = &params["capabilities"]["textDocument"]["codeLens"];
    assert!(declared.is_object());
    assert!(
        declared["dynamicRegistration"].is_null(),
        "a registration's resolveProvider would be dropped"
    );
    assert!(params["capabilities"]["workspace"]["codeLens"].is_null());
    Ok(())
}

/// A resolved lens maps its title and command; a lens a server deferred has
/// neither until it is resolved; a command with no name only informs.
#[test]
fn lenses_map_their_title_and_command() -> Result<(), serde_json::Error> {
    let range = json!({"start": {"line": 4, "character": 2}, "end": {"line": 4, "character": 9}});
    let shown = code_lens_from_lsp(serde_json::from_value(json!({
        "range": range,
        "command": {"title": "Run test", "command": "test.run", "arguments": [1]}
    }))?);
    assert_eq!(shown.title.as_deref(), Some("Run test"));
    assert_eq!(shown.command, Some(CommandId("test.run".into())));
    assert_eq!(shown.range.start, LineCol::new(4, 2));
    let deferred = code_lens_from_lsp(serde_json::from_value(json!({
        "range": range, "data": {"lens": 1}
    }))?);
    assert!(!deferred.is_resolved());
    assert_eq!(deferred.command, None);
    let informs = code_lens_from_lsp(serde_json::from_value(json!({
        "range": range, "command": {"title": "3 references", "command": ""}
    }))?);
    assert_eq!(informs.title.as_deref(), Some("3 references"));
    assert_eq!(informs.command, None);
    Ok(())
}

/// The list is asked for the document, and a deferred lens resolves by
/// handing its own `data` back; the answer keeps the lens where it was.
#[tokio::test]
async fn a_lens_lists_and_resolves_with_its_own_data() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let listing = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let lens = json!({
            "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 5}},
            "data": {"lens": 7}
        });
        server.respond(&id, json!([lens])).await;
        (server, request)
    });
    let lenses = client.code_lenses(Path::new("/w/a.rs")).await?;
    let (mut server, request) = listing.await?;
    assert_eq!(request["method"], "textDocument/codeLens");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    let lens = lenses.first().ok_or("one lens")?.clone();
    assert!(!lens.is_resolved());

    let resolving = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let mut resolved = request["params"].clone();
        resolved["command"] = json!({"title": "2 implementations", "command": "show"});
        // A server is not supposed to move a lens it resolves; if one does,
        // the caller's range still stands.
        resolved["range"]["start"]["line"] = json!(0);
        server.respond(&id, resolved).await;
        request
    });
    let resolved = client.resolve_code_lens(&lens).await?;
    let request = resolving.await?;
    assert_eq!(request["method"], "codeLens/resolve");
    assert_eq!(request["params"]["data"], json!({"lens": 7}));
    assert_eq!(resolved.title.as_deref(), Some("2 implementations"));
    assert_eq!(resolved.range, lens.range);
    Ok(())
}

/// Nothing to resolve -- the lens already has its title, it has no payload,
/// or the server does not resolve lenses -- comes back at once, with no
/// request for a reply that would never come.
#[tokio::test]
async fn nothing_to_resolve_comes_back_unchanged() -> TestResult {
    let mut capabilities = everything();
    capabilities["codeLensProvider"] = json!({});
    let (client, _server) = connect(capabilities).await?;
    let deferred = CodeLens {
        payload: Some(
            json!({"range": {"start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 0}},
                             "data": 1})
            .to_string(),
        ),
        ..CodeLens::default()
    };
    let answer =
        tokio::time::timeout(Duration::from_secs(2), client.resolve_code_lens(&deferred)).await??;
    assert_eq!(answer, deferred);

    let (client, _server) = connect(everything()).await?;
    for lens in [
        CodeLens::default(),
        CodeLens {
            title: Some("Run".into()),
            ..deferred
        },
    ] {
        let answer =
            tokio::time::timeout(Duration::from_secs(2), client.resolve_code_lens(&lens)).await??;
        assert_eq!(answer, lens);
    }
    Ok(())
}

/// Without a lens provider, listing is refused before anything is sent.
#[tokio::test]
async fn listing_is_refused_without_the_capability() -> TestResult {
    let mut capabilities = everything();
    if let Some(caps) = capabilities.as_object_mut() {
        caps.remove("codeLensProvider");
    }
    let (client, _server) = connect(capabilities).await?;
    let refused = client.code_lenses(Path::new("/w/a.rs")).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "textDocument/codeLens"
            })
        ),
        "{refused:?}"
    );
    Ok(())
}

/// A lens's command runs with the arguments its server gave it; a lens that
/// only informs runs nothing, and a command the server does not run (a
/// client-side one) is refused without being sent.
#[tokio::test]
async fn a_lens_runs_its_command_with_its_arguments() -> TestResult {
    let mut capabilities = everything();
    capabilities["executeCommandProvider"] = json!({"commands": ["test.run"]});
    let (client, mut server) = connect(capabilities).await?;
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
    let lens = |command: &str| -> Result<CodeLens, serde_json::Error> {
        Ok(code_lens_from_lsp(serde_json::from_value(json!({
            "range": range,
            "command": {"title": "Run", "command": command, "arguments": [{"test": "a"}]}
        }))?))
    };
    let running = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server.respond(&id, Value::Null).await;
        request
    });
    client.run_code_lens(&lens("test.run")?).await?;
    let request = running.await?;
    assert_eq!(request["method"], "workspace/executeCommand");
    assert_eq!(request["params"]["command"], "test.run");
    assert_eq!(request["params"]["arguments"], json!([{"test": "a"}]));

    client.run_code_lens(&lens("")?).await?;
    client.run_code_lens(&CodeLens::default()).await?;
    let refused = client.run_code_lens(&lens("editor.showReferences")?).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "workspace/executeCommand"
            })
        ),
        "{refused:?}"
    );
    Ok(())
}

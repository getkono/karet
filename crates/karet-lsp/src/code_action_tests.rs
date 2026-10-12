//! Code actions over the wire: the context a request carries, resolving a
//! deferred edit, and running a command.

use std::time::Duration;

use karet_core::CodeAction;
use karet_core::LineCol;
use karet_core::Range;
use serde_json::Value;
use serde_json::json;

use super::CodeActionTrigger;
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

fn span(start: (u32, u32), end: (u32, u32)) -> Value {
    json!({"start": {"line": start.0, "character": start.1},
           "end": {"line": end.0, "character": end.1}})
}

/// The request hands back the diagnostics the server published that touch
/// the range -- `data` included, which is where some servers keep the fix --
/// says why it was asked, and passes the kind filter on; kinds the server
/// returns anyway are filtered out of the answer.
#[tokio::test]
async fn a_request_carries_touching_diagnostics_and_its_kind_filter() -> TestResult {
    let (client, mut server) = connect(everything()).await?;
    let mut published = client.diagnostics();
    let near = json!({"range": span((2, 4), (2, 9)), "message": "unused", "data": {"fix": 1}});
    let far = json!({"range": span((9, 0), (9, 3)), "message": "elsewhere"});
    server
        .send(
            &json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                      "params": {"uri": "file:///w/a.rs", "diagnostics": [near, far]}}),
        )
        .await;
    published.recv().await?;

    let server_task = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server
            .respond(
                &id,
                json!([
                    {"title": "Remove it", "kind": "quickfix"},
                    {"title": "Extract", "kind": "refactor.extract"}
                ]),
            )
            .await;
        request
    });
    // A caret resting at the diagnostic's end still touches it.
    let caret = Range {
        start: LineCol::new(2, 9),
        end: LineCol::new(2, 9),
    };
    let actions = client
        .code_actions(
            Path::new("/w/a.rs"),
            caret,
            &["quickfix".to_owned()],
            CodeActionTrigger::Automatic,
        )
        .await?;
    let request = server_task.await?;
    assert_eq!(request["method"], "textDocument/codeAction");
    let context = &request["params"]["context"];
    assert_eq!(context["diagnostics"].as_array().map(Vec::len), Some(1));
    assert_eq!(context["diagnostics"][0]["message"], "unused");
    assert_eq!(context["diagnostics"][0]["data"], json!({"fix": 1}));
    assert_eq!(context["only"], json!(["quickfix"]));
    assert_eq!(context["triggerKind"], json!(2));
    let titles: Vec<&str> = actions.iter().map(|action| action.title.as_str()).collect();
    assert_eq!(titles, ["Remove it"]);
    Ok(())
}

/// An action that arrived without its edit is resolved with its own `data`,
/// and comes back with the edit, still resolvable and runnable.
#[tokio::test]
async fn a_deferred_edit_is_resolved_with_the_actions_own_data() -> TestResult {
    let mut capabilities = everything();
    capabilities["codeActionProvider"] = json!({"resolveProvider": true});
    let (client, mut server) = connect(capabilities).await?;
    let listed = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server
            .respond(&id, json!([{"title": "Import", "data": {"token": 42}}]))
            .await;
        server
    });
    let range = Range::default();
    let actions = client.code_action(Path::new("/w/a.rs"), range).await?;
    let mut server = listed.await?;
    let action = actions.first().ok_or("one action")?.clone();
    assert_eq!(action.edit, None);

    let resolving = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        let mut resolved = request["params"].clone();
        resolved["edit"] = json!({"changes": {"file:///w/a.rs": [
            {"range": span((0, 0), (0, 0)), "newText": "use a;\n"}
        ]}});
        server.respond(&id, resolved).await;
        request
    });
    let resolved = client.resolve_code_action(&action).await?;
    let request = resolving.await?;
    assert_eq!(request["method"], "codeAction/resolve");
    assert_eq!(request["params"]["data"], json!({"token": 42}));
    let edit = resolved.edit.ok_or("the resolved action has an edit")?;
    assert_eq!(edit.changes.len(), 1);
    assert_eq!(resolved.title, "Import");
    assert!(
        resolved.payload.is_some(),
        "the resolved action keeps its wire form"
    );
    Ok(())
}

/// Nothing to resolve -- the server does not resolve, the action already has
/// its edit, or it is not a server's -- comes back at once, with no request
/// for a reply that would never come.
#[tokio::test]
async fn nothing_to_resolve_comes_back_unchanged() -> TestResult {
    let (client, _server) = connect(everything()).await?;
    let unresolvable = CodeAction {
        title: "Import".into(),
        payload: Some(json!({"title": "Import", "data": 1}).to_string()),
        ..CodeAction::default()
    };
    let answer = tokio::time::timeout(
        Duration::from_secs(2),
        client.resolve_code_action(&unresolvable),
    )
    .await??;
    assert_eq!(answer, unresolvable);
    Ok(())
}

/// A command runs with the arguments its server gave it; a command the server
/// did not list is refused without being sent.
#[tokio::test]
async fn commands_run_with_their_arguments_and_unlisted_ones_are_refused() -> TestResult {
    let mut capabilities = everything();
    capabilities["executeCommandProvider"] = json!({"commands": ["fix.apply"]});
    let (client, mut server) = connect(capabilities).await?;
    let action = CodeAction {
        title: "Fix".into(),
        payload: Some(
            json!({"title": "Fix", "command": {"title": "Fix", "command": "fix.apply",
                                                "arguments": [{"at": 3}]}})
            .to_string(),
        ),
        ..CodeAction::default()
    };
    let running = tokio::spawn(async move {
        let request = server.recv().await;
        let id = request["id"].clone();
        server.respond(&id, Value::Null).await;
        request
    });
    client.run_code_action_command(&action).await?;
    let request = running.await?;
    assert_eq!(request["method"], "workspace/executeCommand");
    assert_eq!(request["params"]["command"], "fix.apply");
    assert_eq!(request["params"]["arguments"], json!([{"at": 3}]));

    let refused = client.execute_command("other.command", Vec::new()).await;
    assert!(
        matches!(
            refused,
            Err(LspError::Unsupported {
                method: "workspace/executeCommand"
            })
        ),
        "{refused:?}"
    );
    // An action carrying no command runs nothing.
    client
        .run_code_action_command(&CodeAction::default())
        .await?;
    Ok(())
}

/// The handshake promises what this module does: literals of every kind,
/// preferred and disabled flags, `data` round trips, lazily resolved edits,
/// and commands.
#[tokio::test]
async fn the_handshake_declares_code_action_support() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let actions = &params["capabilities"]["textDocument"]["codeAction"];
    let kinds = &actions["codeActionLiteralSupport"]["codeActionKind"]["valueSet"];
    for kind in [
        "quickfix",
        "refactor",
        "source",
        "source.organizeImports",
        "source.fixAll",
    ] {
        assert!(
            kinds
                .as_array()
                .is_some_and(|set| set.contains(&json!(kind))),
            "{kind} missing from {kinds}"
        );
    }
    assert_eq!(actions["isPreferredSupport"], json!(true));
    assert_eq!(actions["disabledSupport"], json!(true));
    assert_eq!(actions["dataSupport"], json!(true));
    assert_eq!(actions["resolveSupport"]["properties"], json!(["edit"]));
    assert!(params["capabilities"]["workspace"]["executeCommand"].is_object());
    Ok(())
}

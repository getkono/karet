//! Declaration, type definition and references over the wire: the method and
//! params each sends, how its answer maps to file locations, and the refusal
//! a server that never advertised one gets instead of a request.

use karet_core::LineCol;
use serde_json::Value;
use serde_json::json;

use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::tests::FakeServer;
use crate::tests::TestResult;
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

fn location(uri: &str, line: u32, character: u32) -> Value {
    json!({"uri": uri, "range": {"start": {"line": line, "character": character},
                                 "end": {"line": line, "character": character + 3}}})
}

/// Answer the next request with `result` after checking its method, and hand
/// the request back for assertions on its params.
async fn answer(mut server: FakeServer, method: &'static str, result: Value) -> Value {
    let request = server.recv().await;
    assert_eq!(request["method"], method);
    server.respond(&request["id"], result).await;
    request
}

#[tokio::test]
async fn references_ask_for_the_declaration_and_keep_server_order() -> TestResult {
    let (client, server) = connect(json!({"referencesProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        "textDocument/references",
        json!([
            location("file:///w/b.rs", 4, 2),
            // A non-file URI has nowhere to jump to and is dropped.
            location("untitled:Scratch", 0, 0),
            location("file:///w/a.rs", 1, 8),
        ]),
    ));
    let found = client
        .references(Path::new("/w/a.rs"), LineCol::new(1, 8), true)
        .await?;
    let request = answered.await?;
    assert_eq!(request["params"]["context"]["includeDeclaration"], true);
    assert_eq!(
        request["params"]["position"],
        json!({"line": 1, "character": 8})
    );
    assert_eq!(
        found
            .iter()
            .map(|l| (l.path.clone(), l.range.start))
            .collect::<Vec<_>>(),
        vec![
            (Path::new("/w/b.rs").to_path_buf(), LineCol::new(4, 2)),
            (Path::new("/w/a.rs").to_path_buf(), LineCol::new(1, 8)),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_null_references_answer_is_no_references() -> TestResult {
    let (client, server) = connect(json!({"referencesProvider": true})).await?;
    let answered = tokio::spawn(answer(server, "textDocument/references", Value::Null));
    let found = client
        .references(Path::new("/w/a.rs"), LineCol::new(0, 0), false)
        .await?;
    let request = answered.await?;
    assert_eq!(request["params"]["context"]["includeDeclaration"], false);
    assert!(found.is_empty());
    Ok(())
}

#[tokio::test]
async fn declaration_maps_a_single_location() -> TestResult {
    let (client, server) = connect(json!({"declarationProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        "textDocument/declaration",
        location("file:///w/a.h", 3, 5),
    ));
    let found = client
        .declaration(Path::new("/w/a.c"), LineCol::new(9, 1))
        .await?;
    answered.await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, Path::new("/w/a.h"));
    assert_eq!(found[0].range.start, LineCol::new(3, 5));
    Ok(())
}

#[tokio::test]
async fn type_definition_lands_on_a_links_selection_range() -> TestResult {
    let (client, server) = connect(json!({"typeDefinitionProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        "textDocument/typeDefinition",
        json!([{
            "targetUri": "file:///w/types.rs",
            "targetRange": {"start": {"line": 2, "character": 0},
                            "end": {"line": 6, "character": 1}},
            "targetSelectionRange": {"start": {"line": 2, "character": 11},
                                     "end": {"line": 2, "character": 17}}
        }]),
    ));
    let found = client
        .type_definition(Path::new("/w/a.rs"), LineCol::new(0, 4))
        .await?;
    answered.await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, Path::new("/w/types.rs"));
    assert_eq!(found[0].range.start, LineCol::new(2, 11));
    Ok(())
}

/// None of the three reaches the wire when the server never offered it: a
/// request sent anyway would come back as an error indistinguishable from a
/// broken server.
#[tokio::test]
async fn unadvertised_navigation_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"definitionProvider": true})).await?;
    let (doc, pos) = (Path::new("/w/a.rs"), LineCol::new(0, 0));
    assert!(matches!(
        client.declaration(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/declaration"
        })
    ));
    assert!(matches!(
        client.type_definition(doc, pos).await,
        Err(LspError::Unsupported {
            method: "textDocument/typeDefinition"
        })
    ));
    assert!(matches!(
        client.references(doc, pos, true).await,
        Err(LspError::Unsupported {
            method: "textDocument/references"
        })
    ));
    Ok(())
}

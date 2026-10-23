//! Linked editing ranges, on-type formatting and will-save edits over the
//! wire: the params each sends, what each answer maps to, and the refusals a
//! server that never offered them gets instead of a request.

use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use serde_json::Value;
use serde_json::json;

use super::LinkedEditingRanges;
use super::SaveReason;
use crate::Indentation;
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

/// Answer the next request, which must be `method`, with `result`; the request.
async fn answer(mut server: FakeServer, method: &'static str, result: Value) -> Value {
    let request = server.recv().await;
    assert_eq!(request["method"], method);
    server.respond(&request["id"], result).await;
    request
}

fn lsp_range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start},
           "end": {"line": line, "character": end}})
}

fn range(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

const ON_TYPE: &str = "textDocument/onTypeFormatting";

fn on_type_server() -> Value {
    json!({"documentOnTypeFormattingProvider": {
        "firstTriggerCharacter": "}",
        "moreTriggerCharacter": [";", "\n"]
    }})
}

#[tokio::test]
async fn linked_ranges_keep_their_order_and_word_pattern() -> TestResult {
    let (client, server) = connect(json!({"linkedEditingRangeProvider": true})).await?;
    let answered = tokio::spawn(answer(
        server,
        "textDocument/linkedEditingRange",
        json!({"ranges": [lsp_range(0, 1, 4), lsp_range(2, 2, 5)], "wordPattern": "[a-z]+"}),
    ));
    let linked = client
        .linked_editing_ranges(Path::new("/w/a.html"), LineCol::new(0, 2))
        .await?;
    let request = answered.await?;
    assert_eq!(
        request["params"]["position"],
        json!({"line": 0, "character": 2})
    );
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.html");
    assert_eq!(
        linked,
        Some(LinkedEditingRanges {
            ranges: vec![range(0, 1, 4), range(2, 2, 5)],
            word_pattern: Some("[a-z]+".to_owned()),
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_null_linked_answer_is_none() -> TestResult {
    let (client, server) = connect(json!({"linkedEditingRangeProvider": {}})).await?;
    let answered = tokio::spawn(answer(
        server,
        "textDocument/linkedEditingRange",
        Value::Null,
    ));
    let linked = client
        .linked_editing_ranges(Path::new("/w/a.html"), LineCol::new(0, 0))
        .await?;
    answered.await?;
    assert_eq!(linked, None);
    Ok(())
}

#[tokio::test]
async fn unadvertised_linked_editing_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"hoverProvider": true})).await?;
    assert!(matches!(
        client
            .linked_editing_ranges(Path::new("/w/a.html"), LineCol::new(0, 0))
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/linkedEditingRange"
        })
    ));
    Ok(())
}

#[tokio::test]
async fn a_trigger_character_sends_its_position_character_and_indentation() -> TestResult {
    let (client, server) = connect(on_type_server()).await?;
    let answered = tokio::spawn(answer(
        server,
        ON_TYPE,
        json!([{"range": lsp_range(1, 0, 2), "newText": "    "}]),
    ));
    let indentation = Indentation {
        tab_size: 2,
        insert_spaces: false,
    };
    let edits = client
        .on_type_formatting(Path::new("/w/a.c"), LineCol::new(1, 3), ';', indentation)
        .await?;
    let request = answered.await?;
    let params = &request["params"];
    assert_eq!(params["position"], json!({"line": 1, "character": 3}));
    assert_eq!(params["ch"], ";");
    assert_eq!(params["options"]["tabSize"], 2);
    assert_eq!(params["options"]["insertSpaces"], false);
    assert_eq!(
        edits,
        vec![TextEdit {
            range: range(1, 0, 2),
            new_text: "    ".to_owned(),
        }]
    );
    Ok(())
}

/// The first trigger character counts as much as the others; a newline is
/// sent as one, the way servers list it.
#[tokio::test]
async fn the_first_trigger_and_a_newline_both_trigger() -> TestResult {
    for ch in ['}', '\n'] {
        let (client, server) = connect(on_type_server()).await?;
        let answered = tokio::spawn(answer(server, ON_TYPE, Value::Null));
        let edits = client
            .on_type_formatting(
                Path::new("/w/a.c"),
                LineCol::new(0, 1),
                ch,
                Indentation::default(),
            )
            .await?;
        let request = answered.await?;
        assert_eq!(request["params"]["ch"], ch.to_string());
        assert!(edits.is_empty(), "a null answer is no edits");
    }
    Ok(())
}

/// Falsifier: were a non-trigger sent, the fake server would see this
/// request first and fail the method assertion; the next request it reads is
/// the one for the real trigger.
#[tokio::test]
async fn a_character_the_server_does_not_format_on_sends_nothing() -> TestResult {
    let (client, server) = connect(on_type_server()).await?;
    let answered = tokio::spawn(answer(server, ON_TYPE, json!([])));
    let path = Path::new("/w/a.c");
    let at = LineCol::new(0, 1);
    let skipped = client
        .on_type_formatting(path, at, 'x', Indentation::default())
        .await?;
    assert!(skipped.is_empty());
    client
        .on_type_formatting(path, at, ';', Indentation::default())
        .await?;
    let request = answered.await?;
    assert_eq!(request["params"]["ch"], ";");
    Ok(())
}

#[tokio::test]
async fn unadvertised_on_type_formatting_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"documentFormattingProvider": true})).await?;
    assert!(matches!(
        client
            .on_type_formatting(
                Path::new("/w/a.c"),
                LineCol::new(0, 0),
                ';',
                Indentation::default()
            )
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/onTypeFormatting"
        })
    ));
    Ok(())
}

#[tokio::test]
async fn will_save_sends_the_reason_and_returns_the_edits() -> TestResult {
    for (reason, wire_reason) in [
        (SaveReason::Manual, 1),
        (SaveReason::AfterDelay, 2),
        (SaveReason::FocusOut, 3),
    ] {
        let (client, server) =
            connect(json!({"textDocumentSync": {"willSaveWaitUntil": true}})).await?;
        let answered = tokio::spawn(answer(
            server,
            "textDocument/willSaveWaitUntil",
            json!([{"range": lsp_range(0, 0, 0), "newText": "use x;\n"}]),
        ));
        let edits = client
            .will_save_wait_until(Path::new("/w/a.rs"), reason)
            .await?;
        let request = answered.await?;
        assert_eq!(request["params"]["reason"], wire_reason);
        assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
        assert_eq!(
            edits,
            vec![TextEdit {
                range: range(0, 0, 0),
                new_text: "use x;\n".to_owned(),
            }]
        );
    }
    Ok(())
}

#[tokio::test]
async fn will_save_not_asked_for_is_refused_without_a_request() -> TestResult {
    let (client, _server) = connect(json!({"textDocumentSync": 2})).await?;
    assert!(matches!(
        client
            .will_save_wait_until(Path::new("/w/a.rs"), SaveReason::Manual)
            .await,
        Err(LspError::Unsupported {
            method: "textDocument/willSaveWaitUntil"
        })
    ));
    Ok(())
}

/// The client says it sends all three, so a server that only offers them to
/// a client that declares them still does.
#[tokio::test]
async fn the_handshake_declares_all_three() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let text = &params["capabilities"]["textDocument"];
    assert_eq!(text["synchronization"]["willSaveWaitUntil"], json!(true));
    assert!(text["linkedEditingRange"].is_object());
    assert!(text["onTypeFormatting"].is_object());
    // Its registration options carry the trigger characters, which a
    // registration would drop: not invited.
    assert!(text["onTypeFormatting"]["dynamicRegistration"].is_null());
    Ok(())
}

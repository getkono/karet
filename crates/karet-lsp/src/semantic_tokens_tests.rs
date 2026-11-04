//! Semantic tokens: decoding the packed array against a legend, the whole and
//! delta requests over the wire, and what the handshake promises about them.

use std::time::Duration;

use karet_core::LineCol;
use karet_core::SemanticToken;
use karet_core::SemanticTokenModifiers as M;
use karet_core::SemanticTokenType as T;
use karet_core::TokenId;
use serde_json::Value;
use serde_json::json;

use super::SemanticTokens;
use super::WireEdit;
use crate::LspClient;
use crate::LspError;
use crate::Path;
use crate::SemanticTokensLegend;
use crate::ServerRefresh;
use crate::tests::FakeServer;
use crate::tests::TestResult;
use crate::tests::wire;

fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: ["variable", "widget", "macro"].map(str::to_owned).to_vec(),
        token_modifiers: ["mutable", "consuming", "deprecated"]
            .map(str::to_owned)
            .to_vec(),
    }
}

fn token(line: u32, col: u32, length: u32, token: TokenId) -> SemanticToken {
    SemanticToken {
        start: LineCol::new(line, col),
        length,
        token,
    }
}

/// Positions accumulate relative to the previous token -- including one that
/// is dropped -- the start resetting on a new line; modifier bits map by the
/// legend's names, an unknown one ignored.
#[test]
fn decoding_follows_relative_positions_and_the_legend() {
    let tokens = SemanticTokens {
        result_id: None,
        data: vec![
            0, 4, 3, 0, 0b001, // line 0, col 4: variable.mutable
            0, 6, 2, 1, 0, // col 10: `widget`, outside the vocabulary
            0, 4, 5, 2, 0b110, // col 14: macro.deprecated (consuming ignored)
            2, 1, 0, 0, 0, // line 2, col 1: no length
            0, 3, 1, 0, 0, // line 2, col 4: variable
            9, // a trailing partial group
        ],
    };
    assert_eq!(
        tokens.decode(&legend()),
        [
            token(0, 4, 3, TokenId::semantic(T::Variable, M::MUTABLE)),
            token(0, 14, 5, TokenId::semantic(T::Macro, M::DEPRECATED)),
            token(2, 4, 1, TokenId::semantic(T::Variable, M::NONE)),
        ]
    );
}

/// A type index past the legend is dropped rather than misread.
#[test]
fn an_index_outside_the_legend_is_dropped() {
    let tokens = SemanticTokens {
        result_id: None,
        data: vec![0, 0, 1, 7, 0, 0, 2, 1, 0, 1 << 31],
    };
    assert_eq!(
        tokens.decode(&legend()),
        [token(0, 2, 1, TokenId::semantic(T::Variable, M::NONE))]
    );
}

fn edit(start: u32, delete_count: u32, data: &[u32]) -> WireEdit {
    WireEdit {
        start,
        delete_count,
        data: data.to_vec(),
    }
}

/// Edits name spans of the array as it was, whatever order they arrive in.
#[test]
fn delta_edits_apply_against_the_previous_array() -> Result<(), LspError> {
    let mut tokens = SemanticTokens {
        result_id: None,
        data: (0..15).collect(),
    };
    tokens.apply(vec![edit(0, 5, &[]), edit(10, 0, &[90, 91])])?;
    assert_eq!(tokens.data, [5, 6, 7, 8, 9, 90, 91, 10, 11, 12, 13, 14]);
    Ok(())
}

#[test]
fn a_delta_edit_past_the_array_or_overlapping_another_is_refused() {
    let base = SemanticTokens {
        result_id: None,
        data: (0..10).collect(),
    };
    let mut past = base.clone();
    assert!(matches!(
        past.apply(vec![edit(8, 5, &[])]),
        Err(LspError::Protocol(_))
    ));
    let mut overlapping = base;
    assert!(matches!(
        overlapping.apply(vec![edit(0, 6, &[]), edit(5, 5, &[])]),
        Err(LspError::Protocol(_))
    ));
}

fn provider(delta: bool) -> Value {
    json!({
        "textDocumentSync": 1,
        "semanticTokensProvider": {
            "legend": {
                "tokenTypes": ["variable", "widget", "macro"],
                "tokenModifiers": ["mutable", "consuming", "deprecated"]
            },
            "full": {"delta": delta}
        }
    })
}

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

/// Serve one request with `result`, returning the request as received.
async fn serve(server: &mut FakeServer, result: Value) -> Value {
    let request = server.recv().await;
    server.respond(&request["id"].clone(), result).await;
    request
}

/// The first request asks for the whole document; the next hands back its
/// result id and gets edits, applied to the array it kept.
#[tokio::test]
async fn a_whole_answer_then_a_delta_against_it() -> TestResult {
    let (client, mut server) = connect(provider(true)).await?;
    let doc = Path::new("/w/a.rs");
    let first = tokio::spawn(async move {
        let request = serve(
            &mut server,
            json!({"resultId": "1", "data": [0, 0, 1, 0, 0, 0, 2, 3, 2, 0]}),
        )
        .await;
        (server, request)
    });
    let whole = client.semantic_tokens(doc, None).await?;
    let (mut server, request) = first.await?;
    assert_eq!(request["method"], "textDocument/semanticTokens/full");
    assert_eq!(request["params"]["textDocument"]["uri"], "file:///w/a.rs");
    assert_eq!(whole.result_id.as_deref(), Some("1"));

    let second = tokio::spawn(async move {
        let request = serve(
            &mut server,
            json!({"resultId": "2", "edits": [{"start": 5, "deleteCount": 5, "data": [1, 4, 2, 0, 1]}]}),
        )
        .await;
        (server, request)
    });
    let delta = client.semantic_tokens(doc, Some(&whole)).await?;
    let (_server, request) = second.await?;
    assert_eq!(request["method"], "textDocument/semanticTokens/full/delta");
    assert_eq!(request["params"]["previousResultId"], "1");
    assert_eq!(delta.result_id.as_deref(), Some("2"));
    assert_eq!(delta.data, [0, 0, 1, 0, 0, 1, 4, 2, 0, 1]);
    let legend = client.semantic_tokens_legend().ok_or("no legend")?;
    assert_eq!(
        delta.decode(&legend),
        [
            token(0, 0, 1, TokenId::semantic(T::Variable, M::NONE)),
            token(1, 4, 2, TokenId::semantic(T::Variable, M::MUTABLE)),
        ]
    );
    Ok(())
}

/// A server without deltas is asked for the whole document every time, and a
/// delta request answered whole -- or with `null` -- is taken at its word.
#[tokio::test]
async fn without_deltas_or_with_a_whole_reply_the_answer_is_taken_whole() -> TestResult {
    let (client, mut server) = connect(provider(false)).await?;
    let doc = Path::new("/w/a.rs");
    let previous = SemanticTokens {
        result_id: Some("1".to_owned()),
        data: vec![0, 0, 1, 0, 0],
    };
    let served = tokio::spawn(async move {
        let first = serve(&mut server, json!({"data": [0, 1, 1, 2, 0]})).await;
        let second = serve(&mut server, Value::Null).await;
        (first, second)
    });
    let whole = client.semantic_tokens(doc, Some(&previous)).await?;
    let empty = client.semantic_tokens(doc, Some(&whole)).await?;
    let (first, second) = served.await?;
    assert_eq!(first["method"], "textDocument/semanticTokens/full");
    assert_eq!(second["method"], "textDocument/semanticTokens/full");
    assert_eq!(whole.result_id, None);
    assert_eq!(whole.data, [0, 1, 1, 2, 0]);
    assert_eq!(empty, SemanticTokens::default());
    Ok(())
}

/// A server that never offered semantic tokens is never asked for them.
#[tokio::test]
async fn a_server_without_semantic_tokens_is_not_asked() -> TestResult {
    let (client, mut server) = connect(json!({"textDocumentSync": 1})).await?;
    let refused = client.semantic_tokens(Path::new("/w/a.rs"), None).await;
    assert!(matches!(refused, Err(LspError::Unsupported { .. })));
    assert!(client.semantic_tokens_legend().is_none());
    let nothing = tokio::time::timeout(Duration::from_millis(200), server.recv()).await;
    assert!(nothing.is_err(), "a request reached the wire");
    Ok(())
}

/// The handshake asks for whole and delta requests in the relative format,
/// naming the vocabulary's types and modifiers, and promises to refresh.
#[tokio::test]
async fn the_handshake_declares_semantic_tokens_and_their_refresh() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let declared = &params["capabilities"]["textDocument"]["semanticTokens"];
    assert_eq!(declared["requests"]["full"], json!({"delta": true}));
    assert_eq!(declared["formats"], json!(["relative"]));
    assert_eq!(declared["multilineTokenSupport"], json!(false));
    let types = declared["tokenTypes"].as_array().ok_or("no token types")?;
    assert!(types.contains(&json!("unresolvedReference")));
    assert_eq!(types.len(), T::ALL.len());
    let modifiers = declared["tokenModifiers"]
        .as_array()
        .ok_or("no modifiers")?;
    assert!(modifiers.contains(&json!("mutable")));
    assert_eq!(
        params["capabilities"]["workspace"]["semanticTokens"]["refreshSupport"],
        json!(true)
    );
    Ok(())
}

#[tokio::test]
async fn a_semantic_token_refresh_is_answered_and_surfaced() -> TestResult {
    let (client, mut server) = connect(provider(true)).await?;
    let mut refreshes = client.refreshes();
    server
        .send(&json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/semanticTokens/refresh"}))
        .await;
    let answer = server.recv().await;
    assert_eq!(answer["id"], json!(3));
    assert_eq!(answer["result"], json!(null));
    let surfaced = tokio::time::timeout(Duration::from_secs(5), refreshes.recv()).await??;
    assert_eq!(surfaced, ServerRefresh::SemanticTokens);
    Ok(())
}

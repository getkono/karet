//! The `window/*` traffic, server configuration, and watched files.
//!
//! Split from `lib_tests`, which owns the wire harness. Each test drives the
//! real connection against the scripted fake server, so what is pinned is the
//! wire: what the client sends, and what it answers.

use std::time::Duration;

use serde_json::Value;
use serde_json::json;

use super::FakeServer;
use super::TestResult;
use super::wire;
use crate::ConnectOptions;
use crate::FileChange;
use crate::FileChangeKind;
use crate::LspClient;
use crate::MessageLevel;
use crate::Path;
use crate::ProgressToken;
use crate::ServerMessageKind;
use crate::WorkProgressKind;
use crate::config;

/// How long a test waits for something the client should have sent.
const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn the_handshake_declares_the_window_and_workspace_promises() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move { server.handshake().await });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let params = server_task.await?;
    let caps = &params["capabilities"];
    assert_eq!(caps["window"]["workDoneProgress"], json!(true));
    assert_eq!(
        caps["window"]["showMessage"]["messageActionItem"]["additionalPropertiesSupport"],
        json!(true)
    );
    assert_eq!(caps["workspace"]["configuration"], json!(true));
    assert_eq!(
        caps["workspace"]["didChangeWatchedFiles"],
        json!({"dynamicRegistration": true, "relativePatternSupport": true})
    );
    Ok(())
}

#[tokio::test]
async fn work_done_progress_is_relayed_and_partial_results_are_not() -> TestResult {
    let ((read, write), mut server) = wire();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let _ = ready_rx.await;
        for value in [
            json!({"kind": "begin", "title": "Indexing", "percentage": 0, "cancellable": true}),
            // Partial-result progress: same method, no `kind`.
            json!([{"name": "partial"}]),
            json!({"kind": "report", "message": "3/10", "percentage": 130}),
            json!({"kind": "end", "message": "done"}),
        ] {
            server
                .send(&json!({"jsonrpc": "2.0", "method": "$/progress",
                              "params": {"token": "ra/index", "value": value}}))
                .await;
        }
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut progress = client.work_progress();
    let _ = ready_tx.send(());
    let begin = tokio::time::timeout(WAIT, progress.recv()).await??;
    assert_eq!(begin.token, ProgressToken::String("ra/index".into()));
    assert_eq!(begin.kind, WorkProgressKind::Begin);
    assert_eq!(begin.title.as_deref(), Some("Indexing"));
    assert_eq!(begin.cancellable, Some(true));
    let report = tokio::time::timeout(WAIT, progress.recv()).await??;
    assert_eq!(report.kind, WorkProgressKind::Report);
    assert_eq!(report.message.as_deref(), Some("3/10"));
    assert_eq!(report.percentage, Some(100), "clamped to 100");
    let end = tokio::time::timeout(WAIT, progress.recv()).await??;
    assert_eq!(end.kind, WorkProgressKind::End);

    client.cancel_work(&begin.token)?;
    let cancel = server_task.await?;
    assert_eq!(cancel["method"], "window/workDoneProgress/cancel");
    assert_eq!(cancel["params"]["token"], json!("ra/index"));
    Ok(())
}

#[tokio::test]
async fn numeric_progress_tokens_round_trip() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server.recv().await
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    client.cancel_work(&ProgressToken::Number(7))?;
    let cancel = server_task.await?;
    assert_eq!(cancel["params"]["token"], json!(7));
    assert_eq!(ProgressToken::Number(7).to_string(), "7");
    Ok(())
}

#[tokio::test]
async fn show_and_log_messages_are_relayed_with_their_level() -> TestResult {
    let ((read, write), mut server) = wire();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let _ = ready_rx.await;
        for (method, kind, text) in [
            ("window/showMessage", 1, "cargo metadata failed"),
            ("window/logMessage", 4, "loaded 3 crates"),
            ("window/logMessage", 9, "unknown level"),
        ] {
            server
                .send(&json!({"jsonrpc": "2.0", "method": method,
                              "params": {"type": kind, "message": text}}))
                .await;
        }
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut messages = client.messages();
    let _ = ready_tx.send(());
    let shown = tokio::time::timeout(WAIT, messages.recv()).await??;
    assert_eq!(shown.kind, ServerMessageKind::Show);
    assert_eq!(shown.level, MessageLevel::Error);
    assert_eq!(shown.text, "cargo metadata failed");
    let logged = tokio::time::timeout(WAIT, messages.recv()).await??;
    assert_eq!(logged.kind, ServerMessageKind::Log);
    assert_eq!(logged.level, MessageLevel::Log);
    let unknown = tokio::time::timeout(WAIT, messages.recv()).await??;
    assert_eq!(unknown.level, MessageLevel::Log, "an unknown level is kept");
    server_task.await?;
    Ok(())
}

/// Send one `window/showMessageRequest` with id `id` and return the answer.
async fn ask(server: &mut FakeServer, id: i64) -> Value {
    server
        .send(&json!({
            "jsonrpc": "2.0", "id": id, "method": "window/showMessageRequest",
            "params": {"type": 2, "message": "Reload workspace?",
                       "actions": [{"title": "Reload", "id": 41}, {"title": "Later"}]}
        }))
        .await;
    server.recv().await
}

#[tokio::test]
async fn an_untaken_message_request_is_answered_null() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        ask(&mut server, 1).await
    });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let answer = server_task.await?;
    assert_eq!(answer["id"], json!(1));
    assert_eq!(answer["result"], Value::Null);
    assert!(answer.get("error").is_none());
    Ok(())
}

#[tokio::test]
async fn a_taken_message_request_answers_the_chosen_action_verbatim() -> TestResult {
    let ((read, write), mut server) = wire();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        let _ = ready_rx.await;
        let chosen = ask(&mut server, 1).await;
        let dropped = ask(&mut server, 2).await;
        let out_of_range = ask(&mut server, 3).await;
        (chosen, dropped, out_of_range)
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    let mut requests = client.message_requests().ok_or("stream taken once")?;
    assert!(client.message_requests().is_none(), "only once");
    let _ = ready_tx.send(());

    let first = tokio::time::timeout(WAIT, requests.recv())
        .await?
        .ok_or("a request")?;
    assert_eq!(first.level, MessageLevel::Warning);
    assert_eq!(first.message, "Reload workspace?");
    assert_eq!(first.actions, vec!["Reload".to_owned(), "Later".to_owned()]);
    first.respond(Some(0));
    let second = tokio::time::timeout(WAIT, requests.recv())
        .await?
        .ok_or("a request")?;
    drop(second);
    let third = tokio::time::timeout(WAIT, requests.recv())
        .await?
        .ok_or("a request")?;
    third.respond(Some(9));

    let (chosen, dropped, out_of_range) = server_task.await?;
    assert_eq!(chosen["result"], json!({"title": "Reload", "id": 41}));
    assert_eq!(dropped["result"], Value::Null);
    assert_eq!(out_of_range["result"], Value::Null);
    Ok(())
}

#[test]
fn a_section_is_found_by_literal_key_then_by_dotted_path() {
    let settings = json!({
        "rust-analyzer": {"cargo": {"features": "all"}},
        "python.analysis": {"typeCheckingMode": "strict"},
    });
    assert_eq!(
        config::section(&settings, Some("rust-analyzer.cargo")),
        json!({"features": "all"})
    );
    assert_eq!(
        config::section(&settings, Some("python.analysis")),
        json!({"typeCheckingMode": "strict"})
    );
    assert_eq!(config::section(&settings, Some("gopls")), Value::Null);
    assert_eq!(
        config::section(&settings, Some("rust-analyzer.nope.deeper")),
        Value::Null
    );
    assert_eq!(config::section(&settings, None), settings);
    assert_eq!(config::section(&settings, Some("")), settings);
}

#[tokio::test]
async fn configuration_is_answered_from_settings_and_pushed_on_change() -> TestResult {
    let ((read, write), mut server) = wire();
    let (answered_tx, answered_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let init = server.recv().await;
        assert_eq!(
            init["params"]["initializationOptions"],
            json!({"lens": false})
        );
        let id = init["id"].clone();
        server.respond(&id, json!({"capabilities": {}})).await;
        assert_eq!(server.recv().await["method"], "initialized");
        let pushed = server.recv().await;
        assert_eq!(pushed["method"], "workspace/didChangeConfiguration");
        assert_eq!(
            pushed["params"]["settings"]["gopls"]["gofumpt"],
            json!(true)
        );

        server
            .send(
                &json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/configuration",
                          "params": {"items": [{"section": "gopls"}, {"section": "other"}]}}),
            )
            .await;
        let answer = server.recv().await;
        assert_eq!(answer["result"], json!([{"gofumpt": true}, null]));
        let _ = answered_tx.send(());

        let pushed = server.recv().await;
        assert_eq!(pushed["method"], "workspace/didChangeConfiguration");
        assert_eq!(
            pushed["params"]["settings"]["gopls"]["gofumpt"],
            json!(false)
        );
        server
            .send(
                &json!({"jsonrpc": "2.0", "id": 6, "method": "workspace/configuration",
                          "params": {"items": [{"section": "gopls.gofumpt"}]}}),
            )
            .await;
        server.recv().await
    });
    let options = ConnectOptions::default()
        .with_initialization_options(Some(json!({"lens": false})))
        .with_settings(Some(json!({"gopls": {"gofumpt": true}})));
    let client = LspClient::connect_configured(read, write, Path::new("/w"), options).await?;
    // The first exchange finishes before the settings change under it.
    answered_rx.await?;
    client.did_change_configuration(json!({"gopls": {"gofumpt": false}}))?;
    let answer = server_task.await?;
    assert_eq!(answer["result"], json!([false]));
    Ok(())
}

#[tokio::test]
async fn no_settings_means_no_push_and_null_answers() -> TestResult {
    let ((read, write), mut server) = wire();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        server
            .send(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "workspace/configuration",
                          "params": {"items": [{"section": "x"}, {}]}}),
            )
            .await;
        // The first thing back is the answer, not a configuration push.
        server.recv().await
    });
    let _client = LspClient::connect(read, write, Path::new("/w")).await?;
    let answer = server_task.await?;
    assert_eq!(answer["id"], json!(1));
    assert_eq!(answer["result"], json!([null, null]));
    Ok(())
}

/// Register `watchers` under `id`, then wait for the acknowledgement.
async fn register_watchers(server: &mut FakeServer, id: &str, watchers: Value) {
    server
        .send(&json!({
            "jsonrpc": "2.0", "id": format!("req-{id}"), "method": "client/registerCapability",
            "params": {"registrations": [{
                "id": id, "method": "workspace/didChangeWatchedFiles",
                "registerOptions": {"watchers": watchers}
            }]}
        }))
        .await;
    let ack = server.recv().await;
    assert_eq!(ack["result"], Value::Null);
}

#[tokio::test]
async fn watched_file_changes_are_filtered_through_the_registered_watchers() -> TestResult {
    let ((read, write), mut server) = wire();
    let (registered_tx, registered_rx) = tokio::sync::oneshot::channel::<()>();
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel::<()>();
    let (unregistered_tx, unregistered_rx) = tokio::sync::oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        server.handshake().await;
        register_watchers(
            &mut server,
            "toml",
            json!([
                {"globPattern": "**/Cargo.toml"},
                // Deletions only, under a base folder.
                {"globPattern": {"baseUri": "file:///w/src", "pattern": "**/*.rs"}, "kind": 4},
            ]),
        )
        .await;
        let _ = registered_tx.send(());
        let changes = server.recv().await;
        let _ = sent_rx.await;
        server
            .send(
                &json!({"jsonrpc": "2.0", "id": "un", "method": "client/unregisterCapability",
                          "params": {"unregisterations": [{"id": "toml",
                              "method": "workspace/didChangeWatchedFiles"}]}}),
            )
            .await;
        let _ = server.recv().await;
        let _ = unregistered_tx.send(());
        let after = server.recv().await;
        (changes, after)
    });
    let client = LspClient::connect(read, write, Path::new("/w")).await?;
    // Nothing registered yet: nothing goes out.
    assert_eq!(
        client.did_change_watched_files(&[FileChange::new(
            "/w/Cargo.toml",
            FileChangeKind::Changed
        )])?,
        0
    );
    let _ = registered_rx.await;
    let sent = client.did_change_watched_files(&[
        FileChange::new("/w/crates/a/Cargo.toml", FileChangeKind::Changed),
        FileChange::new("/w/src/lib.rs", FileChangeKind::Changed),
        FileChange::new("/w/src/gone.rs", FileChangeKind::Deleted),
        FileChange::new("/elsewhere/src/gone.rs", FileChangeKind::Deleted),
        FileChange::new("/w/README.md", FileChangeKind::Created),
    ])?;
    assert_eq!(sent, 2);
    let _ = sent_tx.send(());
    let _ = unregistered_rx.await;
    assert_eq!(
        client.did_change_watched_files(&[FileChange::new(
            "/w/Cargo.toml",
            FileChangeKind::Changed
        )])?,
        0,
        "unregistering drops the registration's watchers"
    );
    client.custom_notify("test/sentinel", json!({}))?;
    let (changes, after) = server_task.await?;
    assert_eq!(changes["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(
        changes["params"]["changes"],
        json!([
            {"uri": "file:///w/crates/a/Cargo.toml", "type": 2},
            {"uri": "file:///w/src/gone.rs", "type": 3},
        ])
    );
    assert_eq!(after["method"], "test/sentinel");
    Ok(())
}

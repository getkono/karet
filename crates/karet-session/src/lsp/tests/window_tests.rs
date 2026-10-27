//! What a server says to the user, and what the session tells it back:
//! progress, messages, questions, settings, watched files.
//!
//! Each test runs a server scripted inline over the real wire, because what is
//! under test is the trip -- a message relayed from a live connection to an
//! [`Event`], and a [`Command`] or reload turned back into a notification the
//! server actually receives.

use karet_watch::FsEvent;
use karet_watch::FsEventKind;
use serde_json::Value;

use super::*;
use crate::api::LanguageServerMessageId;
use crate::api::WorkPhase;
use crate::config::schema::LspServer;

/// A connector whose server answers the handshake, sends `after_open` once a
/// document is opened, and reports every message it receives -- the
/// `initialize` request included -- to `observed`.
fn scripted(after_open: Vec<Value>, observed: mpsc::UnboundedSender<Value>) -> Connector {
    Arc::new(move |spec: LspSpec, _root| {
        let after_open = after_open.clone();
        let observed = observed.clone();
        let options = karet_lsp::ConnectOptions::from(&spec);
        Box::pin(async move {
            let (client_end, server_end) = tokio::io::duplex(1 << 20);
            let (server_read, mut server_write) = tokio::io::split(server_end);
            tokio::spawn(async move {
                let mut reader = BufReader::new(server_read);
                let Some(init) = read_msg(&mut reader).await else {
                    return;
                };
                let id = init["id"].clone();
                let _ = observed.send(init);
                write_msg(
                    &mut server_write,
                    &json!({"jsonrpc": "2.0", "id": id,
                            "result": {"capabilities": {"textDocumentSync": 1}}}),
                )
                .await;
                while let Some(message) = read_msg(&mut reader).await {
                    let opened = message["method"] == "textDocument/didOpen";
                    let _ = observed.send(message);
                    if opened {
                        for push in &after_open {
                            write_msg(&mut server_write, push).await;
                        }
                    }
                }
            });
            let (read, write) = tokio::io::split(client_end);
            LspClient::connect_configured(read, write, Path::new("/tmp"), options).await
        })
    })
}

/// The first message the server received with `method`, within the deadline.
async fn received(observed: &mut mpsc::UnboundedReceiver<Value>, method: &str) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = observed.recv().await {
            if message["method"] == method {
                return Some(message);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// The first reply the server received to its request `id`.
async fn reply_to(observed: &mut mpsc::UnboundedReceiver<Value>, id: &str) -> Option<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = observed.recv().await {
            if message.get("method").is_none() && message["id"] == id {
                return Some(message);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

fn progress(kind: &str) -> Value {
    json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx",
        "value": {"kind": kind, "title": "Indexing\u{1b}[2J", "percentage": 40,
                  "cancellable": true}}})
}

#[tokio::test]
async fn what_a_server_says_reaches_the_client_and_the_client_answers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "lib.rs", "fn a() {}\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let pushes = vec![
        progress("begin"),
        json!({"jsonrpc": "2.0", "method": "window/showMessage",
               "params": {"type": 1, "message": "cargo metadata\nfailed"}}),
        json!({"jsonrpc": "2.0", "method": "window/logMessage",
               "params": {"type": 4, "message": "trace\u{1b}[31m line"}}),
        json!({"jsonrpc": "2.0", "id": "q1", "method": "window/showMessageRequest",
               "params": {"type": 2, "message": "Reload?",
                          "actions": [{"title": "Later"}, {"title": "Reload", "x": 1}]}}),
    ];
    let (session, mut events) = session_with_connector(scripted(pushes, observed_tx));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;

    let (mut work, mut shown, mut question) = (None, None, None);
    while work.is_none() || shown.is_none() || question.is_none() {
        let (_, event) = next_event(&mut events).await.ok_or("an event")?;
        match event {
            Event::LanguageServerWork { work: report } => work = Some(report),
            Event::Notification {
                kind: NotificationKind::Lsp,
                severity,
                message,
            } => shown = Some((severity, message)),
            Event::LanguageServerMessageRequest {
                request,
                server,
                message,
                actions,
                ..
            } => question = Some((request, server, message, actions)),
            _ => {},
        }
    }
    let work = work.ok_or("work")?;
    assert_eq!(work.server, LanguageServerId::RustAnalyzer);
    assert_eq!(work.phase, WorkPhase::Begin);
    assert_eq!(work.title.as_deref(), Some("Indexing [2J"), "sanitised");
    assert_eq!(work.percentage, Some(40));
    assert_eq!(work.cancellable, Some(true));
    let (severity, message) = shown.ok_or("shown")?;
    assert_eq!(severity, karet_core::Severity::Error);
    assert_eq!(message, "rust-analyzer: cargo metadata failed");
    let (request, server, message, actions) = question.ok_or("question")?;
    assert_eq!(server, LanguageServerId::RustAnalyzer);
    assert_eq!(message, "Reload?");
    assert_eq!(actions, vec!["Later".to_owned(), "Reload".to_owned()]);

    backend.send(
        backend.next_id(),
        LanguageServerCommand::AnswerMessage {
            request,
            action: Some(1),
        }
        .into(),
    )?;
    let answer = reply_to(&mut observed, "q1").await.ok_or("no answer")?;
    assert_eq!(answer["result"], json!({"title": "Reload", "x": 1}));
    // A second answer to the same question is a no-op, not a second reply.
    backend.send(
        backend.next_id(),
        LanguageServerCommand::AnswerMessage {
            request,
            action: Some(0),
        }
        .into(),
    )?;
    backend.send(
        backend.next_id(),
        LanguageServerCommand::AnswerMessage {
            request: LanguageServerMessageId(999),
            action: None,
        }
        .into(),
    )?;

    backend.send(
        backend.next_id(),
        LanguageServerCommand::CancelWork {
            server: work.server.clone(),
            root: work.root.clone(),
            token: work.token.clone(),
        }
        .into(),
    )?;
    // Everything the server receives before the cancel was sent before it, so
    // a second reply to `q1` would be among it.
    let cancel = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = observed.recv().await {
            assert!(
                message.get("method").is_some() || message["id"] != "q1",
                "a question was answered twice"
            );
            if message["method"] == "window/workDoneProgress/cancel" {
                return Some(message);
            }
        }
        None
    })
    .await?
    .ok_or("no cancel")?;
    assert_eq!(cancel["params"]["token"], json!("idx"));

    let log_request = backend.next_id();
    backend.send(
        log_request,
        LanguageServerCommand::Log {
            server: LanguageServerId::RustAnalyzer,
        }
        .into(),
    )?;
    let entries = loop {
        let (id, event) = next_event(&mut events).await.ok_or("a log")?;
        if let Event::LanguageServerLog { server, entries } = event {
            assert_eq!(id, Some(log_request));
            assert_eq!(server, LanguageServerId::RustAnalyzer);
            break entries;
        }
    };
    let texts: Vec<&str> = entries.iter().map(|entry| entry.text.as_str()).collect();
    assert_eq!(texts, vec!["cargo metadata\nfailed", "trace [31m line"]);
    assert_eq!(entries[1].severity, karet_core::Severity::Hint);
    Ok(())
}

/// A manager whose servers are scripted, with `settings` for rust-analyzer
/// given through a commandless entry.
fn configured_manager(
    settings: Option<Value>,
    initialization_options: Option<Value>,
    observed: mpsc::UnboundedSender<Value>,
    pushes: Vec<Value>,
) -> (LspManager, mpsc::UnboundedReceiver<LspUpdate>) {
    let mut lsp = LspSettings::default();
    lsp.servers.insert(
        "rust-analyzer".to_owned(),
        LspServer {
            settings,
            initialization_options,
            ..LspServer::default()
        },
    );
    let (mut manager, updates) = LspManager::new(lsp, None, None, None);
    manager.set_connector(scripted(pushes, observed));
    (manager, updates)
}

#[tokio::test]
async fn settings_reach_the_server_and_a_settings_reload_pushes_without_a_restart() -> TestResult {
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, _updates) = configured_manager(
        Some(json!({"rust-analyzer": {"cargo": {"features": "all"}}})),
        Some(json!({"lens": false})),
        observed_tx,
        Vec::new(),
    );
    let path = PathBuf::from("/tmp/settings.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, String::new);
    let init = received(&mut observed, "initialize")
        .await
        .ok_or("no initialize")?;
    assert_eq!(
        init["params"]["initializationOptions"],
        json!({"lens": false})
    );
    let pushed = received(&mut observed, "workspace/didChangeConfiguration")
        .await
        .ok_or("no startup push")?;
    assert_eq!(
        pushed["params"]["settings"]["rust-analyzer"]["cargo"]["features"],
        json!("all")
    );
    let _ = received(&mut observed, "textDocument/didOpen").await;
    let key = SlotKey::new(LanguageServerId::RustAnalyzer, Path::new("/tmp"));
    let token = manager.servers.get(&key).map(|slot| slot.token);

    let mut changed = manager.settings.clone();
    if let Some(entry) = changed.servers.get_mut("rust-analyzer") {
        entry.settings = Some(json!({"rust-analyzer": {"cargo": {"features": []}}}));
    }
    assert!(
        manager.reconfigure(changed.clone()).is_none(),
        "a settings-only reload retires nothing"
    );
    assert_eq!(
        manager.servers.get(&key).map(|slot| slot.token),
        token,
        "the same task serves on"
    );
    let pushed = received(&mut observed, "workspace/didChangeConfiguration")
        .await
        .ok_or("no reload push")?;
    assert_eq!(
        pushed["params"]["settings"]["rust-analyzer"]["cargo"]["features"],
        json!([])
    );

    // Initialization options are read only at the handshake: changing them
    // restarts the server.
    if let Some(entry) = changed.servers.get_mut("rust-analyzer") {
        entry.initialization_options = Some(json!({"lens": true}));
    }
    assert!(manager.reconfigure(changed).is_some());
    Ok(())
}

/// A manager whose only `lsp.servers` entry is `entry`, keyed by the language.
fn language_keyed(entry: LspServer, observed: mpsc::UnboundedSender<Value>) -> LspManager {
    let mut lsp = LspSettings::default();
    lsp.servers.insert("rust".to_owned(), entry);
    let (mut manager, _updates) = LspManager::new(lsp, None, None, None);
    manager.set_connector(scripted(Vec::new(), observed));
    manager
}

#[tokio::test]
async fn a_commandless_language_entry_configures_the_builtin_and_disabled_turns_it_off()
-> TestResult {
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let path = PathBuf::from("/tmp/commandless.rs");
    let mut manager = language_keyed(
        LspServer {
            settings: Some(json!({"rust-analyzer": {"checkOnSave": false}})),
            ..LspServer::default()
        },
        observed_tx.clone(),
    );
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, String::new);
    assert!(
        manager.is_running(&LanguageServerId::RustAnalyzer),
        "no command keeps karet's launch"
    );
    let pushed = received(&mut observed, "workspace/didChangeConfiguration")
        .await
        .ok_or("the language entry's settings never arrived")?;
    assert_eq!(
        pushed["params"]["settings"]["rust-analyzer"]["checkOnSave"],
        json!(false)
    );

    let mut manager = language_keyed(
        LspServer {
            enabled: false,
            ..LspServer::default()
        },
        observed_tx,
    );
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, String::new);
    assert!(!manager.is_running(&LanguageServerId::RustAnalyzer));
    Ok(())
}

#[tokio::test]
async fn watched_file_changes_reach_only_a_server_that_asked() -> TestResult {
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let register = json!({"jsonrpc": "2.0", "id": "w1", "method": "client/registerCapability",
        "params": {"registrations": [{"id": "watch", "method": "workspace/didChangeWatchedFiles",
            "registerOptions": {"watchers": [{"globPattern": "**/Cargo.toml"}]}}]}});
    let (mut manager, _updates) = configured_manager(None, None, observed_tx, vec![register]);
    let path = PathBuf::from("/tmp/watched.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, String::new);
    let _ = reply_to(&mut observed, "w1")
        .await
        .ok_or("registration unanswered")?;

    manager.files_changed(&FsEvent {
        kind: FsEventKind::Modified,
        paths: vec![
            PathBuf::from("/tmp/notes.md"),
            PathBuf::from("/tmp/crate/Cargo.toml"),
        ],
    });
    let changes = received(&mut observed, "workspace/didChangeWatchedFiles")
        .await
        .ok_or("no watched-file notification")?;
    assert_eq!(
        changes["params"]["changes"],
        json!([{"uri": "file:///tmp/crate/Cargo.toml", "type": 2}])
    );
    Ok(())
}

/// A connector whose server asks `count` questions (`q0`, `q1`, ...) in
/// batches of `batch`: the first once a document is opened, each next one when
/// it receives a `window/workDoneProgress/cancel`, so no burst outruns the
/// relay. Every message the server receives goes to `observed`.
fn asking(count: usize, batch: usize, observed: mpsc::UnboundedSender<Value>) -> Connector {
    Arc::new(move |spec: LspSpec, _root| {
        let observed = observed.clone();
        let options = karet_lsp::ConnectOptions::from(&spec);
        Box::pin(async move {
            let (client_end, server_end) = tokio::io::duplex(1 << 20);
            let (server_read, mut server_write) = tokio::io::split(server_end);
            tokio::spawn(async move {
                let mut reader = BufReader::new(server_read);
                let Some(init) = read_msg(&mut reader).await else {
                    return;
                };
                let id = init["id"].clone();
                write_msg(
                    &mut server_write,
                    &json!({"jsonrpc": "2.0", "id": id,
                            "result": {"capabilities": {"textDocumentSync": 1}}}),
                )
                .await;
                let mut asked = 0;
                while let Some(message) = read_msg(&mut reader).await {
                    let next = message["method"] == "textDocument/didOpen"
                        || message["method"] == "window/workDoneProgress/cancel";
                    let _ = observed.send(message);
                    if next {
                        let until = count.min(asked + batch);
                        for index in asked..until {
                            let question = json!({"jsonrpc": "2.0", "id": format!("q{index}"),
                                "method": "window/showMessageRequest",
                                "params": {"type": 3, "message": "Go?",
                                           "actions": [{"title": "Yes"}]}});
                            write_msg(&mut server_write, &question).await;
                        }
                        asked = until;
                    }
                }
            });
            let (read, write) = tokio::io::split(client_end);
            LspClient::connect_configured(read, write, Path::new("/tmp"), options).await
        })
    })
}

#[tokio::test]
async fn a_full_question_queue_answers_its_oldest_none_to_make_room() -> TestResult {
    use super::window::QUESTION_CAPACITY;
    const BATCH: usize = 11;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(asking(QUESTION_CAPACITY + 1, BATCH, observed_tx));
    let path = PathBuf::from("/tmp/questions.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, String::new);

    let mut state = WindowState::default();
    let mut ids = Vec::new();
    while ids.len() <= QUESTION_CAPACITY {
        let update = tokio::time::timeout(Duration::from_secs(10), updates.recv())
            .await?
            .ok_or("updates closed")?;
        let LspUpdate::Window {
            key,
            update: WindowUpdate::Request(question),
            ..
        } = update
        else {
            continue;
        };
        ids.push(state.ask(question));
        if ids.len() % BATCH == 0 {
            // Any notice the server receives releases its next batch.
            manager.cancel_work(&key.provider, &key.root, &crate::api::WorkToken::Number(0));
        }
    }

    let evicted = reply_to(&mut observed, "q0")
        .await
        .ok_or("the oldest question was not answered")?;
    assert_eq!(evicted["result"], Value::Null, "answered no action");
    assert!(
        !state.answer(ids[0], Some(0)),
        "the evicted question is gone"
    );
    assert!(
        state.answer(ids[1], Some(0)),
        "the next oldest is still held"
    );
    let kept = reply_to(&mut observed, "q1")
        .await
        .ok_or("the held question was not answered")?;
    assert_eq!(kept["result"], json!({"title": "Yes"}));
    Ok(())
}

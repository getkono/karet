use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use karet_core::Change;
use karet_core::CompletionItem;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_text::EditCause;
use serde_json::json;
use server_double::Behavior;
use server_double::failing_connector;
// The framing helpers live with the double but belong to `lsp::tests`: a test
// that scripts its own server inline speaks the same wire this one does.
use server_double::read_msg;
use server_double::test_connector;
use server_double::write_msg;
use tokio::io::BufReader;

use super::*;
use crate::api::Command;
use crate::api::Event;
use crate::api::LanguageServerInstanceStatus;
use crate::api::LanguageServerStatus;
use crate::backend::Backend;
use crate::backend::local_session;
use crate::session::EventRx;
use crate::session::Session;
use crate::session::SessionConfig;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn reconfigure_retires_updates_from_old_server_tasks() {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    let old = LspUpdate::Completions {
        generation: 0,
        request: RequestId(1),
        doc: DocumentId(1),
        version: 1,
        items: Vec::new(),
    };
    assert!(manager.accepts(&old));

    let settings = LspSettings {
        enabled: false,
        ..LspSettings::default()
    };
    assert!(manager.reconfigure(settings.clone()).is_some());
    assert!(!manager.accepts(&old));
    assert!(
        manager.reconfigure(settings).is_none(),
        "an identical snapshot is a no-op"
    );
}

#[tokio::test]
async fn last_document_close_retires_the_server_slot() {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/owned.rs");
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    assert!(manager.is_running(&LanguageServerId::RustAnalyzer));
    let _ = manager.document_closed(Some("rust"), &path);
    assert!(!manager.is_running(&LanguageServerId::RustAnalyzer));
}

#[tokio::test]
async fn javascript_and_typescript_share_one_builtin_process() {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(
        Some("javascript"),
        Some("javascript"),
        Path::new("/tmp/a.js"),
        1,
        String::new,
    );
    let _ = manager.document_opened(
        Some("typescript"),
        Some("typescript"),
        Path::new("/tmp/b.ts"),
        1,
        String::new,
    );
    assert_eq!(manager.servers.len(), 1);
    let _ = manager.document_closed(Some("javascript"), Path::new("/tmp/a.js"));
    assert!(manager.is_running(&LanguageServerId::TypeScript));
    let _ = manager.document_closed(Some("typescript"), Path::new("/tmp/b.ts"));
    assert!(!manager.is_running(&LanguageServerId::TypeScript));
}

#[tokio::test]
async fn tsx_routes_to_typescript_with_protocol_specific_language_id() -> TestResult {
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let _ = manager.document_opened(
        Some("tsx"),
        Some("typescriptreact"),
        Path::new("/tmp/component.tsx"),
        1,
        || "export const Component = () => <main />;".into(),
    );

    assert!(manager.is_running(&LanguageServerId::TypeScript));
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), observed_rx.recv())
            .await?
            .ok_or("language server did not receive TSX didOpen")?;
        if message["method"] == "textDocument/didOpen" {
            assert_eq!(
                message["params"]["textDocument"]["languageId"],
                json!("typescriptreact")
            );
            break;
        }
    }
    Ok(())
}

#[tokio::test]
async fn relative_root_and_document_paths_reach_lsp_as_absolute_uris() -> TestResult {
    let root = PathBuf::from("relative-workspace");
    let path = root.join("main.rs");
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), Some(root), None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));

    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let _ = manager.document_changed(Some("rust"), &path, 2, || "fn changed() {}".into());
    manager.document_saved(Some("rust"), &path, || "fn changed() {}".into());
    let _ = manager.document_closed(Some("rust"), &path);

    let mut methods = Vec::new();
    while methods
        .last()
        .is_none_or(|method| method != "textDocument/didClose")
    {
        let message = tokio::time::timeout(Duration::from_secs(2), observed_rx.recv())
            .await?
            .ok_or("language server did not receive document sync")?;
        let uri = message["params"]["textDocument"]["uri"]
            .as_str()
            .ok_or("document URI was not a string")?;
        assert!(
            uri.starts_with("file:///") && uri.ends_with("/relative-workspace/main.rs"),
            "expected an absolute file URI, got {uri}"
        );
        methods.push(
            message["method"]
                .as_str()
                .ok_or("document sync method was not a string")?
                .to_owned(),
        );
    }
    assert_eq!(
        methods,
        [
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/didClose"
        ]
    );
    Ok(())
}

// --- session-level helpers ---------------------------------------------

fn rust_file(dir: &tempfile::TempDir, name: &str, text: &str) -> Option<PathBuf> {
    let path = dir.path().join(name);
    std::fs::write(&path, text).ok()?;
    Some(path)
}

async fn next_event(events: &mut EventRx) -> Option<(Option<RequestId>, Event)> {
    tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .ok()
        .flatten()
}

async fn await_opened(events: &mut EventRx) -> Option<(DocumentId, u64)> {
    while let Some((_, event)) = next_event(events).await {
        if let Event::Opened { doc, version } = event {
            return Some((doc, version));
        }
    }
    None
}

async fn await_completions(
    events: &mut EventRx,
) -> Option<(Option<RequestId>, DocumentId, u64, Vec<CompletionItem>)> {
    while let Some((rid, event)) = next_event(events).await {
        if let Event::Completions {
            doc,
            version,
            items,
        } = event
        {
            return Some((rid, doc, version, items));
        }
    }
    None
}

async fn await_inlay_hints(
    events: &mut EventRx,
) -> Option<(
    Option<RequestId>,
    DocumentId,
    u64,
    Vec<karet_core::InlayHint>,
)> {
    loop {
        let (id, event) = next_event(events).await?;
        if let Event::InlayHints {
            doc,
            version,
            hints,
        } = event
        {
            return Some((id, doc, version, hints));
        }
    }
}

async fn await_symbols(
    events: &mut EventRx,
) -> Option<(Option<RequestId>, DocumentId, Vec<Symbol>)> {
    while let Some((request, event)) = next_event(events).await {
        if let Event::Symbols { doc, symbols } = event {
            return Some((request, doc, symbols));
        }
    }
    None
}
fn session_with_connector(connector: Connector) -> (Session, EventRx) {
    let (mut session, events, _snaps) = Session::new(SessionConfig::default());
    session.set_lsp_connector(connector);
    (session, events)
}

// --- the tests -----------------------------------------------------------

#[tokio::test]
async fn unsupported_language_answers_empty_immediately() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", "plain text\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;
    let (rid, cdoc, cversion, items) = await_completions(&mut events).await.ok_or("no answer")?;
    assert_eq!(rid, Some(request));
    assert_eq!((cdoc, cversion), (doc, version));
    assert!(items.is_empty());
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "no server for .txt");
    Ok(())
}

#[tokio::test]
async fn disabled_setting_spawns_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let mut config = SessionConfig::default();
    config.settings.lsp.enabled = false;
    let (mut session, mut events, _snaps) = Session::new(config);
    session.set_lsp_connector(test_connector(Behavior::Normal, None, Arc::clone(&spawns)));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;
    let (rid, _, _, items) = await_completions(&mut events).await.ok_or("no answer")?;
    assert_eq!(rid, Some(request));
    assert!(items.is_empty());
    assert_eq!(spawns.load(Ordering::SeqCst), 0, "disabled means no spawns");
    Ok(())
}

#[tokio::test]
async fn missing_binary_warns_once_and_answers_empty() -> TestResult {
    let dir = tempfile::tempdir()?;
    let first = rust_file(&dir, "a.rs", "fn a() {}\n").ok_or("write failed")?;
    let second = rust_file(&dir, "b.rs", "fn b() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(failing_connector(Arc::clone(&spawns)));
    let backend = local_session(session, None);

    // Two documents of the same language: one spawn attempt, one warning.
    for path in [first, second] {
        backend.send(
            backend.next_id(),
            Command::OpenDocument {
                path,
                language: None,
            },
        )?;
    }
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;

    // Drain until the completion answer; count LSP warnings seen on the way.
    let mut lsp_warnings = 0;
    let mut answered = false;
    while let Some((rid, event)) = next_event(&mut events).await {
        match event {
            Event::Notification {
                kind: NotificationKind::Lsp,
                ..
            } => lsp_warnings += 1,
            Event::Completions { items, .. } => {
                assert_eq!(rid, Some(request));
                assert!(items.is_empty());
                answered = true;
                break;
            },
            _ => {},
        }
    }
    assert!(answered, "a dead server must still answer completions");
    assert_eq!(lsp_warnings, 1, "exactly one missing-binary warning");
    assert_eq!(spawns.load(Ordering::SeqCst), 1, "one attempt, remembered");
    Ok(())
}

#[tokio::test]
async fn server_death_is_reported_and_completions_stay_answered() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DieAfterHandshake,
        None,
        Arc::clone(&spawns),
    ));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let request = backend.next_id();
    backend.send(
        request,
        Command::Completion {
            doc,
            position: LineCol::new(0, 0),
        },
    )?;

    let mut died_notice = false;
    let mut answered = false;
    while let Some((rid, event)) = next_event(&mut events).await {
        match event {
            Event::Notification {
                kind: NotificationKind::Lsp,
                message,
                ..
            } => {
                assert!(message.contains("stopped"), "unexpected: {message}");
                died_notice = true;
                if answered {
                    break;
                }
            },
            Event::Completions { items, .. } => {
                assert_eq!(rid, Some(request));
                assert!(items.is_empty());
                answered = true;
                if died_notice {
                    break;
                }
            },
            _ => {},
        }
    }
    assert!(answered && died_notice);
    Ok(())
}

#[tokio::test]
async fn crashed_server_restarts_and_replays_open_documents() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn recovered() {}\n").ok_or("write failed")?;
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DieOnce,
        Some(observed_tx),
        Arc::clone(&spawns),
    ));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    backend.send(
        backend.next_id(),
        Command::Completion {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;
    let _ = await_completions(&mut events)
        .await
        .ok_or("no outage answer")?;

    tokio::time::sleep(Duration::from_millis(500)).await;
    backend.send(
        backend.next_id(),
        Command::Completion {
            doc,
            position: LineCol::new(0, 3),
        },
    )?;
    let (_, _, _, items) = await_completions(&mut events)
        .await
        .ok_or("no recovered answer")?;
    assert_eq!(items.len(), 1);
    assert!(spawns.load(Ordering::SeqCst) >= 2);
    let mut replayed = false;
    while let Ok(Some(message)) =
        tokio::time::timeout(Duration::from_millis(100), observed_rx.recv()).await
    {
        if message["method"] == "textDocument/didOpen"
            && message["params"]["textDocument"]["text"] == "fn recovered() {}\n"
        {
            replayed = true;
            break;
        }
    }
    assert!(
        replayed,
        "reconnected server must receive the latest full text"
    );
    Ok(())
}

mod disabled_tests;
mod format_tests;
mod intelligence_tests;
mod inventory_tests;
mod jdtls_tests;
mod launch_tests;
mod liveness_tests;
mod manual_provider_tests;
mod reopen_tests;
mod restart_tests;
mod retirement_tests;
mod roundtrip_tests;
mod routing_tests;
mod server_double;

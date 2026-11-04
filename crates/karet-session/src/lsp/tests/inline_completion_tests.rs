//! The inline-completion contract at the `Command`/`Event` seam: an ask goes
//! out in the server's UTF-16 columns, the items come back in buffer columns
//! naming the server that proposed them, and an accept reaches that server
//! as the command its item carried.

use karet_core::InlineCompletionTrigger;

use super::*;

/// Wait until the server has received a message with `method`, returning it.
async fn await_method(
    observed: &mut mpsc::UnboundedReceiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let found = async {
        loop {
            let message = observed.recv().await?;
            if message["method"] == method {
                return Some(message);
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), found)
        .await?
        .ok_or_else(|| format!("the server never saw {method}").into())
}

/// The answer to `request`: its document, version, server and items.
async fn await_inline(
    events: &mut EventRx,
    request: RequestId,
) -> Option<(
    DocumentId,
    u64,
    LanguageServerId,
    Vec<karet_core::InlineCompletionItem>,
)> {
    let answer = async {
        loop {
            let (id, event) = next_event(events).await?;
            if let (
                true,
                Event::InlineCompletions {
                    doc,
                    version,
                    server,
                    items,
                },
            ) = (id == Some(request), event)
            {
                return Some((doc, version, server, items));
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .ok()
        .flatten()
}

/// '😀' is one buffer column but two UTF-16 units: the caret at buffer column
/// 3 goes out as character 4, and the item's range 0..4 comes back as 0..3.
/// Getting either wrong puts the ghost text one cell off on any line with a
/// non-ASCII character, and replaces the wrong text on accept.
#[tokio::test]
async fn an_ask_round_trips_with_utf16_conversion_and_an_accept_runs_its_command() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "😀ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    let (doc, version) = await_opened(&mut events).await.ok_or("no Opened")?;

    let request = backend.next_id();
    backend.send(
        request,
        LspCommand::InlineCompletion {
            doc,
            position: LineCol::new(0, 3),
            trigger: InlineCompletionTrigger::Automatic,
        }
        .into(),
    )?;
    let asked = await_method(&mut observed, "textDocument/inlineCompletion").await?;
    assert_eq!(
        asked["params"]["position"],
        json!({"line": 0, "character": 4})
    );
    assert_eq!(asked["params"]["context"]["triggerKind"], json!(2));

    let (adoc, aversion, server, items) = await_inline(&mut events, request)
        .await
        .ok_or("no InlineCompletions event")?;
    assert_eq!((adoc, aversion), (doc, version));
    assert_eq!(server, LanguageServerId::RustAnalyzer);
    let item = items.first().cloned().ok_or("no item")?;
    assert_eq!(item.insert_text, "😀ab = 1;");
    assert_eq!(
        item.range,
        Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 3),
        }
    );

    backend.send(
        backend.next_id(),
        LspCommand::AcceptInlineCompletion { doc, server, item }.into(),
    )?;
    let ran = await_method(&mut observed, "workspace/executeCommand").await?;
    assert_eq!(ran["params"]["command"], json!("double.accept"));
    assert_eq!(ran["params"]["arguments"], json!([1]));
    Ok(())
}

/// An accept names the server that proposed the item; one naming a server
/// that is not serving the document reaches nobody rather than whichever
/// server happens to be attached.
#[tokio::test]
async fn an_accept_for_another_server_reaches_nobody() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;
    let item = karet_core::InlineCompletionItem {
        payload: Some(
            json!({"insertText": "x", "command": {"title": "", "command": "double.accept"}})
                .to_string(),
        ),
        ..karet_core::InlineCompletionItem::new("x", LineCol::new(0, 2))
    };
    backend.send(
        backend.next_id(),
        LspCommand::AcceptInlineCompletion {
            doc,
            server: LanguageServerId::new("copilot-language-server"),
            item,
        }
        .into(),
    )?;
    // A hover goes out after it on the same connection, so once the server
    // has seen the hover it would already have seen any accept.
    backend.send(
        backend.next_id(),
        LspCommand::Hover {
            doc,
            position: LineCol::new(0, 0),
        }
        .into(),
    )?;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), observed.recv())
            .await?
            .ok_or("server stream ended")?;
        assert_ne!(
            message["method"], "workspace/executeCommand",
            "an accept reached a server that never proposed the item"
        );
        if message["method"] == "textDocument/hover" {
            return Ok(());
        }
    }
}

/// The answer to `request`, if it is an account outcome.
async fn await_account(
    events: &mut EventRx,
    request: RequestId,
) -> Option<(LanguageServerId, crate::api::AccountOutcome)> {
    let answer = async {
        loop {
            let (id, event) = next_event(events).await?;
            if let (true, Event::LanguageServerAccount { server, outcome }) =
                (id == Some(request), event)
            {
                return Some((server, outcome));
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .ok()
        .flatten()
}

/// Signing in hands the user the device code at once, runs the command the
/// server returned to complete it, and reports what that command answered as
/// a second outcome of the same request.
#[tokio::test]
async fn sign_in_answers_the_device_code_and_runs_the_completing_command() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "ab\n").ok_or("write failed")?;
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, Some(observed_tx), spawns));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path,
            language: None,
        }
        .into(),
    )?;
    await_opened(&mut events).await.ok_or("no Opened")?;
    await_method(&mut observed, "textDocument/didOpen").await?;

    let request = backend.next_id();
    backend.send(
        request,
        LanguageServerCommand::SignIn {
            server: LanguageServerId::RustAnalyzer,
        }
        .into(),
    )?;
    let (server, outcome) = await_account(&mut events, request)
        .await
        .ok_or("the sign-in went unanswered")?;
    assert_eq!(server, LanguageServerId::RustAnalyzer);
    assert_eq!(
        outcome,
        crate::api::AccountOutcome::DeviceCode {
            user_code: "ABCD-EFGH".to_owned(),
            verification_uri: None,
        }
    );
    let ran = await_method(&mut observed, "workspace/executeCommand").await?;
    assert_eq!(ran["params"]["command"], json!("double.run"));
    assert_eq!(ran["params"]["arguments"], json!([2]));
    let (_, landed) = await_account(&mut events, request)
        .await
        .ok_or("the completed sign-in was never reported")?;
    assert_eq!(
        landed,
        crate::api::AccountOutcome::SignedIn { user: None },
        "the completing command answered, so the sign-in landed"
    );
    Ok(())
}

/// With the server not running there is nobody to sign in through: the
/// answer says so at once rather than leaving the user waiting.
#[tokio::test]
async fn sign_in_with_no_server_running_fails_at_once() -> TestResult {
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) =
        session_with_connector(test_connector(Behavior::Normal, None, spawns));
    let backend = local_session(session, None);
    let request = backend.next_id();
    backend.send(
        request,
        LanguageServerCommand::SignIn {
            server: LanguageServerId::new("copilot-language-server"),
        }
        .into(),
    )?;
    let (_, outcome) = await_account(&mut events, request)
        .await
        .ok_or("the sign-in went unanswered")?;
    assert!(matches!(
        outcome,
        crate::api::AccountOutcome::Failed { message } if message.contains("not running")
    ));
    Ok(())
}

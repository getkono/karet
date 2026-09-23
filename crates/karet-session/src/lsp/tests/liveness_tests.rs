//! Noticing that a connected server is gone, and cleaning up after it.
//!
//! Separate from `restart_tests`, which covers *whether* a provider is worth
//! retrying. These cover how the loss is detected in the first place -- the half
//! that used to depend on the user happening to ask the server something.

use super::*;

/// A server that exits while nobody is typing must be noticed anyway.
///
/// The defect: detection was demand-driven. The task learned a connection had
/// died only when a request it made came back `Closed`, so with nothing
/// outstanding the state stayed `Running`, the editor badge stayed green, and the
/// retry clock did not start until the user's next keystroke. Nothing here sends
/// a command after the open, and `didOpen` is a notification that cannot come
/// back failed, so only a liveness arm can pass this.
#[tokio::test]
async fn a_server_that_dies_while_idle_is_noticed_without_being_asked() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DieWhenIdle,
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

    let mut retrying = false;
    let mut reported: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !(retrying && reported.is_some()) {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        match event {
            Event::LanguageServerRuntimeChanged { state, .. } => {
                retrying = retrying || state == LanguageServerRuntimeState::Retrying;
            },
            // Catching the exit sooner must not make it quieter: the death is
            // reported the same way whether a failing call or the connection
            // itself revealed it.
            Event::Notification {
                kind: NotificationKind::Lsp,
                message,
                ..
            } if message.contains("stopped") => reported = Some(message),
            _ => {},
        }
    }
    assert!(
        retrying,
        "an idle death left the provider reading as healthy"
    );
    let reported = reported.ok_or("an idle death was never reported to the user")?;
    // The death is attributed to the *provider*, never to its slot key. The key
    // is `provider@/absolute/repository/root`, and rendering it whole puts the
    // user's filesystem layout in a toast. Asserted on the message the session
    // really emitted, because a test that rebuilds the sentence itself stops
    // constraining the code that writes it.
    assert!(
        reported.contains("rust-analyzer"),
        "the death did not name its provider: {reported}"
    );
    assert!(
        !reported.contains('/'),
        "the death notification carried a filesystem path: {reported}"
    );
    Ok(())
}

/// A dead server's diagnostics must not outlive it.
///
/// They were only ever inserted, never removed -- not on death, not on
/// reconfigure, not on a generation bump -- so a crashed server's markers stayed
/// on screen, pointing at lines the user went on to edit away.
#[tokio::test]
async fn a_dead_servers_diagnostics_are_cleared() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(test_connector(
        Behavior::DieAfterDiagnostics,
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

    let mut published = false;
    let mut cleared = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && !cleared {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::DiagnosticsPublished { diagnostics, .. } = event {
            if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("must not outlive"))
            {
                published = true;
            } else if published {
                cleared = true;
            }
        }
    }
    assert!(published, "the server's diagnostic never arrived");
    assert!(
        cleared,
        "the diagnostic outlived the server that published it"
    );
    Ok(())
}

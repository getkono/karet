//! When karet gives up on a language server, and when it must not.

use super::*;

/// A binary that is not there will not be there on the next attempt either.
/// Before this, the task retried five times, opened a five-minute circuit, then
/// retried five more times, forever.
#[tokio::test]
async fn a_server_that_can_never_start_stops_being_retried() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = session_with_connector(failing_connector(Arc::clone(&spawns)));
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;

    let mut unavailable = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !unavailable {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            assert_ne!(
                state,
                LanguageServerRuntimeState::CircuitOpen,
                "a launch that can never succeed should not spend the circuit"
            );
            unavailable = state == LanguageServerRuntimeState::Unavailable;
        }
    }
    assert!(unavailable, "expected the provider to become unavailable");

    // Terminal means terminal: no further attempts.
    let attempts = spawns.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(spawns.load(Ordering::SeqCst), attempts);
    Ok(())
}

/// The counterpart, and the real risk in giving up at all: a failure that a
/// retry could fix must still be retried. A broker that was briefly unreachable
/// says nothing about whether the server can run.
///
/// (A server that connects and *then* dies is covered by
/// `crashed_server_restarts_and_replays_open_documents`.)
#[tokio::test]
async fn a_transient_failure_is_retried_rather_than_giving_up() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&spawns);
    let connector: Connector = Arc::new(move |spec, _root| {
        counter.fetch_add(1, Ordering::SeqCst);
        let failure = karet_lsp::LaunchFailure::host(
            spec.command.clone(),
            spec.args.clone(),
            "shared broker unreachable",
        );
        Box::pin(async move { Err(LspError::Launch(Box::new(failure))) })
    });
    let (session, mut events) = session_with_connector(connector);
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && spawns.load(Ordering::SeqCst) < 2 {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            assert_ne!(
                state,
                LanguageServerRuntimeState::Unavailable,
                "a host-side failure must not be treated as permanent"
            );
        }
    }
    assert!(
        spawns.load(Ordering::SeqCst) >= 2,
        "a transient failure must be retried"
    );
    Ok(())
}

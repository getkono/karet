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

/// A provider karet gave up on is re-resolved only once its executable exists.
///
/// Two failure modes bracket this. Keeping the slot made the verdict permanent:
/// resolution short-circuits on a live slot, so installing the binary changed
/// nothing until the user found the manager and pressed Restart. Retiring it
/// unconditionally was worse: every later document open re-execs the same missing
/// binary and reports the same failure again -- an unbounded stream of identical
/// notifications for a command that never existed.
///
/// Observed through `runtime_states` rather than a spawn count, because lifting
/// the verdict is exactly what clears that entry, and it is synchronous.
#[tokio::test]
async fn a_provider_given_up_on_is_retried_only_once_its_binary_appears() -> TestResult {
    let dir = tempfile::tempdir()?;
    let binary = dir.path().join("pretend-analyzer");
    // Declared through `lsp.languages`, which is how a configured primary is
    // actually selected: `lsp.servers` is looked up by the language's named
    // provider or by the language itself, never by a built-in provider id. Naming
    // the entry `rust-analyzer` would leave resolution falling through to whatever
    // real rust-analyzer happens to be on this machine's PATH.
    let mut settings = LspSettings::default();
    settings.languages.insert(
        "rust".to_owned(),
        crate::config::schema::LspLanguage {
            servers: vec!["pretend-analyzer".to_owned()],
            ..crate::config::schema::LspLanguage::default()
        },
    );
    settings.servers.insert(
        "pretend-analyzer".to_owned(),
        crate::config::schema::LspServer {
            command: binary.to_string_lossy().into_owned(),
            ..crate::config::schema::LspServer::default()
        },
    );
    let (mut manager, _updates) =
        LspManager::new(settings, Some(dir.path().to_path_buf()), None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = dir.path().join("main.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let verdict = (
        LanguageServerId::new("pretend-analyzer"),
        crate::lsp::absolute_path(dir.path()),
    );

    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    manager.note_runtime(
        verdict.0.clone(),
        verdict.1.clone(),
        LanguageServerRuntimeState::Unavailable,
        Some("no such file".to_owned()),
    );

    // Still absent: the verdict stands, and nothing is re-attempted.
    manager.document_opened(Some("rust"), Some("rust"), &path, 2, || {
        "fn main() {}".into()
    });
    assert!(
        manager.runtime_states.contains_key(&verdict),
        "an absent binary lifted the verdict, which is where the notification storm came from"
    );

    // Now it is there. The next open lifts the verdict and re-resolves.
    std::fs::write(&binary, "#!/bin/sh\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }
    manager.document_opened(Some("rust"), Some("rust"), &path, 3, || {
        "fn main() {}".into()
    });
    assert!(
        !manager.runtime_states.contains_key(&verdict),
        "installing the binary did not bring the provider back"
    );
    Ok(())
}

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

/// Repeatedly opening files of a language whose configured command does not exist
/// must not re-attempt the launch each time.
///
/// An earlier attempt at recovery retired the slot whenever the task reported
/// `Unavailable`, so that installing a binary would be picked up. But
/// `Unavailable` is reached only when a spec *was* resolved and its launch failed
/// permanently, and `configured_spec` performs no existence check -- so pointing
/// `lsp.servers` at a path that does not exist produced a fresh task, a fresh
/// exec, and a fresh persistent failure notification for every file opened.
///
/// Recovery for the case users actually hit needs no mechanism at all: a built-in
/// provider that resolves to nothing gets no slot, and resolution short-circuits
/// only on a slot, so installing it by hand is picked up on the next open.
#[tokio::test]
async fn a_configured_command_that_is_missing_is_attempted_once_not_per_open() -> TestResult {
    let dir = tempfile::tempdir()?;
    let binary = dir.path().join("does-not-exist");
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
    let provider = LanguageServerId::new("pretend-analyzer");
    let key = crate::lsp::SlotKey::new(provider.clone(), crate::lsp::absolute_path(dir.path()));

    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    manager.note_runtime(
        &key,
        LanguageServerRuntimeState::Unavailable,
        Some("no such file".to_owned()),
    );

    for version in 2..8 {
        let _ = manager.document_opened(Some("rust"), Some("rust"), &path, version, || {
            "fn main() {}".into()
        });
    }
    assert_eq!(
        manager.servers.len(),
        1,
        "each open built another server task for a command that cannot run"
    );
    // Asserted through the inventory the panel is drawn from, rather than against
    // the manager's own storage: the verdict matters because the *user* must see
    // it, and after this change the slot is the only thing that can carry it.
    let reported = manager
        .inventory([path.clone()])
        .into_iter()
        .find(|status| status.server == provider)
        .and_then(|status| {
            status
                .instances
                .into_iter()
                .find(|instance| instance.root == key.root)
        })
        .ok_or("the provider was missing from the inventory")?;
    assert_eq!(
        reported.runtime,
        LanguageServerRuntimeState::Unavailable,
        "the verdict was discarded, so the next open would re-attempt the launch"
    );
    assert_eq!(reported.error.as_deref(), Some("no such file"));
    Ok(())
}

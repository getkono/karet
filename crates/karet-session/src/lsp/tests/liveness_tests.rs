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
    let mut reported = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !(retrying && reported) {
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
                ..
            } => reported = true,
            _ => {},
        }
    }
    assert!(
        retrying,
        "an idle death left the provider reading as healthy"
    );
    assert!(reported, "an idle death was never reported to the user");
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

/// A task shutting down must not clear the markers of the task that replaced it
/// under the same key.
///
/// Generation cannot fence this. `document_closed` retires a slot with no
/// generation bump when the last document of a language closes, and the next open
/// recreates the identical `{provider}@{root}` key at the same generation -- so a
/// task still running `shutdown` (up to ten seconds) looked exactly like the one
/// now serving, and its parting clear wiped live diagnostics.
#[tokio::test]
async fn a_retiring_task_cannot_clear_its_replacements_diagnostics() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/reopened.rs");
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let key = manager
        .servers
        .keys()
        .next()
        .cloned()
        .ok_or("the open produced no server slot")?;
    let retiring = manager
        .servers
        .get(&key)
        .map(|slot| slot.token)
        .ok_or("the slot has no token")?;

    // The last document closes, then another opens: same key, same generation.
    manager.document_closed(Some("rust"), &path);
    manager.document_opened(Some("rust"), Some("rust"), &path, 2, || {
        "fn main() {}".into()
    });
    let serving = manager
        .servers
        .get(&key)
        .map(|slot| slot.token)
        .ok_or("the reopen produced no server slot")?;
    assert_ne!(
        retiring, serving,
        "the replacement reused the retired token"
    );

    assert!(
        !manager.accepts(&LspUpdate::DiagnosticsCleared {
            token: retiring,
            server: key.clone(),
        }),
        "a retiring task was allowed to clear its replacement's diagnostics"
    );
    assert!(
        manager.accepts(&LspUpdate::DiagnosticsCleared {
            token: serving,
            server: key.clone(),
        }),
        "the task that owns the key was refused its own clear"
    );

    // Once nobody owns the key, the orphaned markers must still be clearable.
    manager.document_closed(Some("rust"), &path);
    assert!(
        manager.accepts(&LspUpdate::DiagnosticsCleared {
            token: retiring,
            server: key,
        }),
        "markers with no live owner were left stranded"
    );
    Ok(())
}

/// A task that owns the key may write and erase its layer; one that has been
/// replaced may do neither.
///
/// The erase half alone was not enough. A retiring task sits inside `shutdown` for
/// up to ten seconds with its diagnostics forwarder still running, so it could
/// publish once more at an unchanged generation *after* its replacement had
/// cleared the layer -- leaving a dead server's markers that nothing would ever
/// remove, because its own parting clear is correctly refused by then.
#[tokio::test]
async fn only_the_task_that_owns_a_layer_may_write_it() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/owned-layer.rs");
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let key = manager
        .servers
        .keys()
        .next()
        .cloned()
        .ok_or("the open produced no server slot")?;
    let retiring = manager
        .servers
        .get(&key)
        .map(|slot| slot.token)
        .ok_or("the slot has no token")?;

    // Same key, same generation, different owner.
    manager.document_closed(Some("rust"), &path);
    manager.document_opened(Some("rust"), Some("rust"), &path, 2, || {
        "fn main() {}".into()
    });
    let serving = manager
        .servers
        .get(&key)
        .map(|slot| slot.token)
        .ok_or("the reopen produced no server slot")?;

    let publish = |token| LspUpdate::Diagnostics {
        token,
        server: key.clone(),
        path: path.clone(),
        version: None,
        diagnostics: Vec::new(),
    };
    assert!(
        !manager.accepts(&publish(retiring)),
        "a retired task was allowed to write over a live layer"
    );
    assert!(
        manager.accepts(&publish(serving)),
        "the owning task was refused its own publish"
    );
    Ok(())
}

/// A provider that gives up permanently must still ask for its layer to be
/// cleared, even though it never publishes anything itself.
///
/// That branch reports `Unavailable` and then drains its channel forever rather
/// than exiting, so the parting clear at the end of the task is unreachable from
/// it -- and it keeps holding the key, so a previous task's clear is refused.
/// Without this, markers inherited from a task that did publish would never be
/// removed. Asserted on the instruction rather than its effect: in this fixture
/// nothing was ever published, so clearing is correctly a downstream no-op.
#[tokio::test]
async fn a_provider_that_gives_up_asks_for_its_layer_to_be_cleared() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("main.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    let (mut manager, mut updates) = LspManager::new(
        LspSettings::default(),
        Some(dir.path().to_path_buf()),
        None,
        None,
    );
    manager.set_connector(failing_connector(Arc::new(AtomicUsize::new(0))));
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });

    let mut unavailable = false;
    let mut asked_to_clear = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !(unavailable && asked_to_clear) {
        let Ok(Some(update)) = tokio::time::timeout(Duration::from_secs(2), updates.recv()).await
        else {
            break;
        };
        match update {
            LspUpdate::RuntimeState { state, .. } => {
                unavailable = unavailable || state == LanguageServerRuntimeState::Unavailable;
            },
            LspUpdate::DiagnosticsCleared { .. } => asked_to_clear = true,
            _ => {},
        }
    }
    assert!(unavailable, "the provider never reported giving up");
    assert!(
        asked_to_clear,
        "giving up never asked for its layer to be cleared"
    );
    Ok(())
}

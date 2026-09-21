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

/// A replacement that never connects for a *retryable* reason must still clear
/// what it inherited.
///
/// The hole this closes, reproduced by review: taking over a key refuses the
/// predecessor's own clear, and a launch that keeps failing with a retryable
/// cause -- a handshake timeout, a briefly unreachable broker -- never reaches the
/// permanent branch that clears, never connects, and loops `Retrying` into
/// `CircuitOpen` forever. The dead server's markers stayed on the document for the
/// rest of the session, drifting onto lines the user had since edited.
#[tokio::test]
async fn a_replacement_that_never_connects_still_clears_what_it_inherited() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("main.rs");
    std::fs::write(&path, "fn main() {}\n")?;
    // Retryable, so the task backs off rather than giving up: the branch that
    // already cleared is deliberately not the one under test.
    let connector: Connector = Arc::new(move |spec, _root| {
        let failure = karet_lsp::LaunchFailure::new(
            spec.command.clone(),
            spec.args.clone(),
            karet_lsp::LaunchCause::Timeout,
        );
        Box::pin(async move { Err(LspError::Launch(Box::new(failure))) })
    });
    let (mut manager, mut updates) = LspManager::new(
        LspSettings::default(),
        Some(dir.path().to_path_buf()),
        None,
        None,
    );
    manager.set_connector(connector);
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });

    let mut retrying = false;
    let mut asked_to_clear = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !asked_to_clear {
        let Ok(Some(update)) = tokio::time::timeout(Duration::from_secs(3), updates.recv()).await
        else {
            break;
        };
        match update {
            LspUpdate::RuntimeState { state, .. } => {
                retrying = retrying
                    || matches!(
                        state,
                        LanguageServerRuntimeState::Retrying
                            | LanguageServerRuntimeState::CircuitOpen
                    );
            },
            LspUpdate::DiagnosticsCleared { .. } => asked_to_clear = true,
            _ => {},
        }
    }
    assert!(retrying, "a retryable launch failure did not retry");
    assert!(
        asked_to_clear,
        "a replacement that never connected left its inherited markers in place"
    );
    Ok(())
}

/// A task shutting down must not overwrite the lifecycle state of the task that
/// replaced it under the same key.
///
/// Reproduced by review as `[Starting, Running, Stopped]`: the last accepted state
/// for a *serving* provider was `Stopped`, which the badge renders as `failed`.
/// `RuntimeState` is identified by `(provider, root)` -- exactly the identity that
/// is retired and re-taken with no generation bump when the last document of a
/// language closes and another opens -- and the retiring task emits its parting
/// `Stopped` only after `client.shutdown()`, which waits up to five seconds on a
/// busy server.
#[tokio::test]
async fn a_retiring_task_cannot_overwrite_its_replacements_state() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let first = PathBuf::from("/tmp/clobber-a.rs");
    manager.document_opened(Some("rust"), Some("rust"), &first, 1, || {
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

    // The last document of the language closes, then another opens: same key, same
    // generation, new owner.
    manager.document_closed(Some("rust"), &first);
    let second = PathBuf::from("/tmp/clobber-b.rs");
    manager.document_opened(Some("rust"), Some("rust"), &second, 1, || {
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

    let stopped = |token| LspUpdate::RuntimeState {
        token,
        server: LanguageServerId::RustAnalyzer,
        root: PathBuf::from("/tmp"),
        state: LanguageServerRuntimeState::Stopped,
        error: None,
    };
    assert!(
        !manager.accepts(&stopped(retiring)),
        "a retiring task was allowed to report a serving provider as stopped"
    );
    assert!(
        manager.accepts(&stopped(serving)),
        "the task that owns the slot was refused its own state report"
    );
    Ok(())
}

/// Retiring a slot drops the state recorded for it.
///
/// Necessary because the fence above refuses updates from a task with no slot: a
/// leftover `Running` could no longer be corrected, and the inventory prefers a
/// recorded state over slot presence, so it would keep describing a task that no
/// longer exists. With no entry it falls back to `Idle`, which is the truth.
#[tokio::test]
async fn retiring_a_slot_drops_the_state_recorded_for_it() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/retired-state.rs");
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let root = manager
        .servers
        .values()
        .next()
        .map(|slot| slot.root.clone())
        .ok_or("the open produced no server slot")?;
    manager.note_runtime(
        LanguageServerId::RustAnalyzer,
        root.clone(),
        LanguageServerRuntimeState::Running,
        None,
    );
    assert!(
        manager
            .runtime_states
            .contains_key(&(LanguageServerId::RustAnalyzer, root.clone())),
        "the reported state was not recorded"
    );

    manager.document_closed(Some("rust"), &path);
    assert!(
        !manager
            .runtime_states
            .contains_key(&(LanguageServerId::RustAnalyzer, root)),
        "a retired slot left its state behind, where nothing can ever correct it"
    );
    Ok(())
}

/// A task that outlives its slot must not raise a launch failure about the
/// provider now serving in its place.
///
/// The disconnected loop awaits `connector(...)` without polling its channel, and
/// that await runs to the handshake timeout -- thirty seconds. So a task can
/// outlive its own slot by that long: close the last file of a language, open
/// another, and the first task's `SpawnFailed` would raise a warning card saying
/// the provider failed to start while the badge beside it reads ready. The card
/// never auto-dismisses.
#[tokio::test]
async fn a_task_that_outlived_its_slot_cannot_report_a_launch_failure() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let first = PathBuf::from("/tmp/outlived-a.rs");
    manager.document_opened(Some("rust"), Some("rust"), &first, 1, || {
        "fn main() {}".into()
    });
    let outlived = manager
        .servers
        .values()
        .next()
        .map(|slot| slot.token)
        .ok_or("the open produced no server slot")?;

    // Retired and re-taken, with no generation bump anywhere.
    manager.document_closed(Some("rust"), &first);
    let second = PathBuf::from("/tmp/outlived-b.rs");
    manager.document_opened(Some("rust"), Some("rust"), &second, 1, || {
        "fn main() {}".into()
    });
    let serving = manager
        .servers
        .values()
        .next()
        .map(|slot| slot.token)
        .ok_or("the reopen produced no server slot")?;
    assert_ne!(
        outlived, serving,
        "the replacement reused the retired token"
    );

    let spawn_failed = |token| LspUpdate::SpawnFailed {
        token,
        server: LanguageServerId::RustAnalyzer,
        root: PathBuf::from("/tmp"),
        command: "rust-analyzer".to_owned(),
        reason: "did not answer the handshake".to_owned(),
        permanent: false,
    };
    assert!(
        !manager.accepts(&spawn_failed(outlived)),
        "a task that outlived its slot reported a failure about a serving provider"
    );
    assert!(
        manager.accepts(&spawn_failed(serving)),
        "the task that owns the slot was refused its own launch failure"
    );

    // The same fence covers the other two things a task says about its own slot.
    assert!(
        !manager.accepts(&LspUpdate::ServerDied {
            token: outlived,
            language: "rust".to_owned(),
        }),
        "a task that outlived its slot reported its death"
    );
    assert!(
        !manager.accepts(&LspUpdate::ServerStatus {
            token: outlived,
            server: "rust-analyzer".to_owned(),
            message: "37% importing".to_owned(),
        }),
        "a task that outlived its slot reported progress"
    );
    Ok(())
}

/// Retiring a slot must *tell* somebody, not just forget.
///
/// The gap this closes: dropping the recorded state made the manager's own answer
/// correct, but nothing reads that map except an inventory query, and a client
/// caches the last state it was told. With no report on retirement the panel kept
/// rendering `running` -- and offering a Restart that silently did nothing -- for a
/// provider whose process was dead. The task's own parting report cannot do this
/// job: by then its slot is gone, which is what its ownership fence refuses.
#[tokio::test]
async fn retiring_a_slot_reports_that_nothing_is_serving() -> TestResult {
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/retire-report.rs");
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    // Drain whatever the open produced, so the assertion below is about the close.
    while let Ok(update) = updates.try_recv() {
        assert!(
            !matches!(update, LspUpdate::SlotRetired { .. }),
            "opening a document retired a slot"
        );
    }

    manager.document_closed(Some("rust"), &path);
    let mut retired = None;
    while let Ok(update) = updates.try_recv() {
        if let LspUpdate::SlotRetired { server, root, .. } = update {
            retired = Some((server, root));
        }
    }
    let (server, root) = retired.ok_or("closing the last document reported nothing")?;
    assert_eq!(server, LanguageServerId::RustAnalyzer);
    assert!(
        manager.accepts(&LspUpdate::SlotRetired {
            generation: manager.generation,
            server,
            root,
        }),
        "the retirement report was refused, so no client would ever hear it"
    );
    Ok(())
}

/// Zero is never a live slot's token.
///
/// `FailureTally` derives `Default`, so a tally built that way carries token zero.
/// Had slots started numbering there, such a tally's reports would be believed and
/// attributed to the session's first server rather than refused.
#[tokio::test]
async fn the_first_slot_does_not_take_the_default_token() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    manager.document_opened(
        Some("rust"),
        Some("rust"),
        &PathBuf::from("/tmp/first-token.rs"),
        1,
        || "fn main() {}".into(),
    );
    let first = manager
        .servers
        .values()
        .next()
        .map(|slot| slot.token)
        .ok_or("the open produced no server slot")?;
    assert_ne!(first, 0, "the first slot took the default token");
    assert!(
        !manager.accepts(&LspUpdate::ServerDied {
            token: 0,
            language: "rust".to_owned(),
        }),
        "a report carrying the default token was believed"
    );
    Ok(())
}

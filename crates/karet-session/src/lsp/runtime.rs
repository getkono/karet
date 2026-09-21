use super::commands::OpenDocument;
use super::commands::answer_empty;
use super::commands::remember_document;
use super::forward::forward_diagnostics;
use super::health::FailureTally;
use super::health::{self};
use super::*;

pub(super) struct ServerTask {
    pub(super) spec: LspSpec,
    pub(super) root: PathBuf,
    pub(super) language: String,
    pub(super) provider: LanguageServerId,
    pub(super) rx: mpsc::Receiver<ServerCmd>,
    pub(super) updates: mpsc::UnboundedSender<LspUpdate>,
    pub(super) connector: Connector,
    pub(super) generation: u64,
    /// Identifies this task as the owner of its slot, so a clear it sends while
    /// shutting down cannot be mistaken for one from its replacement.
    pub(super) token: u64,
}

/// The per-language server task: serialize document sync and requests, restart
/// closed processes with backoff, and replay the authoritative open-document set.
pub(super) async fn server_task(task: ServerTask) {
    let ServerTask {
        spec,
        root,
        language,
        provider,
        mut rx,
        updates,
        connector,
        generation,
        token,
    } = task;
    let report_state = |state, error: Option<String>| {
        let _ = updates.send(LspUpdate::RuntimeState {
            token,
            server: provider.clone(),
            root: root.clone(),
            state,
            error,
        });
    };
    report_state(LanguageServerRuntimeState::Starting, None);
    let mut client: Option<LspClient> = None;
    let mut diagnostic_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut documents = HashMap::<PathBuf, OpenDocument>::new();
    let mut pending: Option<(PathBuf, i32, String)> = None;
    let mut restart_delay = RESTART_MIN_DELAY;
    let mut next_restart = Instant::now();
    let mut failures = VecDeque::<Instant>::new();
    let mut spawn_failure_reported = false;
    // Whether this task ever reached a working connection. A server that has
    // worked here before is worth retrying; one that has never started is not.
    let mut ever_connected = false;
    // The verdict on the *current* connection, reset with every new one: a run of
    // timeouts on a server that has since been replaced says nothing about its
    // successor.
    let mut tally = FailureTally::new(token);
    // Silent deaths, windowed separately: the failure window is too short to hold
    // even one hang cycle, and an unwindowed count never forgives.
    let mut hangs = VecDeque::<Instant>::new();
    // When the current connection was established, so a disconnect can tell a
    // server that worked from one that died on arrival.
    let mut connected_at: Option<Instant> = None;
    // When this provider's published diagnostics stop being worth trusting. Armed
    // on every disconnect, disarmed by a reconnect inside the window.
    let mut clear_diagnostics_at: Option<Instant> = None;

    loop {
        if client.is_none() {
            if let Some(deadline) = clear_diagnostics_at
                && Instant::now() >= deadline
            {
                clear_diagnostics_at = None;
                // `language` is the slot key the forwarder publishes under, not a
                // bare provider id -- reconstructing one here cleared nothing.
                let _ = updates.send(LspUpdate::DiagnosticsCleared {
                    token,
                    server: language.clone(),
                });
            }
            // Sleep to whichever deadline comes first. The grace window usually
            // outlasts the first two reconnect attempts, so it is normally the
            // reconnect that wakes us and the grace never fires at all.
            let wake_at =
                clear_diagnostics_at.map_or(next_restart, |grace| next_restart.min(grace));
            if Instant::now() < wake_at {
                let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(wake_at));
                tokio::pin!(sleep);
                tokio::select! {
                    cmd = rx.recv() => {
                        let Some(cmd) = cmd else {
                            break;
                        };
                        remember_document(&mut documents, &cmd);
                        answer_empty(&updates, cmd, generation);
                        continue;
                    },
                    () = &mut sleep => {},
                }
            }
            // Woken by the grace deadline rather than the retry clock: go round so
            // the block above fires it, then sleep out the rest of the backoff.
            if Instant::now() < next_restart {
                continue;
            }

            let now = Instant::now();
            while failures
                .front()
                .is_some_and(|failure| now.duration_since(*failure) > RESTART_WINDOW)
            {
                failures.pop_front();
            }
            match connector(spec.clone(), root.clone()).await {
                Ok(candidate) => {
                    let mut replay_failed = false;
                    for (path, document) in &documents {
                        if candidate
                            .did_open(path, &document.language, document.version, &document.text)
                            .await
                            .is_err()
                        {
                            replay_failed = true;
                            break;
                        }
                    }
                    if replay_failed {
                        failures.push_back(now);
                        next_restart = now + restart_delay;
                        restart_delay = (restart_delay * 2).min(RESTART_MAX_DELAY);
                        report_state(
                            LanguageServerRuntimeState::Retrying,
                            Some("document replay failed".to_owned()),
                        );
                        continue;
                    }
                    diagnostic_task = Some(forward_diagnostics(
                        &candidate,
                        updates.clone(),
                        language.clone(),
                        provider.key().to_owned(),
                        token,
                    ));
                    client = Some(candidate);
                    ever_connected = true;
                    connected_at = Some(Instant::now());
                    tally = FailureTally::new(token);
                    // Back inside the window: the markers on screen are about to
                    // be republished, so they were never stale.
                    clear_diagnostics_at = None;
                    // The failure budget is *not* cleared here. Connecting is not
                    // the same as working: a server that exits the moment it has
                    // read `didOpen` connects perfectly every time, so clearing
                    // the budget on connect meant such a loop could never open the
                    // circuit. It used to be gated by the user typing, which hid
                    // the cost; now that a death is noticed immediately, an
                    // uncounted loop would respawn forever. The budget is cleared
                    // on disconnect instead, and only for a connection that lasted
                    // long enough to count as real.
                    restart_delay = RESTART_MIN_DELAY;
                    spawn_failure_reported = false;
                    tracing::info!(language, "language server connected");
                    report_state(LanguageServerRuntimeState::Running, None);
                    continue;
                },
                Err(error) => {
                    tracing::warn!(language, command = %spec.command, error = %error, "language server failed to start");
                    // A task that has never connected has published nothing, so
                    // anything on screen under its key came from a predecessor and
                    // is stale by definition. Arming the grace here is what stops
                    // it being stale forever: taking over a key refuses the
                    // predecessor's own clear, and a launch that keeps failing for
                    // a *retryable* reason -- a handshake timeout, a broker that is
                    // briefly unreachable -- never reaches the permanent branch
                    // below, never connects, and so would never clear anything.
                    if !ever_connected {
                        clear_diagnostics_at = clear_diagnostics_at
                            .or_else(|| Some(Instant::now() + DIAGNOSTIC_GRACE));
                    }
                    let launch = match &error {
                        LspError::Launch(failure) => Some(failure.as_ref()),
                        _ => None,
                    };
                    if !spawn_failure_reported {
                        let _ = updates.send(LspUpdate::SpawnFailed {
                            token,
                            server: provider.clone(),
                            root: root.clone(),
                            // The spec, not the failure's own argv: through
                            // the supervisor and the broker the process karet
                            // literally ran is a hidden re-exec of the editor
                            // binary, and the user needs to see the provider
                            // launch they configured.
                            command: std::iter::once(spec.command.as_str())
                                .chain(spec.args.iter().map(String::as_str))
                                .collect::<Vec<_>>()
                                .join(" "),
                            reason: launch.map_or_else(
                                || error.to_string(),
                                karet_lsp::LaunchFailure::diagnosis,
                            ),
                            permanent: launch.is_some_and(|failure| failure.cause.is_permanent()),
                        });
                        spawn_failure_reported = true;
                    }
                    let permanent = launch.is_some_and(|failure| failure.cause.is_permanent());
                    // Nothing about a binary that is absent or unusable changes
                    // by waiting, so this stops rather than respawning it every
                    // five minutes forever. A server that once connected and
                    // then started exiting is a different case -- it keeps the
                    // circuit, which is a cooldown, not a verdict.
                    if permanent && !ever_connected {
                        tracing::warn!(language, error = %error, "language server is unavailable");
                        report_state(
                            LanguageServerRuntimeState::Unavailable,
                            Some(error.to_string()),
                        );
                        // This task owns the key and will never publish anything,
                        // so whatever is on screen under it belongs to a previous
                        // task and nothing else will ever replace it. Cleared here
                        // because the exit below is unreachable from this branch --
                        // it drains instead of breaking, so the parting clear at
                        // the end of the task never runs.
                        let _ = updates.send(LspUpdate::DiagnosticsCleared {
                            token,
                            server: language.clone(),
                        });
                        // Drain rather than return: callers must keep getting
                        // empty answers instead of waiting on a dead channel.
                        while let Some(cmd) = rx.recv().await {
                            remember_document(&mut documents, &cmd);
                            answer_empty(&updates, cmd, generation);
                        }
                        return;
                    }
                    failures.push_back(now);
                    next_restart = if failures.len() >= RESTART_LIMIT {
                        tracing::warn!(language, "language server restart circuit opened");
                        report_state(
                            LanguageServerRuntimeState::CircuitOpen,
                            Some(error.to_string()),
                        );
                        now + CIRCUIT_COOLDOWN
                    } else {
                        report_state(
                            LanguageServerRuntimeState::Retrying,
                            Some(error.to_string()),
                        );
                        let next = now + restart_delay;
                        restart_delay = (restart_delay * 2).min(RESTART_MAX_DELAY);
                        next
                    };
                    continue;
                },
            }
        }

        // Three things can wake the connected loop, and the third is the one that
        // was missing: the connection dying on its own. Without that arm a server
        // that exits while the user reads rather than types stays `Running` until
        // the next request happens to fail.
        let cmd =
            match health::next_wake(&mut rx, client.as_ref(), CHANGE_DEBOUNCE, pending.is_some())
                .await
            {
                health::Wake::Lost => {
                    let _ = client.take();
                    if let Some(task) = diagnostic_task.take() {
                        task.abort();
                    }
                    // Reported exactly as a death found by a failing call is, so
                    // catching the exit sooner does not make it quieter.
                    tally.note_lost(&updates, &language);
                    // The buffered edit dies with the connection it was headed for.
                    // The replay set holds the document's whole text, so the reconnect
                    // re-opens it entire rather than applying a stale delta.
                    pending = None;
                    let (delay, state) = health::charge_disconnect(
                        connected_at,
                        &mut hangs,
                        tally.hung(),
                        &mut failures,
                        &mut restart_delay,
                        &language,
                    );
                    connected_at = None;
                    next_restart = Instant::now() + delay;
                    clear_diagnostics_at = Some(Instant::now() + DIAGNOSTIC_GRACE);
                    report_state(state, None);
                    continue;
                },
                health::Wake::Quiet => {
                    let Some(active) = client.as_ref() else {
                        continue;
                    };
                    let mut dead = false;
                    flush_pending(
                        active,
                        &mut pending,
                        &mut dead,
                        &mut tally,
                        &updates,
                        &language,
                    )
                    .await;
                    if dead {
                        let _ = client.take();
                        if let Some(task) = diagnostic_task.take() {
                            task.abort();
                        }
                        let (delay, state) = health::charge_disconnect(
                            connected_at,
                            &mut hangs,
                            tally.hung(),
                            &mut failures,
                            &mut restart_delay,
                            &language,
                        );
                        connected_at = None;
                        next_restart = Instant::now() + delay;
                        clear_diagnostics_at = Some(Instant::now() + DIAGNOSTIC_GRACE);
                        report_state(state, None);
                    }
                    continue;
                },
                health::Wake::Command(cmd) => cmd,
            };
        let Some(cmd) = cmd else {
            break; // the session dropped the manager
        };
        remember_document(&mut documents, &cmd);
        let Some(active) = client.as_ref() else {
            answer_empty(&updates, cmd, generation);
            continue;
        };
        let mut dead = false;
        match cmd {
            ServerCmd::DidChange {
                path,
                version,
                text,
            } => {
                // Coalesce successive edits to the same document; an edit to a
                // different document flushes the previous one first (order).
                if pending.as_ref().is_some_and(|(p, ..)| *p != path) {
                    flush_pending(
                        active,
                        &mut pending,
                        &mut dead,
                        &mut tally,
                        &updates,
                        &language,
                    )
                    .await;
                }
                if !dead {
                    pending = Some((path, version, text));
                }
            },
            ServerCmd::DidOpen {
                path,
                language: document_language,
                version,
                text,
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                if !dead {
                    let result = active
                        .did_open(&path, &document_language, version, &text)
                        .await;
                    tally.note(result, &mut dead, &updates, &language);
                }
            },
            ServerCmd::DidClose { path } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                if !dead {
                    let result = active.did_close(&path).await;
                    tally.note(result, &mut dead, &updates, &language);
                }
            },
            ServerCmd::DidSave { path, text } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                if !dead {
                    let result = active.did_save(&path, Some(&text)).await;
                    tally.note(result, &mut dead, &updates, &language);
                }
            },
            ServerCmd::Completion {
                request,
                doc,
                version,
                path,
                position,
            } => {
                // The server must see the latest text before completing in it.
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let items = if dead {
                    Vec::new()
                } else {
                    match tally.observe(active.completion(&path, position).await) {
                        Ok(items) => items,
                        Err(e) => {
                            tally.note::<()>(Err(e), &mut dead, &updates, &language);
                            Vec::new()
                        },
                    }
                };
                let _ = updates.send(LspUpdate::Completions {
                    generation,
                    request,
                    doc,
                    version,
                    items,
                });
            },
            ServerCmd::DocumentSymbols {
                request,
                doc,
                version,
                path,
            } => {
                // Symbol ranges must describe the same text revision as the request.
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    match tally.observe(active.document_symbols(&path).await) {
                        Ok(symbols) => symbols,
                        Err(error) => {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            Vec::new()
                        },
                    }
                };
                let _ = updates.send(LspUpdate::Symbols {
                    generation,
                    request,
                    doc,
                    version,
                    symbols,
                });
            },
            ServerCmd::Hover {
                request,
                doc,
                version,
                path,
                position,
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let hover = if dead {
                    None
                } else {
                    tally
                        .observe(active.hover(&path, position).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            None
                        })
                };
                let _ = updates.send(LspUpdate::Hover {
                    generation,
                    request,
                    doc,
                    version,
                    hover,
                });
            },
            ServerCmd::Definition {
                request,
                doc,
                version,
                path,
                position,
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let locations = if dead {
                    Vec::new()
                } else {
                    tally
                        .observe(active.definition(&path, position).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            Vec::new()
                        })
                };
                let _ = updates.send(LspUpdate::Definitions {
                    generation,
                    request,
                    doc,
                    version,
                    locations,
                });
            },
            ServerCmd::WorkspaceSymbols { request, query } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    tally
                        .observe(active.workspace_symbols(&query).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            Vec::new()
                        })
                };
                let _ = updates.send(LspUpdate::WorkspaceSymbols {
                    generation,
                    request,
                    symbols,
                });
            },
            ServerCmd::Rename {
                request,
                path,
                position,
                new_name,
                ..
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let edit = if dead {
                    WorkspaceEdit::default()
                } else {
                    tally
                        .observe(active.rename(&path, position, &new_name).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            WorkspaceEdit::default()
                        })
                };
                let _ = updates.send(LspUpdate::WorkspaceEdit {
                    generation,
                    request,
                    edit,
                });
            },
            ServerCmd::Formatting {
                request,
                doc,
                version,
                path,
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                )
                .await;
                let edits = if dead {
                    Vec::new()
                } else {
                    tally
                        .observe(active.formatting(&path).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(Err(error), &mut dead, &updates, &language);
                            Vec::new()
                        })
                };
                let _ = updates.send(LspUpdate::Formatting {
                    generation,
                    request,
                    doc,
                    version,
                    edits,
                });
            },
        }
        if dead {
            let _ = client.take();
            if let Some(task) = diagnostic_task.take() {
                task.abort();
            }
            pending = None;
            let (delay, state) = health::charge_disconnect(
                connected_at,
                &mut hangs,
                tally.hung(),
                &mut failures,
                &mut restart_delay,
                &language,
            );
            connected_at = None;
            next_restart = Instant::now() + delay;
            clear_diagnostics_at = Some(Instant::now() + DIAGNOSTIC_GRACE);
            report_state(state, None);
        }
    }
    if let Some(client) = client {
        let _ = client.shutdown().await;
    }
    if let Some(task) = diagnostic_task {
        task.abort();
    }
    // Retirement is the other way a provider's diagnostics become stale, and the
    // grace timer cannot cover it: `reconfigure`, `restart` and the last
    // `didClose` all retire a slot by dropping its sender, so the task leaves
    // through the channel-closed `break` above without ever reaching the
    // disconnected branch where the grace fires. Cleared here with no grace,
    // because there is no reconnect coming to make the markers true again -- and
    // after a generation bump the key can change, so nothing would ever replace
    // them.
    let _ = updates.send(LspUpdate::DiagnosticsCleared {
        token,
        server: language.clone(),
    });
    // No parting lifecycle report. A task reaches here only because its channel
    // closed, which means every sender was dropped, which means its slot was
    // removed -- so an ownership-fenced report would be refused by construction.
    // Sending one anyway would read as a live signal to the next person to touch
    // this, who would find it "not showing up" and loosen the fence, restoring the
    // zombie-overwrites-its-replacement bug the fence exists to prevent.
    //
    // Nothing is lost: retiring a slot drops its recorded state, so the provider
    // falls back to `Idle` -- "no open document needs it", which is exactly what
    // has just become true.
}

/// Send the pending `didChange`, if any.
async fn flush_pending(
    client: &LspClient,
    pending: &mut Option<(PathBuf, i32, String)>,
    dead: &mut bool,
    tally: &mut FailureTally,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    language: &str,
) {
    if *dead {
        *pending = None;
        return;
    }
    if let Some((path, version, text)) = pending.take() {
        let result = client.did_change(&path, version, &text).await;
        tally.note(result, dead, updates, language);
    }
}

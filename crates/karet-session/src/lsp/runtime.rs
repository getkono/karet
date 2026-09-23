use super::commands::OpenDocument;
use super::commands::answer_empty;
use super::commands::remember_document;
use super::forward::forward_diagnostics;
use super::health::FailureTally;
use super::health::{self};
use super::*;
use crate::session::FORMAT_ON_SAVE_DEADLINE_MS;

/// Slack over the session's own deadline before this task stops waiting.
///
/// The session sweeps [`FORMAT_ON_SAVE_DEADLINE_MS`] on its backup tick, so its
/// effective deadline is that plus up to one tick. Giving up any sooner would
/// race the sweep and throw away an answer that was about to be used.
const FORMATTING_SWEEP_MARGIN_MS: u64 = 3_000;

/// How long `server_task` waits for a `textDocument/formatting` reply.
///
/// Tied to the session's [`FORMAT_ON_SAVE_DEADLINE_MS`] rather than chosen
/// independently: past that deadline the save has been committed unformatted,
/// so an answer arriving later has nobody left waiting for it -- while the wait
/// itself is serial, and holds every other command for this server (diagnostics,
/// completions, `didChange` flushes) behind it.
///
/// `karet-jsonrpc`'s 30-second request timeout is the only other bound on this
/// await, and it is three times too long to hold the task for: it exists to
/// decide that a *connection* is hung, which is a different question from how
/// long one save may wait.
pub(super) const FORMATTING_DEADLINE: Duration =
    Duration::from_millis(FORMAT_ON_SAVE_DEADLINE_MS + FORMATTING_SWEEP_MARGIN_MS);

pub(super) struct ServerTask {
    pub(super) spec: LspSpec,
    pub(super) root: PathBuf,
    pub(super) language: String,
    pub(super) provider: LanguageServerId,
    pub(super) rx: mpsc::Receiver<ServerCmd>,
    pub(super) updates: mpsc::UnboundedSender<LspUpdate>,
    pub(super) connector: Connector,
    pub(super) generation: u64,
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
    } = task;
    let report_state = |state, error: Option<String>| {
        let _ = updates.send(LspUpdate::RuntimeState {
            generation,
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
    let mut tally = FailureTally::default();
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
                    generation,
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
                        generation,
                    ));
                    client = Some(candidate);
                    ever_connected = true;
                    connected_at = Some(Instant::now());
                    tally = FailureTally::default();
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
                    let launch = match &error {
                        LspError::Launch(failure) => Some(failure.as_ref()),
                        _ => None,
                    };
                    if !spawn_failure_reported {
                        let _ = updates.send(LspUpdate::SpawnFailed {
                            generation,
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
                    tally.note_lost(&updates, &language, generation);
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
                        generation,
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
                        generation,
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
                    generation,
                )
                .await;
                if !dead {
                    let result = active
                        .did_open(&path, &document_language, version, &text)
                        .await;
                    tally.note(result, &mut dead, &updates, &language, generation);
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
                    generation,
                )
                .await;
                if !dead {
                    let result = active.did_close(&path).await;
                    tally.note(result, &mut dead, &updates, &language, generation);
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
                    generation,
                )
                .await;
                if !dead {
                    let result = active.did_save(&path, Some(&text)).await;
                    tally.note(result, &mut dead, &updates, &language, generation);
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
                    generation,
                )
                .await;
                let items = if dead {
                    Vec::new()
                } else {
                    match tally.observe(active.completion(&path, position).await) {
                        Ok(items) => items,
                        Err(e) => {
                            tally.note::<()>(Err(e), &mut dead, &updates, &language, generation);
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
                    generation,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    match tally.observe(active.document_symbols(&path).await) {
                        Ok(symbols) => symbols,
                        Err(error) => {
                            tally.note::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
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
                    generation,
                )
                .await;
                let hover = if dead {
                    None
                } else {
                    tally
                        .observe(active.hover(&path, position).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
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
                    generation,
                )
                .await;
                let locations = if dead {
                    Vec::new()
                } else {
                    tally
                        .observe(active.definition(&path, position).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
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
                    generation,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    tally
                        .observe(active.workspace_symbols(&query).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
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
                    generation,
                )
                .await;
                let edit = if dead {
                    WorkspaceEdit::default()
                } else {
                    tally
                        .observe(active.rename(&path, position, &new_name).await)
                        .unwrap_or_else(|error| {
                            tally.note::<()>(
                                Err(error),
                                &mut dead,
                                &updates,
                                &language,
                                generation,
                            );
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
                indentation,
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &mut tally,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                // A server that never advertised the method can only answer
                // "method not found". Asking anyway would spend a round trip --
                // on every save, once format-on-save is on -- to learn what the
                // handshake already said.
                let advertised = !dead && active.supports_formatting();
                // Every ending but a successful reply leaves the file unformatted,
                // and each one is reported as such so the session can fall back on
                // its own formatter. A connection that died, and a request that
                // errored, format exactly as much as a server that never offered
                // the method: nothing.
                // The wait is bounded here as well as by the save, because the
                // two waits cost different things: the save's deadline gives up
                // on the answer, while this one gives the *task* back. Every
                // command for this server -- diagnostics, completions, a
                // `didChange` flush -- queues behind this `await`.
                let (formatted, edits) = if !advertised {
                    (false, Vec::new())
                } else {
                    match tokio::time::timeout(
                        FORMATTING_DEADLINE,
                        active.formatting(&path, indentation),
                    )
                    .await
                    {
                        Ok(answer) => match tally.observe(answer) {
                            Ok(edits) => (true, edits),
                            Err(error) => {
                                tally.note::<()>(
                                    Err(error),
                                    &mut dead,
                                    &updates,
                                    &language,
                                    generation,
                                );
                                (false, Vec::new())
                            },
                        },
                        // Deliberately not charged to the connection and not a
                        // death: a formatter slower than one save can wait for is
                        // not a server that has stopped answering. The request is
                        // left to expire on its own in the JSON-RPC layer.
                        Err(_elapsed) => {
                            tracing::warn!(
                                language,
                                "formatting outlasted the save that asked for it; \
                                 saving unformatted"
                            );
                            (false, Vec::new())
                        },
                    }
                };
                let _ = updates.send(LspUpdate::Formatting {
                    generation,
                    request,
                    doc,
                    version,
                    formatted,
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
    // No parting clear. Retirement is the other way a provider's markers go stale,
    // but clearing them from here needs to know who *owns* a layer: a task leaving
    // through the channel-closed `break` has already lost its slot, possibly to a
    // replacement under the identical key, and a clear at that moment can wipe the
    // live server's markers instead. Establishing ownership is deferred to its own
    // change; until then retired markers persist until something republishes, which
    // is what they did before any of this.
    report_state(LanguageServerRuntimeState::Stopped, None);
}

/// Send the pending `didChange`, if any.
async fn flush_pending(
    client: &LspClient,
    pending: &mut Option<(PathBuf, i32, String)>,
    dead: &mut bool,
    tally: &mut FailureTally,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    language: &str,
    generation: u64,
) {
    if *dead {
        *pending = None;
        return;
    }
    if let Some((path, version, text)) = pending.take() {
        let result = client.did_change(&path, version, &text).await;
        tally.note(result, dead, updates, language, generation);
    }
}

use super::*;

/// Answer a request command with an empty set (used whenever no live server can
/// answer, so the client is never left waiting).
fn answer_empty(updates: &mpsc::UnboundedSender<LspUpdate>, cmd: ServerCmd, generation: u64) {
    match cmd {
        ServerCmd::Completion {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Completions {
                generation,
                request,
                doc,
                version,
                items: Vec::new(),
            });
        },
        ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Symbols {
                generation,
                request,
                doc,
                version,
                symbols: Vec::new(),
            });
        },
        ServerCmd::Hover {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Hover {
                generation,
                request,
                doc,
                version,
                hover: None,
            });
        },
        ServerCmd::Definition {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Definitions {
                generation,
                request,
                doc,
                version,
                locations: Vec::new(),
            });
        },
        ServerCmd::WorkspaceSymbols { request, .. } => {
            let _ = updates.send(LspUpdate::WorkspaceSymbols {
                generation,
                request,
                symbols: Vec::new(),
            });
        },
        ServerCmd::Rename { request, .. } => {
            let _ = updates.send(LspUpdate::WorkspaceEdit {
                generation,
                request,
                edit: WorkspaceEdit::default(),
            });
        },
        ServerCmd::Formatting {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Formatting {
                generation,
                request,
                doc,
                version,
                edits: Vec::new(),
            });
        },
        ServerCmd::DidOpen { .. }
        | ServerCmd::DidChange { .. }
        | ServerCmd::DidClose { .. }
        | ServerCmd::DidSave { .. } => {},
    }
}

#[derive(Clone)]
struct OpenDocument {
    language: String,
    version: i32,
    text: String,
}

fn remember_document(documents: &mut HashMap<PathBuf, OpenDocument>, cmd: &ServerCmd) {
    match cmd {
        ServerCmd::DidOpen {
            path,
            language,
            version,
            text,
        } => {
            documents.insert(
                path.clone(),
                OpenDocument {
                    language: language.clone(),
                    version: *version,
                    text: text.clone(),
                },
            );
        },
        ServerCmd::DidChange {
            path,
            version,
            text,
        } => {
            if let Some(document) = documents.get_mut(path) {
                document.version = *version;
                document.text.clone_from(text);
            }
        },
        ServerCmd::DidSave { path, text } => {
            if let Some(document) = documents.get_mut(path) {
                document.text.clone_from(text);
            }
        },
        ServerCmd::DidClose { path } => {
            documents.remove(path);
        },
        ServerCmd::Completion { .. }
        | ServerCmd::DocumentSymbols { .. }
        | ServerCmd::Hover { .. }
        | ServerCmd::Definition { .. }
        | ServerCmd::WorkspaceSymbols { .. }
        | ServerCmd::Rename { .. }
        | ServerCmd::Formatting { .. } => {},
    }
}

fn forward_diagnostics(
    client: &LspClient,
    updates: mpsc::UnboundedSender<LspUpdate>,
    language: String,
    generation: u64,
) -> tokio::task::JoinHandle<()> {
    let mut diagnostic_rx = client.diagnostics();
    tokio::spawn(async move {
        loop {
            match diagnostic_rx.recv().await {
                Ok(publication) => {
                    let _ = updates.send(LspUpdate::Diagnostics {
                        generation,
                        server: language.clone(),
                        path: publication.path,
                        version: publication.version,
                        diagnostics: publication.diagnostics,
                    });
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "language-server diagnostic subscriber lagged");
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

/// The per-language server task: serialize document sync and requests, restart
/// closed processes with backoff, and replay the authoritative open-document set.
pub(super) async fn server_task(
    spec: LspSpec,
    root: PathBuf,
    language: String,
    mut rx: mpsc::Receiver<ServerCmd>,
    updates: mpsc::UnboundedSender<LspUpdate>,
    connector: Connector,
    generation: u64,
) {
    let mut client: Option<LspClient> = None;
    let mut diagnostic_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut documents = HashMap::<PathBuf, OpenDocument>::new();
    let mut pending: Option<(PathBuf, i32, String)> = None;
    let mut restart_delay = RESTART_MIN_DELAY;
    let mut next_restart = Instant::now();
    let mut failures = VecDeque::<Instant>::new();
    let mut spawn_failure_reported = false;

    loop {
        if client.is_none() {
            if Instant::now() < next_restart {
                let sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(next_restart));
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
                        continue;
                    }
                    diagnostic_task = Some(forward_diagnostics(
                        &candidate,
                        updates.clone(),
                        language.clone(),
                        generation,
                    ));
                    client = Some(candidate);
                    failures.clear();
                    restart_delay = RESTART_MIN_DELAY;
                    spawn_failure_reported = false;
                    tracing::info!(language, "language server connected");
                    continue;
                },
                Err(error) => {
                    tracing::warn!(language, command = %spec.command, error = %error, "language server failed to start");
                    if !spawn_failure_reported {
                        let _ = updates.send(LspUpdate::SpawnFailed {
                            generation,
                            language: language.clone(),
                            command: spec.command.clone(),
                        });
                        spawn_failure_reported = true;
                    }
                    failures.push_back(now);
                    next_restart = if failures.len() >= RESTART_LIMIT {
                        tracing::warn!(language, "language server restart circuit opened");
                        now + CIRCUIT_COOLDOWN
                    } else {
                        let next = now + restart_delay;
                        restart_delay = (restart_delay * 2).min(RESTART_MAX_DELAY);
                        next
                    };
                    continue;
                },
            }
        }

        let cmd = if pending.is_some() {
            match tokio::time::timeout(CHANGE_DEBOUNCE, rx.recv()).await {
                Ok(cmd) => cmd,
                Err(_quiet) => {
                    let Some(active) = client.as_ref() else {
                        continue;
                    };
                    let mut dead = false;
                    flush_pending(
                        active,
                        &mut pending,
                        &mut dead,
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
                        next_restart = Instant::now() + restart_delay;
                    }
                    continue;
                },
            }
        } else {
            rx.recv().await
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                if !dead {
                    let result = active
                        .did_open(&path, &document_language, version, &text)
                        .await;
                    note_failure(result, &mut dead, &updates, &language, generation);
                }
            },
            ServerCmd::DidClose { path } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                if !dead {
                    let result = active.did_close(&path).await;
                    note_failure(result, &mut dead, &updates, &language, generation);
                }
            },
            ServerCmd::DidSave { path, text } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                if !dead {
                    let result = active.did_save(&path, Some(&text)).await;
                    note_failure(result, &mut dead, &updates, &language, generation);
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let items = if dead {
                    Vec::new()
                } else {
                    match active.completion(&path, position).await {
                        Ok(items) => items,
                        Err(e) => {
                            note_failure::<()>(Err(e), &mut dead, &updates, &language, generation);
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    match active.document_symbols(&path).await {
                        Ok(symbols) => symbols,
                        Err(error) => {
                            note_failure::<()>(
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let hover = if dead {
                    None
                } else {
                    active.hover(&path, position).await.unwrap_or_else(|error| {
                        note_failure::<()>(Err(error), &mut dead, &updates, &language, generation);
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let locations = if dead {
                    Vec::new()
                } else {
                    active
                        .definition(&path, position)
                        .await
                        .unwrap_or_else(|error| {
                            note_failure::<()>(
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let symbols = if dead {
                    Vec::new()
                } else {
                    active
                        .workspace_symbols(&query)
                        .await
                        .unwrap_or_else(|error| {
                            note_failure::<()>(
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
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let edit = if dead {
                    WorkspaceEdit::default()
                } else {
                    active
                        .rename(&path, position, &new_name)
                        .await
                        .unwrap_or_else(|error| {
                            note_failure::<()>(
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
            } => {
                flush_pending(
                    active,
                    &mut pending,
                    &mut dead,
                    &updates,
                    &language,
                    generation,
                )
                .await;
                let edits = if dead {
                    Vec::new()
                } else {
                    active.formatting(&path).await.unwrap_or_else(|error| {
                        note_failure::<()>(Err(error), &mut dead, &updates, &language, generation);
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
            next_restart = Instant::now() + restart_delay;
        }
    }
    if let Some(client) = client {
        let _ = client.shutdown().await;
    }
    if let Some(task) = diagnostic_task {
        task.abort();
    }
}

/// Send the pending `didChange`, if any.
async fn flush_pending(
    client: &LspClient,
    pending: &mut Option<(PathBuf, i32, String)>,
    dead: &mut bool,
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
        note_failure(result, dead, updates, language, generation);
    }
}

/// Record a client-call failure: a closed connection kills the server slot
/// (reported once); other errors are logged and the task keeps going.
fn note_failure<T>(
    result: Result<T, LspError>,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    language: &str,
    generation: u64,
) {
    match result {
        Ok(_) => {},
        Err(LspError::Closed) => {
            if !*dead {
                *dead = true;
                let _ = updates.send(LspUpdate::ServerDied {
                    generation,
                    language: language.to_owned(),
                });
            }
        },
        Err(e) => {
            tracing::warn!(language, error = %e, "language server call failed");
        },
    }
}

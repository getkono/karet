//! What a client is told when a provider stops serving.
//!
//! Separate from `liveness_tests`, which covers a server that *dies* while karet
//! still wants it. These cover the other way a provider stops: karet retires it
//! deliberately -- the last document closes, settings change, or the user asks
//! for a restart -- and a task that has been retired must not be able to speak
//! for the slot it no longer holds.
//!
//! Everything here asserts through `Command`/`Event`. Nothing reads
//! `manager.servers`, `manager.accepts`, or any other manager internal: the
//! defect these replace was a test that passed against a deliberately broken
//! implementation because it asserted on an instruction rather than its effect.

use super::*;

/// Find one provider's instance at one root in an inventory answer.
fn instance(
    servers: &[LanguageServerStatus],
    provider: &str,
    root: &Path,
) -> Option<LanguageServerInstanceStatus> {
    servers
        .iter()
        .find(|status| status.server.key() == provider)?
        .instances
        .iter()
        .find(|instance| instance.root == root)
        .cloned()
}

/// A session rooted at `root`, so the inventory keeps reporting that root after
/// its documents close.
///
/// `session_with_connector` leaves `roots` empty, and `inventory` derives the
/// roots it reports from the open documents plus the workspace root. With
/// neither, a closed document's root leaves the inventory altogether and the row
/// under test disappears rather than going idle. A real editor always has a
/// workspace root, which is exactly the case where a stale state would be seen.
fn session_rooted_at(root: &Path, connector: Connector) -> (Session, EventRx) {
    let (mut session, events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    });
    session.set_lsp_connector(connector);
    (session, events)
}

/// Ask the backend for its inventory and wait for the answering event.
async fn inventory(
    backend: &impl Backend,
    events: &mut EventRx,
) -> Option<Vec<LanguageServerStatus>> {
    let request = backend.next_id();
    backend.send(request, Command::LanguageServerStatus).ok()?;
    while let Some((id, event)) = next_event(events).await {
        if let Event::LanguageServerStatus { servers } = event
            && id == Some(request)
        {
            return Some(servers);
        }
    }
    None
}

/// Closing the last document that needed a provider leaves nothing running, and
/// the inventory must say so.
///
/// The defect: runtime state lived in a map beside the slots, and
/// `document_closed` dropped the slot without dropping the state. The row then
/// reported the state the dead task had last published -- `Running` -- beside an
/// `open_documents` of 0 read from the (absent) slot. Two halves of one row,
/// disagreeing, and the panel offered a Restart for a process that was gone.
///
/// Falsified by: giving `ServerSlot::runtime` a second home. Restore the
/// `runtime_states` map in `lsp.rs` and have `inventory_instance` prefer it over
/// the slot, and this reports `Running` with 0 documents again.
#[tokio::test]
async fn a_provider_whose_last_document_closed_is_reported_idle_with_no_documents() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let root = crate::lsp::absolute_path(dir.path());
    let (session, mut events) = session_rooted_at(
        &root,
        test_connector(Behavior::Normal, None, Arc::new(AtomicUsize::new(0))),
    );
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, _) = await_opened(&mut events).await.ok_or("no Opened")?;

    // Wait for the provider to actually be serving, so "idle afterwards" is a
    // transition this test caused rather than a state it started in.
    let mut running = false;
    while !running {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            running = state == LanguageServerRuntimeState::Running;
        }
    }
    assert!(running, "the provider never reported itself running");

    let serving = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing from the inventory while serving")?;
    assert_eq!(serving.runtime, LanguageServerRuntimeState::Running);
    assert_eq!(serving.open_documents, 1);

    backend.send(backend.next_id(), Command::CloseDocument { doc })?;

    let retired = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing from the inventory after the close")?;
    assert_eq!(
        retired.runtime,
        LanguageServerRuntimeState::Idle,
        "a provider with no process must not be reported as running"
    );
    assert_eq!(retired.open_documents, 0);
    assert_eq!(retired.error, None);
    Ok(())
}

/// A provider that cannot start reports `Unavailable` for as long as karet is
/// still holding the slot open for it, and that survives repeated opens.
///
/// The counterpart to the test above: "no slot means idle" must not have become
/// "a failure is forgotten". A permanently failed task keeps draining its inbox
/// rather than returning, so its slot -- and its verdict -- outlive the failure.
///
/// Falsified by: making `server_task` `return` instead of draining after it
/// reports `Unavailable` (`runtime.rs`), which drops the slot and turns this row
/// back into `Idle` with no error.
#[tokio::test]
async fn a_provider_that_cannot_start_keeps_reporting_why() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let root = crate::lsp::absolute_path(dir.path());
    let (session, mut events) =
        session_rooted_at(&root, failing_connector(Arc::new(AtomicUsize::new(0))));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path,
            language: None,
        },
    )?;
    await_opened(&mut events).await.ok_or("no Opened")?;

    let mut unavailable = false;
    while !unavailable {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            unavailable = state == LanguageServerRuntimeState::Unavailable;
        }
    }
    assert!(
        unavailable,
        "the provider never reported itself unavailable"
    );

    let reported = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing from the inventory")?;
    assert_eq!(
        reported.runtime,
        LanguageServerRuntimeState::Unavailable,
        "a provider karet has given up on must keep saying so"
    );
    assert!(
        reported.error.is_some(),
        "the reason a provider cannot start is the whole value of the row"
    );
    Ok(())
}

/// A scripted server that marks `target`, then keeps serving.
///
/// Two things let the retirement tests below tell layers apart. It stamps each
/// diagnostic's `source` with its own command, and `source` travels untouched to
/// `Event::DiagnosticsPublished` -- so one published set names every provider
/// that contributed to it. And it publishes for a file it was never opened on,
/// which is ordinary language-server behaviour (a project-wide error belongs to
/// the file it is in, not the file being edited) and is exactly the case where a
/// retired provider's markers used to survive: the document outlives the slot.
///
/// It stays alive after publishing. A server that *dies* arms the one-second
/// grace clear, and then a passing test could not tell markers cleared because
/// the provider was retired from markers cleared because it died.
pub(super) fn marking_connector(
    target: PathBuf,
    opened: Option<mpsc::UnboundedSender<String>>,
) -> Connector {
    Arc::new(move |spec, root| {
        let target = target.clone();
        let opened = opened.clone();
        let source = PathBuf::from(&spec.command).file_name().map_or_else(
            || spec.command.clone(),
            |name| name.to_string_lossy().into(),
        );
        Box::pin(async move {
            let (client_end, server_end) = tokio::io::duplex(1 << 20);
            let (server_read, mut server_write) = tokio::io::split(server_end);
            tokio::spawn(async move {
                let mut reader = BufReader::new(server_read);
                let Some(init) = read_msg(&mut reader).await else {
                    return;
                };
                write_msg(
                    &mut server_write,
                    &json!({"jsonrpc": "2.0", "id": init["id"], "result": {"capabilities": {}}}),
                )
                .await;
                let _initialized = read_msg(&mut reader).await;
                let mut marked = false;
                while let Some(msg) = read_msg(&mut reader).await {
                    if msg["method"] == "textDocument/didOpen"
                        && let Some(opened) = &opened
                    {
                        let _ = opened.send(source.clone());
                    }
                    if msg["method"] == "textDocument/didOpen" && !marked {
                        marked = true;
                        write_msg(
                            &mut server_write,
                            &json!({"jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": format!("file://{}", target.display()),
                                "diagnostics": [{
                                    "range": {
                                        "start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 1}
                                    },
                                    "severity": 1,
                                    "source": source,
                                    "message": "marked by a provider"
                                }]
                            }}),
                        )
                        .await;
                    }
                }
            });
            let (read, write) = tokio::io::split(client_end);
            LspClient::connect(read, write, &root).await
        })
    })
}

/// Drain until one published set for `doc` satisfies `wanted`, returning its
/// sources.
///
/// Deliberately asks about *one* published set rather than comparing two events
/// across time. "The retired provider's marker is absent" is equally true of an
/// empty screen, of a provider that never published, and of a clear that wiped
/// every layer -- so it is only worth asserting beside a surviving provider in
/// the same payload.
async fn await_sources(
    events: &mut EventRx,
    doc: DocumentId,
    wanted: impl Fn(&[String]) -> bool,
) -> Option<Vec<String>> {
    loop {
        let (_, event) = next_event(events).await?;
        if let Event::DiagnosticsPublished {
            doc: published,
            diagnostics,
        } = event
            && published == doc
        {
            let mut sources: Vec<String> = diagnostics
                .iter()
                .filter_map(|diagnostic| diagnostic.source.clone())
                .collect();
            sources.sort();
            if wanted(&sources) {
                return Some(sources);
            }
        }
    }
}

/// Open one document and return its id.
pub(super) async fn open(
    backend: &impl Backend,
    events: &mut EventRx,
    path: &Path,
) -> Option<DocumentId> {
    backend
        .send(
            backend.next_id(),
            Command::OpenDocument {
                path: path.to_path_buf(),
                language: None,
            },
        )
        .ok()?;
    await_opened(events).await.map(|(doc, _)| doc)
}

/// Retiring a provider takes its markers with it, and only its own.
///
/// This is the acceptance criterion of issue #278, and the case PR #277 could
/// not land. A provider retired by the last of its documents closing kept every
/// marker it had published on files that were still open, because no retirement
/// path cleared a layer at all -- the markers stayed until something republished,
/// which for a retired provider is never.
///
/// Both halves are asserted in one payload. Absence alone would also be
/// satisfied by clearing every provider's layer, which is the opposite defect
/// and the one that wipes a live server's markers.
///
/// Falsified by (each independently):
/// - dropping `clear_lsp_diagnostic_layer` from `Session::adopt_retirement`:
///   the retired provider's marker survives.
/// - clearing every layer there instead of the retired keys: the surviving
///   provider's marker disappears and never comes back, because it publishes
///   once on `didOpen`.
#[tokio::test]
async fn retiring_one_provider_leaves_another_providers_markers_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = crate::lsp::absolute_path(dir.path());
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "plain text\n")?;
    let rust = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let python = dir.path().join("main.py");
    std::fs::write(&python, "x = 1\n")?;

    let mut settings = LspSettings::default();
    for (language, command) in [("rust", "server-rust"), ("python", "server-python")] {
        settings.servers.insert(
            language.to_owned(),
            crate::config::schema::LspServer {
                command: command.to_owned(),
                ..crate::config::schema::LspServer::default()
            },
        );
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.clone()],
        settings: crate::config::Settings {
            lsp: settings,
            ..crate::config::Settings::default()
        },
        ..SessionConfig::default()
    });
    session.set_lsp_connector(marking_connector(crate::lsp::absolute_path(&notes), None));
    let backend = local_session(session, None);

    // `notes.txt` selects no provider of its own; it is only ever marked *by*
    // the two that the other files start.
    let notes_doc = open(&backend, &mut events, &notes)
        .await
        .ok_or("notes.txt never opened")?;
    let _rust_doc = open(&backend, &mut events, &rust)
        .await
        .ok_or("main.rs never opened")?;
    let python_doc = open(&backend, &mut events, &python)
        .await
        .ok_or("main.py never opened")?;

    // Python starts two providers, not one: the configured primary and the
    // built-in diagnostics companion. Both mark `notes.txt`, so closing the file
    // retires two slots and leaves one -- which is the shape worth testing.
    let both = await_sources(&mut events, notes_doc, |sources| {
        sources.iter().any(|s| s == "server-rust") && sources.iter().any(|s| s == "server-python")
    })
    .await
    .ok_or("both providers never marked notes.txt")?;
    assert!(both.len() >= 2, "expected several layers, got {both:?}");

    // Closing the last Python document retires its providers -- and nothing else.
    backend.send(
        backend.next_id(),
        Command::CloseDocument { doc: python_doc },
    )?;

    // Asserted on the settled set, not the first payload that drops one source:
    // layers are cleared one at a time, each republishing, so an intermediate
    // payload can still carry a provider that is about to go.
    let after = await_sources(&mut events, notes_doc, |sources| sources == ["server-rust"])
        .await
        .ok_or("the retired providers' markers were never cleared")?;
    assert_eq!(
        after,
        ["server-rust"],
        "retiring a provider must clear its own layer and leave every other alone"
    );
    Ok(())
}

/// Restarting one provider leaves every other provider running.
///
/// `Command::RestartLanguageServer` had no test at all, and it retired *every*
/// slot: restarting one server silently restarted all of them, and uninstalling
/// Ruff stopped rust-analyzer as a side effect. The markers are the visible
/// consequence -- a provider nobody asked about lost its own, and only got them
/// back if it happened to republish.
///
/// Falsified by (each independently):
/// - `retire_matching(|_| true)` in `LspManager::restart`: the untouched
///   provider's markers go too.
/// - reopening by language rather than by the documents the retirement
///   detached: these providers are configured, not built-in, so nothing matches
///   and the restarted provider never comes back.
#[tokio::test]
async fn restarting_one_provider_leaves_the_others_running() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = crate::lsp::absolute_path(dir.path());
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "plain text\n")?;
    let rust = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let python = dir.path().join("main.py");
    std::fs::write(&python, "x = 1\n")?;

    let mut settings = LspSettings::default();
    for (language, command) in [("rust", "server-rust"), ("python", "server-python")] {
        settings.servers.insert(
            language.to_owned(),
            crate::config::schema::LspServer {
                command: command.to_owned(),
                ..crate::config::schema::LspServer::default()
            },
        );
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.clone()],
        settings: crate::config::Settings {
            lsp: settings,
            ..crate::config::Settings::default()
        },
        ..SessionConfig::default()
    });
    session.set_lsp_connector(marking_connector(crate::lsp::absolute_path(&notes), None));
    let backend = local_session(session, None);

    let notes_doc = open(&backend, &mut events, &notes)
        .await
        .ok_or("notes.txt never opened")?;
    let _rust_doc = open(&backend, &mut events, &rust)
        .await
        .ok_or("main.rs never opened")?;
    let _python_doc = open(&backend, &mut events, &python)
        .await
        .ok_or("main.py never opened")?;

    await_sources(&mut events, notes_doc, |sources| {
        sources.iter().any(|s| s == "server-rust") && sources.iter().any(|s| s == "server-python")
    })
    .await
    .ok_or("both providers never marked notes.txt")?;

    backend.send(
        backend.next_id(),
        Command::RestartLanguageServer {
            server: LanguageServerId::new("rust"),
        },
    )?;

    // Asserted on the retirement *report*, not on a payload of markers. Under
    // the defect this guards against both providers' markers vanish and then
    // come back, so any assertion about markers here is a race -- and a test
    // that races is one that can pass against a broken implementation, which is
    // the failure this issue exists to stop repeating. (Written that way first,
    // and it did pass the mutation.) `adopt_retirement` emits exactly one `Idle`
    // per slot it retires, so "was Python retired too?" has a direct answer.
    let mut rust_returned = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !rust_returned && tokio::time::Instant::now() < deadline {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { server, state, .. } = event {
            assert!(
                !(server.key() == "python" && state == LanguageServerRuntimeState::Idle),
                "restarting rust retired python as well"
            );
            rust_returned = server.key() == "rust" && state == LanguageServerRuntimeState::Running;
        }
    }
    assert!(
        rust_returned,
        "the restarted provider never came back -- the reopen matched no documents"
    );
    Ok(())
}

/// A retirement signals that the inventory is stale, and the inventory a client
/// then asks for no longer offers a Restart.
///
/// This is the half of the acceptance criterion the per-transition event cannot
/// reach. That event carries state, and a client patching state field by field
/// ends with `runtime: Idle` beside whatever `open_documents` it was last told
/// -- and `restartable` is true if *either* says the provider is alive, so the
/// button outlives the process. Re-reading the inventory settles both at once.
///
/// The signal is deliberately not the rows themselves: building an inventory
/// probes `PATH` and the install journal per provider and root, and most
/// sessions have nobody looking at the result. The session says the cache is
/// wrong; asking is the client's move.
///
/// Falsified by: dropping `self.lsp_inventory_stale = true` from
/// `adopt_retirement`. The `Idle` transition still arrives, nothing announces
/// that the count changed, and a client that patches state alone keeps the dead
/// Restart.
#[tokio::test]
async fn retiring_a_provider_reports_an_inventory_that_offers_no_restart() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let root = crate::lsp::absolute_path(dir.path());
    let (session, mut events) = session_rooted_at(
        &root,
        test_connector(Behavior::Normal, None, Arc::new(AtomicUsize::new(0))),
    );
    let backend = local_session(session, None);

    let doc = open(&backend, &mut events, &path)
        .await
        .ok_or("main.rs never opened")?;
    let serving = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing while serving")?;
    assert!(
        serving.restartable(),
        "a serving provider is what Restart is for"
    );

    backend.send(backend.next_id(), Command::CloseDocument { doc })?;

    // Unsolicited: nothing after this point asks for anything.
    let mut signalled = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !signalled && tokio::time::Instant::now() < deadline {
        let Some((id, event)) = next_event(&mut events).await else {
            break;
        };
        if matches!(event, Event::LanguageServerInventoryStale) {
            assert_eq!(id, None, "a staleness signal answers no request");
            signalled = true;
        }
    }
    assert!(
        signalled,
        "the retirement never told the client its inventory was stale"
    );

    // What the client gets when it takes the session up on it.
    let current = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing after the retirement")?;
    assert_eq!(current.open_documents, 0);
    assert_eq!(current.runtime, LanguageServerRuntimeState::Idle);
    assert!(
        !current.restartable(),
        "the panel would still offer a Restart for a provider with no process"
    );
    Ok(())
}

/// The inventory a client reads after a restart still counts the open document.
///
/// This is what a *signal* buys over a pushed snapshot, and why the fix for the
/// dead-Restart defect is the signal rather than the push. `restart` is a
/// retirement followed by a reopen, and between the two the provider has no
/// process and no documents. A snapshot built at the retirement describes that
/// gap, and nothing afterwards corrects it -- the transition event carries
/// `runtime` and `error` only, and `open_documents` has no event at all, so the
/// panel would read "0 document(s)" for a serving provider until the user
/// pressed Refresh. A signal cannot go stale that way: the client answers it
/// with a fresh command, and the actor runs commands to completion in order, so
/// the answer necessarily describes the settled state.
///
/// Falsified by: dropping `reopen_documents_at` from `Session::restart_lsp`.
/// The provider is then retired and never comes back, and `open_documents`
/// reads 0.
#[tokio::test]
async fn a_restart_reports_the_document_it_reopened() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "fn main() {}\n").ok_or("write failed")?;
    let root = crate::lsp::absolute_path(dir.path());
    let (session, mut events) = session_rooted_at(
        &root,
        test_connector(Behavior::Normal, None, Arc::new(AtomicUsize::new(0))),
    );
    let backend = local_session(session, None);

    let _doc = open(&backend, &mut events, &path)
        .await
        .ok_or("main.rs never opened")?;
    let serving = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing while serving")?;
    assert_eq!(serving.open_documents, 1);

    backend.send(
        backend.next_id(),
        Command::RestartLanguageServer {
            server: LanguageServerId::RustAnalyzer,
        },
    )?;

    let mut signalled = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !signalled && tokio::time::Instant::now() < deadline {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        signalled = matches!(event, Event::LanguageServerInventoryStale);
    }
    assert!(
        signalled,
        "the restart never signalled that the cache was stale"
    );

    // The client takes the session up on it, which is the whole point of the
    // signal landing where it does.
    let after = inventory(&backend, &mut events)
        .await
        .and_then(|servers| instance(&servers, "rust-analyzer", &root))
        .ok_or("rust-analyzer missing after the restart")?;
    assert_eq!(
        after.open_documents, 1,
        "the restarted provider was reported with no documents attached"
    );
    assert!(
        after.restartable(),
        "a provider that came back is still what Restart is for"
    );
    Ok(())
}

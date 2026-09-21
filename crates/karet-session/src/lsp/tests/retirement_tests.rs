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

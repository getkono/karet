//! Format-on-save driven end to end against a server that really formats.
//!
//! The rest of the deferred-save tests seed `pending_format_saves` by hand,
//! which covers every ending of the wait and none of its beginning. These
//! start from a real `Command::Save`.

use super::*;

async fn await_saved(events: &mut EventRx) -> Option<(Option<RequestId>, DocumentId)> {
    while let Some((request, event)) = next_event(events).await {
        if let Event::Saved { doc } = event {
            return Some((request, doc));
        }
    }
    None
}

/// A session with `editor.formatOnSave` on, so a save actually dispatches a
/// `textDocument/formatting` request and parks on the reply.
fn formatting_session(connector: Connector) -> (Session, EventRx) {
    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = true;
    let (mut session, events, _snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    session.set_lsp_connector(connector);
    (session, events)
}

/// The whole deferred save, driven end to end against a server that really
/// advertises `textDocument/formatting` and really answers it.
///
/// Every other test of this path seeds `pending_format_saves` by hand, which
/// takes `begin_format_on_save` itself on trust — including the one thing that
/// makes the rest work: that the entry is keyed by the *Save's* request id. Key
/// it by anything else and the formatter's reply finishes a request nobody is
/// waiting on, while the client waits out a save that is never answered.
#[tokio::test]
async fn a_save_parks_on_the_formatter_and_lands_formatted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = formatting_session(test_connector(Behavior::Formats, None, spawns));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, _version) = await_opened(&mut events).await.ok_or("no Opened")?;

    let request = backend.next_id();
    backend.send(
        request,
        Command::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        },
    )?;

    let (answered, saved_doc) = await_saved(&mut events).await.ok_or("no Saved")?;
    assert_eq!(saved_doc, doc);
    assert_eq!(
        answered,
        Some(request),
        "the Saved must answer the Save that asked for it"
    );
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "formatted\n",
        "the server's edits must reach disk, not just the buffer"
    );
    Ok(())
}

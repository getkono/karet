//! Format-on-save driven end to end against a server that really formats.
//!
//! The rest of the deferred-save tests seed `pending_format_saves` by hand,
//! which covers every ending of the wait and none of its beginning. These
//! start from a real `Command::Save`.

use serde_json::Value;
use tokio::sync::mpsc;

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

/// [`formatting_session`] with its swaps redirected into `swap_dir` (never the
/// user's real data directory) and `files.backup` set explicitly.
fn formatting_session_with_swaps(
    connector: Connector,
    swap_dir: &Path,
    backup: bool,
) -> (Session, EventRx) {
    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = true;
    settings.files.backup = backup;
    let (mut session, events, _snaps) = Session::new(SessionConfig {
        settings,
        swap_dir: Some(swap_dir.to_path_buf()),
        ..SessionConfig::default()
    });
    session.set_lsp_connector(connector);
    (session, events)
}

/// The `Opened` already sitting on the event stream.
fn opened_doc(events: &mut EventRx) -> Option<DocumentId> {
    let mut found = None;
    while let Some((_, event)) = events.try_recv() {
        if let Event::Opened { doc, .. } = event {
            found = Some(doc);
        }
    }
    found
}

/// Whether a `Saved` is sitting on the event stream.
fn saved_now(events: &mut EventRx) -> bool {
    let mut found = false;
    while let Some((_, event)) = events.try_recv() {
        found |= matches!(event, Event::Saved { .. });
    }
    found
}

/// Open `path`, replace its first line with `edited\n`, and save — driving the
/// session by hand rather than through the backend actor.
///
/// Nothing here is `.await`ed, so on the current-thread test runtime the server
/// task never gets to run: the save is still parked on a formatting reply that
/// cannot have arrived when the caller makes its assertions. That is the window
/// the swap has to exist in, and the only way to observe it — let the reply
/// land and the save that follows it removes the swap again.
fn park_a_save(session: &mut Session, events: &mut EventRx, path: &Path) -> Option<DocumentId> {
    session.handle(
        RequestId(1),
        Command::OpenDocument {
            path: path.to_path_buf(),
            language: None,
        },
    );
    let doc = opened_doc(events)?;
    let change = Change::new(
        0,
        vec![TextEdit {
            range: Range {
                start: karet_core::LineCol::new(0, 0),
                end: karet_core::LineCol::new(1, 0),
            },
            new_text: "edited\n".to_string(),
        }],
    );
    session.handle(
        RequestId(2),
        Command::ApplyChange {
            doc,
            change,
            cause: EditCause::Replace,
        },
    );
    session.handle(
        RequestId(3),
        Command::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        },
    );
    Some(doc)
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

/// A save that parks on a formatter has deferred its only write to disk. The
/// buffer's sole other copy for the length of that wait is the swap — and the
/// backup interval would not have written one: this buffer has been dirty for
/// microseconds, not the thirty seconds `files.backupInterval` asks for. The
/// force-quit path tells the user an abandoned save is recoverable from
/// backups, which is true only if the swap is written when the wait begins.
#[tokio::test]
async fn a_deferred_save_writes_a_swap_before_the_formatter_answers() -> TestResult {
    let dir = tempfile::tempdir()?;
    let swapdir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (mut session, mut events) = formatting_session_with_swaps(
        test_connector(Behavior::Formats, None, spawns),
        swapdir.path(),
        true,
    );

    let _doc = park_a_save(&mut session, &mut events, &path).ok_or("no Opened")?;

    assert!(
        !saved_now(&mut events),
        "the save must still be parked on the formatter"
    );
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "original\n",
        "the deferred write has not happened yet — that is the window under test"
    );
    assert_eq!(
        crate::backup::scan(swapdir.path()).len(),
        1,
        "a deferred save must leave a recoverable copy behind it"
    );
    Ok(())
}

/// The swap is only worth having if it holds what the user asked to save. It is
/// written from the buffer as it stands when the save is issued — before the
/// formatter has been anywhere near it — so it is the unsaved edits, not the
/// stale text still on disk.
#[tokio::test]
async fn the_swap_behind_a_deferred_save_holds_the_unsaved_buffer() -> TestResult {
    let dir = tempfile::tempdir()?;
    let swapdir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (mut session, mut events) = formatting_session_with_swaps(
        test_connector(Behavior::Formats, None, spawns),
        swapdir.path(),
        true,
    );

    let _doc = park_a_save(&mut session, &mut events, &path).ok_or("no Opened")?;

    let swaps = crate::backup::scan(swapdir.path());
    let record = swaps.first().ok_or("no swap written")?;
    assert_eq!(
        record.content, "edited\n",
        "the swap must hold the buffer the save was asked to write"
    );
    assert_eq!(record.meta.original, path);
    Ok(())
}

/// `files.backup = false` is the user saying they do not want swap files, and a
/// deferred save is not an exception to it. The save itself is unaffected: it
/// still parks, still takes the formatter's edits, and still lands.
#[tokio::test]
async fn a_deferred_save_writes_no_swap_when_backups_are_off() -> TestResult {
    let dir = tempfile::tempdir()?;
    let swapdir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events) = formatting_session_with_swaps(
        test_connector(Behavior::Formats, None, spawns),
        swapdir.path(),
        false,
    );
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

    let (_answered, saved_doc) = await_saved(&mut events).await.ok_or("no Saved")?;
    assert_eq!(saved_doc, doc);
    assert_eq!(
        std::fs::read_to_string(&path)?,
        "formatted\n",
        "switching backups off must not cost the formatting or the save"
    );
    assert!(
        crate::backup::scan(swapdir.path()).is_empty(),
        "backups are off: no swap, deferred save or not"
    );
    Ok(())
}

/// A server is free to take `textDocument/formatting` and never answer it. The
/// server task awaits that reply inline in its serial command loop, so until it
/// returns this server publishes no diagnostics, answers no completions and
/// flushes no `didChange`. Left to `karet-jsonrpc` the wait is thirty seconds —
/// twenty of them past the point the save gave up and went to disk unformatted.
///
/// The clock is tokio's virtual one (`start_paused`), so this measures the
/// bound rather than waiting it out: idle time is advanced to the next timer, and
/// the assertion is on *which* timer that turns out to be.
#[tokio::test(start_paused = true)]
async fn a_formatter_that_never_answers_does_not_wedge_its_server() -> TestResult {
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::FormatsNever,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/wedged.rs");
    manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    assert!(
        manager.formatting(
            Some("rust"),
            RequestId(1),
            DocumentId(1),
            1,
            &path,
            karet_lsp::Indentation::default(),
        ),
        "the request must reach a server for this to test anything"
    );

    let started = tokio::time::Instant::now();
    let answer = loop {
        match updates.recv().await {
            Some(LspUpdate::Formatting {
                formatted,
                edits,
                request,
                ..
            }) => {
                break (formatted, edits, request);
            },
            Some(_other) => continue,
            None => return Err("the server task ended without answering".into()),
        }
    };
    let waited = started.elapsed();

    assert_eq!(answer.2, RequestId(1));
    assert!(
        !answer.0 && answer.1.is_empty(),
        "giving up must report the shape every other non-success ending uses"
    );
    assert!(
        waited <= crate::lsp::runtime::FORMATTING_DEADLINE,
        "the wait must be bounded by the formatting deadline, not by the \
         JSON-RPC request timeout (waited {waited:?})"
    );

    // And the task is genuinely back: the next command for this server is served
    // by the same connection, which was never declared dead.
    assert!(manager.completion(
        Some("rust"),
        RequestId(2),
        DocumentId(1),
        1,
        &path,
        karet_core::LineCol::new(0, 0),
    ));
    loop {
        match updates.recv().await {
            Some(LspUpdate::Completions { items, .. }) => {
                assert_eq!(items.len(), 1, "the server is still answering");
                break;
            },
            Some(_other) => continue,
            None => return Err("the server task was left wedged on the formatter".into()),
        }
    }
    Ok(())
}

/// The `FormattingOptions` the client sent with the last `textDocument/formatting`
/// it issued, out of everything the scripted server received.
fn formatting_options_sent(observed: &mut mpsc::UnboundedReceiver<Value>) -> Option<Value> {
    let mut found = None;
    while let Ok(message) = observed.try_recv() {
        if message["method"] == "textDocument/formatting" {
            found = Some(message["params"]["options"].clone());
        }
    }
    found
}

/// The request has to carry the *user's* indentation.
///
/// `FormattingOptions` is not advisory: it is the only statement of how this
/// buffer is indented that a whole-document format gets, and a server that
/// honours it (clangd with no `.clang-format`, jdtls, lua-language-server,
/// omnisharp) reindents the entire file to match. Built from a constant, the
/// request said four-spaces on every save no matter what the user configured —
/// so an `insertSpaces: false` project was silently re-spaced by its own editor
/// every Ctrl+S. The per-language override is asserted too, because that is the
/// layer that gets lost first: the resolution has to happen against the
/// document's selector, not against the global editor settings.
#[tokio::test]
async fn a_formatting_request_carries_the_configured_indentation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let (observed, mut received) = mpsc::unbounded_channel();

    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = true;
    settings.editor.tab_size = 2;
    settings.editor.insert_spaces = false;
    if let Some(selector) = crate::config::schema::LanguageSelector::from_language("rust") {
        settings.editor.language_overrides.insert(
            selector,
            crate::config::schema::EditorOverride {
                tab_size: Some(3),
                ..crate::config::schema::EditorOverride::default()
            },
        );
    }
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    session.set_lsp_connector(test_connector(
        Behavior::Formats,
        Some(observed),
        Arc::new(AtomicUsize::new(0)),
    ));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, _version) = await_opened(&mut events).await.ok_or("no Opened")?;
    backend.send(
        backend.next_id(),
        Command::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        },
    )?;
    await_saved(&mut events).await.ok_or("no Saved")?;

    let options = formatting_options_sent(&mut received).ok_or("no formatting request observed")?;
    assert_eq!(
        options["tabSize"],
        json!(3),
        "the per-language tabSize override must reach the server, not the \
         global 2 and certainly not the hard-coded 4"
    );
    assert_eq!(
        options["insertSpaces"],
        json!(false),
        "a tabs-indented project must not be re-spaced by its own editor"
    );
    Ok(())
}

/// `.editorconfig` is the layer that decides how a file is indented.
///
/// Every other consumer reads the document's *resolved* settings, which
/// `editorconfig::resolve` produces by overlaying each matching
/// `.editorconfig` on top of the language defaults — the editor types by it,
/// and the save path already trims and terminates by it. Resolving the
/// formatting request one layer lower would tell the server the opposite of
/// what the buffer believes, and a server that honours `FormattingOptions`
/// would then reindent the whole file against the project's own rules on
/// every save.
#[tokio::test]
async fn a_formatting_request_carries_the_editorconfig_indentation() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join(".editorconfig"),
        "root = true\n\n[*.rs]\nindent_style = tab\ntab_width = 8\n",
    )?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let (observed, mut received) = mpsc::unbounded_channel();

    // Settings say the opposite of the file: two-column spaces. The
    // `.editorconfig` overlay has to win, or it is not a layer at all.
    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = true;
    settings.editor.tab_size = 2;
    settings.editor.insert_spaces = true;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    session.set_lsp_connector(test_connector(
        Behavior::Formats,
        Some(observed),
        Arc::new(AtomicUsize::new(0)),
    ));
    let backend = local_session(session, None);

    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.clone(),
            language: None,
        },
    )?;
    let (doc, _version) = await_opened(&mut events).await.ok_or("no Opened")?;
    backend.send(
        backend.next_id(),
        Command::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        },
    )?;
    await_saved(&mut events).await.ok_or("no Saved")?;

    let options = formatting_options_sent(&mut received).ok_or("no formatting request observed")?;
    assert_eq!(
        options["insertSpaces"],
        json!(false),
        "`indent_style = tab` must reach the formatter, not the settings layer's spaces"
    );
    assert_eq!(
        options["tabSize"],
        json!(8),
        "with tabs the width the server is told is the tab stop, not the indent step"
    );
    Ok(())
}

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
/// the assertion is on *which* timer that turns out to be. The save asks for a
/// two-second `formatOnSaveTimeout`, well short of the ten-second default, so a
/// task that ignored the one it was handed would overrun the assertion.
#[tokio::test(start_paused = true)]
async fn a_formatter_that_never_answers_does_not_wedge_its_server() -> TestResult {
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::FormatsNever,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    let path = PathBuf::from("/tmp/wedged.rs");
    // No retirement to adopt: this manager has one slot and nothing has replaced
    // it, and the test asserts on the task's liveness rather than on any layer.
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, 1, || {
        "fn main() {}".into()
    });
    let save_timeout = Duration::from_secs(2);
    assert!(
        manager.formatting(
            Some("rust"),
            RequestId(1),
            DocumentId(1),
            1,
            &path,
            crate::lsp::FormattingAsk {
                save_timeout,
                ..crate::lsp::FormattingAsk::default()
            },
        ),
        "the request must reach a server for this to test anything"
    );

    let started = tokio::time::Instant::now();
    // Bounded, on the same virtual clock: the manager holds a sender, so a task
    // that is gone leaves this channel open and silent, and an unbounded wait
    // would hang the suite rather than fail it.
    let answer = loop {
        let next = tokio::time::timeout(Duration::from_secs(60), updates.recv())
            .await
            .map_err(|_elapsed| "the server task never answered")?;
        match next {
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
        waited <= crate::lsp::runtime::formatting_deadline(save_timeout),
        "the wait must be bounded by the save's own timeout, not by the \
         default or the JSON-RPC request timeout (waited {waited:?})"
    );

    // And the task is genuinely back: the next command for this server is served
    // by the same connection, which was never declared dead.
    assert!(manager.completion(
        Some("rust"),
        RequestId(2),
        DocumentId(1),
        1,
        &path,
        (
            karet_core::LineCol::new(0, 0),
            karet_core::CompletionTrigger::Invoked
        ),
    ));
    loop {
        match updates.recv().await {
            Some(LspUpdate::Completions { list, .. }) => {
                assert_eq!(list.items.len(), 1, "the server is still answering");
                break;
            },
            Some(_other) => continue,
            None => return Err("the server task was left wedged on the formatter".into()),
        }
    }
    Ok(())
}

/// A `Command::Save` hands the server task the configured
/// `editor.formatOnSaveTimeout`, not the default.
///
/// The test above calls `LspManager::formatting` directly, which takes
/// `begin_format_on_save` building the request on trust. This one starts from
/// the save. The session's own sweep reads a wall clock that the paused tokio
/// clock does not advance, so here only the server task can give up. It must do
/// so within the deadline derived from the configured 2.5 s. A task handed the
/// 10 s default would wait 13 s and overrun the assertion.
#[tokio::test(start_paused = true)]
async fn a_save_hands_the_server_the_configured_format_timeout() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\n").ok_or("write failed")?;
    let save_timeout = Duration::from_millis(2_500);

    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = true;
    settings.editor.format_on_save_timeout = 2_500;
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    session.set_lsp_connector(test_connector(
        Behavior::FormatsNever,
        None,
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
    let started = tokio::time::Instant::now();
    backend.send(
        backend.next_id(),
        Command::Save {
            doc,
            cause: crate::api::SaveCause::Manual,
        },
    )?;
    let (_request, saved) = tokio::time::timeout(Duration::from_secs(60), await_saved(&mut events))
        .await
        .map_err(|_elapsed| "the save never landed")?
        .ok_or("no Saved")?;
    let waited = started.elapsed();

    assert_eq!(saved, doc);
    assert!(
        waited >= save_timeout,
        "the save must wait for the formatter before giving up (waited {waited:?})"
    );
    assert!(
        waited <= crate::lsp::runtime::formatting_deadline(save_timeout),
        "the server task must give up at the configured timeout, not the \
         default (waited {waited:?})"
    );
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

// ---- Format Document and Format Selection: `Command::Format`, asked for by hand.

/// A session that does not format on save, so only a `Format` formats, with
/// its snapshot stream kept so a test can watch the text change.
fn manual_session(connector: Option<Connector>) -> (Session, EventRx, crate::local::SnapshotRx) {
    let mut settings = crate::config::Settings::default();
    settings.editor.format_on_save = false;
    let (mut session, events, snaps) = Session::new(SessionConfig {
        settings,
        ..SessionConfig::default()
    });
    if let Some(connector) = connector {
        session.set_lsp_connector(connector);
    }
    (session, events, snaps)
}

fn format_command(doc: DocumentId, scope: crate::api::FormatScope) -> Command {
    Command::Format(crate::api::FormatRequest { doc, scope })
}

fn caret(line: u32, col: u32) -> Range {
    let at = karet_core::LineCol::new(line, col);
    Range { start: at, end: at }
}

/// Whether a snapshot of `doc` holding exactly `text` arrives, bounded.
async fn await_text(snaps: &mut crate::local::SnapshotRx, doc: DocumentId, text: &str) -> bool {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), snaps.recv()).await {
            Ok(Some((id, snap))) if id == doc && snap.buffer.text() == text => return true,
            Ok(Some(_)) => {},
            _ => return false,
        }
    }
}

/// Open `path` on a backend running `session`, returning the backend and the
/// opened document.
async fn open_on_backend(
    session: Session,
    events: &mut EventRx,
    path: &Path,
) -> Result<(crate::backend::LocalBackend, DocumentId), Box<dyn std::error::Error + Send + Sync>> {
    let backend = local_session(session, None);
    backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: path.to_path_buf(),
            language: None,
        },
    )?;
    let (doc, _version) = await_opened(events).await.ok_or("no Opened")?;
    Ok((backend, doc))
}

/// Format Document puts the server's edits in the buffer, and only there: it
/// is an edit like any other, so the file on disk waits for a save.
#[tokio::test]
async fn format_document_applies_the_servers_edits_to_the_buffer_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "original\nkept\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events, mut snaps) =
        manual_session(Some(test_connector(Behavior::Formats, None, spawns)));
    let (backend, doc) = open_on_backend(session, &mut events, &path).await?;

    backend.send(
        backend.next_id(),
        format_command(doc, crate::api::FormatScope::Document),
    )?;

    assert!(
        await_text(&mut snaps, doc, "formatted\nkept\n").await,
        "the server's edit must reach the buffer"
    );
    assert_eq!(std::fs::read_to_string(&path)?, "original\nkept\n");
    Ok(())
}

/// Format Selection asks for each selected stretch once: a bare caret stands
/// for its line, two carets on one line are one request, and every answer
/// lands together, against the text the requests were made for.
#[tokio::test]
async fn format_selection_formats_each_selected_stretch_once() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "aaa\nbbb\nccc\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (observed_tx, mut observed) = mpsc::unbounded_channel();
    let (session, mut events, mut snaps) = manual_session(Some(test_connector(
        Behavior::Normal,
        Some(observed_tx),
        spawns,
    )));
    let (backend, doc) = open_on_backend(session, &mut events, &path).await?;

    let selection = Range {
        start: karet_core::LineCol::new(2, 0),
        end: karet_core::LineCol::new(2, 3),
    };
    backend.send(
        backend.next_id(),
        format_command(
            doc,
            crate::api::FormatScope::Ranges(vec![caret(0, 1), selection, caret(0, 2)]),
        ),
    )?;

    assert!(
        await_text(&mut snaps, doc, "formatted\nbbb\nformatted\n").await,
        "both selected lines must be formatted, and nothing between them"
    );
    let mut asked = Vec::new();
    while let Ok(message) = observed.try_recv() {
        if message["method"] == "textDocument/rangeFormatting" {
            asked.push(message["params"]["range"]["start"]["line"].clone());
        }
    }
    assert_eq!(asked, vec![json!(0), json!(2)], "one request per stretch");
    Ok(())
}

/// The next `FeatureUnsupported` on the stream, with the request it answers.
async fn await_unsupported(
    events: &mut EventRx,
) -> Option<(Option<RequestId>, karet_core::ServerFeature)> {
    while let Some((request, event)) = next_event(events).await {
        if let Event::FeatureUnsupported { feature, .. } = event {
            return Some((request, feature));
        }
    }
    None
}

/// A server that does not offer the method is named, on the request that
/// asked -- the user pressed a key and is owed a reason nothing changed.
#[tokio::test]
async fn a_format_the_server_does_not_offer_says_so() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "main.rs", "aaa\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events, _snaps) =
        manual_session(Some(test_connector(Behavior::Bare, None, spawns)));
    let (backend, doc) = open_on_backend(session, &mut events, &path).await?;

    let request = backend.next_id();
    backend.send(
        request,
        format_command(doc, crate::api::FormatScope::Ranges(vec![caret(0, 0)])),
    )?;
    assert_eq!(
        await_unsupported(&mut events).await,
        Some((Some(request), karet_core::ServerFeature::RangeFormatting))
    );

    let request = backend.next_id();
    backend.send(
        request,
        format_command(doc, crate::api::FormatScope::Document),
    )?;
    assert_eq!(
        await_unsupported(&mut events).await,
        Some((Some(request), karet_core::ServerFeature::Formatting))
    );
    Ok(())
}

/// The notification answering `request`, if one is sitting on the stream.
fn notice_for(events: &mut EventRx, request: RequestId) -> Option<String> {
    let mut found = None;
    while let Some((id, event)) = events.try_recv() {
        if let (Some(id), Event::Notification { message, .. }) = (id, event)
            && id == request
        {
            found = Some(message);
        }
    }
    found
}

/// With nothing to format a file -- no server, and no built-in formatter for
/// its language -- a Format says so instead of silently doing nothing.
#[test]
fn a_format_with_no_formatter_says_so() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", "aaa\n").ok_or("write failed")?;
    let (mut session, mut events, _snaps) = manual_session(None);
    session.handle(
        RequestId(1),
        Command::OpenDocument {
            path,
            language: None,
        },
    );
    let doc = opened_doc(&mut events).ok_or("no Opened")?;

    session.handle(
        RequestId(2),
        format_command(doc, crate::api::FormatScope::Document),
    );
    assert_eq!(
        notice_for(&mut events, RequestId(2)).as_deref(),
        Some("format: nothing formats this file")
    );
    session.handle(
        RequestId(3),
        format_command(doc, crate::api::FormatScope::Ranges(vec![caret(0, 0)])),
    );
    assert_eq!(
        notice_for(&mut events, RequestId(3)).as_deref(),
        Some("format selection: nothing formats selections in this file")
    );
    Ok(())
}

/// TOML has a built-in formatter, and Format Document uses it when no server
/// formats the file -- exactly as a save would.
#[cfg(feature = "toml-format")]
#[tokio::test]
async fn format_document_falls_back_on_the_builtin_toml_formatter() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "Cargo.toml", "[package]\nname=\"x\"\n").ok_or("write failed")?;
    let spawns = Arc::new(AtomicUsize::new(0));
    let (session, mut events, mut snaps) = manual_session(Some(failing_connector(spawns)));
    let (backend, doc) = open_on_backend(session, &mut events, &path).await?;

    backend.send(
        backend.next_id(),
        format_command(doc, crate::api::FormatScope::Document),
    )?;

    assert!(await_text(&mut snaps, doc, "[package]\nname = \"x\"\n").await);
    Ok(())
}

/// Edits computed against text the user has since changed describe text that
/// is no longer there: they are dropped, and the request is told why. A save's
/// late answer, by contrast, is never mistaken for a Format.
#[test]
fn a_format_answer_for_a_superseded_version_is_dropped() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = rust_file(&dir, "notes.txt", "aaa\n").ok_or("write failed")?;
    let (mut session, mut events, _snaps) = manual_session(None);
    session.handle(
        RequestId(1),
        Command::OpenDocument {
            path,
            language: None,
        },
    );
    let doc = opened_doc(&mut events).ok_or("no Opened")?;
    let edit = |new_text: &str| TextEdit {
        range: Range {
            start: karet_core::LineCol::new(0, 0),
            end: karet_core::LineCol::new(0, 3),
        },
        new_text: new_text.to_owned(),
    };
    let stale = session
        .document(doc)
        .map(|d| d.version())
        .ok_or("no document")?;
    session.handle(
        RequestId(2),
        Command::ApplyChange {
            doc,
            change: Change::new(stale, vec![edit("bbb")]),
            cause: EditCause::Replace,
        },
    );
    let text = |session: &Session| session.document(doc).map(|d| d.buffer.text());

    session.apply_lsp_update(LspUpdate::Formatting {
        generation: 0,
        request: RequestId(3),
        doc,
        version: stale,
        formatted: true,
        edits: vec![edit("zzz")],
        scope: crate::lsp::FormattingScope::Document,
    });
    assert_eq!(text(&session).as_deref(), Some("bbb\n"));
    assert_eq!(
        notice_for(&mut events, RequestId(3)).as_deref(),
        Some("format: the document changed before the formatter answered; nothing was applied")
    );

    let current = session
        .document(doc)
        .map(|d| d.version())
        .ok_or("no document")?;
    session.apply_lsp_update(LspUpdate::Formatting {
        generation: 0,
        request: RequestId(4),
        doc,
        version: current,
        formatted: true,
        edits: vec![edit("zzz")],
        scope: crate::lsp::FormattingScope::Save,
    });
    assert_eq!(
        text(&session).as_deref(),
        Some("bbb\n"),
        "an answer for a save nobody is waiting on applies nothing"
    );
    Ok(())
}

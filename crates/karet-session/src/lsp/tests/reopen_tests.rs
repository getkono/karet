//! Which providers a reopen actually talks to.
//!
//! Split from `retirement_tests` because the subject is the *other* half of a
//! scoped retirement: not what goes away, but what must be left alone.

use super::retirement_tests::marking_connector;
use super::retirement_tests::open;
use super::*;

/// A provider that was not restarted is not told about its documents twice.
///
/// `didOpen` for a document a server already has open is a protocol error, and
/// scoping a restart to one provider is what makes it reachable: the language's
/// other providers keep running and keep their documents, and the reopen that
/// follows the restart fans out to all of them. The same is reachable without a
/// restart, when installing a provider reopens documents whose other providers
/// never went anywhere -- so this is a pre-existing hole that scoping widened.
///
/// Falsified by: discarding the `HashSet::insert` result in
/// `LspManager::document_opened` and sending `DidOpen` unconditionally, which is
/// what it did before. The companion then records two opens for one file.
#[tokio::test]
async fn a_provider_that_was_not_restarted_is_not_reopened() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = crate::lsp::absolute_path(dir.path());
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "plain text\n")?;
    let python = dir.path().join("main.py");
    std::fs::write(&python, "x = 1\n")?;

    // One document, two providers: a primary and a diagnostics companion.
    let mut settings = LspSettings::default();
    settings.languages.insert(
        "python".to_owned(),
        crate::config::schema::LspLanguage {
            servers: vec!["python".to_owned()],
            diagnostics: vec!["extra".to_owned()],
            ..crate::config::schema::LspLanguage::default()
        },
    );
    for (id, command) in [("python", "server-python"), ("extra", "server-extra")] {
        settings.servers.insert(
            id.to_owned(),
            crate::config::schema::LspServer {
                command: command.to_owned(),
                ..crate::config::schema::LspServer::default()
            },
        );
    }
    let (opened_tx, mut opened_rx) = mpsc::unbounded_channel();
    let (mut session, mut events, _snaps) = Session::new(SessionConfig {
        roots: vec![root.clone()],
        settings: crate::config::Settings {
            lsp: settings,
            ..crate::config::Settings::default()
        },
        ..SessionConfig::default()
    });
    session.set_lsp_connector(marking_connector(
        crate::lsp::absolute_path(&notes),
        Some(opened_tx),
    ));
    let backend = local_session(session, None);

    open(&backend, &mut events, &python)
        .await
        .ok_or("main.py never opened")?;

    // Both providers see the file once, before anything is restarted.
    let mut opens = Vec::new();
    while opens.len() < 2 {
        let source = tokio::time::timeout(Duration::from_secs(10), opened_rx.recv())
            .await?
            .ok_or("a provider never opened the document")?;
        opens.push(source);
    }
    opens.sort();
    assert_eq!(opens, ["server-extra", "server-python"]);

    backend.send(
        backend.next_id(),
        Command::RestartLanguageServer {
            server: LanguageServerId::new("python"),
        },
    )?;

    // The restarted provider opens it again on its fresh task. The companion,
    // which was never retired, must not: it still has the file open.
    let reopened = tokio::time::timeout(Duration::from_secs(10), opened_rx.recv())
        .await?
        .ok_or("the restarted provider never reopened the document")?;
    assert_eq!(reopened, "server-python");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), opened_rx.recv())
            .await
            .is_err(),
        "a provider that was not restarted was sent a second didOpen"
    );
    Ok(())
}

/// A document opened under a non-absolute path is still reopened by a restart.
///
/// A document is stored under the path the client opened it with; the manager
/// records what it sent the server, which is always absolute. Matching the two
/// as written meant `karet main.rs` -- the commonest invocation there is --
/// retired its provider on Restart and then found no documents to reopen it
/// with, leaving the server down for the rest of the session with nothing said.
///
/// The path here differs from its absolute form only by a `/./`, which
/// `std::path::absolute` removes. That reproduces the mismatch exactly without
/// depending on the process working directory, which is global to the test
/// binary and cannot be changed safely from one test.
///
/// Falsified by: matching on `document.path` instead of its absolute form in
/// `reopen_documents_at` -- the provider never reports `Running` again.
#[tokio::test]
async fn a_document_opened_by_an_unnormalised_path_is_still_reopened() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = crate::lsp::absolute_path(dir.path());
    let notes = dir.path().join("notes.txt");
    std::fs::write(&notes, "plain text\n")?;
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n")?;
    // The same file, named relatively. Built by walking up from the process
    // working directory rather than by changing it: the working directory is
    // global to the test binary and cannot be moved safely from one test.
    //
    // `..` is the key. `std::path::absolute` drops `.` components but keeps
    // `..`, and `Path`'s own equality normalises `.` away too -- so a `./`
    // spelling is invisible to both and reproduces nothing. A `../` one is
    // resolved by the kernel when opening the file and by nobody else, so the
    // stored path and the absolutised one really do differ.
    let cwd = std::env::current_dir()?;
    let mut relative = PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    let mut absolute_tail = root.components();
    absolute_tail.next();
    relative.extend(absolute_tail);
    let unnormalised = relative.join("main.rs");
    assert_ne!(
        unnormalised,
        crate::lsp::absolute_path(&unnormalised),
        "the test needs a path that differs from its absolute form"
    );

    let mut settings = LspSettings::default();
    settings.servers.insert(
        "rust".to_owned(),
        crate::config::schema::LspServer {
            command: "server-rust".to_owned(),
            ..crate::config::schema::LspServer::default()
        },
    );
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

    open(&backend, &mut events, &unnormalised)
        .await
        .ok_or("the document never opened")?;
    let mut running = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !running && tokio::time::Instant::now() < deadline {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            running = state == LanguageServerRuntimeState::Running;
        }
    }
    assert!(running, "the provider never started");

    backend.send(
        backend.next_id(),
        Command::RestartLanguageServer {
            server: LanguageServerId::new("rust"),
        },
    )?;

    let mut restarted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !restarted && tokio::time::Instant::now() < deadline {
        let Some((_, event)) = next_event(&mut events).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            restarted = state == LanguageServerRuntimeState::Running;
        }
    }
    assert!(
        restarted,
        "the provider was retired and never reopened: Restart killed it for good"
    );
    Ok(())
}

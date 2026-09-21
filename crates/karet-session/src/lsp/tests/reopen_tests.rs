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

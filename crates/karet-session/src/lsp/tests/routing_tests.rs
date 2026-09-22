//! Which slot a document's commands are addressed to, and when a provider that
//! could not be resolved is allowed to say so again.
//!
//! Both are single predicates that nothing else exercises: mutation testing
//! found each could be inverted or emptied with the whole suite still green.

use super::*;

/// A document is routed only to a slot that is *both* primary and holds it.
///
/// The two conditions look redundant and are not. A language's diagnostics
/// companion holds the same documents as its primary without being one, and a
/// primary holds only the documents it was told about -- so relaxing the pair to
/// either one alone sends a document's commands to a server that never opened
/// it, which is a protocol error, or to a companion that cannot answer them.
///
/// Falsified by: `slot.primary || slot.documents.contains(path)` in
/// `LspManager::existing_server`. The live primary then answers for a file it
/// has never been told about.
#[tokio::test]
async fn a_document_is_routed_only_to_a_primary_that_holds_it() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));

    let held = crate::lsp::absolute_path(Path::new("/tmp/held.rs"));
    let _ = manager.document_opened(Some("rust"), Some("rust"), &held, 1, || {
        "fn main() {}".into()
    });
    assert!(
        manager.existing_server(Some("rust"), &held).is_some(),
        "the slot that was told about this document did not answer for it"
    );

    // Same language, same live primary, a file it was never told about.
    let unknown = crate::lsp::absolute_path(Path::new("/tmp/unknown.rs"));
    assert!(
        manager.existing_server(Some("rust"), &unknown).is_none(),
        "a primary answered for a document it has never been told about"
    );
    Ok(())
}

/// Installing a provider lets karet say it is still missing, if it still is.
///
/// The missing-provider report is suppressed after the first one, so the user is
/// told once rather than per document opened. An install is the event that can
/// make it wrong: if resolution still fails afterwards, the user has to hear so,
/// and without clearing the suppression they never would.
///
/// Falsified by: replacing `LspManager::installed` with `()`, which mutation
/// testing found nothing objected to. The second report is then swallowed and a
/// provider that failed to install goes quiet.
#[tokio::test]
async fn installing_a_provider_lets_it_report_being_missing_again() -> TestResult {
    let (mut manager, mut updates) = LspManager::new(LspSettings::default(), None, None, None);

    let provider = LanguageServerId::RustAnalyzer;
    manager.report_unresolved(provider.clone(), "rust");
    // `try_recv`, never `recv().await`: the send is synchronous, so the update is
    // already there. Awaiting it would turn "the report was suppressed" into a
    // hang rather than a failure -- which is what mutation testing saw when this
    // test was first written.
    assert!(
        updates.try_recv().is_ok(),
        "an unresolved provider was never reported at all"
    );

    manager.report_unresolved(provider.clone(), "rust");
    assert!(
        updates.try_recv().is_err(),
        "a provider reported itself missing twice without an install in between"
    );

    manager.installed(provider.clone());
    manager.report_unresolved(provider, "rust");
    assert!(
        updates.try_recv().is_ok(),
        "a provider that failed to install went quiet instead of reporting again"
    );
    Ok(())
}

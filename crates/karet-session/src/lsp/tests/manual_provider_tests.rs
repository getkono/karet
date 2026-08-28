//! Which missing providers karet offers to install, and which it explains.
//!
//! These drive `report_unresolved` directly rather than through a document
//! open, because `spec_for` carries a `#[cfg(test)]` fallback that resolves
//! every built-in to its bare launch spec — under `cfg(test)` a built-in is
//! never unresolved, so the branch under test is unreachable from a session.

use super::*;

/// A manager with no workspace and no managed registry, which is the state in
/// which every built-in resolves to nothing.
fn manager() -> (LspManager, mpsc::UnboundedReceiver<LspUpdate>) {
    LspManager::new(LspSettings::default(), None, None, None)
}

#[test]
fn a_provider_karet_can_install_is_offered() {
    let (mut manager, mut updates) = manager();
    manager.report_unresolved(LanguageServerId::RustAnalyzer, "rust");
    let update = updates.try_recv().ok();
    assert!(
        matches!(update, Some(LspUpdate::InstallRequired { ref server, .. })
            if *server == LanguageServerId::RustAnalyzer),
        "rust-analyzer is managed, so karet should offer to install it"
    );
}

/// taplo's releases carry no publisher digest, so karet has no recipe for it.
/// It used to be offered anyway, and accepting produced an install that failed
/// with "taplo is available from the project or PATH but has no managed
/// installer".
#[test]
fn a_provider_karet_cannot_install_is_explained_instead_of_offered() -> TestResult {
    let (mut manager, mut updates) = manager();
    manager.report_unresolved(LanguageServerId::new("taplo"), "toml");
    let Some(LspUpdate::ManualInstallRequired {
        server,
        command,
        reason,
        ..
    }) = updates.try_recv().ok()
    else {
        return Err("taplo should report a manual install, not an offer".into());
    };
    assert_eq!(server, LanguageServerId::new("taplo"));
    assert_eq!(command, "taplo");
    assert!(reason.contains("SHA-256"), "{reason}");
    Ok(())
}

/// The notice names the executable to put on `PATH`, which for several
/// providers is not the provider's own id.
#[test]
fn the_manual_notice_names_the_executable_karet_looked_for() -> TestResult {
    let (mut manager, mut updates) = manager();
    manager.report_unresolved(LanguageServerId::new("haskell-language-server"), "haskell");
    let Some(LspUpdate::ManualInstallRequired { command, .. }) = updates.try_recv().ok() else {
        return Err("expected a manual-install report".into());
    };
    assert_eq!(command, "haskell-language-server-wrapper");
    Ok(())
}

#[test]
fn every_manual_builtin_reports_rather_than_offers() {
    for provider in catalog::builtin_providers() {
        let server = LanguageServerId::new(provider.key);
        let (mut manager, mut updates) = manager();
        manager.report_unresolved(server.clone(), "any");
        let offered = matches!(
            updates.try_recv().ok(),
            Some(LspUpdate::InstallRequired { .. })
        );
        assert_eq!(
            offered,
            crate::lsp_registry::managed_provider(&server),
            "{} is offered an install it cannot complete",
            provider.key
        );
    }
}

#[test]
fn a_provider_is_reported_once_per_generation() {
    let (mut manager, mut updates) = manager();
    manager.report_unresolved(LanguageServerId::new("taplo"), "toml");
    manager.report_unresolved(LanguageServerId::new("taplo"), "toml");
    assert!(updates.try_recv().is_ok());
    assert!(
        updates.try_recv().is_err(),
        "opening a second TOML file must not repeat the notice"
    );
}

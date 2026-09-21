//! What the inventory reports for a provider the user switched off.
//!
//! The distinction these pin is that settings select a provider by the id named
//! in `lsp.languages.<language>.servers`, or by the language's own name -- never
//! by a built-in provider id. Anything reporting on "disabled" has to use the same
//! rule the launch path does, or it disagrees with what actually runs.

use super::*;

/// A provider the user's settings actually suppress must report as disabled, not
/// as an available provider that simply has not been needed yet.
///
/// The inventory used to set `enabled` from the *global* `lsp.enabled` alone, and
/// fell through to the built-in launch table for a suppressed provider -- so it
/// advertised a command that would never be run, and the editor badge read that
/// as healthy-and-idle.
#[test]
fn a_suppressed_provider_reports_as_disabled() -> TestResult {
    let mut settings = LspSettings::default();
    settings.languages.insert(
        "rust".to_owned(),
        crate::config::schema::LspLanguage {
            servers: vec!["company-rust".to_owned()],
            ..crate::config::schema::LspLanguage::default()
        },
    );
    settings.servers.insert(
        "company-rust".to_owned(),
        crate::config::schema::LspServer {
            command: "company-rust".to_owned(),
            enabled: false,
            ..crate::config::schema::LspServer::default()
        },
    );
    let (manager, _updates) = LspManager::new(settings, None, None, None);
    let inventory = manager.inventory([PathBuf::from("/workspace/main.rs")]);
    let status = inventory
        .iter()
        .find(|status| status.server == LanguageServerId::new("company-rust"))
        .ok_or("the configured provider is missing from the inventory")?;

    assert!(
        !status.enabled,
        "a provider the user switched off read as enabled"
    );
    for instance in &status.instances {
        assert!(
            instance.command.is_none(),
            "a suppressed provider advertised a command it will never run"
        );
        assert_eq!(instance.source, LanguageServerSource::Unavailable);
    }
    Ok(())
}

/// The counterpart, and the reason this is asked through `configured_primary`: an
/// `lsp.servers` entry keyed by a *built-in provider id* is never read by the
/// launch path, so it must not be reported as disabling anything.
///
/// Reporting it as disabled made the inventory lie in the opposite direction --
/// a rust-analyzer that is spawned and serving, badged `off`, with its real
/// runtime state and error hidden from the manager.
#[test]
fn an_entry_the_launch_path_never_reads_does_not_disable_anything() -> TestResult {
    let mut settings = LspSettings::default();
    settings.servers.insert(
        LanguageServerId::RustAnalyzer.key().to_owned(),
        crate::config::schema::LspServer {
            command: "rust-analyzer".to_owned(),
            enabled: false,
            ..crate::config::schema::LspServer::default()
        },
    );
    let (manager, _updates) = LspManager::new(settings, None, None, None);
    let inventory = manager.inventory([PathBuf::from("/workspace/main.rs")]);
    let status = inventory
        .iter()
        .find(|status| status.server == LanguageServerId::RustAnalyzer)
        .ok_or("rust-analyzer is missing from the inventory")?;
    assert!(
        status.enabled,
        "an entry the launch path ignores was reported as disabling the provider"
    );
    Ok(())
}

/// An entry keyed by the *language's own name* suppresses the built-in provider
/// without ever naming it, and the inventory has to notice.
///
/// `lsp.servers.rust = { enabled = false }` with no `lsp.languages.rust` stops
/// rust-analyzer launching -- `configured_primary` falls back to the language name
/// -- so comparing the suppressed id against the provider id alone left a language
/// with no server at all reporting as healthy and idle.
#[test]
fn a_language_keyed_entry_disables_the_builtin_it_suppresses() -> TestResult {
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "rust".to_owned(),
        crate::config::schema::LspServer {
            command: "whatever".to_owned(),
            enabled: false,
            ..crate::config::schema::LspServer::default()
        },
    );
    let (manager, _updates) = LspManager::new(settings, None, None, None);
    let inventory = manager.inventory([PathBuf::from("/workspace/main.rs")]);
    let status = inventory
        .iter()
        .find(|status| status.server == LanguageServerId::RustAnalyzer)
        .ok_or("rust-analyzer is missing from the inventory")?;
    assert!(
        !status.enabled,
        "a language whose only provider is suppressed reported as having one"
    );
    for instance in &status.instances {
        assert!(
            instance.command.is_none(),
            "a suppressed provider advertised a command it will never run"
        );
    }
    Ok(())
}

/// The global switch keeps working, and does not depend on any per-provider entry.
#[test]
fn the_global_switch_disables_every_provider() {
    let settings = LspSettings {
        enabled: false,
        ..LspSettings::default()
    };
    let (manager, _updates) = LspManager::new(settings, None, None, None);
    let inventory = manager.inventory([PathBuf::from("/workspace/main.rs")]);
    assert!(!inventory.is_empty());
    assert!(
        inventory.iter().all(|status| !status.enabled),
        "the global switch left a provider reporting as enabled"
    );
}

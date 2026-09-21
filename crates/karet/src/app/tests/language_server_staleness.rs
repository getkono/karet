//! What an inventory *staleness signal* does to the manager tab.
//!
//! The session sends one whenever it retires a provider: the document count has
//! no event of its own, so a client patching rows field by field ends up
//! offering a Restart for a process that is gone. The signal carries no rows --
//! building them is expensive and most sessions have nobody looking -- so the
//! client's answer to it is a fresh request, and the interesting case is what
//! happens to the older answer still in flight.
//!
//! Its own file: `app/tests/language_servers.rs` has a handful of code lines
//! left under the workspace ceiling.

use super::support::*;
use crate::app::*;

/// A provider with a live instance, which is what a Restart is offered for.
///
/// Local rather than shared: `support::language_server_status` reports two open
/// documents and a different argv, and these assertions are about a single
/// serving instance.
fn serving(server: LanguageServerId) -> LanguageServerStatus {
    LanguageServerStatus {
        server,
        languages: vec!["rust".to_owned()],
        enabled: true,
        managed: true,
        manual_install_reason: None,
        installed: Some("1.2.3".to_owned()),
        ever_installed: true,
        declined: false,
        cleanup_pending: false,
        instances: vec![karet_session::LanguageServerInstanceStatus {
            root: PathBuf::from("/workspace"),
            source: karet_session::LanguageServerSource::Managed,
            command: Some("/bin/server".to_owned()),
            args: Vec::new(),
            runtime: karet_session::LanguageServerRuntimeState::Running,
            open_documents: 1,
            error: None,
        }],
    }
}

/// A staleness signal makes the client ask again, and the older answer already
/// in flight is then refused.
///
/// This is the ordering the signal exists to survive. The session says only that
/// the cache is wrong, so the client asks; but an answer to the *previous*
/// request may still be on its way, and that one was computed before whatever
/// made the cache stale. Adopting it would cache exactly the state the signal
/// was sent to correct, and nothing afterwards would ask again -- which is how
/// a retired provider kept offering a Restart for a process that was gone.
///
/// Falsified by: dropping the `self.inventory_request != request` guard from
/// `LanguageServerRuntimeModel::replace`, which lets the stale answer land.
#[test]
fn a_staleness_signal_refuses_the_answer_that_predates_it() {
    let backend = std::sync::Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.open_language_servers();
    let first = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.inventory_request,
        _ => panic!("expected the language-server manager"),
    };
    assert!(first.is_some(), "opening the tab should ask for the list");

    app.on_backend_event(None, karet_session::Event::LanguageServerInventoryStale);

    let second = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.inventory_request,
        _ => panic!("expected the language-server manager"),
    };
    assert!(
        second.is_some() && second != first,
        "the staleness signal did not make the client ask again"
    );

    // The answer to the first request lands late. It describes the session as it
    // was before the retirement, so it must not be taken.
    app.on_backend_event(
        first,
        karet_session::Event::LanguageServerStatus {
            servers: vec![serving(LanguageServerId::RustAnalyzer)],
        },
    );
    assert!(
        app.lsp_runtime.servers.is_empty(),
        "an answer that predates the staleness signal was adopted"
    );
    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => assert_eq!(
            view.inventory_request, second,
            "a stale answer cancelled the request the tab was waiting on"
        ),
        _ => panic!("expected the language-server manager"),
    }

    // The answer to the re-query is the one that counts.
    app.on_backend_event(
        second,
        karet_session::Event::LanguageServerStatus {
            servers: vec![
                serving(LanguageServerId::RustAnalyzer),
                serving(LanguageServerId::Clangd),
            ],
        },
    );
    assert_eq!(
        app.lsp_runtime.servers.len(),
        2,
        "the answer to the re-query was discarded"
    );
    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => assert!(view.inventory_request.is_none()),
        _ => panic!("expected the language-server manager"),
    }
}

/// The panel offers Restart on exactly the session's own predicate.
///
/// The button used to be painted, and its click guarded, by two byte-identical
/// copies of `restartable` living in the client -- so the inventory could say a
/// provider had no process while the panel went on offering to restart it. Both
/// copies are gone; `LanguageServerInstanceStatus::restartable` is the only
/// definition, and this pins that the strip really asks it.
///
/// Falsified by: painting the Restart action unconditionally in
/// `ui::language_servers::actions::server_actions`.
#[test]
fn restart_is_offered_for_a_live_instance_and_not_a_retired_one() {
    let backend = std::sync::Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.open_language_servers();

    answer_inventory(&mut app, vec![serving(LanguageServerId::RustAnalyzer)]);
    let _ = screen(&mut app, 120, 24);
    assert!(
        offers_restart(&app),
        "a serving provider is what Restart is for"
    );

    // Exactly what the session reports for a provider it has retired.
    let mut retired = serving(LanguageServerId::RustAnalyzer);
    for instance in &mut retired.instances {
        instance.runtime = karet_session::LanguageServerRuntimeState::Idle;
        instance.open_documents = 0;
    }
    assert!(
        !retired.restartable(),
        "the fixture is not the retired shape the session reports"
    );
    answer_inventory(&mut app, vec![retired]);
    let _ = screen(&mut app, 120, 24);
    assert!(
        !offers_restart(&app),
        "the panel offered a Restart for a provider with no process"
    );
}

/// Whether the manager's action strip currently paints a Restart button.
fn offers_restart(app: &App) -> bool {
    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view
            .action_hits
            .iter()
            .any(|hit| hit.action == crate::tab::LanguageServerAction::Restart),
        _ => panic!("expected the language-server manager"),
    }
}

//! What an *unsolicited* language-server inventory does to the manager tab.
//!
//! The session pushes one whenever it retires a provider, because the document
//! count has no event of its own and a client patching state field by field ends
//! up offering a Restart for a process that is gone. A push answers no request,
//! which is the whole difference from the inventory the tab asks for itself.
//!
//! Its own file: `app/tests/language_servers.rs` has a handful of code lines
//! left under the workspace ceiling.

use super::support::*;
use crate::app::*;

/// A provider with a live instance, which is what a Restart is offered for.
///
/// Local rather than shared: the neighbouring fixture is private to its own
/// file, and a sibling branch is moving it to `support`. Duplicating four
/// fields is cheaper than colliding with that.
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

/// A pushed inventory must not cancel the request the tab already has out.
///
/// Both carry the same shape, so the tab used to treat a push as the answer to
/// whatever it was waiting for: it cleared the pending request, and the real
/// reply -- which is *newer* -- then arrived with nothing waiting for it and was
/// dropped. The tab kept the older rows, and the guard that keeps one inventory
/// request in flight at a time was gone with it.
///
/// Falsified by: clearing `inventory_request` in `adopt_pushed_servers` (or
/// routing an untagged payload through `set_servers`, which does the same).
#[test]
fn a_pushed_inventory_does_not_cancel_the_tabs_own_request() {
    let backend = std::sync::Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.open_language_servers();
    let pending = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.inventory_request,
        _ => panic!("expected the language-server manager"),
    };
    assert!(pending.is_some(), "opening the tab should ask for the list");

    app.on_backend_event(
        None,
        karet_session::Event::LanguageServerStatus {
            servers: vec![serving(LanguageServerId::RustAnalyzer)],
        },
    );

    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => {
            assert_eq!(
                view.inventory_request, pending,
                "a push cancelled the request the tab was waiting on"
            );
            assert_eq!(view.servers.len(), 1, "the pushed rows were not adopted");
            assert!(
                view.loading_since.is_none(),
                "rows arrived, so the loading placeholder should stop"
            );
        },
        _ => panic!("expected the language-server manager"),
    }

    // The answer to the original request still lands, and still wins.
    app.on_backend_event(
        pending,
        karet_session::Event::LanguageServerStatus {
            servers: vec![
                serving(LanguageServerId::RustAnalyzer),
                serving(LanguageServerId::Clangd),
            ],
        },
    );
    match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => {
            assert_eq!(
                view.servers.len(),
                2,
                "the fresher answer to the tab's own request was discarded"
            );
            assert!(view.inventory_request.is_none());
        },
        _ => panic!("expected the language-server manager"),
    }
}

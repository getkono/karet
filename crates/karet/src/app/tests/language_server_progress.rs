//! The card a managed operation shows while it runs, and the outcome that
//! replaces it.

use super::support::*;
use crate::app::*;

// A confirmed install used to report itself only inside the manager tab, so a
// user who approved one from an editor buffer saw nothing more. These pin the
// card that reaches them wherever they are.

#[test]
fn an_install_approved_outside_the_manager_still_reports_progress() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    // Deliberately no `open_language_servers()`: this is the case the old policy
    // left silent.
    app.begin_language_server_install(LanguageServerId::Texlab);
    let request = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| sent.last().map(|(request, _)| *request));

    app.on_backend_event(
        request,
        SessionEvent::LanguageServerProgress {
            server: LanguageServerId::Texlab,
            downloaded: 7 * 1024 * 1024,
            total: Some(14 * 1024 * 1024),
        },
    );

    let painted = screen(&mut app, 100, 18).join("\n");
    assert!(painted.contains("Installing"), "{painted}");
    assert!(painted.contains("texlab"), "{painted}");
    assert!(painted.contains("50%"), "the size is weighable: {painted}");
    assert!(painted.contains("14.0 MB"), "{painted}");
}

#[test]
fn progress_updates_in_place_instead_of_stacking() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.begin_language_server_install(LanguageServerId::Texlab);
    let request = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| sent.last().map(|(request, _)| *request));

    for downloaded in [1024, 2048, 4096] {
        app.on_backend_event(
            request,
            SessionEvent::LanguageServerProgress {
                server: LanguageServerId::Texlab,
                downloaded,
                total: Some(8192),
            },
        );
    }
    assert_eq!(
        app.notifications.active().len(),
        1,
        "one card that changes, not one card per chunk"
    );
}

#[test]
fn a_download_with_no_declared_size_reports_bytes_rather_than_a_made_up_percent() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.begin_language_server_install(LanguageServerId::Texlab);
    let request = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| sent.last().map(|(request, _)| *request));

    app.on_backend_event(
        request,
        SessionEvent::LanguageServerProgress {
            server: LanguageServerId::Texlab,
            downloaded: 2048,
            total: None,
        },
    );
    let painted = screen(&mut app, 100, 18).join("\n");
    assert!(painted.contains("2.0 KB"), "{painted}");
    assert!(!painted.contains('%'), "no invented percentage: {painted}");
}

#[test]
fn a_running_install_adds_no_idle_repaint() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    assert!(app.next_wake().is_none());
    app.begin_language_server_install(LanguageServerId::Texlab);
    // The card is repainted by progress events, not by a clock. Registering a
    // frame-interval wake here would repaint identical output forever, since
    // nothing on the timer path rebuilds the card.
    assert!(
        app.next_wake().is_none(),
        "a download must not put the loop into a busy repaint"
    );
}

#[test]
fn an_update_check_is_not_worth_a_card() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.check_all_language_servers();
    assert!(
        app.notifications.active().is_empty(),
        "a metadata check the user asked for and watched needs no announcement"
    );
}

#[test]
fn a_failure_survives_another_operations_next_progress_tick() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.begin_language_server_install(LanguageServerId::Texlab);
    app.begin_language_server_install(LanguageServerId::RustAnalyzer);
    let requests: Vec<RequestId> = backend
        .sent
        .lock()
        .map(|sent| sent.iter().map(|(request, _)| *request).collect())
        .unwrap_or_default();

    app.on_backend_event(
        requests.first().copied(),
        SessionEvent::Notification {
            severity: Severity::Error,
            kind: NotificationKind::Lsp,
            message: "language-server registry: checksum mismatch".to_string(),
        },
    );
    // The other install is still going. Its next tick must not erase the report
    // of the one that failed — an error card is the outcome the user most needs,
    // and it is the one that never auto-expires.
    app.on_backend_event(
        requests.get(1).copied(),
        SessionEvent::LanguageServerProgress {
            server: LanguageServerId::RustAnalyzer,
            downloaded: 4096,
            total: Some(8192),
        },
    );

    let titles: Vec<String> = app
        .notifications
        .active()
        .iter()
        .map(|notification| notification.title.clone())
        .collect();
    assert!(
        titles
            .iter()
            .any(|title| title.contains("checksum mismatch")),
        "the failure is still reported: {titles:?}"
    );
    assert!(
        titles.iter().any(|title| title.contains("Installing")),
        "and the running download still says so: {titles:?}"
    );
}

#[test]
fn a_completion_survives_another_operations_next_progress_tick() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.begin_language_server_install(LanguageServerId::Texlab);
    app.begin_language_server_install(LanguageServerId::RustAnalyzer);
    let requests: Vec<RequestId> = backend
        .sent
        .lock()
        .map(|sent| sent.iter().map(|(request, _)| *request).collect())
        .unwrap_or_default();

    app.on_backend_event(
        requests.first().copied(),
        SessionEvent::LanguageServerChanged {
            server: LanguageServerId::Texlab,
            version: "1.3.0".to_string(),
            restart_required: false,
        },
    );
    app.on_backend_event(
        requests.get(1).copied(),
        SessionEvent::LanguageServerProgress {
            server: LanguageServerId::RustAnalyzer,
            downloaded: 4096,
            total: Some(8192),
        },
    );

    let titles: Vec<String> = app
        .notifications
        .active()
        .iter()
        .map(|notification| notification.title.clone())
        .collect();
    assert!(
        titles.iter().any(|title| title.contains("1.3.0")),
        "the finished install is still reported: {titles:?}"
    );
}

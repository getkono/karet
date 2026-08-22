//! Manager-level tests for the jdtls launch polish: the JDK preflight gate
//! and the `language/status` → status-update forwarding.

use super::*;

fn manager_with_test_connector() -> (LspManager, mpsc::UnboundedReceiver<LspUpdate>) {
    let (mut manager, updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        Behavior::Normal,
        None,
        Arc::new(AtomicUsize::new(0)),
    ));
    (manager, updates)
}

#[tokio::test]
async fn cached_preflight_failure_blocks_the_server() {
    let (mut manager, _updates) = manager_with_test_connector();
    manager.jdtls_preflight = Some(Some("no jdk".to_owned()));
    manager.document_opened(
        Some("java"),
        Some("java"),
        Path::new("/tmp/Main.java"),
        1,
        String::new,
    );
    assert!(
        manager.servers.is_empty(),
        "a failed preflight must not spawn"
    );
}

#[tokio::test]
async fn passing_preflight_spawns_the_server() {
    let (mut manager, _updates) = manager_with_test_connector();
    manager.jdtls_preflight = Some(None);
    manager.document_opened(
        Some("java"),
        Some("java"),
        Path::new("/tmp/Main.java"),
        1,
        String::new,
    );
    assert_eq!(manager.servers.len(), 1);
}

#[tokio::test]
async fn language_status_notifications_surface_as_status_updates() {
    let (mut manager, mut updates) = manager_with_test_connector();
    manager.jdtls_preflight = Some(None);
    manager.document_opened(
        Some("java"),
        Some("java"),
        Path::new("/tmp/Status.java"),
        1,
        String::new,
    );
    let status = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match updates.recv().await {
                Some(LspUpdate::ServerStatus {
                    server, message, ..
                }) => break Some((server, message)),
                Some(_) => {},
                None => break None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    assert_eq!(
        status,
        Some(("jdtls".to_owned(), "37% Importing projects".to_owned()))
    );
}

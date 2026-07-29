fn language_server_status(
    server: LanguageServerId,
    language: &str,
    managed: bool,
) -> LanguageServerStatus {
    LanguageServerStatus {
        server,
        languages: vec![language.to_string()],
        enabled: true,
        managed,
        installed: managed.then(|| "1.2.3".to_string()),
        cleanup_pending: false,
        instances: vec![karet_session::LanguageServerInstanceStatus {
            root: PathBuf::from("/workspace"),
            source: if managed {
                karet_session::LanguageServerSource::Managed
            } else {
                karet_session::LanguageServerSource::Path
            },
            command: Some("/bin/server".to_string()),
            args: vec!["--stdio".to_string()],
            runtime: karet_session::LanguageServerRuntimeState::Running,
            open_documents: 2,
            error: None,
        }],
    }
}

#[test]
fn language_server_manager_is_a_singleton_and_requests_inventory() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());

    app.dispatch(Command::ManageLanguageServers);
    app.dispatch(Command::ManageLanguageServers);

    assert_eq!(
        app.all_tabs()
            .filter(|tab| matches!(tab.kind, TabKind::LanguageServers(_)))
            .count(),
        1
    );
    let status_requests = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| matches!(command, SessionCommand::LanguageServerStatus))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(status_requests, 1);
}

#[test]
fn language_server_manager_filters_navigates_and_checks_selected_provider() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(None, vec![
        language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
        language_server_status(LanguageServerId::Clangd, "c", false),
    ]);

    app.set_language_server_filter("rust".to_string());
    app.dispatch(Command::LanguageServerCheckSelected);

    let targeted = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::CheckLanguageServerUpdates {
                        server: Some(server)
                    } if *server == LanguageServerId::RustAnalyzer
                )
            })
        })
        .unwrap_or_default();
    assert!(targeted);
    let TabKind::LanguageServers(view) = &app.tabs[app.active].kind else {
        panic!("expected language-server manager");
    };
    assert_eq!(view.visible_indices().len(), 1);
    assert_eq!(view.selected_id(), Some(LanguageServerId::RustAnalyzer));
}

#[test]
fn language_server_manager_renders_inventory_controls_and_detail() {
    let mut app = app();
    app.open_language_servers();
    app.show_language_server_status(None, vec![
        language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
        language_server_status(LanguageServerId::Clangd, "c, cpp", false),
    ]);
    app.language_server_select(1);

    let rendered = screen(&mut app, 120, 24).join("\n");
    assert!(rendered.contains("Refresh"));
    assert!(rendered.contains("Check all"));
    assert!(
        rendered.contains("rust-analyzer"),
        "manager screen:\n{rendered}"
    );
    assert!(rendered.contains("Karet-managed"));
    assert!(rendered.contains("/bin/server --stdio"));
}

#[test]
fn language_server_manager_mouse_selects_rows_and_runs_toolbar_actions() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(None, vec![
        language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
        language_server_status(LanguageServerId::Clangd, "c", false),
    ]);
    let _ = screen(&mut app, 120, 24);
    let (table, offset, refresh) = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => (
            view.table_rect,
            view.offset,
            view.action_hits
                .iter()
                .find(|(_, action)| *action == crate::tab::LanguageServerAction::Refresh)
                .map(|(rect, _)| *rect),
        ),
        _ => panic!("expected language-server manager"),
    };
    assert_eq!(offset, 0);
    assert!(app.handle_language_server_click(table.x, table.y + 3));
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view) if view.selected == 1
    ));

    let Some(refresh) = refresh else {
        panic!("refresh action was not rendered");
    };
    assert!(app.handle_language_server_click(refresh.x, refresh.y));
    let status_requests = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter(|(_, command)| matches!(command, SessionCommand::LanguageServerStatus))
                .count()
        })
        .unwrap_or_default();
    assert_eq!(status_requests, 2);
}

#[test]
fn language_server_manager_delays_its_loading_placeholder() {
    let mut app = app();
    app.open_language_servers();
    assert!(app.next_wake().is_some_and(|wake| wake <= LOADING_REVEAL_DELAY));
    assert!(!screen(&mut app, 100, 18).join("\n").contains("Loading"));

    if let TabKind::LanguageServers(view) = &mut app.tabs[app.active].kind {
        view.loading_since = Some(Instant::now() - LOADING_REVEAL_DELAY);
    }
    assert!(
        screen(&mut app, 100, 18)
            .join("\n")
            .contains("Loading language servers")
    );
}

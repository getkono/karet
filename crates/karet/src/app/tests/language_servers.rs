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
                .find(|hit| hit.action == crate::tab::LanguageServerAction::Refresh)
                .map(|hit| hit.rect),
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

#[test]
fn language_server_manager_only_renders_applicable_row_actions() {
    let installed = language_server_status(LanguageServerId::RustAnalyzer, "rust", true);
    let mut missing = language_server_status(LanguageServerId::Texlab, "tex", true);
    missing.installed = None;
    missing.instances[0].runtime = karet_session::LanguageServerRuntimeState::Idle;
    missing.instances[0].open_documents = 0;
    let external = language_server_status(LanguageServerId::Clangd, "c", false);

    let mut app = app();
    app.open_language_servers();
    app.show_language_server_status(None, vec![
        installed.clone(),
        missing.clone(),
        external.clone(),
    ]);
    let rendered = screen(&mut app, 120, 28).join("\n");
    assert!(!rendered.contains("Install/Update"));
    assert!(rendered.contains("Check updates"));
    assert!(rendered.contains("Install"));
    assert!(rendered.contains("Restart"));
    assert!(rendered.contains("Uninstall"));

    let TabKind::LanguageServers(view) = &app.tabs[app.active].kind else {
        panic!("expected language-server manager");
    };
    let actions = |server: &LanguageServerId| {
        view.action_hits
            .iter()
            .filter(|hit| hit.server.as_ref() == Some(server))
            .map(|hit| hit.action)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        actions(&missing.server),
        vec![crate::tab::LanguageServerAction::Primary]
    );
    assert_eq!(
        actions(&external.server),
        vec![crate::tab::LanguageServerAction::Restart]
    );
    let installed_actions = actions(&installed.server);
    assert!(installed_actions.contains(&crate::tab::LanguageServerAction::Primary));
    assert!(installed_actions.contains(&crate::tab::LanguageServerAction::Restart));
    assert!(installed_actions.contains(&crate::tab::LanguageServerAction::Uninstall));
}

#[test]
fn language_server_row_action_targets_its_own_server() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(None, vec![
        language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
        language_server_status(LanguageServerId::Clangd, "c", false),
    ]);
    let _ = screen(&mut app, 120, 24);
    let hit = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view
            .action_hits
            .iter()
            .find(|hit| {
                hit.action == crate::tab::LanguageServerAction::Primary
                    && hit.server.as_ref() == Some(&LanguageServerId::RustAnalyzer)
            })
            .cloned(),
        _ => None,
    };
    let Some(hit) = hit else {
        panic!("expected rust-analyzer row action");
    };
    assert!(app.handle_language_server_click(hit.rect.x, hit.rect.y));
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
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view)
            if view.selected_id() == Some(LanguageServerId::RustAnalyzer)
    ));
}

#[test]
fn language_server_actions_wrap_without_disappearing_on_narrow_views() {
    let mut app = app();
    app.open_language_servers();
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );
    let rendered = screen(&mut app, 50, 24).join("\n");
    assert!(rendered.contains("Check updates"));
    assert!(rendered.contains("Restart"));
    assert!(rendered.contains("Uninstall"));
}

#[test]
fn active_file_lsp_badge_reacts_to_runtime_state_and_color() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = app();
    app.push_tab(text_tab("/workspace/src/main.rs", "fn main() {}\n"));
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw shell");
    let running = terminal.backend().buffer();
    let running_row = (0..100)
        .map(|x| running[(x, 11)].symbol())
        .collect::<String>();
    let running_x = running_row.find("LSP in sync").expect("running LSP badge");
    assert_eq!(
        running[(u16::try_from(running_x).unwrap_or_default(), 11)].fg,
        app.theme
            .role(ThemeRole::DiagnosticHint)
            .to_ratatui()
    );

    app.on_backend_event(
        None,
        SessionEvent::LanguageServerRuntimeChanged {
            server: LanguageServerId::RustAnalyzer,
            root: PathBuf::from("/workspace"),
            state: LanguageServerRuntimeState::CircuitOpen,
            error: Some("protocol error: cannot convert relative path . to a file URI".to_string()),
        },
    );
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("redraw shell");
    let crashed = terminal.backend().buffer();
    let crashed_row = (0..100)
        .map(|x| crashed[(x, 11)].symbol())
        .collect::<String>();
    let crashed_x = crashed_row.find("LSP crashed").expect("crashed LSP badge");
    assert_eq!(
        crashed[(u16::try_from(crashed_x).unwrap_or_default(), 11)].fg,
        app.theme
            .role(ThemeRole::DiagnosticError)
            .to_ratatui()
    );
}

#[test]
fn runtime_protocol_failures_remain_as_deduplicated_notifications() {
    let mut app = app();
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );
    let root = PathBuf::from("/workspace");
    app.update_language_server_runtime(
        LanguageServerId::RustAnalyzer,
        root.clone(),
        LanguageServerRuntimeState::Retrying,
        Some("transport reset".to_string()),
    );
    app.update_language_server_runtime(
        LanguageServerId::RustAnalyzer,
        root,
        LanguageServerRuntimeState::CircuitOpen,
        Some("protocol error: cannot convert relative path . to a file URI".to_string()),
    );

    let lsp_failures = app
        .notifications
        .active()
        .into_iter()
        .filter(|notification| notification.kind == NotificationKind::Lsp)
        .collect::<Vec<_>>();
    assert_eq!(lsp_failures.len(), 1);
    assert_eq!(lsp_failures[0].severity, Severity::Error);
    assert!(lsp_failures[0].timeout.is_none());
    assert!(
        lsp_failures[0]
            .title
            .contains("protocol error: cannot convert relative path . to a file URI")
    );
}

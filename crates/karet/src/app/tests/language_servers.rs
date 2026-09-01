use super::support::*;
use crate::app::*;

fn language_server_status(
    server: LanguageServerId,
    language: &str,
    managed: bool,
) -> LanguageServerStatus {
    LanguageServerStatus {
        ever_installed: managed,
        declined: false,
        server,
        languages: vec![language.to_string()],
        enabled: true,
        managed,
        manual_install_reason: (!managed).then(|| "install with the project toolchain".to_string()),
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
    app.show_language_server_status(
        None,
        vec![
            language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
            language_server_status(LanguageServerId::Clangd, "c", false),
        ],
    );

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
    app.show_language_server_status(
        None,
        vec![
            language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
            language_server_status(LanguageServerId::Clangd, "c, cpp", false),
        ],
    );
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

    app.language_server_select(-1);
    let rendered = screen(&mut app, 120, 24).join("\n");
    assert!(rendered.contains("manual install"));
    assert!(rendered.contains("install with the project toolchain"));
}

#[test]
fn language_server_manager_mouse_selects_rows_and_runs_toolbar_actions() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(
        None,
        vec![
            language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
            language_server_status(LanguageServerId::Clangd, "c", false),
        ],
    );
    let _ = screen(&mut app, 120, 24);
    let (second_row, offset, refresh) = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => (
            view.row_hits.get(1).map(|(rect, _)| *rect),
            view.offset,
            view.action_hits
                .iter()
                .find(|hit| hit.action == crate::tab::LanguageServerAction::Refresh)
                .map(|hit| hit.rect),
        ),
        _ => panic!("expected language-server manager"),
    };
    assert_eq!(offset, 0);
    let Some(second_row) = second_row else {
        panic!("expected a second language-server row");
    };
    assert!(app.handle_language_server_click(
        second_row.x.saturating_add(1),
        second_row.y.saturating_add(1)
    ));
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
    assert!(
        app.next_wake()
            .is_some_and(|wake| wake <= LOADING_REVEAL_DELAY)
    );
    assert!(!screen(&mut app, 100, 18).join("\n").contains("Loading"));

    if let TabKind::LanguageServers(view) = &mut app.tabs[app.active].kind {
        view.loading_since = Some(Pending::revealed());
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
    let mut manual = language_server_status(LanguageServerId::Gopls, "go", false);
    manual.instances[0].source = karet_session::LanguageServerSource::Unavailable;
    manual.instances[0].command = None;
    manual.instances[0].runtime = karet_session::LanguageServerRuntimeState::Idle;
    manual.instances[0].open_documents = 0;

    let mut app = app();
    app.open_language_servers();
    app.show_language_server_status(
        None,
        vec![
            installed.clone(),
            missing.clone(),
            external.clone(),
            manual.clone(),
        ],
    );
    let rendered = screen(&mut app, 120, 28).join("\n");
    assert!(!rendered.contains("Install/Update"));
    assert!(rendered.contains("Check updates"));
    assert!(rendered.contains("Install"));
    assert!(rendered.contains("Restart"));
    assert!(rendered.contains("Uninstall"));
    assert!(rendered.contains("Install manually"));

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
    assert!(actions(&manual.server).is_empty());
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
    app.show_language_server_status(
        None,
        vec![
            language_server_status(LanguageServerId::RustAnalyzer, "rust", true),
            language_server_status(LanguageServerId::Clangd, "c", false),
        ],
    );
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
fn language_server_install_action_is_the_only_install_approval() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    let mut missing = language_server_status(LanguageServerId::Texlab, "tex", true);
    missing.installed = None;
    app.show_language_server_status(None, vec![missing]);

    app.language_server_action(
        crate::tab::LanguageServerAction::Primary,
        Some(LanguageServerId::Texlab),
    );

    let install_sent = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::InstallLanguageServer { server }
                        if *server == LanguageServerId::Texlab
                )
            })
        })
        .unwrap_or_default();
    assert!(install_sent);
    assert!(app.overlay.is_none());
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view)
            if view.pending.iter().any(|pending| {
                pending.kind == crate::tab::LanguageServerPendingKind::Install
            })
    ));
}

#[test]
fn language_server_installs_are_independent_per_provider() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    let mut texlab = language_server_status(LanguageServerId::Texlab, "tex", true);
    texlab.installed = None;
    let mut zls = language_server_status(LanguageServerId::Zls, "zig", true);
    zls.installed = None;
    app.show_language_server_status(None, vec![texlab, zls]);

    app.language_server_action(
        crate::tab::LanguageServerAction::Primary,
        Some(LanguageServerId::Texlab),
    );
    app.language_server_action(
        crate::tab::LanguageServerAction::Primary,
        Some(LanguageServerId::Zls),
    );

    let installs = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(_, command)| match command {
                    SessionCommand::InstallLanguageServer { server } => Some(server.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        installs,
        vec![LanguageServerId::Texlab, LanguageServerId::Zls]
    );
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view) if view.pending.len() == 2
    ));
    // Two provider rows, plus the single shared card: one download at a time is
    // what the user needs told, not a stack of five.
    assert_eq!(
        screen(&mut app, 120, 24)
            .join("\n")
            .matches("Installing")
            .count(),
        3
    );
}

#[test]
fn language_server_install_state_survives_manager_reopen() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend);
    app.open_language_servers();
    let mut missing = language_server_status(LanguageServerId::Texlab, "tex", true);
    missing.installed = None;
    app.show_language_server_status(None, vec![missing.clone()]);
    app.begin_language_server_install(LanguageServerId::Texlab);

    app.close_tab_at(app.active);
    app.open_language_servers();
    app.show_language_server_status(None, vec![missing]);

    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view)
            if view.pending.iter().any(|pending| {
                pending.server.as_ref() == Some(&LanguageServerId::Texlab)
            })
    ));
    assert!(screen(&mut app, 100, 18).join("\n").contains("Installing"));
}

#[test]
fn language_server_install_progress_stays_in_manager() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    let mut missing = language_server_status(LanguageServerId::Texlab, "tex", true);
    missing.installed = None;
    app.show_language_server_status(None, vec![missing]);
    // Drop the cards the setup raised, so what survives below is the install's.
    app.notifications.dismiss_all();
    app.begin_language_server_install(LanguageServerId::Texlab);
    let request = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| sent.last().map(|(request, _)| *request));

    app.on_backend_event(
        None,
        SessionEvent::LanguageServerProgress {
            server: LanguageServerId::Texlab,
            downloaded: 50,
            total: Some(100),
        },
    );

    // The bytes reach the manager tab and the one tagged operation card. The
    // progress event raises no second notification of its own — a card per tick
    // would bury whatever else the editor is trying to say.
    let tags: Vec<Option<&str>> = app
        .notifications
        .active()
        .iter()
        .map(|note| note.tag.as_deref())
        .collect();
    assert_eq!(tags, vec![Some("lsp.operation")]);
    assert!(
        screen(&mut app, 100, 18)
            .join("\n")
            .contains("Installing… 50%")
    );

    app.on_backend_event(
        request,
        SessionEvent::LanguageServerChanged {
            server: LanguageServerId::Texlab,
            version: "1.3.0".to_string(),
            restart_required: false,
        },
    );
    // Success supersedes the progress card under the same tag, and the manager's
    // own row-level label goes back to idle.
    let cards: Vec<String> = app
        .notifications
        .active()
        .iter()
        .map(|notification| notification.title.clone())
        .collect();
    assert!(
        cards.iter().any(|title| title.contains("1.3.0")),
        "the outcome is reported: {cards:?}"
    );
    assert!(
        !cards.iter().any(|title| title.contains("Installing")),
        "the progress card was replaced, not stacked: {cards:?}"
    );
}

#[test]
fn language_server_install_failure_stays_in_manager() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    let mut missing = language_server_status(LanguageServerId::Texlab, "tex", true);
    missing.installed = None;
    app.show_language_server_status(None, vec![missing]);
    app.begin_language_server_install(LanguageServerId::Texlab);
    let request = backend
        .sent
        .lock()
        .ok()
        .and_then(|sent| sent.last().map(|(request, _)| *request));

    app.on_backend_event(
        request,
        SessionEvent::Notification {
            severity: Severity::Error,
            kind: NotificationKind::Lsp,
            message: "language-server registry: checksum mismatch".to_string(),
        },
    );

    // A failure the user never sees is the worst outcome of the three, so it is
    // reported wherever they are — and errors never auto-expire.
    let failure = app
        .notifications
        .active()
        .iter()
        .find(|notification| notification.kind == NotificationKind::Lsp)
        .map(|notification| (notification.title.clone(), notification.timeout));
    let (title, timeout) = failure.unwrap_or_default();
    assert!(title.contains("checksum mismatch"), "{title}");
    assert!(timeout.is_none(), "an error card waits to be dismissed");
    assert!(
        screen(&mut app, 120, 18)
            .join("\n")
            .contains("checksum mismatch")
    );
}

#[test]
fn language_server_update_action_applies_the_visible_plan_directly() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );
    let plan = LanguageServerPlanId(9);
    app.prompt_language_server_updates(
        None,
        plan,
        vec![LanguageServerChange {
            server: LanguageServerId::RustAnalyzer,
            current: Some("1.2.3".into()),
            target: "1.3.0".into(),
            download_bytes: Some(42),
        }],
    );

    app.language_server_action(
        crate::tab::LanguageServerAction::Primary,
        Some(LanguageServerId::RustAnalyzer),
    );

    let apply_sent = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter().any(|(_, command)| {
                matches!(
                    command,
                    SessionCommand::ApplyLanguageServerPlan {
                        plan: applied,
                        servers,
                    } if *applied == plan && servers == &[LanguageServerId::RustAnalyzer]
                )
            })
        })
        .unwrap_or_default();
    assert!(apply_sent);
    assert!(app.overlay.is_none());
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view)
            if view.pending.iter().any(|pending| {
                pending.kind == crate::tab::LanguageServerPendingKind::Update
            })
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
fn language_server_table_borders_and_runtime_text_are_semantic() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut status = language_server_status(LanguageServerId::RustAnalyzer, "rust", true);
    status.instances[0].runtime = LanguageServerRuntimeState::CircuitOpen;
    status.instances[0].error = Some("protocol failure".to_string());
    let mut app = app();
    app.open_language_servers();
    app.show_language_server_status(None, vec![status]);

    let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw shell");
    let buffer = terminal.backend().buffer();
    let table = match &app.tabs[app.active].kind {
        TabKind::LanguageServers(view) => view.table_rect,
        _ => panic!("expected language-server manager"),
    };
    assert_eq!(buffer[(table.x, table.y)].symbol(), "┌");
    assert_eq!(buffer[(table.x, table.y.saturating_add(1))].symbol(), "│");
    assert!(
        (table.x..table.right())
            .any(|x| { (table.y..table.bottom()).any(|y| buffer[(x, y)].symbol() == "─") }),
        "table should render row separators"
    );

    let semantic_cell = |needle: &str| {
        let needle = needle
            .chars()
            .map(|character| character.to_string())
            .collect::<Vec<_>>();
        (0_u16..24).find_map(|y| {
            (0_u16..120).find_map(|x| {
                needle
                    .iter()
                    .enumerate()
                    .all(|(offset, expected)| {
                        u16::try_from(offset)
                            .ok()
                            .filter(|offset| x.saturating_add(*offset) < 120)
                            .is_some_and(|offset| {
                                buffer[(x.saturating_add(offset), y)].symbol() == expected
                            })
                    })
                    .then(|| buffer[(x, y)].clone())
            })
        })
    };
    assert_eq!(
        semantic_cell("circuit open").map(|cell| cell.fg),
        Some(app.theme.role(ThemeRole::DiagnosticHint).to_ratatui())
    );
    assert_eq!(
        semantic_cell("Error: protocol failure").map(|cell| cell.fg),
        Some(app.theme.role(ThemeRole::DiagnosticError).to_ratatui())
    );
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
        app.theme.role(ThemeRole::DiagnosticHint).to_ratatui()
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
        app.theme.role(ThemeRole::DiagnosticError).to_ratatui()
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

#[test]
fn completed_language_server_uninstall_clears_only_its_pending_request() {
    let backend = Arc::new(RecordingBackend::new());
    let mut app = app();
    app.backend = Some(backend.clone());
    app.open_language_servers();
    app.show_language_server_status(
        None,
        vec![language_server_status(
            LanguageServerId::RustAnalyzer,
            "rust",
            true,
        )],
    );

    app.begin_language_server_uninstall(LanguageServerId::RustAnalyzer);
    app.begin_language_server_uninstall(LanguageServerId::RustAnalyzer);
    let requests = backend
        .sent
        .lock()
        .map(|sent| {
            sent.iter()
                .filter_map(|(request, command)| {
                    matches!(
                        command,
                        SessionCommand::UninstallLanguageServer { server }
                            if *server == LanguageServerId::RustAnalyzer
                    )
                    .then_some(*request)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(requests.len(), 2);

    app.on_backend_event(
        requests.first().copied(),
        SessionEvent::LanguageServerRemoved {
            server: LanguageServerId::RustAnalyzer,
            cleanup_pending: false,
        },
    );
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view)
            if view.pending.iter().map(|pending| pending.request).collect::<Vec<_>>()
                == requests.get(1).copied().into_iter().collect::<Vec<_>>()
    ));
    assert!(
        screen(&mut app, 100, 18)
            .join("\n")
            .contains("Uninstalling")
    );

    app.on_backend_event(
        requests.get(1).copied(),
        SessionEvent::LanguageServerRemoved {
            server: LanguageServerId::RustAnalyzer,
            cleanup_pending: false,
        },
    );
    assert!(matches!(
        &app.tabs[app.active].kind,
        TabKind::LanguageServers(view) if view.pending.is_empty() && view.loading_since.is_none()
    ));
    let rendered = screen(&mut app, 100, 18).join("\n");
    assert!(!rendered.contains("Uninstalling"));
    assert!(rendered.contains("Install"));
    let cards: Vec<String> = app
        .notifications
        .active()
        .iter()
        .map(|notification| notification.title.clone())
        .collect();
    assert!(
        cards.iter().any(|title| title.starts_with("uninstalled")),
        "the outcome replaced the progress card: {cards:?}"
    );
    assert!(
        !cards.iter().any(|title| title.contains("Uninstalling")),
        "the progress card went with the operation: {cards:?}"
    );
}

use super::*;
use crate::tab::LanguageServerAction;
use crate::tab::LanguageServersViewState;

impl App {
    pub(super) fn open_language_servers(&mut self) {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| matches!(tab.kind, TabKind::LanguageServers(_)))
        {
            self.select_tab(index);
            return;
        }
        let stored = self.stored.iter().find_map(|(pane, stored)| {
            stored
                .tabs
                .iter()
                .position(|tab| matches!(tab.kind, TabKind::LanguageServers(_)))
                .map(|index| (*pane, index))
        });
        if let Some((pane, index)) = stored {
            self.focus_pane_switch(pane);
            self.select_tab(index);
            return;
        }

        let request = self.send_command_id(SessionCommand::LanguageServerStatus);
        self.push_tab(Tab::language_servers(request));
    }

    fn language_servers_mut(&mut self) -> Option<&mut LanguageServersViewState> {
        let tab = self.tabs.get_mut(self.active)?;
        let TabKind::LanguageServers(view) = &mut tab.kind else {
            return None;
        };
        Some(view)
    }

    fn selected_language_server(&self) -> Option<LanguageServerStatus> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::LanguageServers(view) = &tab.kind else {
            return None;
        };
        view.selected_server().cloned()
    }

    pub(super) fn refresh_language_servers(&mut self) {
        let request = self.send_command_id(SessionCommand::LanguageServerStatus);
        if let Some(view) = self.language_servers_mut() {
            view.inventory_request = request;
            view.loading_since = Some(Instant::now());
            view.error = None;
        }
    }

    pub(super) fn language_server_select(&mut self, delta: i32) {
        if let Some(view) = self.language_servers_mut() {
            view.select_relative(delta);
        }
    }

    pub(super) fn check_selected_language_server(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        if !status.managed {
            self.status = Some(format!(
                "{} is supplied externally and has no Karet update channel",
                status.server.display_name()
            ));
            return;
        }
        let request = self.send_command_id(SessionCommand::CheckLanguageServerUpdates {
            server: Some(status.server),
        });
        if let Some(view) = self.language_servers_mut() {
            view.pending = request;
            view.loading_since = Some(Instant::now());
            view.error = None;
        }
    }

    pub(super) fn check_all_language_servers(&mut self) {
        let request =
            self.send_command_id(SessionCommand::CheckLanguageServerUpdates { server: None });
        if let Some(view) = self.language_servers_mut() {
            view.pending = request;
            view.loading_since = Some(Instant::now());
            view.error = None;
        }
        self.status = Some("checking managed language servers for updates…".to_string());
    }

    pub(super) fn language_server_primary_action(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        if !status.managed {
            self.status = Some(format!(
                "{} is resolved from configuration or PATH",
                status.server.display_name()
            ));
            return;
        }
        let planned = self.language_servers_mut().and_then(|view| {
            view.changes
                .iter()
                .find(|change| change.server == status.server)
                .cloned()
                .zip(view.plan)
        });
        if let Some((change, plan)) = planned {
            self.overlay = Some(Overlay::text(
                format!(
                    "{} {} → {} · type update to approve",
                    change.server.display_name(),
                    change.current.as_deref().unwrap_or("missing"),
                    change.target
                ),
                TextPurpose::ApplyLanguageServerPlan {
                    plan,
                    servers: vec![change.server],
                },
            ));
        } else if status.installed.is_none() {
            self.prompt_language_server_install(status.server);
        } else {
            self.check_selected_language_server();
        }
    }

    pub(super) fn restart_selected_language_server(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        if status.instances.is_empty() {
            self.status = Some(format!(
                "{} has no process in this session",
                status.server.display_name()
            ));
            return;
        }
        self.send_command(SessionCommand::RestartLanguageServer {
            server: status.server.clone(),
        });
        self.status = Some(format!("restarting {}…", status.server.display_name()));
    }

    pub(super) fn uninstall_selected_language_server(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        if !status.managed || status.installed.is_none() {
            self.status = Some(format!(
                "{} is not installed by Karet",
                status.server.display_name()
            ));
            return;
        }
        self.overlay = Some(Overlay::text(
            format!(
                "Deactivate {} for future sessions · type uninstall to confirm",
                status.server.display_name()
            ),
            TextPurpose::UninstallLanguageServer {
                server: status.server,
            },
        ));
    }

    pub(super) fn prompt_language_server_filter(&mut self) {
        self.overlay = Some(Overlay::text(
            "Filter by server or language (submit empty text to clear)",
            TextPurpose::FilterLanguageServers,
        ));
    }

    pub(super) fn set_language_server_filter(&mut self, filter: String) {
        if let Some(view) = self.language_servers_mut() {
            view.filter = filter;
            view.selected = 0;
            view.offset = 0;
        }
    }

    pub(super) fn language_server_action(&mut self, action: LanguageServerAction) {
        match action {
            LanguageServerAction::Refresh => self.refresh_language_servers(),
            LanguageServerAction::CheckSelected => self.check_selected_language_server(),
            LanguageServerAction::CheckAll => self.check_all_language_servers(),
            LanguageServerAction::Primary => self.language_server_primary_action(),
            LanguageServerAction::Restart => self.restart_selected_language_server(),
            LanguageServerAction::Uninstall => self.uninstall_selected_language_server(),
            LanguageServerAction::Filter => self.prompt_language_server_filter(),
        }
    }

    pub(super) fn handle_language_server_click(&mut self, column: u16, row: u16) -> bool {
        let action = self.tabs.get(self.active).and_then(|tab| {
            let TabKind::LanguageServers(view) = &tab.kind else {
                return None;
            };
            view.action_hits
                .iter()
                .find_map(|(rect, action)| rect_contains(*rect, (column, row)).then_some(*action))
        });
        if let Some(action) = action {
            self.language_server_action(action);
            return true;
        }
        let Some(view) = self.language_servers_mut() else {
            return false;
        };
        if !rect_contains(view.table_rect, (column, row)) {
            return true;
        }
        let display_row = usize::from(row.saturating_sub(view.table_rect.y).saturating_sub(2));
        let selected = view.offset.saturating_add(display_row);
        if selected < view.visible_indices().len() {
            view.selected = selected;
        }
        true
    }

    pub(super) fn prompt_language_server_install(&mut self, server: LanguageServerId) {
        if self.overlay.is_none() {
            self.overlay = Some(Overlay::text(
                format!(
                    "{} is not installed · type install to download it",
                    server.display_name()
                ),
                TextPurpose::InstallLanguageServer { server },
            ));
        } else {
            self.notify(
                Severity::Warning,
                NotificationKind::Lsp,
                format!(
                    "{} is not installed; open Language Servers to install it",
                    server.display_name()
                ),
            );
        }
    }

    pub(super) fn show_language_server_status(
        &mut self,
        request: Option<RequestId>,
        servers: Vec<LanguageServerStatus>,
    ) {
        let total = servers.len();
        let available = servers
            .iter()
            .filter(|server| {
                server
                    .instances
                    .iter()
                    .any(|instance| instance.command.is_some())
            })
            .count();
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind
                && (request.is_none() || view.inventory_request == request)
            {
                view.set_servers(servers.clone());
            }
        }
        self.status = Some(format!("{available}/{total} language servers available"));
    }

    pub(super) fn prompt_language_server_updates(
        &mut self,
        plan: LanguageServerPlanId,
        changes: Vec<LanguageServerChange>,
    ) {
        let mut manager_open = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                manager_open = true;
                view.plan = Some(plan);
                view.changes.clone_from(&changes);
                view.pending = None;
                view.loading_since = None;
                view.error = None;
            }
        }
        if changes.is_empty() {
            self.notify(
                Severity::Information,
                NotificationKind::Lsp,
                "language servers are up to date",
            );
            return;
        }
        if manager_open {
            self.status = Some(format!(
                "{} language-server update(s) available",
                changes.len()
            ));
            return;
        }
        let summary = changes
            .iter()
            .map(|change| {
                format!(
                    "{} {} → {}",
                    change.server.display_name(),
                    change.current.as_deref().unwrap_or("missing"),
                    change.target
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.overlay = Some(Overlay::text(
            format!("{summary} · type update to approve these exact versions"),
            TextPurpose::ApplyLanguageServerPlan {
                plan,
                servers: changes.iter().map(|change| change.server.clone()).collect(),
            },
        ));
    }

    pub(super) fn show_language_server_progress(
        &mut self,
        server: LanguageServerId,
        downloaded: u64,
        total: Option<u64>,
    ) {
        self.status = Some(total.map_or_else(
            || format!("downloading {}: {downloaded} bytes", server.display_name()),
            |total| {
                format!(
                    "downloading {}: {downloaded}/{total} bytes",
                    server.display_name()
                )
            },
        ));
    }

    pub(super) fn finish_language_server_change(
        &mut self,
        server: LanguageServerId,
        version: String,
        restart_required: bool,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                if let Some(status) = view.servers.iter_mut().find(|item| item.server == server) {
                    status.installed = Some(version.clone());
                    status.cleanup_pending = false;
                }
                view.changes.retain(|change| change.server != server);
                if view.changes.is_empty() {
                    view.plan = None;
                }
                view.pending = None;
                view.loading_since = None;
            }
        }
        let suffix = if restart_required {
            " · restart to use it in this session"
        } else {
            ""
        };
        self.notify(
            Severity::Information,
            NotificationKind::Lsp,
            format!("installed {} {version}{suffix}", server.display_name()),
        );
    }

    pub(super) fn finish_language_server_remove(
        &mut self,
        server: LanguageServerId,
        cleanup_pending: bool,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                if let Some(status) = view.servers.iter_mut().find(|item| item.server == server) {
                    status.installed = None;
                    status.cleanup_pending = cleanup_pending;
                    status.instances.clear();
                }
                view.changes.retain(|change| change.server != server);
                if view.changes.is_empty() {
                    view.plan = None;
                }
            }
        }
        let suffix = if cleanup_pending {
            "; payload cleanup is deferred until shared processes exit"
        } else {
            ""
        };
        self.notify(
            Severity::Information,
            NotificationKind::Lsp,
            format!("uninstalled {}{suffix}", server.display_name()),
        );
    }

    pub(super) fn update_language_server_runtime(
        &mut self,
        server: LanguageServerId,
        root: PathBuf,
        state: karet_session::LanguageServerRuntimeState,
        error: Option<String>,
    ) {
        let mut missing_instance = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind
                && let Some(status) = view.servers.iter_mut().find(|item| item.server == server)
            {
                if let Some(instance) = status.instances.iter_mut().find(|item| item.root == root) {
                    instance.runtime = state;
                    instance.error.clone_from(&error);
                } else {
                    missing_instance = true;
                }
            }
        }
        if missing_instance {
            let request = self.send_command_id(SessionCommand::LanguageServerStatus);
            for tab in self.all_tabs_mut() {
                if let TabKind::LanguageServers(view) = &mut tab.kind {
                    view.inventory_request = request;
                }
            }
        }
    }
}

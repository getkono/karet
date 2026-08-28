mod progress;
mod prompts;

use super::*;
use crate::tab::LanguageServerAction;
use crate::tab::LanguageServerPending;
use crate::tab::LanguageServerPendingKind;
use crate::tab::LanguageServersViewState;

/// Lifecycle states retained independently of whether the manager tab is open.
#[derive(Default)]
pub(super) struct LanguageServerRuntimeModel {
    servers: Vec<LanguageServerStatus>,
    inventory_request: Option<RequestId>,
    operations: Vec<LanguageServerPending>,
    operation_error: Option<String>,
}

/// The active file's compact language-server condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LanguageServerBadge {
    Idle,
    Starting,
    InSync,
    Retrying,
    Crashed,
    Unavailable,
}

impl LanguageServerBadge {
    fn priority(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::InSync => 1,
            Self::Starting => 2,
            Self::Retrying => 3,
            Self::Crashed => 4,
            Self::Unavailable => 5,
        }
    }
}

impl LanguageServerRuntimeModel {
    fn replace(&mut self, request: Option<RequestId>, servers: Vec<LanguageServerStatus>) {
        self.servers = servers;
        if request.is_none() || self.inventory_request == request {
            self.inventory_request = None;
        }
    }

    fn update(
        &mut self,
        server: &LanguageServerId,
        root: &Path,
        state: LanguageServerRuntimeState,
        error: &Option<String>,
    ) -> bool {
        let Some(instance) = self
            .servers
            .iter_mut()
            .find(|status| status.server == *server)
            .and_then(|status| {
                status
                    .instances
                    .iter_mut()
                    .find(|instance| instance.root == root)
            })
        else {
            return false;
        };
        instance.runtime = state;
        instance.error.clone_from(error);
        true
    }

    fn badge_for(&self, path: &Path, language: &str) -> Option<LanguageServerBadge> {
        let language = language.to_lowercase();
        self.servers
            .iter()
            .filter(|status| {
                status.enabled
                    && status
                        .languages
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(&language))
            })
            .flat_map(|status| &status.instances)
            .filter(|instance| path_contains_or_equals(&instance.root, path))
            .map(|instance| {
                if instance.command.is_none()
                    || instance.source == karet_session::LanguageServerSource::Unavailable
                {
                    return LanguageServerBadge::Unavailable;
                }
                match instance.runtime {
                    LanguageServerRuntimeState::Idle => LanguageServerBadge::Idle,
                    LanguageServerRuntimeState::Starting => LanguageServerBadge::Starting,
                    LanguageServerRuntimeState::Running => LanguageServerBadge::InSync,
                    LanguageServerRuntimeState::Retrying => LanguageServerBadge::Retrying,
                    LanguageServerRuntimeState::CircuitOpen
                    | LanguageServerRuntimeState::Stopped => LanguageServerBadge::Crashed,
                    LanguageServerRuntimeState::Unavailable => LanguageServerBadge::Unavailable,
                    _ => LanguageServerBadge::Unavailable,
                }
            })
            .max_by_key(|badge| badge.priority())
    }

    fn start_operation(&mut self, operation: LanguageServerPending) {
        self.operations
            .retain(|pending| pending.request != operation.request);
        self.operations.push(operation);
        self.operation_error = None;
    }

    fn update_progress(&mut self, server: &LanguageServerId, downloaded: u64, total: Option<u64>) {
        if let Some(operation) = self
            .operations
            .iter_mut()
            .find(|pending| pending.server.as_ref() == Some(server))
        {
            operation.downloaded = Some(downloaded);
            operation.total = total;
        }
    }

    fn finish_operation(&mut self, request: Option<RequestId>, server: Option<&LanguageServerId>) {
        self.operations.retain(|pending| {
            let request_matches = request.is_none_or(|request| pending.request == request);
            let server_matches =
                server.is_none_or(|server| pending.server.as_ref() == Some(server));
            !(request_matches && server_matches)
        });
    }

    fn fail_operation(&mut self, request: RequestId, message: &str) -> bool {
        let matched = self
            .operations
            .iter()
            .any(|pending| pending.request == request);
        if matched {
            self.operations.retain(|pending| pending.request != request);
            self.operation_error = Some(message.to_owned());
        }
        matched
    }
}

impl App {
    /// The active code file's language-server lifecycle badge, when covered.
    pub(crate) fn active_language_server_badge(&self) -> Option<LanguageServerBadge> {
        let TabKind::Code { path, language, .. } = &self.tabs.get(self.active)?.kind else {
            return None;
        };
        self.lsp_runtime.badge_for(path, language)
    }

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

        let request = self.send(SessionCommand::LanguageServerStatus);
        self.push_tab(Tab::language_servers(request));
        self.sync_language_server_operations();
    }

    fn language_servers_mut(&mut self) -> Option<&mut LanguageServersViewState> {
        let tab = self.tabs.get_mut(self.active)?;
        let TabKind::LanguageServers(view) = &mut tab.kind else {
            return None;
        };
        Some(view)
    }

    pub(in crate::app) fn selected_language_server(&self) -> Option<LanguageServerStatus> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::LanguageServers(view) = &tab.kind else {
            return None;
        };
        view.selected_server().cloned()
    }

    fn language_server(&self, server: &LanguageServerId) -> Option<LanguageServerStatus> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::LanguageServers(view) = &tab.kind else {
            return None;
        };
        view.servers
            .iter()
            .find(|status| status.server == *server)
            .cloned()
    }

    fn set_language_server_pending(
        &mut self,
        request: Option<RequestId>,
        server: Option<LanguageServerId>,
        kind: LanguageServerPendingKind,
    ) {
        let Some(request) = request else {
            return;
        };
        self.lsp_runtime.start_operation(LanguageServerPending {
            request,
            server,
            kind,
            downloaded: None,
            total: None,
        });
        self.sync_language_server_operations();
        self.sync_language_server_toast();
    }

    fn sync_language_server_operations(&mut self) {
        let operations = self.lsp_runtime.operations.clone();
        let error = self.lsp_runtime.operation_error.clone();
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                view.pending.clone_from(&operations);
                view.error.clone_from(&error);
            }
        }
    }

    pub(super) fn fail_language_server_operation(
        &mut self,
        request: RequestId,
        message: &str,
    ) -> bool {
        let failed = self.lsp_runtime.fail_operation(request, message);
        if failed {
            self.sync_language_server_operations();
            self.sync_language_server_toast();
            // Untagged on purpose. The progress cards share one tag so they can
            // update in place, and an outcome that joined them would be erased by
            // the next tick of any *other* running operation — losing exactly the
            // report the user most needs, since errors never auto-expire.
            self.notify(
                Severity::Error,
                NotificationKind::Lsp,
                format!("language server: {message}"),
            );
        }
        failed
    }

    pub(in crate::app) fn refresh_language_servers(&mut self) {
        let request = self.send(SessionCommand::LanguageServerStatus);
        if let Some(view) = self.language_servers_mut() {
            view.inventory_request = request;
            view.loading_since = Some(Pending::start());
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
        self.check_language_server(status.server);
    }

    fn check_language_server(&mut self, server: LanguageServerId) {
        let Some(status) = self.language_server(&server) else {
            return;
        };
        if !status.managed {
            self.status = Some(format!(
                "{} is supplied externally and has no Karet update channel",
                status.server.display_name()
            ));
            return;
        }
        let request = self.send(SessionCommand::CheckLanguageServerUpdates {
            server: Some(status.server.clone()),
        });
        self.set_language_server_pending(
            request,
            Some(status.server),
            LanguageServerPendingKind::CheckSelected,
        );
    }

    pub(super) fn check_all_language_servers(&mut self) {
        let request = self.send(SessionCommand::CheckLanguageServerUpdates { server: None });
        self.set_language_server_pending(request, None, LanguageServerPendingKind::CheckAll);
        self.status = Some("checking managed language servers for updates…".to_string());
    }

    pub(super) fn language_server_primary_action(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        self.language_server_primary_action_for(status.server);
    }

    fn language_server_primary_action_for(&mut self, server: LanguageServerId) {
        let Some(status) = self.language_server(&server) else {
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
            let install = change.current.is_none();
            self.apply_language_server_plan(plan, vec![change.server], install);
        } else if status.installed.is_none() {
            self.begin_language_server_install(status.server);
        } else {
            self.check_language_server(status.server);
        }
    }

    pub(super) fn restart_selected_language_server(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        self.restart_language_server(status.server);
    }

    fn restart_language_server(&mut self, server: LanguageServerId) {
        let Some(status) = self.language_server(&server) else {
            return;
        };
        if !status.instances.iter().any(|instance| {
            instance.open_documents > 0
                || !matches!(
                    instance.runtime,
                    karet_session::LanguageServerRuntimeState::Idle
                        | karet_session::LanguageServerRuntimeState::Stopped
                )
        }) {
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
        self.uninstall_language_server_prompt(status.server);
    }

    fn uninstall_language_server_prompt(&mut self, server: LanguageServerId) {
        let Some(status) = self.language_server(&server) else {
            return;
        };
        if !status.managed || status.installed.is_none() {
            self.status = Some(format!(
                "{} is not installed by Karet",
                status.server.display_name()
            ));
            return;
        }
        let name = status.server.display_name().to_string();
        let version = status.installed.clone().unwrap_or_default();
        self.confirm_action(
            format!("Uninstall {name}?"),
            format!(
                "Deactivates {name} {version} and retires its files. Documents in \
                 this language lose completions, diagnostics and go-to-definition \
                 until it is installed again."
            ),
            "Keep it installed",
            format!("Uninstall {name}"),
            ConfirmAction::UninstallLanguageServer(status.server),
        );
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

    pub(super) fn language_server_action(
        &mut self,
        action: LanguageServerAction,
        server: Option<LanguageServerId>,
    ) {
        match action {
            LanguageServerAction::Refresh => self.refresh_language_servers(),
            LanguageServerAction::CheckAll => self.check_all_language_servers(),
            LanguageServerAction::Primary => {
                if let Some(server) = server {
                    self.language_server_primary_action_for(server);
                } else {
                    self.language_server_primary_action();
                }
            },
            LanguageServerAction::Restart => {
                if let Some(server) = server {
                    self.restart_language_server(server);
                } else {
                    self.restart_selected_language_server();
                }
            },
            LanguageServerAction::Uninstall => {
                if let Some(server) = server {
                    self.uninstall_language_server_prompt(server);
                } else {
                    self.uninstall_selected_language_server();
                }
            },
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
                .find(|hit| rect_contains(hit.rect, (column, row)))
                .cloned()
        });
        if let Some(hit) = action {
            if let Some(server) = hit.server.as_ref()
                && let Some(view) = self.language_servers_mut()
                && let Some(selected) = view.visible_indices().iter().position(|&index| {
                    view.servers
                        .get(index)
                        .is_some_and(|status| status.server == *server)
                })
            {
                view.selected = selected;
            }
            self.language_server_action(hit.action, hit.server);
            return true;
        }
        let Some(view) = self.language_servers_mut() else {
            return false;
        };
        if !rect_contains(view.table_rect, (column, row)) {
            return true;
        }
        if let Some(server) = view
            .row_hits
            .iter()
            .find_map(|(rect, server)| rect_contains(*rect, (column, row)).then(|| server.clone()))
            && let Some(selected) = view.visible_indices().iter().position(|&index| {
                view.servers
                    .get(index)
                    .is_some_and(|status| status.server == server)
            })
        {
            view.selected = selected;
        }
        true
    }

    pub(super) fn update_language_server_hover(&mut self, column: u16, row: u16) {
        if let Some(view) = self.language_servers_mut() {
            let point = (column, row);
            view.action_hover = view
                .action_hits
                .iter()
                .any(|hit| rect_contains(hit.rect, point))
                .then_some(point);
        }
    }

    pub(super) fn begin_language_server_install(&mut self, server: LanguageServerId) {
        let request = self.send(SessionCommand::InstallLanguageServer {
            server: server.clone(),
        });
        self.set_language_server_pending(request, Some(server), LanguageServerPendingKind::Install);
    }

    pub(super) fn apply_language_server_plan(
        &mut self,
        plan: LanguageServerPlanId,
        servers: Vec<LanguageServerId>,
        install: bool,
    ) {
        let target = (servers.len() == 1).then(|| servers[0].clone());
        let request = self.send(SessionCommand::ApplyLanguageServerPlan { plan, servers });
        self.set_language_server_pending(
            request,
            target,
            if install {
                LanguageServerPendingKind::Install
            } else {
                LanguageServerPendingKind::Update
            },
        );
    }

    pub(super) fn begin_language_server_uninstall(&mut self, server: LanguageServerId) {
        let request = self.send(SessionCommand::UninstallLanguageServer {
            server: server.clone(),
        });
        self.set_language_server_pending(
            request,
            Some(server),
            LanguageServerPendingKind::Uninstall,
        );
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
        self.lsp_runtime.replace(request, servers.clone());
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
        request: Option<RequestId>,
        plan: LanguageServerPlanId,
        changes: Vec<LanguageServerChange>,
    ) {
        if let Some(request) = request {
            self.lsp_runtime.finish_operation(Some(request), None);
        }
        let mut manager_open = false;
        let mut adopted = false;
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                manager_open = true;
                let matches = request.is_none()
                    || view
                        .pending
                        .iter()
                        .any(|pending| Some(pending.request) == request);
                if !matches {
                    continue;
                }
                adopted = true;
                view.plan = Some(plan);
                view.changes.clone_from(&changes);
                view.pending
                    .retain(|pending| Some(pending.request) != request);
                view.loading_since = None;
                view.error = None;
            }
        }
        self.sync_language_server_operations();
        if manager_open && !adopted {
            return;
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
        let bytes: u64 = changes
            .iter()
            .filter_map(|change| change.download_bytes)
            .sum();
        let size = if bytes > 0 {
            format!(" Downloads about {}.", human_bytes(bytes))
        } else {
            String::new()
        };
        let count = changes.len();
        self.confirm(ConfirmDialog::new(
            if count == 1 {
                "Update this language server?".to_string()
            } else {
                format!("Update {count} language servers?")
            },
            format!("Applies exactly these versions: {summary}.{size}"),
            vec![
                ConfirmChoice::custom("Keep current versions", ConfirmAction::Cancel),
                ConfirmChoice::custom(
                    "Update",
                    ConfirmAction::ApplyLanguageServerPlan {
                        plan,
                        servers: changes.iter().map(|change| change.server.clone()).collect(),
                    },
                ),
            ],
        ));
    }

    pub(super) fn show_language_server_progress(
        &mut self,
        server: LanguageServerId,
        downloaded: u64,
        total: Option<u64>,
    ) {
        self.lsp_runtime.update_progress(&server, downloaded, total);
        self.sync_language_server_operations();
        self.sync_language_server_toast();
    }

    pub(super) fn finish_language_server_change(
        &mut self,
        request: Option<RequestId>,
        server: LanguageServerId,
        version: String,
        _restart_required: bool,
    ) {
        self.lsp_runtime.finish_operation(request, Some(&server));
        self.sync_language_server_toast();
        // Untagged: an outcome must survive another operation's next progress
        // tick (see `fail_language_server_operation`).
        self.notify(
            Severity::Information,
            NotificationKind::Lsp,
            format!("{} {version} is ready", server.display_name()),
        );
        if let Some(status) = self
            .lsp_runtime
            .servers
            .iter_mut()
            .find(|item| item.server == server)
        {
            status.installed = Some(version.clone());
            status.cleanup_pending = false;
        }
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
            }
        }
        self.sync_language_server_operations();
    }

    pub(super) fn finish_language_server_remove(
        &mut self,
        request: Option<RequestId>,
        server: LanguageServerId,
        cleanup_pending: bool,
    ) {
        self.lsp_runtime.finish_operation(request, Some(&server));
        self.sync_language_server_toast();
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
        self.sync_language_server_operations();
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
        let cached = self.lsp_runtime.update(&server, &root, state, &error);
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
        if (!cached || missing_instance) && self.lsp_runtime.inventory_request.is_none() {
            let request = self.send(SessionCommand::LanguageServerStatus);
            self.lsp_runtime.inventory_request = request;
            if missing_instance {
                for tab in self.all_tabs_mut() {
                    if let TabKind::LanguageServers(view) = &mut tab.kind {
                        view.inventory_request = request;
                    }
                }
            }
        }
        if let Some(error) = error
            && matches!(
                state,
                LanguageServerRuntimeState::Retrying
                    | LanguageServerRuntimeState::CircuitOpen
                    | LanguageServerRuntimeState::Unavailable
                    | LanguageServerRuntimeState::Stopped
            )
        {
            let (severity, state_label) = match state {
                LanguageServerRuntimeState::Retrying => (Severity::Warning, "retrying"),
                LanguageServerRuntimeState::CircuitOpen => {
                    (Severity::Error, "crashed (circuit open)")
                },
                LanguageServerRuntimeState::Unavailable => (Severity::Error, "unavailable"),
                LanguageServerRuntimeState::Stopped => (Severity::Error, "stopped"),
                _ => return,
            };
            self.notify_tagged(
                severity,
                NotificationKind::Lsp,
                format!("{} {state_label}: {error}", server.display_name()),
                Some(format!("lsp.runtime.{}.{}", server.key(), root.display())),
            );
        }
    }
}

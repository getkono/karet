mod badge;
mod progress;
mod prompts;

use super::*;
use crate::tab::LanguageServerAction;
use crate::tab::LanguageServerPending;
use crate::tab::LanguageServerPendingKind;
use crate::tab::LanguageServersViewState;

/// Lifecycle states retained independently of whether the manager tab is open.
#[derive(Default)]
pub(crate) struct LanguageServerRuntimeModel {
    /// The client's one copy of the inventory, sorted for display.
    pub(crate) servers: Vec<LanguageServerStatus>,
    inventory_request: Option<RequestId>,
    operations: Vec<LanguageServerPending>,
    operation_error: Option<String>,
}

pub(crate) use badge::LanguageServerBadge;
pub(crate) use badge::LanguageServerBadgeSummary;

impl LanguageServerRuntimeModel {
    /// Adopt an inventory, unless a newer request is already out for a better one.
    ///
    /// The session signals staleness whenever a retirement invalidates what it
    /// last said, and the client answers that by asking again. An answer
    /// computed before that signal describes the session as it was *before* the
    /// thing that made it stale, so it is refused: adopting it would cache
    /// exactly the state the signal existed to correct, and nothing afterwards
    /// would ask again.
    ///
    /// Refused only while a *different* request is outstanding, which is the
    /// only situation in which a better answer is known to be coming. With
    /// nothing pending there is no fresher answer to prefer, so the rows are
    /// taken.
    fn replace(
        &mut self,
        request: Option<RequestId>,
        mut servers: Vec<LanguageServerStatus>,
    ) -> bool {
        if self.inventory_request.is_some() && self.inventory_request != request {
            return false;
        }
        // Sorted here because the order is a property of the inventory, not of
        // any one view of it. Every view reads these rows.
        servers.sort_by_key(|status| status.server.display_name().to_lowercase());
        self.servers = servers;
        self.inventory_request = None;
        true
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

    fn badge_for(&self, path: &Path, language: &str) -> Option<LanguageServerBadgeSummary> {
        badge::badge_for(&self.servers, path, language)
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
    /// One tab's language-server badge, when it is a code file a provider covers.
    ///
    /// Takes a tab rather than reading the active one so a background pane can be
    /// badged too: every pane reports its own file, not the focused pane's.
    pub(crate) fn language_server_badge_for(
        &self,
        tab: &Tab,
    ) -> Option<LanguageServerBadgeSummary> {
        let TabKind::Code { path, language, .. } = &tab.kind else {
            return None;
        };
        self.lsp_runtime.badge_for(path, language)
    }

    /// The active code file's language-server badge, when covered.
    pub(crate) fn active_language_server_badge(&self) -> Option<LanguageServerBadgeSummary> {
        self.language_server_badge_for(self.tabs.get(self.active)?)
    }

    /// Re-read the local inventory, so the badge reflects a condition the backend
    /// has just discovered.
    ///
    /// Coalesced against an in-flight request: opening ten Go files reports ten
    /// unresolved providers, and each one arriving must not queue its own scan.
    /// The query performs no network I/O -- it reads settings, the project, `PATH`
    /// and the install journal.
    pub(in crate::app) fn refresh_language_server_inventory(&mut self) {
        if self.lsp_runtime.inventory_request.is_some() {
            return;
        }
        self.request_language_server_inventory();
    }

    /// Ask the session for a current inventory, and record the request in the one
    /// place every reader of the answer consults.
    ///
    /// The cache refuses any answer but the outstanding one, so a request issued
    /// without recording it here would have its answer dropped on the floor. One
    /// helper rather than three call sites each remembering to do it.
    fn request_language_server_inventory(&mut self) -> Option<RequestId> {
        let request = self.send(SessionCommand::LanguageServerStatus);
        self.lsp_runtime.inventory_request = request;
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
                view.inventory_request = request;
            }
        }
        request
    }

    /// The session says the inventory this client cached no longer describes it.
    ///
    /// Answering it with a fresh request is the whole protocol: the session sends
    /// no rows, because building them is expensive and most sessions have nobody
    /// looking at them. So only ask when something is actually reading the cache
    /// -- an open manager tab, or a populated cache the per-pane badges draw
    /// from. A client that never asked for an inventory has nothing to correct.
    ///
    /// Deliberately not coalesced against an in-flight request. That request's
    /// answer was computed before the signal, so it describes the session as it
    /// was before the retirement -- exactly what must not be cached.
    pub(in crate::app) fn language_server_inventory_stale(&mut self) {
        let reading = !self.lsp_runtime.servers.is_empty()
            || self
                .all_tabs()
                .any(|tab| matches!(tab.kind, TabKind::LanguageServers(_)));
        if !reading {
            return;
        }
        self.request_language_server_inventory();
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

        let request = self.request_language_server_inventory();
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

    fn language_servers(&self) -> Option<&LanguageServersViewState> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::LanguageServers(view) = &tab.kind else {
            return None;
        };
        Some(view)
    }

    /// Move the manager's selection onto `server`, if the filter admits it.
    ///
    /// Two phases because the rows and the view live in different fields: the
    /// row is found while both are borrowed immutably, and only the write takes
    /// the view mutably.
    fn focus_language_server(&mut self, server: &LanguageServerId) {
        let selected = self.language_servers().and_then(|view| {
            view.visible_indices(&self.lsp_runtime.servers)
                .iter()
                .position(|&index| {
                    self.lsp_runtime
                        .servers
                        .get(index)
                        .is_some_and(|status| status.server == *server)
                })
        });
        if let Some(selected) = selected
            && let Some(view) = self.language_servers_mut()
        {
            view.selected = selected;
        }
    }

    pub(in crate::app) fn selected_language_server(&self) -> Option<LanguageServerStatus> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::LanguageServers(view) = &tab.kind else {
            return None;
        };
        view.selected_server(&self.lsp_runtime.servers).cloned()
    }

    fn language_server(&self, server: &LanguageServerId) -> Option<LanguageServerStatus> {
        self.lsp_runtime
            .servers
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
                Report::Failure,
                NotificationKind::Lsp,
                format!("language server: {message}"),
            );
        }
        failed
    }

    pub(in crate::app) fn refresh_language_servers(&mut self) {
        self.request_language_server_inventory();
        if let Some(view) = self.language_servers_mut() {
            view.loading_since = Some(Pending::start());
            view.error = None;
        }
    }

    pub(super) fn language_server_select(&mut self, delta: i32) {
        let visible = self
            .language_servers()
            .map(|view| view.visible_indices(&self.lsp_runtime.servers).len());
        if let Some(visible) = visible
            && let Some(view) = self.language_servers_mut()
        {
            view.select_relative(visible, delta);
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
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!(
                    "{} is supplied externally and has no Karet update channel",
                    status.server.display_name()
                ),
            );
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
        // Deliberately silent: the check answers with "up to date" or "N updates
        // available" a moment later, and that outcome is the announcement. A card
        // for the request as well would say the same thing twice.
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
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!(
                    "{} is resolved from configuration or PATH",
                    status.server.display_name()
                ),
            );
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
        if !status.restartable() {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!(
                    "{} has no process in this session",
                    status.server.display_name()
                ),
            );
            return;
        }
        self.send_command(SessionCommand::RestartLanguageServer {
            server: status.server.clone(),
        });
        // `Activity`, not a tracked progress card: a restart registers no pending
        // operation, so nothing would ever retire a persistent card — and reusing
        // the download tag would clobber an install running alongside it.
        self.notify(
            Report::Activity,
            NotificationKind::Lsp,
            format!("restarting {}…", status.server.display_name()),
        );
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
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!("{} is not installed by Karet", status.server.display_name()),
            );
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
            if let Some(server) = hit.server.clone() {
                self.focus_language_server(&server);
            }
            self.language_server_action(hit.action, hit.server);
            return true;
        }
        let Some(view) = self.language_servers() else {
            return false;
        };
        if !rect_contains(view.table_rect, (column, row)) {
            return true;
        }
        let clicked = view
            .row_hits
            .iter()
            .find_map(|(rect, server)| rect_contains(*rect, (column, row)).then(|| server.clone()));
        if let Some(server) = clicked {
            self.focus_language_server(&server);
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
        // Each view remembers its selection by provider, so it has to be read
        // against the rows that were in force when the user made it -- before
        // the swap below replaces them.
        let anchors: Vec<Option<LanguageServerId>> = self
            .all_tabs()
            .filter_map(|tab| match &tab.kind {
                TabKind::LanguageServers(view) => Some(view.selected_id(&self.lsp_runtime.servers)),
                _ => None,
            })
            .collect();
        // One guard, here. Whether an answer is worth adopting is a fact about
        // the *client*, not about any one view of it, so it is decided once and
        // the views follow. Checking again per tab was defence in depth that
        // measured nothing: either check alone hid a defect in the other.
        if !self.lsp_runtime.replace(request, servers) {
            // Not the answer we are waiting for: a newer request is out, issued
            // because the session told us this one's rows are already stale.
            return;
        }
        // No card for the count: this only ever fills the Language Servers tab,
        // whose table already lists every server and whether it is available.
        let rows = self.lsp_runtime.servers.clone();
        let mut anchors = anchors.into_iter();
        for tab in self.all_tabs_mut() {
            let TabKind::LanguageServers(view) = &mut tab.kind else {
                continue;
            };
            view.resync(&rows, anchors.next().flatten());
        }
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
                Report::Outcome,
                NotificationKind::Lsp,
                "language servers are up to date",
            );
            return;
        }
        if manager_open {
            self.notify(
                Report::Outcome,
                NotificationKind::Lsp,
                format!("{} language-server update(s) available", changes.len()),
            );
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
            Report::Outcome,
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
        if let Some(status) = self
            .lsp_runtime
            .servers
            .iter_mut()
            .find(|item| item.server == server)
        {
            status.installed = None;
            status.cleanup_pending = cleanup_pending;
            status.instances.clear();
        }
        for tab in self.all_tabs_mut() {
            if let TabKind::LanguageServers(view) = &mut tab.kind {
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
            Report::Outcome,
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
        // The low-latency half of the resync, which issue #278 explicitly leaves
        // in place: the badge must not wait for a round trip to show that a
        // server started. It patches the one copy, and it patches only the two
        // fields the event actually carries -- everything else, `open_documents`
        // above all, is corrected by the inventory the session tells us to ask
        // for. This event being the *only* thing that ever corrected the cache
        // is what left a dead Restart on screen.
        let patched = self.lsp_runtime.update(&server, &root, state, &error);
        if !patched && self.lsp_runtime.inventory_request.is_none() {
            // A transition for a provider or root the cache has never heard of:
            // the inventory predates it, so ask for one that does not.
            self.request_language_server_inventory();
        }
        if let Some(error) = error
            && matches!(
                state,
                LanguageServerRuntimeState::Retrying
                    | LanguageServerRuntimeState::CircuitOpen
                    | LanguageServerRuntimeState::Unavailable
            )
        {
            let (severity, state_label) = match state {
                LanguageServerRuntimeState::Retrying => (Severity::Warning, "retrying"),
                LanguageServerRuntimeState::CircuitOpen => {
                    (Severity::Error, "crashed (circuit open)")
                },
                LanguageServerRuntimeState::Unavailable => (Severity::Error, "unavailable"),
                _ => return,
            };
            self.notify_tagged(
                Report::from_severity(severity),
                NotificationKind::Lsp,
                format!("{} {state_label}: {error}", server.display_name()),
                Some(format!("lsp.runtime.{}.{}", server.key(), root.display())),
            );
        }
    }
}

//! View-local state for the language-server manager tab.
//!
//! Split from `tab.rs` to keep it under the file-size ceiling.

use std::time::Instant;

use karet_session::LanguageServerChange;
use karet_session::LanguageServerId;
use karet_session::LanguageServerPlanId;
use karet_session::LanguageServerStatus;
use ratatui::layout::Rect;

/// A clickable operation in the language-server manager's action strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LanguageServerAction {
    Refresh,
    CheckAll,
    Primary,
    Restart,
    Uninstall,
    Filter,
}

/// One in-flight registry operation shown by the language-server manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LanguageServerPendingKind {
    CheckSelected,
    CheckAll,
    Install,
    Update,
    Uninstall,
}

/// Request correlation and presentation state for a registry operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LanguageServerPending {
    pub(crate) request: karet_session::RequestId,
    pub(crate) server: Option<LanguageServerId>,
    pub(crate) kind: LanguageServerPendingKind,
    pub(crate) downloaded: Option<u64>,
    pub(crate) total: Option<u64>,
}

/// A clickable manager action from the most recently rendered frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LanguageServerActionHit {
    pub(crate) rect: Rect,
    pub(crate) action: LanguageServerAction,
    pub(crate) server: Option<LanguageServerId>,
}

/// View-local inventory, selection, update-plan, and hit-testing state.
pub(crate) struct LanguageServersViewState {
    pub(crate) servers: Vec<LanguageServerStatus>,
    pub(crate) selected: usize,
    pub(crate) offset: usize,
    pub(crate) filter: String,
    pub(crate) loading_since: Option<Instant>,
    pub(crate) inventory_request: Option<karet_session::RequestId>,
    pub(crate) pending: Vec<LanguageServerPending>,
    pub(crate) plan: Option<LanguageServerPlanId>,
    pub(crate) changes: Vec<LanguageServerChange>,
    pub(crate) error: Option<String>,
    pub(crate) table_rect: Rect,
    pub(crate) action_hits: Vec<LanguageServerActionHit>,
    pub(crate) row_hits: Vec<(Rect, LanguageServerId)>,
    pub(crate) action_hover: Option<(u16, u16)>,
}

impl LanguageServersViewState {
    #[must_use]
    pub(crate) fn loading(inventory_request: Option<karet_session::RequestId>) -> Self {
        Self {
            servers: Vec::new(),
            selected: 0,
            offset: 0,
            filter: String::new(),
            loading_since: Some(Instant::now()),
            inventory_request,
            pending: Vec::new(),
            plan: None,
            changes: Vec::new(),
            error: None,
            table_rect: Rect::default(),
            action_hits: Vec::new(),
            row_hits: Vec::new(),
            action_hover: None,
        }
    }

    #[must_use]
    pub(crate) fn visible_indices(&self) -> Vec<usize> {
        let query = self.filter.trim().to_lowercase();
        self.servers
            .iter()
            .enumerate()
            .filter_map(|(index, status)| {
                (query.is_empty()
                    || status.server.display_name().to_lowercase().contains(&query)
                    || status
                        .languages
                        .iter()
                        .any(|language| language.to_lowercase().contains(&query)))
                .then_some(index)
            })
            .collect()
    }

    #[must_use]
    pub(crate) fn selected_server(&self) -> Option<&LanguageServerStatus> {
        let index = self.visible_indices().get(self.selected).copied()?;
        self.servers.get(index)
    }

    #[must_use]
    pub(crate) fn selected_id(&self) -> Option<LanguageServerId> {
        self.selected_server().map(|status| status.server.clone())
    }

    pub(crate) fn select_relative(&mut self, delta: i32) {
        let count = self.visible_indices().len();
        if count == 0 {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected =
            (self.selected as i64 + i64::from(delta)).clamp(0, (count - 1) as i64) as usize;
    }

    pub(crate) fn set_servers(&mut self, mut servers: Vec<LanguageServerStatus>) {
        let selected = self.selected_id();
        servers.sort_by_key(|status| status.server.display_name().to_lowercase());
        self.servers = servers;
        self.selected = selected
            .and_then(|server| {
                self.visible_indices().iter().position(|&index| {
                    self.servers
                        .get(index)
                        .is_some_and(|status| status.server == server)
                })
            })
            .unwrap_or(0);
        self.offset = self.offset.min(self.selected);
        self.loading_since = None;
        self.inventory_request = None;
        self.error = None;
    }
}

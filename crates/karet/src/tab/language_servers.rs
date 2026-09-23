//! View-local state for the language-server manager tab.
//!
//! Split from `tab.rs` to keep it under the file-size ceiling.

use karet_session::LanguageServerChange;
use karet_session::LanguageServerId;
use karet_session::LanguageServerPlanId;
use karet_session::LanguageServerStatus;
use ratatui::layout::Rect;

use crate::app::Pending;

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

impl LanguageServerPendingKind {
    /// Whether this operation is worth a notification of its own.
    ///
    /// Update *checks* are metadata requests the user asked for and watched
    /// happen; installs, updates and uninstalls change the machine and can run
    /// long, so they are the ones that owe an answer wherever the user is.
    pub(crate) fn is_download(self) -> bool {
        matches!(self, Self::Install | Self::Update | Self::Uninstall)
    }

    /// How to say this operation is under way.
    pub(crate) fn progressive(self) -> &'static str {
        match self {
            Self::CheckSelected | Self::CheckAll => "Checking",
            Self::Install => "Installing",
            Self::Update => "Updating",
            Self::Uninstall => "Uninstalling",
        }
    }
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
    pub(crate) selected: usize,
    pub(crate) offset: usize,
    pub(crate) filter: String,
    pub(crate) loading_since: Option<Pending>,
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
            selected: 0,
            offset: 0,
            filter: String::new(),
            loading_since: Some(Pending::start()),
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

    /// Which rows the filter admits, as indices into `servers`.
    ///
    /// Takes the rows rather than owning them: the inventory is the client's,
    /// held once on [`LanguageServerRuntimeModel`](crate::app::LanguageServerRuntimeModel),
    /// and this view holds only where the user is looking at it. Two copies of
    /// the rows is what issue #278 was filed about.
    #[must_use]
    pub(crate) fn visible_indices(&self, servers: &[LanguageServerStatus]) -> Vec<usize> {
        let query = self.filter.trim().to_lowercase();
        servers
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

    /// The row the user has selected, if the filter still admits one.
    #[must_use]
    pub(crate) fn selected_server<'a>(
        &self,
        servers: &'a [LanguageServerStatus],
    ) -> Option<&'a LanguageServerStatus> {
        let index = self.visible_indices(servers).get(self.selected).copied()?;
        servers.get(index)
    }

    /// The selected row's provider, which is how selection survives a refresh.
    #[must_use]
    pub(crate) fn selected_id(&self, servers: &[LanguageServerStatus]) -> Option<LanguageServerId> {
        self.selected_server(servers)
            .map(|status| status.server.clone())
    }

    /// Move the selection by `delta` within `visible`, the number of rows the
    /// filter currently admits.
    ///
    /// Takes the count rather than the rows so the caller can read it while the
    /// inventory is borrowed and still take this view mutably afterwards.
    pub(crate) fn select_relative(&mut self, visible: usize, delta: i32) {
        let count = visible;
        if count == 0 {
            self.selected = 0;
            self.offset = 0;
            return;
        }
        self.selected =
            (self.selected as i64 + i64::from(delta)).clamp(0, (count - 1) as i64) as usize;
    }

    /// Re-anchor this view on a freshly adopted inventory.
    ///
    /// `anchor` is the provider that was selected before the swap, read against
    /// the rows that were in force when the user selected it. Selection follows
    /// the provider rather than the row number, so a refresh that adds or drops
    /// a provider does not silently move the cursor onto a different one.
    pub(crate) fn resync(
        &mut self,
        servers: &[LanguageServerStatus],
        anchor: Option<LanguageServerId>,
    ) {
        self.selected = anchor
            .and_then(|server| {
                self.visible_indices(servers).iter().position(|&index| {
                    servers
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

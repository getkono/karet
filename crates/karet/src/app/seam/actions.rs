//! The Seam view's side of the application: opening it, feeding it, and driving it.
//!
//! Every backend answer is checked against what is currently open before it is applied.
//! A reader who has moved on must never be yanked back by a reply to a question they
//! have already abandoned.

use std::collections::HashSet;

use karet_core::NotificationKind;
use karet_session::api::Command as SessionCommand;
use karet_session::api::RequestId;
use karet_session::api::SeamEdgeView;
use karet_session::api::SeamNodeView;
use karet_session::api::SeamPreview;
use karet_session::api::SeamQueryError;
use karet_session::api::SeamSummary;
use karet_session::api::SeamSync;

use super::Reroot;
use super::SeamFocus;
use super::SeamViewState;
use crate::app::App;
use crate::app::Report;
use crate::app::pending::Pending;
use crate::tab::Tab;
use crate::tab::TabKind;

impl Tab {
    /// The title a Seam view on `root` carries.
    ///
    /// Known at the moment the tab appears and never revised: the strip is painted before
    /// any answer arrives, and a title that changed when one did would move the tab the
    /// reader is aiming at.
    fn seam_title(root: &std::path::Path) -> String {
        let named = |path: &std::path::Path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        };
        // Resolved when the path does not name itself: a session launched as `karet .`
        // has a root of `.`, and a tab reading "Seams" names nothing the reader chose.
        let name = named(root)
            .or_else(|| root.canonicalize().ok().as_deref().and_then(named))
            .unwrap_or_else(|| "Seams".to_owned());
        format!("⌗ {name}")
    }

    /// A Seam view reserved for `root`, with its index already requested.
    #[must_use]
    pub fn seam(root: std::path::PathBuf) -> Self {
        Self::new(
            Self::seam_title(&root),
            TabKind::Seam(Box::new(SeamViewState::pending(root))),
        )
    }

    /// Point an open Seam view at a different root.
    ///
    /// Mutated in place rather than replaced, so the tab keeps its view identity: moving
    /// where the view reads is not the same as closing it and opening another.
    pub(crate) fn repoint_seam(&mut self, root: std::path::PathBuf) {
        self.title = Self::seam_title(&root);
        self.kind = TabKind::Seam(Box::new(SeamViewState::pending(root)));
    }
}

impl App {
    /// The Seam view in the active tab, if that is what is open.
    pub(crate) fn active_seam(&mut self) -> Option<&mut SeamViewState> {
        match self.tabs.get_mut(self.active).map(|tab| &mut tab.kind) {
            Some(TabKind::Seam(state)) => Some(state),
            _ => None,
        }
    }

    /// The one Seam view, wherever it is shown.
    ///
    /// Backend answers land here rather than on the active tab. Indexing a repository
    /// takes seconds, and a reader who moved on while it ran must still get the tree they
    /// asked for. The distinction is the point of having both: input acts on the tab you
    /// are looking at, answers go to the tab that asked.
    pub(crate) fn seam_view(&mut self) -> Option<&mut SeamViewState> {
        self.all_tabs_mut().find_map(|tab| match &mut tab.kind {
            TabKind::Seam(state) => Some(&mut **state),
            _ => None,
        })
    }

    /// Where the one Seam tab lives, if it is open: its pane and its index within it.
    fn seam_tab_location(&self) -> Option<(karet_widgets::PaneId, usize)> {
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| matches!(tab.kind, TabKind::Seam(_)))
        {
            return Some((self.layout.focus(), index));
        }
        self.stored.iter().find_map(|(pane, stored)| {
            stored
                .tabs
                .iter()
                .position(|tab| matches!(tab.kind, TabKind::Seam(_)))
                .map(|index| (*pane, index))
        })
    }

    /// Offer the start points the Seam view could be opened on.
    ///
    /// Discovery runs here on the app thread rather than on the backend. The picker *is*
    /// the surface being opened, so rows arriving after it would move the selection under
    /// the reader's fingers, and withholding it until they arrived would be exactly the
    /// delayed surface a picker must never be. Discovery only reads manifests and lists
    /// directories, and it is bounded — the app already runs a heavier walk than this
    /// synchronously to open the quick-open picker.
    pub(crate) fn open_seam_view_picker(&mut self) {
        let items = self.seam_root_candidates();
        self.overlay = Some(crate::overlay::Overlay::seam_roots(items));
    }

    /// The start points on offer: the reader's context, and what discovery found.
    fn seam_root_candidates(&mut self) -> Vec<(String, std::path::PathBuf)> {
        let discovered = karet_seam::discover(&self.root, karet_seam::DiscoveryOptions::default());
        let current = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .and_then(std::path::Path::parent)
            .map(std::path::Path::to_path_buf);
        let explorer = self.explorer_seam_root();
        super::roots::candidates(&self.root.clone(), current, explorer, discovered)
    }

    /// The explorer's selection as a directory, when the panel has one.
    fn explorer_seam_root(&mut self) -> Option<std::path::PathBuf> {
        self.explorer.ensure_built(&self.root);
        let row = self.explorer.selected()?;
        if row.is_dir {
            return Some(row.path.clone());
        }
        row.path.parent().map(std::path::Path::to_path_buf)
    }

    /// Open the Seam view on the workspace root.
    pub(crate) fn open_seam_view(&mut self) {
        self.open_seam_view_at(self.root.clone());
    }

    /// Open the Seam view on `root`.
    ///
    /// The tab is reserved and shown immediately; the index fills in behind it, so the
    /// pane switches at once rather than after a parse of every file under the root.
    ///
    /// An open Seam view is re-pointed rather than joined by a second. One index sits
    /// behind the view, so two of them on different roots would answer each other's
    /// questions — and a view that failed holds nothing worth keeping beside the new one.
    pub(crate) fn open_seam_view_at(&mut self, root: std::path::PathBuf) {
        match self.seam_tab_location() {
            Some((pane, index)) => {
                self.focus_pane_switch(pane);
                self.set_active(index);
                self.focus = crate::app::Focus::Editor;
                if let Some(tab) = self.tabs.get_mut(index) {
                    tab.repoint_seam(root.clone());
                }
            },
            None => self.push_tab(Tab::seam(root.clone())),
        }
        self.apply_seam_settings();
        self.request_seam_index(root, SeamSync::Incremental);
    }

    /// Re-index what changed, keeping everything the stored index still describes.
    pub(crate) fn seam_sync(&mut self) {
        self.start_seam_sync(SeamSync::Incremental);
    }

    /// Throw the stored index away and read every file again.
    ///
    /// The recourse when the stored index is itself suspect. Nothing else can detect that,
    /// which is why it is a button rather than something inferred.
    pub(crate) fn seam_force_sync(&mut self) {
        self.start_seam_sync(SeamSync::Forced);
    }

    /// Start a sync of the open Seam view.
    fn start_seam_sync(&mut self, mode: SeamSync) {
        let Some(root) = self.seam_view().map(|state| state.root.clone()) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: open the Seam view first",
            );
            return;
        };
        if let Some(state) = self.seam_view() {
            state.begin_sync();
        }
        self.request_seam_index(root, mode);
        // Only once the request is actually out: the card is retired by the
        // answering index, which a closed backend will never send.
        if self.seam_index_req.is_some() {
            self.notify_progress(
                NotificationKind::System,
                Self::SEAM_SYNC_TAG.to_string(),
                match mode {
                    SeamSync::Incremental => "seam: syncing…",
                    SeamSync::Forced => "seam: rebuilding from source…",
                },
                None,
            );
        }
    }

    /// Send the index request and record what the view is waiting on.
    fn request_seam_index(&mut self, root: std::path::PathBuf, mode: SeamSync) {
        self.seam_index_req = self.send(SessionCommand::IndexSeams {
            root: Some(root),
            mode,
        });
    }

    /// Seed a freshly opened view from the reader's settings.
    fn apply_seam_settings(&mut self) {
        let settings = self.settings.seam.clone();
        let Some(state) = self.active_seam() else {
            return;
        };
        state.lens_filter = match settings.lens_filter {
            karet_session::config::SeamLensFilter::Demote => super::LensFilter::Demote,
            karet_session::config::SeamLensFilter::Hide => super::LensFilter::Hide,
        };
        state.hide_inactive = settings.hide_inactive;
        state.spine = settings.spine;
        for lens in &settings.default_lenses {
            if let Some(known) = super::LENS_NAMES.iter().find(|name| *name == lens) {
                state.lenses.insert(known);
            }
        }
    }

    /// Re-index a file that changed, when the index actually holds it.
    ///
    /// Scoped by membership rather than by containment: the root may be a whole
    /// repository, and every file in it sits under that root — so asking "is it beneath
    /// us?" would re-index the repository on every save.
    pub(crate) fn reindex_seams(&mut self, path: &std::path::Path, text: String) {
        if !self.seam_view().is_some_and(|state| state.covers(path)) {
            return;
        }
        self.seam_index_req = self.send(SessionCommand::ReindexSeams {
            path: path.to_path_buf(),
            text,
        });
    }

    /// Re-index the Seam view after a document it covers is saved.
    ///
    /// On save rather than on every keystroke: the index describes what is on disk,
    /// and re-parsing a package mid-word would spend the reader's machine on an answer
    /// they have not finished asking for.
    pub(crate) fn reindex_saved_seam(&mut self, doc: karet_session::api::DocumentId) {
        let Some((path, text)) = self.all_tabs().find_map(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(id),
                path,
                text,
                ..
            } if *id == doc => Some((path.clone(), text.clone())),
            _ => None,
        }) else {
            return;
        };
        self.reindex_seams(&path, text);
    }

    /// Copy the query this view is equivalent to.
    ///
    /// The other half of the agent story: an agent can hand the reader a query, and the
    /// reader can hand an agent exactly what they are looking at.
    pub(crate) fn seam_copy_query(&mut self) {
        let Some(query) = self.active_seam().map(|state| state.as_query()) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: open the Seam view first",
            );
            return;
        };
        if query.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: nothing is narrowed — the query would be empty",
            );
            return;
        }
        self.copy_to_clipboard(query, "seam query");
    }

    /// Offer the configurations this package can be read under.
    pub(crate) fn seam_configuration(&mut self) {
        let Some(state) = self.active_seam() else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: open the Seam view first",
            );
            return;
        };
        let available = state.summary.available_configurations.clone();
        let active = state.summary.configuration.clone();
        let Some(next) = next_configuration(&available, &active) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: only one configuration is available",
            );
            return;
        };
        self.seam_index_req = self.send(SessionCommand::SetSeamConfiguration { name: next });
    }

    /// Copy the selected node's identity — its citation form.
    pub(crate) fn seam_copy_identity(&mut self) {
        let Some(id) = self
            .active_seam()
            .and_then(|state| state.selected_id().map(str::to_owned))
        else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "seam: nothing selected",
            );
            return;
        };
        self.copy_to_clipboard(id, "node identity");
    }

    /// Adopt a freshly indexed tree, if this answer is still the one being awaited.
    pub(crate) fn on_seam_indexed(
        &mut self,
        id: Option<RequestId>,
        summary: SeamSummary,
        nodes: Vec<SeamNodeView>,
    ) {
        if !self.awaiting_seam_index(id) {
            return;
        }
        let Some(state) = self.seam_view() else {
            return;
        };
        state.adopt(summary, nodes);
        // Land on something rather than an empty selection, so the facet pane has
        // content the moment the tree arrives.
        if state.selection.is_empty() {
            state.move_row(0);
            // Ask for what the reader just landed on. Without this the detail pane stays
            // empty until they press a key, which reads as a view that failed to load.
            self.request_seam_node();
        }
    }

    /// Merge one package that has finished indexing.
    ///
    /// Arrives while the request is still outstanding, so the request id is checked
    /// without being cleared — the run is not over until `SeamIndexFinished`.
    pub(crate) fn on_seam_package_indexed(
        &mut self,
        id: Option<RequestId>,
        order: usize,
        root: &str,
        nodes: Vec<SeamNodeView>,
        unresolved: Vec<(String, Vec<std::path::PathBuf>)>,
    ) {
        if id.is_none() || id != self.seam_index_req {
            return;
        }
        let Some(state) = self.seam_view() else {
            return;
        };
        let first = state.selection.is_empty();
        state.adopt_package(order, root, nodes, unresolved);
        // Land on something as soon as there is something to land on, so the facet pane
        // has content from the first package rather than from the last.
        if first {
            if let Some(state) = self.seam_view() {
                state.move_row(0);
            }
            self.request_seam_node();
        }
    }

    /// Settle the view once every package is in.
    pub(crate) fn on_seam_index_finished(
        &mut self,
        id: Option<RequestId>,
        summary: SeamSummary,
        parsed: usize,
        files: usize,
    ) {
        if !self.awaiting_seam_index(id) {
            return;
        }
        let syncing = self.seam_view().and_then(|state| state.syncing).is_some();
        if let Some(state) = self.seam_view() {
            state.finish_sync(summary);
            if state.selection.is_empty() {
                state.move_row(0);
            }
        }
        // Said only for a sync the reader asked for: the first index of a view is not a
        // report about what changed, and announcing "0 of 524 files" on open would be
        // noise about work nobody requested.
        if syncing {
            self.notify_tagged(
                Report::Outcome,
                NotificationKind::System,
                match parsed {
                    0 => format!("seam: up to date ({files} files)"),
                    1 => format!("seam: re-read 1 of {files} files"),
                    _ => format!("seam: re-read {parsed} of {files} files"),
                },
                Some(Self::SEAM_SYNC_TAG.to_string()),
            );
        } else {
            // A first index reports nothing, but it still raised a progress card.
            self.notifications.dismiss_tagged(Self::SEAM_SYNC_TAG);
        }
    }

    /// Record that the root could not be indexed.
    pub(crate) fn on_seam_index_failed(&mut self, id: Option<RequestId>, message: String) {
        if !self.awaiting_seam_index(id) {
            return;
        }
        // The view owns the failure text; the card only has to stop claiming the
        // index is still running.
        self.notifications.dismiss_tagged(Self::SEAM_SYNC_TAG);
        if let Some(state) = self.seam_view() {
            state.fail(message);
        }
    }

    /// Whether `id` answers the index request this view is still waiting on.
    ///
    /// Opening at one root and immediately at another leaves the first index running; its
    /// answer must not land on the view that replaced it. One field serves all three
    /// commands that answer with a tree, since only one of them can be outstanding.
    fn awaiting_seam_index(&mut self, id: Option<RequestId>) -> bool {
        if id.is_none() || id != self.seam_index_req {
            return false;
        }
        self.seam_index_req = None;
        true
    }

    /// Apply a query result, keeping a parse failure distinct from an empty match.
    pub(crate) fn on_seam_query_result(
        &mut self,
        id: Option<RequestId>,
        nodes: Vec<String>,
        error: Option<SeamQueryError>,
    ) {
        if id.is_none() || id != self.seam_query_req {
            return;
        }
        self.seam_query_req = None;
        let Some(state) = self.seam_view() else {
            return;
        };
        if error.is_some() {
            // An unreadable query must not silently filter the tree to nothing, which
            // would look exactly like a query that matched nothing.
            state.query_error = error;
            state.query_matches = None;
            return;
        }
        state.query_error = None;
        state.query_matches = Some(nodes.into_iter().collect::<HashSet<_>>());
    }

    /// Attach a node's detail to the node it belongs to, ignoring a reply for another.
    pub(crate) fn on_seam_node_detail(
        &mut self,
        id: Option<RequestId>,
        node: String,
        edges: Vec<SeamEdgeView>,
        preview: Result<SeamPreview, String>,
    ) {
        if id.is_none() || id != self.seam_node_req {
            return;
        }
        self.seam_node_req = None;
        let Some(state) = self.seam_view() else {
            return;
        };
        // A stale reply, for a node the reader has already navigated away from.
        if state.selected_id() != Some(node.as_str()) {
            return;
        }
        state.edges = edges;
        state.preview = Some(preview);
        state.detail_since = None;
        state.facet_row = 0;
    }

    /// Ask the backend for the selected node's edges and source.
    pub(crate) fn request_seam_node(&mut self) {
        let Some(id) = self
            .active_seam()
            .and_then(|state| state.selected_id().map(str::to_owned))
        else {
            return;
        };
        if let Some(state) = self.active_seam() {
            // The block's rows are already reserved on screen; only the shared reveal
            // delay decides whether they say anything while the answer is on its way.
            state.detail_since = Some(Pending::start());
        }
        self.seam_node_req = self.send(SessionCommand::SeamNode { path: id });
    }

    /// Send the current query text for evaluation.
    pub(crate) fn submit_seam_query(&mut self) {
        let Some(text) = self.active_seam().map(|state| state.query.clone()) else {
            return;
        };
        if text.trim().is_empty() {
            if let Some(state) = self.active_seam() {
                state.query_matches = None;
                state.query_error = None;
            }
            return;
        }
        self.seam_query_req = self.send(SessionCommand::SeamQuery { text });
    }

    /// Open the selected node's source in an ordinary editor tab.
    ///
    /// The escape hatch every node offers: the Seam view answers a different question
    /// than the editor, and the reader has to be able to cross back at any point.
    pub(crate) fn open_seam_selection(&mut self) {
        let Some((path, position)) = self.active_seam().and_then(|state| {
            let node = state.selected()?;
            Some((node.file.clone(), node.selection.start))
        }) else {
            return;
        };
        self.jump_to_location(&path, position);
    }

    /// Move the selection within the focused column, then refresh its detail.
    pub(crate) fn seam_move_row(&mut self, delta: isize) {
        let Some(state) = self.active_seam() else {
            return;
        };
        if state.focus == SeamFocus::Facets {
            // In the facet pane the same keys walk the edge list instead.
            state.move_facet_row(delta);
            return;
        }
        state.move_row(delta);
        self.request_seam_node();
    }

    /// Move focus between columns.
    pub(crate) fn seam_move_column(&mut self, delta: isize) {
        let Some(state) = self.active_seam() else {
            return;
        };
        state.move_column(delta);
        self.request_seam_node();
    }

    /// Reroot at the selection, or follow the selected edge from the facet pane.
    pub(crate) fn seam_enter(&mut self) {
        let Some(state) = self.active_seam() else {
            return;
        };
        if state.focus == SeamFocus::Facets {
            self.pivot_seam_edge();
            return;
        }
        if state.focus == SeamFocus::Query {
            self.submit_seam_query();
            return;
        }
        match state.reroot() {
            Reroot::Narrowed | Reroot::Descended => self.request_seam_node(),
            // Nothing to descend into, so Enter falls through to the escape hatch
            // rather than doing nothing at all.
            Reroot::Refused => self.open_seam_selection(),
        }
    }

    /// Step back out of the most recent narrowing.
    pub(crate) fn seam_widen(&mut self) {
        let Some(state) = self.active_seam() else {
            return;
        };
        if state.widen() {
            self.request_seam_node();
        }
    }

    /// Move focus between the spine and the facet pane.
    pub(crate) fn seam_toggle_focus(&mut self) {
        if let Some(state) = self.active_seam() {
            state.focus = match state.focus {
                SeamFocus::Spine => SeamFocus::Facets,
                SeamFocus::Facets | SeamFocus::Query => SeamFocus::Spine,
            };
        }
    }

    /// Focus the query box.
    pub(crate) fn seam_focus_query(&mut self) {
        if let Some(state) = self.active_seam() {
            state.focus = SeamFocus::Query;
        }
    }

    /// Leave the query box, or clear the query when already outside it.
    ///
    /// Two steps, because leaving the box and discarding what was typed are different
    /// intentions and the first must not perform the second.
    pub(crate) fn seam_escape(&mut self) {
        let Some(state) = self.active_seam() else {
            return;
        };
        if state.focus == SeamFocus::Query {
            state.focus = SeamFocus::Spine;
            return;
        }
        state.query.clear();
        state.query_matches = None;
        state.query_error = None;
    }

    /// Toggle one lens by its position in the legend.
    pub(crate) fn seam_toggle_lens(&mut self, index: usize) {
        let Some(lens) = super::LENS_NAMES.get(index).copied() else {
            return;
        };
        if let Some(state) = self.active_seam() {
            state.toggle_lens(lens);
        }
    }

    /// Clear every lens filter.
    pub(crate) fn seam_clear_lenses(&mut self) {
        if let Some(state) = self.active_seam() {
            state.clear_lenses();
        }
    }

    /// Type one character into the query box.
    pub(crate) fn seam_query_char(&mut self, ch: char) {
        if let Some(state) = self.active_seam()
            && state.focus == SeamFocus::Query
        {
            state.query.push(ch);
        }
        self.submit_seam_query();
    }

    /// Delete the last character of the query.
    pub(crate) fn seam_query_backspace(&mut self) {
        if let Some(state) = self.active_seam()
            && state.focus == SeamFocus::Query
        {
            state.query.pop();
        }
        self.submit_seam_query();
    }

    /// Whether the Seam view currently wants raw characters.
    #[must_use]
    pub(crate) fn seam_query_focused(&mut self) -> bool {
        self.active_seam()
            .is_some_and(|state| state.focus == SeamFocus::Query)
    }

    /// Follow the edge the facet pane has selected.
    pub(crate) fn pivot_seam_edge(&mut self) {
        let Some(state) = self.active_seam() else {
            return;
        };
        let Some(edge) = state.edges.get(state.facet_row).cloned() else {
            return;
        };
        let Some(target) = edge.target.clone() else {
            // An external or unresolved endpoint has nothing in this tree to reroot on,
            // and saying so beats a view that silently does nothing.
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                format!("seam: {} points outside this package", edge.kind),
            );
            return;
        };
        let from = state.selected_id().unwrap_or_default().to_owned();
        if state.pivot(&edge.kind, &from, vec![target.clone()]) {
            self.request_seam_node();
        } else {
            // Refusing silently would look like a dropped keypress; the reader is already
            // looking at what the pivot would have shown them.
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                format!("seam: already scoped to {target}"),
            );
        }
    }
}

/// The configuration after `active`, wrapping around.
///
/// Cycling rather than opening a picker: with two or three configurations a picker is
/// more ceremony than the choice deserves, and the header always names the current one.
#[must_use]
pub(crate) fn next_configuration(available: &[String], active: &str) -> Option<String> {
    if available.len() < 2 {
        return None;
    }
    let current = available
        .iter()
        .position(|name| name == active)
        .unwrap_or(0);
    available.get((current + 1) % available.len()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configurations_cycle_and_wrap() {
        let available = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(next_configuration(&available, "a").as_deref(), Some("b"));
        assert_eq!(next_configuration(&available, "c").as_deref(), Some("a"));
        // An unknown active name starts the cycle rather than refusing.
        assert_eq!(next_configuration(&available, "?").as_deref(), Some("b"));
    }

    #[test]
    fn a_single_configuration_offers_nothing_to_switch_to() {
        assert_eq!(next_configuration(&["only".to_owned()], "only"), None);
        assert_eq!(next_configuration(&[], ""), None);
    }
}

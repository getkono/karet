//! The Seam view's side of the application: opening it, feeding it, and driving it.
//!
//! Every backend answer is checked against what is currently open before it is applied.
//! A reader who has moved on must never be yanked back by a reply to a question they
//! have already abandoned.

use std::collections::HashSet;

use karet_session::api::Command as SessionCommand;
use karet_session::api::SeamEdgeView;
use karet_session::api::SeamNodeView;
use karet_session::api::SeamQueryError;
use karet_session::api::SeamSummary;

use super::SeamFocus;
use super::SeamViewState;
use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

impl Tab {
    /// A Seam view reserved for `root`, with its index already requested.
    #[must_use]
    pub fn seam(root: std::path::PathBuf) -> Self {
        let title = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Seams")
            .to_owned();
        Self::new(
            format!("⌗ {title}"),
            TabKind::Seam(Box::new(SeamViewState::pending(root))),
        )
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

    /// Open the Seam view on the workspace root.
    ///
    /// The tab is reserved and shown immediately; the index fills in behind it, so the
    /// pane switches at once rather than after a parse of every file in the package.
    pub(crate) fn open_seam_view(&mut self) {
        let root = self.root.clone();
        self.push_tab(Tab::seam(root.clone()));
        self.apply_seam_settings();
        self.seam_index_req = self.send(SessionCommand::IndexSeams { root: Some(root) });
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

    /// Re-index a file that changed, when it belongs to the package being read.
    ///
    /// Scoped to the package: an edit somewhere else in the workspace has nothing to do
    /// with this tree, and re-indexing on it would be work the reader never asked for.
    pub(crate) fn reindex_seams(&mut self, path: &std::path::Path, text: String) {
        let inside = self
            .active_seam()
            .is_some_and(|state| path.starts_with(&state.root));
        if !inside {
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
            self.status = Some("seam: open the Seam view first".to_owned());
            return;
        };
        if query.is_empty() {
            self.status = Some("seam: nothing is narrowed — the query would be empty".to_owned());
            return;
        }
        self.copy_to_clipboard(query, "seam query");
    }

    /// Offer the configurations this package can be read under.
    pub(crate) fn seam_configuration(&mut self) {
        let Some(state) = self.active_seam() else {
            self.status = Some("seam: open the Seam view first".to_owned());
            return;
        };
        let available = state.summary.available_configurations.clone();
        let active = state.summary.configuration.clone();
        let Some(next) = next_configuration(&available, &active) else {
            self.status = Some("seam: only one configuration is available".to_owned());
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
            self.status = Some("seam: nothing selected".to_owned());
            return;
        };
        self.copy_to_clipboard(id, "node identity");
    }

    /// Adopt a freshly indexed tree, if this answer is still the one being awaited.
    pub(crate) fn on_seam_indexed(&mut self, summary: SeamSummary, nodes: Vec<SeamNodeView>) {
        let Some(state) = self.active_seam() else {
            return;
        };
        state.adopt(summary, nodes);
        // Land on something rather than an empty selection, so the facet pane has
        // content the moment the tree arrives.
        if state.selection.is_empty() {
            state.move_row(0);
        }
    }

    /// Record that the package could not be indexed.
    pub(crate) fn on_seam_index_failed(&mut self, message: String) {
        if let Some(state) = self.active_seam() {
            state.fail(message);
        }
    }

    /// Apply a query result, keeping a parse failure distinct from an empty match.
    pub(crate) fn on_seam_query_result(
        &mut self,
        nodes: Vec<String>,
        error: Option<SeamQueryError>,
    ) {
        let Some(state) = self.active_seam() else {
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

    /// Attach edges to the node they belong to, ignoring a reply for another.
    pub(crate) fn on_seam_node_detail(&mut self, node: String, edges: Vec<SeamEdgeView>) {
        let Some(state) = self.active_seam() else {
            return;
        };
        // A stale reply, for a node the reader has already navigated away from.
        if state.selected_id() != Some(node.as_str()) {
            return;
        }
        state.edges = edges;
        state.facet_row = 0;
    }

    /// Ask the backend for the selected node's edges.
    pub(crate) fn request_seam_node(&mut self) {
        let Some(id) = self
            .active_seam()
            .and_then(|state| state.selected_id().map(str::to_owned))
        else {
            return;
        };
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
            let last = state.edges.len().saturating_sub(1);
            state.facet_row = state.facet_row.saturating_add_signed(delta).min(last);
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
        if state.reroot() {
            self.request_seam_node();
        } else {
            // Nothing to descend into, so Enter falls through to the escape hatch
            // rather than doing nothing at all.
            self.open_seam_selection();
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
            self.status = Some(format!("seam: {} points outside this package", edge.kind));
            return;
        };
        let from = state.selected_id().unwrap_or_default().to_owned();
        if state.pivot(&edge.kind, &from, vec![target]) {
            self.request_seam_node();
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

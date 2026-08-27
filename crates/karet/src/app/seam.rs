//! The Seam view's state and navigation, kept separate from its painting.
//!
//! Everything here is a pure function of the indexed tree and what the reader has done to
//! it, which is what makes the interaction testable without a terminal. The renderer in
//! [`crate::ui::seam`] reads this and draws; it decides nothing.
//!
//! Two rules shape the whole model.
//!
//! **Every narrow is reversible, and the way back is visible.** Rerooting, toggling a
//! lens, and typing a query all push onto one stack that the breadcrumb renders, so the
//! reader can always see how they got here and step back out. A narrowing you cannot undo
//! is a trap, and one you can undo but cannot see is a maze.
//!
//! **State is keyed on node identity, never position.** A rename invalidates that node's
//! place in the view and nothing else — selection falls back to the nearest surviving
//! ancestor rather than to whatever now occupies row four.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use karet_session::api::SeamEdgeView;
use karet_session::api::SeamNodeView;
use karet_session::api::SeamQueryError;
use karet_session::api::SeamSummary;

use super::pending::Pending;

/// The five lens names, in display order. Mirrors `karet_seam::LENSES` without taking a
/// dependency on the engine from the presentation layer.
pub(crate) const LENS_NAMES: [&str; 5] = ["api", "substitution", "variation", "boundary", "hazard"];

/// What has keyboard focus inside the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SeamFocus {
    /// The cascading columns.
    #[default]
    Spine,
    /// The facet pane, where edges are jump targets.
    Facets,
    /// The query box.
    Query,
}

/// How a lens filter treats non-matching rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LensFilter {
    /// Dim them, so the tree keeps its shape.
    #[default]
    Demote,
    /// Remove them.
    Hide,
}

/// One entry on the narrow stack — how the reader got to the current view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Narrow {
    /// Rerooted at a node.
    Scope(String),
    /// Rerooted on the far end of an edge.
    Pivot {
        /// The relation followed.
        edge: String,
        /// Where it was followed from.
        from: String,
        /// What it reached.
        targets: Vec<String>,
    },
}

impl Narrow {
    /// The label shown in the breadcrumb.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Scope(id) => id.rsplit("::").next().unwrap_or(id).to_owned(),
            Self::Pivot { edge, .. } => format!("{edge} ▸"),
        }
    }
}

/// What pressing Enter on a spine row did.
///
/// Three outcomes rather than a bool, because the caller's fallback differs: nothing to
/// descend into means open the source, whereas *already being* the root means step in.
/// Collapsing those two into `false` is what let Enter push a crumb per keystroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reroot {
    /// The view narrowed to the selection, and focus stepped into it.
    Narrowed,
    /// The selection already *is* the current root, so focus stepped in without narrowing.
    Descended,
    /// There is nothing beneath the selection to narrow to.
    Refused,
}

/// The Seam view's whole state.
pub(crate) struct SeamViewState {
    /// Every node, by identity.
    pub(crate) nodes: HashMap<String, SeamNodeView>,
    /// The tree roots, in order.
    pub(crate) roots: Vec<String>,
    /// What the index amounts to, for the header.
    pub(crate) summary: SeamSummary,
    /// The narrow stack; the last entry is the current root.
    pub(crate) narrow: Vec<Narrow>,
    /// One selected identity per column, outermost first.
    pub(crate) selection: Vec<String>,
    /// Which column has focus.
    pub(crate) focused_column: usize,
    /// Per-column scroll offsets.
    pub(crate) offsets: Vec<usize>,
    /// Active lenses; empty means no lens filter.
    pub(crate) lenses: BTreeSet<&'static str>,
    /// How the lens filter treats non-matching rows.
    pub(crate) lens_filter: LensFilter,
    /// Whether configuration-excluded nodes are hidden rather than dimmed.
    pub(crate) hide_inactive: bool,
    /// How the containment tree is rendered.
    pub(crate) spine: karet_session::config::SeamSpine,
    /// The query text as typed.
    pub(crate) query: String,
    /// The query's parse failure, when it has one.
    pub(crate) query_error: Option<SeamQueryError>,
    /// The identities the query matched, when it has been evaluated.
    pub(crate) query_matches: Option<HashSet<String>>,
    /// What has focus.
    pub(crate) focus: SeamFocus,
    /// The selected node's edges, once fetched.
    pub(crate) edges: Vec<SeamEdgeView>,
    /// The selected row of the facet pane, when it has focus.
    pub(crate) facet_row: usize,
    /// Every file the index holds, so an edit elsewhere is not mistaken for one here.
    pub(crate) files: HashSet<PathBuf>,
    /// The in-flight index request, driving the delayed placeholder.
    pub(crate) loading_since: Option<Pending>,
    /// Why the package could not be indexed, when it could not.
    pub(crate) error: Option<String>,
}

impl SeamViewState {
    /// A view awaiting its first index.
    ///
    /// The root it was opened on lives in the tab's title and in the request that was
    /// sent for it; the view itself is identified by what the index answers, so holding a
    /// second copy here would only be something to keep in step.
    pub(crate) fn pending() -> Self {
        Self {
            nodes: HashMap::new(),
            roots: Vec::new(),
            summary: SeamSummary::default(),
            narrow: Vec::new(),
            selection: Vec::new(),
            focused_column: 0,
            offsets: Vec::new(),
            lenses: BTreeSet::new(),
            lens_filter: LensFilter::default(),
            hide_inactive: false,
            spine: karet_session::config::SeamSpine::default(),
            query: String::new(),
            query_error: None,
            query_matches: None,
            focus: SeamFocus::default(),
            edges: Vec::new(),
            facet_row: 0,
            files: HashSet::new(),
            loading_since: Some(Pending::start()),
            error: None,
        }
    }

    /// Adopt a freshly indexed tree, keeping the reader's place where it survives.
    pub(crate) fn adopt(&mut self, summary: SeamSummary, nodes: Vec<SeamNodeView>) {
        self.roots = nodes
            .iter()
            .filter(|node| node.parent.is_none())
            .map(|node| node.id.clone())
            .collect();
        self.files = nodes.iter().map(|node| node.file.clone()).collect();
        self.nodes = nodes
            .into_iter()
            .map(|node| (node.id.clone(), node))
            .collect();
        self.summary = summary;
        self.loading_since = None;
        self.error = None;
        self.repair();
    }

    /// Record that indexing failed.
    pub(crate) fn fail(&mut self, message: String) {
        self.loading_since = None;
        self.error = Some(message);
        self.files.clear();
    }

    /// Drop state whose nodes no longer exist, falling back to the nearest survivor.
    ///
    /// Keyed on identity, so a rename costs the reader that node's place and nothing
    /// else — the rest of the narrow stack and the rest of the selection stand.
    fn repair(&mut self) {
        self.narrow.retain(|narrow| match narrow {
            Narrow::Scope(id) => self.nodes.contains_key(id),
            Narrow::Pivot { targets, .. } => targets.iter().any(|id| self.nodes.contains_key(id)),
        });
        // Truncate the selection at the first entry that no longer resolves, rather than
        // dropping it entirely: the ancestors above it are still where the reader was.
        if let Some(gone) = self
            .selection
            .iter()
            .position(|id| !self.nodes.contains_key(id))
        {
            self.selection.truncate(gone);
        }
        self.focused_column = self.focused_column.min(self.selection.len());
        self.offsets.resize(self.columns().len().max(1), 0);
    }

    /// Whether the tree is empty because nothing has arrived yet.
    pub(crate) fn is_loading(&self) -> bool {
        self.loading_since.is_some()
    }

    /// Whether re-indexing `path` would mean anything to this view.
    ///
    /// Membership, not containment. The root may be a whole repository, in which case
    /// every file in it sits under the root and asking "is it beneath us?" would answer
    /// yes for a file the index never read — and re-index the repository on every save.
    pub(crate) fn covers(&self, path: &std::path::Path) -> bool {
        // A view that failed, or has not answered yet, has no tree to patch. The loading
        // case matters on its own: a re-index is never coalesced away, so one queued
        // behind a first index would run in full against a tree that is about to be
        // replaced anyway.
        self.error.is_none() && !self.is_loading() && self.files.contains(path)
    }

    /// The identities forming the current root set, after any narrowing.
    pub(crate) fn root_set(&self) -> Vec<String> {
        match self.narrow.last() {
            Some(Narrow::Scope(id)) => vec![id.clone()],
            Some(Narrow::Pivot { targets, .. }) => targets.clone(),
            None => self.roots.clone(),
        }
    }

    /// The columns to render: the root set, then each selection's children.
    pub(crate) fn columns(&self) -> Vec<Vec<String>> {
        let mut columns = vec![self.visible(self.root_set())];
        for selected in &self.selection {
            let Some(node) = self.nodes.get(selected) else {
                break;
            };
            if node.children.is_empty() {
                break;
            }
            columns.push(self.visible(node.children.clone()));
        }
        columns
    }

    /// Apply the hide-mode filters to one column's rows.
    ///
    /// Demote mode returns everything and lets the renderer dim; hide mode removes.
    fn visible(&self, ids: Vec<String>) -> Vec<String> {
        ids.into_iter()
            .filter(|id| {
                if self.hide_inactive
                    && self
                        .nodes
                        .get(id)
                        .is_some_and(|n| n.membership == "inactive")
                {
                    return false;
                }
                self.lens_filter == LensFilter::Demote || self.matches(id)
            })
            .collect()
    }

    /// Whether a node passes the active lens and query filters.
    ///
    /// A node whose *subtree* carries the lens passes too — hiding a module because it
    /// carries no facet itself would hide everything underneath it that does.
    pub(crate) fn matches(&self, id: &str) -> bool {
        let Some(node) = self.nodes.get(id) else {
            return false;
        };
        let lens_ok = self.lenses.is_empty()
            || self.lenses.iter().any(|lens| {
                let index = LENS_NAMES.iter().position(|name| name == lens);
                index.is_some_and(|index| node.rollups.get(index).is_some_and(|count| *count > 0))
            });
        let query_ok = self
            .query_matches
            .as_ref()
            .is_none_or(|matched| matched.contains(id));
        lens_ok && query_ok
    }

    /// The node the reader is currently on.
    pub(crate) fn selected(&self) -> Option<&SeamNodeView> {
        self.selection.last().and_then(|id| self.nodes.get(id))
    }

    /// The identity the reader is currently on.
    pub(crate) fn selected_id(&self) -> Option<&str> {
        self.selection.last().map(String::as_str)
    }

    // --- navigation -----------------------------------------------------------

    /// Move the selection within the focused column.
    pub(crate) fn move_row(&mut self, delta: isize) {
        let columns = self.columns();
        let Some(rows) = columns.get(self.focused_column) else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        let current = self
            .selection
            .get(self.focused_column)
            .and_then(|id| rows.iter().position(|row| row == id))
            .unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(1));
        let Some(id) = rows.get(next).cloned() else {
            return;
        };
        // Selecting in a column invalidates everything to its right.
        self.selection.truncate(self.focused_column);
        self.selection.push(id);
        self.edges.clear();
        self.facet_row = 0;
    }

    /// Move focus between columns, descending only where there is something to descend into.
    pub(crate) fn move_column(&mut self, delta: isize) {
        let columns = self.columns();
        let target = self.focused_column.saturating_add_signed(delta);
        if target >= columns.len() {
            return;
        }
        self.focused_column = target;
        // Entering a column with nothing selected lands on its first row.
        if self.selection.len() <= self.focused_column {
            self.move_row(0);
        }
    }

    /// Reroot at the current selection, pushing onto the narrow stack.
    ///
    /// A narrow that would not change the root set is not a narrow. The current scope is
    /// itself the only row of column zero, so pushing it again would grow the breadcrumb
    /// (`pkg > model > model`) while the view stood still — one extra `Backspace` owed per
    /// stray keystroke, and a trail claiming steps that were never taken. Stepping into
    /// the subtree is what the reader meant by the keypress, and it costs no crumb.
    pub(crate) fn reroot(&mut self) -> Reroot {
        let Some(id) = self.selected_id().map(str::to_owned) else {
            return Reroot::Refused;
        };
        if self
            .nodes
            .get(&id)
            .is_none_or(|node| node.children.is_empty())
        {
            return Reroot::Refused;
        }
        if self.root_set() == [id.clone()] {
            self.move_column(1);
            return Reroot::Descended;
        }
        self.narrow.push(Narrow::Scope(id));
        self.selection.clear();
        self.focused_column = 0;
        self.offsets.clear();
        self.move_row(0);
        // Land inside what was just narrowed to. Narrowing and then leaving the reader on
        // the one row they came from is what made Enter look inert and invited the second
        // press that used to cost them a crumb.
        self.move_column(1);
        Reroot::Narrowed
    }

    /// Reroot on the far end of an edge.
    pub(crate) fn pivot(&mut self, edge: &str, from: &str, targets: Vec<String>) -> bool {
        let reachable: Vec<String> = targets
            .into_iter()
            .filter(|id| self.nodes.contains_key(id))
            .collect();
        if reachable.is_empty() {
            return false;
        }
        // A pivot onto the set already on screen records a step that moved nothing, and
        // reverses to nothing — the same trap a repeated reroot used to set.
        if reachable == self.root_set() {
            return false;
        }
        // A pivot pushes onto the same stack as a scope narrow, so it reverses the same
        // way and shows up in the same breadcrumb. Two kinds of narrow with different
        // rules would be two things for the reader to remember.
        self.narrow.push(Narrow::Pivot {
            edge: edge.to_owned(),
            from: from.to_owned(),
            targets: reachable,
        });
        self.selection.clear();
        self.focused_column = 0;
        self.offsets.clear();
        self.focus = SeamFocus::Spine;
        self.move_row(0);
        true
    }

    /// Step back out of the most recent narrow.
    pub(crate) fn widen(&mut self) -> bool {
        let Some(previous) = self.narrow.pop() else {
            return false;
        };
        self.selection.clear();
        self.focused_column = 0;
        self.offsets.clear();
        // Land back on what was rerooted, so stepping out returns the reader to where
        // they were rather than to the top of a list.
        if let Narrow::Scope(id) = previous {
            self.select_path(&id);
        }
        self.move_row(0);
        true
    }

    /// Select `id` by walking down to it from the current root, so the columns line up.
    pub(crate) fn select_path(&mut self, id: &str) {
        let mut chain = Vec::new();
        let mut current = Some(id.to_owned());
        let roots = self.root_set();
        while let Some(step) = current {
            chain.push(step.clone());
            if roots.contains(&step) {
                break;
            }
            current = self.nodes.get(&step).and_then(|node| node.parent.clone());
        }
        chain.reverse();
        if chain.first().is_some_and(|first| roots.contains(first)) {
            self.selection = chain;
            self.focused_column = self.selection.len().saturating_sub(1);
            self.edges.clear();
            self.facet_row = 0;
        }
    }

    // --- filters --------------------------------------------------------------

    /// Turn one lens on or off.
    pub(crate) fn toggle_lens(&mut self, lens: &'static str) {
        if !self.lenses.remove(lens) {
            self.lenses.insert(lens);
        }
    }

    /// Clear every lens filter.
    pub(crate) fn clear_lenses(&mut self) {
        self.lenses.clear();
    }

    /// The query string this view state is equivalent to.
    ///
    /// Everything the reader reached by pressing keys has to serialize, or the
    /// programmatic surface could not express what the UI can — and an agent's narrowing
    /// could not be handed back as something the reader can inspect.
    pub(crate) fn as_query(&self) -> String {
        let mut terms = Vec::new();
        match self.narrow.last() {
            Some(Narrow::Scope(id)) => terms.push(format!("in:{id}")),
            Some(Narrow::Pivot { edge, from, .. }) => {
                terms.push(format!("pivot:{edge}:{from}"));
            },
            None => {},
        }
        terms.extend(self.lenses.iter().map(|lens| format!("lens:{lens}")));
        if !self.query.trim().is_empty() {
            terms.push(self.query.trim().to_owned());
        }
        terms.join(" ")
    }
}

mod actions;
pub(crate) mod roots;

#[cfg(test)]
#[path = "seam/tests.rs"]
mod tests;

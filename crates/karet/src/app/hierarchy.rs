//! Call and type hierarchies: a tree of callers, callees, supertypes or
//! subtypes, grown one level at a time as the user expands it.
//!
//! The tree lives in the Hierarchy sidebar panel ([`SidebarPanel::Hierarchy`]),
//! shown at once, empty, when the command runs; the language server's roots
//! fill it in (`Command::PrepareHierarchy` in, `Event::Hierarchy` out), and the
//! first root is expanded straight away so the tree opens on an answer, not on
//! a lone row. Every other level is asked for only when the user expands its
//! node (`Command::ExpandHierarchy`), so a deep or recursive hierarchy costs
//! nothing until it is walked. Right expands, Left collapses or climbs to the
//! parent, Space toggles, and Enter opens the selected item. Opening an item
//! leaves the panel and its expansion alone, so the next row is one keystroke
//! (Ctrl+8, or the focus toggle) away.

use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;

use super::pending::Pending;
use super::*;

/// How a hierarchy in `direction` is titled.
fn direction_title(direction: HierarchyDirection) -> &'static str {
    match direction {
        HierarchyDirection::IncomingCalls => "Incoming Calls",
        HierarchyDirection::OutgoingCalls => "Outgoing Calls",
        HierarchyDirection::Supertypes => "Supertypes",
        _ => "Subtypes",
    }
}

/// The hierarchy a direction walks, as a notice names it.
fn hierarchy_noun(direction: HierarchyDirection) -> &'static str {
    if direction.is_calls() {
        "call hierarchy"
    } else {
        "type hierarchy"
    }
}

/// What is known of a node's children.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Children {
    /// Never asked for.
    Unasked,
    /// Asked for; the answer has not come.
    Loading,
    /// Answered: the child nodes, possibly none.
    Loaded(Vec<usize>),
}

/// One item in the tree.
#[derive(Clone, Debug)]
struct Node {
    item: HierarchyItem,
    depth: usize,
    parent: Option<usize>,
    children: Children,
    expanded: bool,
}

/// A call or type hierarchy being walked: the nodes found so far, which are
/// expanded, the selected row, and the requests still in flight.
#[derive(Clone, Debug)]
pub(crate) struct HierarchyTree {
    direction: HierarchyDirection,
    /// The document the hierarchy was asked from, whose server is asked to
    /// grow it.
    doc: DocumentId,
    /// The workspace root, which row hints are relative to.
    root: PathBuf,
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// The node behind each visible row, in display order.
    visible: Vec<usize>,
    labels: Vec<String>,
    hints: Vec<Option<String>>,
    selected: usize,
    /// The requests in flight: the node each will fill in (`None`: the
    /// roots), and when it was asked.
    pending: Vec<(RequestId, Option<usize>, Pending)>,
}

/// What adopting an answer did to the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Adopted {
    /// The answer was not for this tree, or not for anything it still waits on.
    Ignored,
    /// The roots came back empty: there is no hierarchy at the caret.
    NoRoots,
    /// The roots came back and fill the tree.
    Roots,
    /// One node's children came back.
    Children,
}

impl HierarchyTree {
    /// An empty tree in `direction`, asked from `doc`, waiting on the roots
    /// that `request` will bring.
    #[must_use]
    pub(crate) fn new(
        direction: HierarchyDirection,
        doc: DocumentId,
        root: PathBuf,
        request: RequestId,
    ) -> Self {
        Self {
            direction,
            doc,
            root,
            nodes: Vec::new(),
            roots: Vec::new(),
            visible: Vec::new(),
            labels: Vec::new(),
            hints: Vec::new(),
            selected: 0,
            pending: vec![(request, None, Pending::start())],
        }
    }

    /// The panel's status line: which way the tree grows.
    pub(crate) fn title(&self) -> &'static str {
        direction_title(self.direction)
    }

    /// One label per visible row: indented by depth, marked by what is known
    /// of its children (`▸` unexpanded, `▾` expanded, `·` none).
    pub(crate) fn rows(&self) -> Vec<&str> {
        self.labels.iter().map(String::as_str).collect()
    }

    /// One hint per visible row: where the item is, workspace-relative and
    /// 1-based.
    pub(crate) fn row_hints(&self) -> Vec<Option<String>> {
        self.hints.clone()
    }

    /// The selected row.
    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// How many rows are visible.
    pub(crate) fn row_count(&self) -> usize {
        self.visible.len()
    }

    /// Move the selection by `delta` rows, clamped to the visible rows.
    pub(crate) fn select_by(&mut self, delta: i32) {
        let step = delta.unsigned_abs() as usize;
        let row = if delta < 0 {
            self.selected.saturating_sub(step)
        } else {
            self.selected.saturating_add(step)
        };
        self.select(row);
    }

    /// Select visible row `row`, clamped to the visible rows.
    pub(crate) fn select(&mut self, row: usize) {
        self.selected = row.min(self.visible.len().saturating_sub(1));
    }

    /// Where the selected item is named, which Enter opens; `None` before the
    /// roots arrive.
    pub(crate) fn selected_location(&self) -> Option<(PathBuf, LineCol)> {
        self.selected_node().map(|node| {
            (
                node.item.location.path.clone(),
                node.item.location.range.start,
            )
        })
    }

    /// Whether the selected node is expanded with children to fold away, so
    /// a toggle collapses it rather than expanding it.
    pub(crate) fn selected_is_open(&self) -> bool {
        self.selected_node().is_some_and(|node| {
            node.expanded
                && !matches!(&node.children, Children::Loaded(children) if children.is_empty())
        })
    }

    /// The column of visible row `row`'s expansion marker, counted from the
    /// start of its label.
    pub(crate) fn marker_column(&self, row: usize) -> Option<usize> {
        let node = self.nodes.get(*self.visible.get(row)?)?;
        Some(node.depth.saturating_mul(2))
    }

    /// The earliest request still in flight, which the delayed loading note
    /// anchors on.
    pub(crate) fn loading(&self) -> Option<Pending> {
        self.pending.first().map(|(_, _, since)| *since)
    }

    /// Backdate every request in flight past the reveal delay, so a test can
    /// paint the loading note without sleeping.
    #[cfg(test)]
    pub(crate) fn reveal_loading(&mut self) {
        for (_, _, since) in &mut self.pending {
            *since = Pending::revealed();
        }
    }

    /// Whether `request` is the one bringing the roots.
    pub(crate) fn awaits_roots(&self, request: RequestId) -> bool {
        self.pending
            .iter()
            .any(|(id, node, _)| *id == request && node.is_none())
    }

    /// The direction the tree grows in.
    pub(crate) fn direction(&self) -> HierarchyDirection {
        self.direction
    }

    /// The document the hierarchy was asked from.
    pub(crate) fn doc(&self) -> DocumentId {
        self.doc
    }

    fn selected_node(&self) -> Option<&Node> {
        self.nodes.get(*self.visible.get(self.selected)?)
    }

    /// Expand the selected node. Returns the node and item to ask the server
    /// about when its children were never asked for; a node whose children
    /// are known, or are on their way, is just shown expanded.
    pub(crate) fn expand_selected(&mut self) -> Option<(usize, HierarchyItem)> {
        let index = *self.visible.get(self.selected)?;
        let node = self.nodes.get_mut(index)?;
        node.expanded = true;
        let ask = (node.children == Children::Unasked).then(|| {
            node.children = Children::Loading;
            (index, node.item.clone())
        });
        self.rebuild();
        ask
    }

    /// Record that `request` will bring `node`'s children; when no request
    /// could be made (`None`), the node has none to show.
    pub(crate) fn track(&mut self, node: usize, request: Option<RequestId>) {
        match request {
            Some(id) => self.pending.push((id, Some(node), Pending::start())),
            None => {
                if let Some(node) = self.nodes.get_mut(node) {
                    node.children = Children::Loaded(Vec::new());
                }
                self.rebuild();
            },
        }
    }

    /// Collapse the selected node, or, when it is already collapsed or is a
    /// leaf, select its parent.
    pub(crate) fn collapse_selected(&mut self) {
        let Some(&index) = self.visible.get(self.selected) else {
            return;
        };
        let Some(node) = self.nodes.get_mut(index) else {
            return;
        };
        // A leaf has nothing to fold away, so Left climbs from it at once.
        let leaf = matches!(&node.children, Children::Loaded(children) if children.is_empty());
        if node.expanded && !leaf {
            node.expanded = false;
            self.rebuild();
        } else if let Some(parent) = node.parent
            && let Some(row) = self.visible.iter().position(|&shown| shown == parent)
        {
            self.selected = row;
        }
    }

    /// Adopt the answer to `request`: the roots, or one node's children.
    pub(crate) fn adopt(&mut self, request: RequestId, items: Vec<HierarchyItem>) -> Adopted {
        let Some(at) = self.pending.iter().position(|(id, ..)| *id == request) else {
            return Adopted::Ignored;
        };
        let (_, parent, _) = self.pending.remove(at);
        let depth = parent
            .and_then(|parent| self.nodes.get(parent))
            .map_or(0, |node| node.depth + 1);
        let added: Vec<usize> = items
            .into_iter()
            .map(|item| {
                self.nodes.push(Node {
                    item,
                    depth,
                    parent,
                    children: Children::Unasked,
                    expanded: false,
                });
                self.nodes.len() - 1
            })
            .collect();
        let adopted = match parent {
            None if added.is_empty() => Adopted::NoRoots,
            None => {
                self.roots = added;
                Adopted::Roots
            },
            Some(parent) => {
                if let Some(node) = self.nodes.get_mut(parent) {
                    node.children = Children::Loaded(added);
                }
                Adopted::Children
            },
        };
        self.rebuild();
        adopted
    }

    /// Recompute the visible rows, keeping the selection on the node it was
    /// on wherever that node now is.
    fn rebuild(&mut self) {
        let keep = self.visible.get(self.selected).copied();
        let mut visible = Vec::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(index) = stack.pop() {
            visible.push(index);
            if let Some(Node {
                expanded: true,
                children: Children::Loaded(children),
                ..
            }) = self.nodes.get(index)
            {
                stack.extend(children.iter().rev());
            }
        }
        self.labels = visible.iter().map(|&index| self.label(index)).collect();
        self.hints = visible.iter().map(|&index| self.hint(index)).collect();
        self.selected = keep
            .and_then(|keep| visible.iter().position(|&shown| shown == keep))
            .unwrap_or(0);
        self.visible = visible;
    }

    fn label(&self, index: usize) -> String {
        let Some(node) = self.nodes.get(index) else {
            return String::new();
        };
        let marker = match (&node.children, node.expanded) {
            (Children::Loaded(children), _) if children.is_empty() => '·',
            (Children::Unasked, _) | (_, false) => '▸',
            (Children::Loading | Children::Loaded(_), true) => '▾',
        };
        format!("{}{marker} {}", "  ".repeat(node.depth), node.item.name)
    }

    fn hint(&self, index: usize) -> Option<String> {
        let location = &self.nodes.get(index)?.item.location;
        let path = location
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&location.path);
        let line = location.range.start.line.saturating_add(1);
        Some(format!("{}:{line}", path.display()))
    }
}

/// Where the sidebar and keyboard focus stood before the Hierarchy panel took
/// them, so a tree that never gets an answer can hand them back.
#[derive(Clone, Copy, Debug)]
struct Before {
    panel: SidebarPanel,
    focus: Focus,
    visible: bool,
}

/// The Hierarchy panel: the tree being walked, if any.
#[derive(Debug, Default)]
pub(crate) struct HierarchyPanel {
    /// The tree the panel shows; `None` until a hierarchy is first asked for,
    /// and again once one comes back empty.
    pub(crate) tree: Option<HierarchyTree>,
    /// What the panel replaced, restored when its tree is dropped unanswered.
    before: Option<Before>,
}

/// The Hierarchy panel's per-frame chrome — same shape as the Spelling panel's.
pub(crate) type HierarchyChrome = SpellingChrome;

impl App {
    /// Open the hierarchy in `direction` of the symbol at the caret: the tree
    /// panel at once, empty until the server's roots fill it in. A tree
    /// already in the panel is replaced, and its late answers are dropped.
    pub(crate) fn open_hierarchy(&mut self, direction: HierarchyDirection) {
        let Some((doc, position)) = self.completion_target() else {
            let noun = hierarchy_noun(direction);
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!("{noun}: open a code file first"),
            );
            return;
        };
        let command = SessionCommand::PrepareHierarchy {
            doc,
            position,
            direction,
        };
        let Some(request) = self.send(command) else {
            return;
        };
        let root = workspace::absolute(&self.root);
        let tree = HierarchyTree::new(direction, doc, root, request);
        // Replacing a tree already on show keeps what that one replaced: the
        // panel itself is not somewhere to return to once its tree is gone.
        let showing = self.hierarchy_showing() && self.hierarchy.tree.is_some();
        if !showing {
            self.hierarchy.before = Some(Before {
                panel: self.sidebar_panel,
                focus: self.focus,
                visible: self.sidebar_visible,
            });
        }
        self.hierarchy.tree = Some(tree);
        self.sidebar_visible = true;
        self.sidebar_panel = SidebarPanel::Hierarchy;
        self.focus = Focus::Sidebar;
    }

    /// Whether the Hierarchy panel is on screen.
    fn hierarchy_showing(&self) -> bool {
        self.sidebar_visible && self.sidebar_panel == SidebarPanel::Hierarchy
    }

    /// Drop a tree that will never get an answer, and when its panel is still
    /// on show, hand the sidebar and focus back to what it replaced.
    fn drop_unanswered_hierarchy(&mut self) {
        self.hierarchy.tree = None;
        let before = self.hierarchy.before.take();
        if self.sidebar_panel != SidebarPanel::Hierarchy {
            return;
        }
        let before = before
            .filter(|before| before.panel != SidebarPanel::Hierarchy)
            .unwrap_or(Before {
                panel: SidebarPanel::Explorer,
                focus: Focus::Editor,
                visible: self.sidebar_visible,
            });
        self.sidebar_panel = before.panel;
        self.sidebar_visible = before.visible;
        if self.focus == Focus::Sidebar {
            self.focus = before.focus;
        }
    }

    /// Show the Hierarchy panel; with no tree to show, say how to get one.
    pub(super) fn show_hierarchy(&mut self) -> bool {
        if self.hierarchy.tree.is_none() {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "hierarchy: show the callers, callees, supertypes or subtypes of a symbol first",
            );
            return false;
        }
        true
    }

    /// Adopt a hierarchy answer into the panel's tree: fill in its roots and
    /// expand the first, or one node's children. An answer for a tree the
    /// user has since replaced finds nothing to fill and is dropped.
    pub(crate) fn on_hierarchy(&mut self, id: Option<RequestId>, items: Vec<HierarchyItem>) {
        let Some(id) = id else {
            return;
        };
        let Some(tree) = self.hierarchy.tree.as_mut() else {
            return;
        };
        let direction = tree.direction();
        match tree.adopt(id, items) {
            Adopted::NoRoots => {
                let reason = self.no_hierarchy_reason(direction);
                self.drop_unanswered_hierarchy();
                self.notify(Report::Refusal, NotificationKind::Lsp, reason);
            },
            Adopted::Roots => self.hierarchy_expand(),
            Adopted::Children | Adopted::Ignored => {},
        }
    }

    /// Expand the selected node of the tree, asking the server for its
    /// children the first time.
    pub(crate) fn hierarchy_expand(&mut self) {
        let Some(tree) = self.hierarchy.tree.as_mut() else {
            return;
        };
        let Some((node, item)) = tree.expand_selected() else {
            return;
        };
        let command = SessionCommand::ExpandHierarchy {
            doc: tree.doc(),
            item: Box::new(item),
            direction: tree.direction(),
        };
        let request = self.send(command);
        if let Some(tree) = self.hierarchy.tree.as_mut() {
            tree.track(node, request);
        }
    }

    /// Collapse the selected node of the tree, or climb to its parent.
    pub(crate) fn hierarchy_collapse(&mut self) {
        if let Some(tree) = self.hierarchy.tree.as_mut() {
            tree.collapse_selected();
        }
    }

    /// Collapse the selected node when it is open, else expand it.
    pub(super) fn hierarchy_toggle(&mut self) {
        let open = self
            .hierarchy
            .tree
            .as_ref()
            .is_some_and(HierarchyTree::selected_is_open);
        if open {
            self.hierarchy_collapse();
        } else {
            self.hierarchy_expand();
        }
    }

    /// Move the tree's selection by `delta` rows.
    pub(super) fn hierarchy_select(&mut self, delta: i32) {
        if let Some(tree) = self.hierarchy.tree.as_mut() {
            tree.select_by(delta);
        }
    }

    /// Open the selected item where it is named. The panel, its selection
    /// and its expansion stay as they are, ready for the next row.
    pub(super) fn open_selected_hierarchy(&mut self) {
        let Some((path, position)) = self
            .hierarchy
            .tree
            .as_ref()
            .and_then(HierarchyTree::selected_location)
        else {
            return;
        };
        self.jump_to_location(&path, position);
    }

    /// Route a click inside the Hierarchy panel: on a row's marker, expand or
    /// collapse it; anywhere else on the row, select and open it.
    pub(super) fn hierarchy_click(&mut self, col: u16, row_y: u16) {
        let rect = self.hierarchy_ui.results_rect;
        if !rect_contains(rect, (col, row_y)) {
            return;
        }
        let Some(tree) = self.hierarchy.tree.as_mut() else {
            return;
        };
        let row = self.hierarchy_ui.offset + usize::from(row_y - rect.y);
        if row >= tree.row_count() {
            return;
        }
        tree.select(row);
        // Each label is drawn one cell in from the panel edge, and its marker
        // and the space after it make a two-cell target.
        let marker = tree
            .marker_column(row)
            .map(|at| usize::from(rect.x) + 1 + at);
        let at = usize::from(col);
        if marker.is_some_and(|marker| (marker..marker + 2).contains(&at)) {
            self.hierarchy_toggle();
        } else {
            self.open_selected_hierarchy();
        }
    }

    /// When the tree is still waiting on `id` for its roots, drop it: the
    /// server has said it does not offer the hierarchy, and that notice
    /// explains the empty answer that follows.
    pub(super) fn refuse_hierarchy(&mut self, id: RequestId) {
        if self
            .hierarchy
            .tree
            .as_ref()
            .is_some_and(|tree| tree.awaits_roots(id))
        {
            self.drop_unanswered_hierarchy();
        }
    }

    /// What the tree is still waiting on, for its delayed loading note.
    pub(crate) fn hierarchy_loading(&self) -> Option<Pending> {
        self.hierarchy.tree.as_ref()?.loading()
    }

    /// What the tree is waiting on while its panel is on screen: the only
    /// time its loading note needs a scheduled reveal.
    pub(super) fn visible_hierarchy_loading(&self) -> Option<Pending> {
        self.hierarchy_showing()
            .then(|| self.hierarchy_loading())
            .flatten()
    }

    /// Why no roots came back, phrased for the refusal it becomes: a server
    /// that is missing or still starting answers instantly with nothing,
    /// which would otherwise read as a claim about the symbol.
    fn no_hierarchy_reason(&self, direction: HierarchyDirection) -> String {
        match self.active_language_server_badge().map(|badge| badge.state) {
            None
            | Some(
                LanguageServerBadge::Off
                | LanguageServerBadge::NotInstalled
                | LanguageServerBadge::NeedsSetup,
            ) => "no language server for this file".to_string(),
            Some(LanguageServerBadge::Failed) => {
                "the language server for this file is not running".to_string()
            },
            Some(LanguageServerBadge::Starting | LanguageServerBadge::Retrying) => {
                "language server is still starting".to_string()
            },
            _ => format!("no {} at the caret", hierarchy_noun(direction)),
        }
    }
}

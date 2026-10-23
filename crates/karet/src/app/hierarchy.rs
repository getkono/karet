//! Call and type hierarchies: a tree of callers, callees, supertypes or
//! subtypes, grown one level at a time as the user expands it.
//!
//! The tree is an overlay ([`Overlay::Hierarchy`]) opened at once, empty, when
//! the command runs; the language server's roots fill it in
//! (`Command::PrepareHierarchy` in, `Event::Hierarchy` out), and the first root
//! is expanded straight away so the tree opens on an answer, not on a lone
//! row. Every other level is asked for only when the user expands its node
//! (`Command::ExpandHierarchy`), so a deep or recursive hierarchy costs
//! nothing until it is walked. Right expands, Left collapses or climbs to the
//! parent, and Enter opens the selected item.

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
    title: String,
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
        let title = format!(
            "{} · → expands · ← collapses · Enter opens",
            direction_title(direction)
        );
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
            title,
            pending: vec![(request, None, Pending::start())],
        }
    }

    /// The overlay title: the direction, and the keys that walk the tree.
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// The selected item's detail (a signature, a module), else its name.
    pub(crate) fn query(&self) -> &str {
        self.selected_node().map_or("", |node| {
            node.item.detail.as_deref().unwrap_or(&node.item.name)
        })
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

    /// Move the selection up a row.
    pub(crate) fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down a row.
    pub(crate) fn select_down(&mut self) {
        self.selected = (self.selected + 1).min(self.visible.len().saturating_sub(1));
    }

    /// Open the selected item, or close when there is none yet.
    pub(crate) fn accept(&self) -> OverlayEvent {
        self.selected_node()
            .map_or(OverlayEvent::Close, |node| OverlayEvent::AcceptLocation {
                path: node.item.location.path.clone(),
                position: node.item.location.range.start,
            })
    }

    /// The earliest request still in flight, which the delayed loading note
    /// anchors on.
    pub(crate) fn loading(&self) -> Option<Pending> {
        self.pending.first().map(|(_, _, since)| *since)
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

impl App {
    /// Open the hierarchy in `direction` of the symbol at the caret: the tree
    /// overlay at once, empty until the server's roots fill it in.
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
        self.overlay = Some(Overlay::Hierarchy(Box::new(tree)));
    }

    /// Adopt a hierarchy answer into the open tree: fill in its roots and
    /// expand the first, or one node's children. An answer for a tree the
    /// user has since closed or replaced finds nothing to fill and is dropped.
    pub(crate) fn on_hierarchy(&mut self, id: Option<RequestId>, items: Vec<HierarchyItem>) {
        let Some(id) = id else {
            return;
        };
        let Some(Overlay::Hierarchy(tree)) = self.overlay.as_mut() else {
            return;
        };
        let direction = tree.direction();
        match tree.adopt(id, items) {
            Adopted::NoRoots => {
                let reason = self.no_hierarchy_reason(direction);
                self.overlay = None;
                self.notify(Report::Refusal, NotificationKind::Lsp, reason);
            },
            Adopted::Roots => self.hierarchy_expand(),
            Adopted::Children | Adopted::Ignored => {},
        }
    }

    /// Expand the selected node of the open tree, asking the server for its
    /// children the first time.
    pub(crate) fn hierarchy_expand(&mut self) {
        let Some(Overlay::Hierarchy(tree)) = self.overlay.as_mut() else {
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
        if let Some(Overlay::Hierarchy(tree)) = self.overlay.as_mut() {
            tree.track(node, request);
        }
    }

    /// Collapse the selected node of the open tree, or climb to its parent.
    pub(crate) fn hierarchy_collapse(&mut self) {
        if let Some(Overlay::Hierarchy(tree)) = self.overlay.as_mut() {
            tree.collapse_selected();
        }
    }

    /// When the open tree is still waiting on `id` for its roots, close it:
    /// the server has said it does not offer the hierarchy, and that notice
    /// explains the empty answer that follows.
    pub(super) fn refuse_hierarchy(&mut self, id: RequestId) {
        if let Some(Overlay::Hierarchy(tree)) = self.overlay.as_ref()
            && tree.awaits_roots(id)
        {
            self.overlay = None;
        }
    }

    /// What the open tree is still waiting on, for its delayed loading note.
    pub(crate) fn hierarchy_loading(&self) -> Option<Pending> {
        match self.overlay.as_ref()? {
            Overlay::Hierarchy(tree) => tree.loading(),
            _ => None,
        }
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

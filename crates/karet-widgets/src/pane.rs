//! The editor's window layout: a binary(-ish) split tree of panes with a focus.
//!
//! [`PaneLayout`] manages only pane *identity* and *geometry* — it maps a screen
//! [`Rect`] to a rect per [`PaneId`], and supports splitting, closing, and moving
//! panes plus drop-zone hit-testing for drag-to-split. The application owns each
//! pane's content (its tabs), keyed by `PaneId`, so this stays a small, headless,
//! unit-testable engine that consumes only `ratatui` geometry.

use ratatui::layout::Rect;

/// Identifies a pane (a leaf window) within a [`PaneLayout`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PaneId(pub u64);

/// The axis a split divides along.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitAxis {
    /// Side-by-side columns (divides width).
    Cols,
    /// Stacked rows (divides height).
    Rows,
}

/// A direction to split a pane in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SplitDir {
    /// New pane to the left.
    Left,
    /// New pane to the right.
    Right,
    /// New pane above.
    Up,
    /// New pane below.
    Down,
}

impl SplitDir {
    /// The axis this direction splits along.
    #[must_use]
    pub fn axis(self) -> SplitAxis {
        match self {
            SplitDir::Left | SplitDir::Right => SplitAxis::Cols,
            SplitDir::Up | SplitDir::Down => SplitAxis::Rows,
        }
    }

    /// Whether the new pane is placed before the existing one (left / up).
    fn before(self) -> bool {
        matches!(self, SplitDir::Left | SplitDir::Up)
    }
}

/// Where within a pane a drag was dropped, classifying the resulting action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropZone {
    /// The middle — move into the pane without splitting.
    Center,
    /// The left edge — split into a new left column.
    Left,
    /// The right edge — split into a new right column.
    Right,
    /// The top edge — split into a new top row.
    Top,
    /// The bottom edge — split into a new bottom row.
    Bottom,
}

impl DropZone {
    /// The split direction this edge zone implies (`None` for [`Center`](Self::Center)).
    #[must_use]
    pub fn split_dir(self) -> Option<SplitDir> {
        match self {
            DropZone::Center => None,
            DropZone::Left => Some(SplitDir::Left),
            DropZone::Right => Some(SplitDir::Right),
            DropZone::Top => Some(SplitDir::Up),
            DropZone::Bottom => Some(SplitDir::Down),
        }
    }
}

/// The minimum pane width, in columns, a split should leave.
pub const MIN_W: u16 = 10;
/// The minimum pane height, in rows, a split should leave.
pub const MIN_H: u16 = 3;

/// One draggable boundary between adjacent children of a pane split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneDivider {
    /// The axis perpendicular to the boundary.
    pub axis: SplitAxis,
    /// Screen column for a column split, or screen row for a row split. The
    /// divider occupies the final cell of the child before the boundary.
    pub position: u16,
    /// Inclusive start of the boundary along its other axis.
    pub start: u16,
    /// Exclusive end of the boundary along its other axis.
    pub end: u16,
    /// Last pane in the child before the boundary.
    pub before: PaneId,
    /// First pane in the child after the boundary.
    pub after: PaneId,
}

impl PaneDivider {
    /// Whether the screen cell `(x, y)` lies on this divider.
    #[must_use]
    pub fn contains(self, x: u16, y: u16) -> bool {
        match self.axis {
            SplitAxis::Cols => x == self.position && y >= self.start && y < self.end,
            SplitAxis::Rows => y == self.position && x >= self.start && x < self.end,
        }
    }
}

/// A node in the pane tree: a single pane (leaf) or a split of child nodes.
enum Node {
    Leaf(PaneId),
    Split {
        axis: SplitAxis,
        children: Vec<Node>,
        weights: Vec<f32>,
    },
}

/// A split tree of panes with a focused pane.
pub struct PaneLayout {
    root: Node,
    focus: PaneId,
    next_id: u64,
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneLayout {
    /// A fresh layout with a single pane holding focus.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: Node::Leaf(PaneId(0)),
            focus: PaneId(0),
            next_id: 1,
        }
    }

    /// The id of the first (root) pane.
    #[must_use]
    pub fn root_pane(&self) -> PaneId {
        first_leaf(&self.root)
    }

    /// The focused pane.
    #[must_use]
    pub fn focus(&self) -> PaneId {
        self.focus
    }

    /// Focus `pane` if it exists.
    pub fn set_focus(&mut self, pane: PaneId) {
        if self.contains(pane) {
            self.focus = pane;
        }
    }

    /// Whether `pane` is a live pane in the tree.
    #[must_use]
    pub fn contains(&self, pane: PaneId) -> bool {
        self.panes().contains(&pane)
    }

    /// The number of panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        self.panes().len()
    }

    /// Every pane, in left-to-right / top-to-bottom tree order.
    #[must_use]
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// Split `pane` in `dir`, returning the new (empty) pane, which becomes focused.
    /// When `pane`'s parent already splits along the matching axis, the new pane is
    /// inserted as an adjacent sibling; otherwise `pane` is wrapped in a new split.
    pub fn split(&mut self, pane: PaneId, dir: SplitDir) -> PaneId {
        let new_id = PaneId(self.next_id);
        self.next_id += 1;
        if self.insert_beside(pane, new_id, dir) {
            self.focus = new_id;
        }
        new_id
    }

    /// Move `pane` to become a `dir`-neighbor of `target` (a whole-pane relocation,
    /// preserving `pane`'s id). A no-op if `pane == target` or either is missing, or
    /// if `pane` is the only pane.
    pub fn move_pane(&mut self, pane: PaneId, target: PaneId, dir: SplitDir) {
        if pane == target || self.pane_count() < 2 || !self.contains(pane) || !self.contains(target)
        {
            return;
        }
        // Detach `pane`, collapse its old parent, then re-insert beside `target`.
        if remove_leaf(&mut self.root, pane).is_some() {
            collapse(&mut self.root);
            if self.insert_beside(target, pane, dir) {
                self.focus = pane;
            }
        }
    }

    /// Remove `pane`, collapsing a split that drops to a single child. Returns a
    /// neighboring pane to focus, or `None` if `pane` was the only pane (the caller
    /// keeps a placeholder pane in that case).
    pub fn close(&mut self, pane: PaneId) -> Option<PaneId> {
        if matches!(self.root, Node::Leaf(id) if id == pane) {
            return None;
        }
        let neighbor = remove_leaf(&mut self.root, pane)?;
        collapse(&mut self.root);
        if self.focus == pane {
            self.focus = neighbor;
        }
        Some(neighbor)
    }

    /// Compute the rectangle for every pane, tiling `area` without gaps or overlap.
    #[must_use]
    pub fn layout(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        layout_in(&self.root, area, &mut out);
        out
    }

    /// The rectangle for `pane` within `area`, if it exists.
    #[must_use]
    pub fn pane_rect(&self, pane: PaneId, area: Rect) -> Option<Rect> {
        self.layout(area)
            .into_iter()
            .find(|(id, _)| *id == pane)
            .map(|(_, r)| r)
    }

    /// Every draggable split boundary in screen coordinates, outer splits first.
    #[must_use]
    pub fn dividers(&self, area: Rect) -> Vec<PaneDivider> {
        let mut out = Vec::new();
        dividers_in(&self.root, area, &mut out);
        out
    }

    /// Move `divider` by `delta` cells, preserving the combined size of its two
    /// adjacent children and enforcing [`MIN_W`] / [`MIN_H`] for every descendant.
    /// Returns whether the divider moved.
    pub fn resize_divider(&mut self, divider: PaneDivider, delta: i16, area: Rect) -> bool {
        resize_divider_in(&mut self.root, area, divider, delta)
    }

    /// Grow focused `pane` toward `dir` by up to `cells`, using the nearest boundary
    /// on that edge. Returns whether a matching boundary moved.
    pub fn resize_pane(&mut self, pane: PaneId, dir: SplitDir, cells: u16, area: Rect) -> bool {
        let Some(rect) = self.pane_rect(pane, area) else {
            return false;
        };
        let center_x = rect.x.saturating_add(rect.width / 2);
        let center_y = rect.y.saturating_add(rect.height / 2);
        let mut candidates: Vec<PaneDivider> = self
            .dividers(area)
            .into_iter()
            .filter(|divider| match dir {
                SplitDir::Left => {
                    divider.axis == SplitAxis::Cols
                        && divider.position.saturating_add(1) == rect.x
                        && center_y >= divider.start
                        && center_y < divider.end
                },
                SplitDir::Right => {
                    divider.axis == SplitAxis::Cols
                        && divider.position.saturating_add(1) == rect.right()
                        && center_y >= divider.start
                        && center_y < divider.end
                },
                SplitDir::Up => {
                    divider.axis == SplitAxis::Rows
                        && divider.position.saturating_add(1) == rect.y
                        && center_x >= divider.start
                        && center_x < divider.end
                },
                SplitDir::Down => {
                    divider.axis == SplitAxis::Rows
                        && divider.position.saturating_add(1) == rect.bottom()
                        && center_x >= divider.start
                        && center_x < divider.end
                },
            })
            .collect();
        candidates.sort_by_key(|divider| divider.end.saturating_sub(divider.start));
        let Some(divider) = candidates.first().copied() else {
            return false;
        };
        let magnitude = i16::try_from(cells).unwrap_or(i16::MAX);
        let delta = match dir {
            SplitDir::Left | SplitDir::Up => -magnitude,
            SplitDir::Right | SplitDir::Down => magnitude,
        };
        self.resize_divider(divider, delta, area)
    }

    /// Whether `pane`'s current rectangle within `area` is large enough to split in
    /// `dir` and leave both halves at least the minimum size.
    #[must_use]
    pub fn can_split(&self, pane: PaneId, dir: SplitDir, area: Rect) -> bool {
        let Some(rect) = self.pane_rect(pane, area) else {
            return false;
        };
        match dir.axis() {
            SplitAxis::Cols => rect.width >= MIN_W.saturating_mul(2),
            SplitAxis::Rows => rect.height >= MIN_H.saturating_mul(2),
        }
    }

    /// Insert `leaf_id` as a `dir`-neighbor of `target`. Returns whether `target`
    /// was found. Shared by [`split`](Self::split) and [`move_pane`](Self::move_pane).
    fn insert_beside(&mut self, target: PaneId, leaf_id: PaneId, dir: SplitDir) -> bool {
        let axis = dir.axis();
        let before = dir.before();
        if matches!(self.root, Node::Leaf(id) if id == target) {
            let old = std::mem::replace(&mut self.root, Node::Leaf(leaf_id));
            self.root = make_split(axis, old, Node::Leaf(leaf_id), before);
            return true;
        }
        insert_in(&mut self.root, target, leaf_id, axis, before)
    }
}

/// Build a split of `existing` and a new `Node::Leaf(new_leaf)` along `axis`, with
/// the new leaf placed first when `before`.
fn make_split(axis: SplitAxis, existing: Node, new_leaf: Node, before: bool) -> Node {
    let children = if before {
        vec![new_leaf, existing]
    } else {
        vec![existing, new_leaf]
    };
    Node::Split {
        axis,
        children,
        weights: vec![0.5, 0.5],
    }
}

/// Recursively insert `leaf_id` beside the leaf `target`. Returns whether it was found.
fn insert_in(
    node: &mut Node,
    target: PaneId,
    leaf_id: PaneId,
    axis: SplitAxis,
    before: bool,
) -> bool {
    let Node::Split {
        axis: node_axis,
        children,
        weights,
    } = node
    else {
        return false;
    };
    let node_axis = *node_axis;
    if let Some(i) = children
        .iter()
        .position(|c| matches!(c, Node::Leaf(id) if *id == target))
    {
        if node_axis == axis {
            // Same axis: insert as an adjacent sibling, halving the target's weight.
            let half = weights.get(i).copied().unwrap_or(1.0) / 2.0;
            if let Some(w) = weights.get_mut(i) {
                *w = half;
            }
            let pos = if before { i } else { i + 1 };
            children.insert(pos, Node::Leaf(leaf_id));
            weights.insert(pos, half);
        } else {
            // Different axis: wrap the target leaf in a new sub-split (its slot's
            // weight is unchanged).
            let old = std::mem::replace(&mut children[i], Node::Leaf(leaf_id));
            children[i] = make_split(axis, old, Node::Leaf(leaf_id), before);
        }
        return true;
    }
    children
        .iter_mut()
        .any(|child| insert_in(child, target, leaf_id, axis, before))
}

/// Remove the leaf `pane` from `node`'s subtree, returning a neighboring pane to
/// focus. Does not collapse single-child splits (call [`collapse`] afterward).
fn remove_leaf(node: &mut Node, pane: PaneId) -> Option<PaneId> {
    let Node::Split {
        children, weights, ..
    } = node
    else {
        return None;
    };
    if let Some(i) = children
        .iter()
        .position(|c| matches!(c, Node::Leaf(id) if *id == pane))
    {
        children.remove(i);
        if i < weights.len() {
            weights.remove(i);
        }
        let neighbor = if i < children.len() {
            i
        } else {
            i.saturating_sub(1)
        };
        return children.get(neighbor).map(first_leaf);
    }
    children
        .iter_mut()
        .find_map(|child| remove_leaf(child, pane))
}

/// Collapse any split that has dropped to a single child, promoting that child.
fn collapse(node: &mut Node) {
    if let Node::Split { children, .. } = node {
        for child in children.iter_mut() {
            collapse(child);
        }
        if children.len() == 1 {
            let only = children.remove(0);
            *node = only;
        }
    }
}

/// The first (top-left) pane in a subtree.
fn first_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { children, .. } => children.first().map_or(PaneId(0), first_leaf),
    }
}

fn last_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { children, .. } => children.last().map_or(PaneId(0), last_leaf),
    }
}

/// Collect every pane id in tree order.
fn collect_leaves(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { children, .. } => {
            for child in children {
                collect_leaves(child, out);
            }
        },
    }
}

/// Tile `area` across `node`'s subtree, appending `(pane, rect)` for each leaf.
fn layout_in(node: &Node, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, area)),
        Node::Split {
            axis,
            children,
            weights,
        } => {
            for (child, sub) in children.iter().zip(split_rects(*axis, weights, area)) {
                layout_in(child, sub, out);
            }
        },
    }
}

fn split_rects(axis: SplitAxis, weights: &[f32], area: Rect) -> Vec<Rect> {
    let total = weights.iter().copied().sum::<f32>().max(f32::EPSILON);
    let extent = match axis {
        SplitAxis::Cols => area.width,
        SplitAxis::Rows => area.height,
    };
    let mut used = 0_u16;
    (0..weights.len())
        .map(|index| {
            let cells = if index + 1 == weights.len() {
                extent.saturating_sub(used)
            } else {
                let weight = weights.get(index).copied().unwrap_or(0.0);
                let raw = (f32::from(extent) * (weight / total)).round() as u16;
                raw.min(extent.saturating_sub(used))
            };
            let rect = match axis {
                SplitAxis::Cols => Rect {
                    x: area.x.saturating_add(used),
                    y: area.y,
                    width: cells,
                    height: area.height,
                },
                SplitAxis::Rows => Rect {
                    x: area.x,
                    y: area.y.saturating_add(used),
                    width: area.width,
                    height: cells,
                },
            };
            used = used.saturating_add(cells);
            rect
        })
        .collect()
}

fn dividers_in(node: &Node, area: Rect, out: &mut Vec<PaneDivider>) {
    let Node::Split {
        axis,
        children,
        weights,
    } = node
    else {
        return;
    };
    let rects = split_rects(*axis, weights, area);
    for index in 0..children.len().saturating_sub(1) {
        let position = match axis {
            SplitAxis::Cols => rects[index].right().saturating_sub(1),
            SplitAxis::Rows => rects[index].bottom().saturating_sub(1),
        };
        let (start, end) = match axis {
            SplitAxis::Cols => (area.y, area.bottom()),
            SplitAxis::Rows => (area.x, area.right()),
        };
        out.push(PaneDivider {
            axis: *axis,
            position,
            start,
            end,
            before: last_leaf(&children[index]),
            after: first_leaf(&children[index + 1]),
        });
    }
    for (child, rect) in children.iter().zip(rects) {
        dividers_in(child, rect, out);
    }
}

fn minimum_extent(node: &Node, axis: SplitAxis) -> u16 {
    match node {
        Node::Leaf(_) => match axis {
            SplitAxis::Cols => MIN_W,
            SplitAxis::Rows => MIN_H,
        },
        Node::Split {
            axis: node_axis,
            children,
            ..
        } if *node_axis == axis => children.iter().fold(0_u16, |sum, child| {
            sum.saturating_add(minimum_extent(child, axis))
        }),
        Node::Split { children, .. } => children
            .iter()
            .map(|child| minimum_extent(child, axis))
            .max()
            .unwrap_or(0),
    }
}

fn resize_divider_in(node: &mut Node, area: Rect, divider: PaneDivider, delta: i16) -> bool {
    let Node::Split {
        axis,
        children,
        weights,
    } = node
    else {
        return false;
    };
    let rects = split_rects(*axis, weights, area);
    if *axis == divider.axis {
        for index in 0..children.len().saturating_sub(1) {
            if last_leaf(&children[index]) != divider.before
                || first_leaf(&children[index + 1]) != divider.after
            {
                continue;
            }
            let sizes: Vec<u16> = rects
                .iter()
                .map(|rect| match axis {
                    SplitAxis::Cols => rect.width,
                    SplitAxis::Rows => rect.height,
                })
                .collect();
            let pair = sizes[index].saturating_add(sizes[index + 1]);
            let minimum_before = minimum_extent(&children[index], *axis);
            let minimum_after = minimum_extent(&children[index + 1], *axis);
            if pair < minimum_before.saturating_add(minimum_after) {
                return false;
            }
            let desired = (i32::from(sizes[index]) + i32::from(delta)).clamp(
                i32::from(minimum_before),
                i32::from(pair.saturating_sub(minimum_after)),
            ) as u16;
            if desired == sizes[index] {
                return false;
            }
            let mut resized = sizes;
            resized[index] = desired;
            resized[index + 1] = pair.saturating_sub(desired);
            *weights = resized.into_iter().map(f32::from).collect();
            return true;
        }
    }
    for (child, rect) in children.iter_mut().zip(rects) {
        if resize_divider_in(child, rect, divider, delta) {
            return true;
        }
    }
    false
}

/// Classify a point within a pane's `rect` as a center or edge drop zone. The edge
/// bands are 25% of the pane's width/height; a corner resolves to the nearest edge.
#[must_use]
pub fn drop_zone(rect: Rect, x: u16, y: u16) -> DropZone {
    if rect.width == 0 || rect.height == 0 || !contains(rect, x, y) {
        return DropZone::Center;
    }
    let w = f32::from(rect.width);
    let h = f32::from(rect.height);
    let dl = f32::from(x - rect.x) / w;
    let dr = f32::from(rect.right().saturating_sub(1).saturating_sub(x)) / w;
    let dt = f32::from(y - rect.y) / h;
    let db = f32::from(rect.bottom().saturating_sub(1).saturating_sub(y)) / h;
    let m = dl.min(dr).min(dt).min(db);
    const BAND: f32 = 0.25;
    if m >= BAND {
        DropZone::Center
    } else if m == dl {
        DropZone::Left
    } else if m == dr {
        DropZone::Right
    } else if m == dt {
        DropZone::Top
    } else {
        DropZone::Bottom
    }
}

/// The rectangle a drop into `zone` of `rect` would highlight (the previewed region
/// the dragged tab would land in).
#[must_use]
pub fn drop_preview_rect(rect: Rect, zone: DropZone) -> Rect {
    let half_w = rect.width / 2;
    let half_h = rect.height / 2;
    match zone {
        DropZone::Center => rect,
        DropZone::Left => Rect {
            width: half_w,
            ..rect
        },
        DropZone::Right => Rect {
            x: rect.x.saturating_add(rect.width - half_w),
            width: half_w,
            ..rect
        },
        DropZone::Top => Rect {
            height: half_h,
            ..rect
        },
        DropZone::Bottom => Rect {
            y: rect.y.saturating_add(rect.height - half_h),
            height: half_h,
            ..rect
        },
    }
}

/// Whether `(x, y)` lies inside `rect`.
fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect::new(0, 0, 80, 24)
    }

    fn rect_of(layout: &PaneLayout, pane: PaneId) -> Rect {
        layout.pane_rect(pane, area()).unwrap_or_default()
    }

    #[test]
    fn single_pane_fills_area() {
        let l = PaneLayout::new();
        assert_eq!(l.pane_count(), 1);
        assert_eq!(rect_of(&l, l.root_pane()), area());
    }

    #[test]
    fn split_right_tiles_two_columns() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right);
        assert_eq!(l.pane_count(), 2);
        assert_eq!(l.focus(), b);
        let ra = rect_of(&l, a);
        let rb = rect_of(&l, b);
        // Left then right, tiling the full width with no gap or overlap.
        assert_eq!(ra.x, 0);
        assert_eq!(rb.x, ra.right());
        assert_eq!(ra.width + rb.width, 80);
        assert_eq!(ra.height, 24);
        assert_eq!(rb.height, 24);
    }

    #[test]
    fn split_left_places_new_pane_first() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Left);
        assert!(rect_of(&l, b).x < rect_of(&l, a).x);
    }

    #[test]
    fn split_down_tiles_two_rows() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Down);
        let ra = rect_of(&l, a);
        let rb = rect_of(&l, b);
        assert_eq!(ra.y, 0);
        assert_eq!(rb.y, ra.bottom());
        assert_eq!(ra.height + rb.height, 24);
        assert_eq!(ra.width, 80);
    }

    #[test]
    fn same_axis_split_inserts_a_sibling_not_a_nested_split() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right);
        let c = l.split(b, SplitDir::Right);
        // Three columns, tiling the width, left-to-right a, b, c.
        assert_eq!(l.pane_count(), 3);
        let (ra, rb, rc) = (rect_of(&l, a), rect_of(&l, b), rect_of(&l, c));
        assert_eq!(ra.x, 0);
        assert_eq!(rb.x, ra.right());
        assert_eq!(rc.x, rb.right());
        assert_eq!(ra.width + rb.width + rc.width, 80);
    }

    #[test]
    fn nested_split_produces_three_tiling_rects() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right); // a | b
        let c = l.split(b, SplitDir::Down); // a | (b / c)
        assert_eq!(l.pane_count(), 3);
        let (ra, rb, rc) = (rect_of(&l, a), rect_of(&l, b), rect_of(&l, c));
        assert_eq!(ra.x, 0);
        assert_eq!(rb.x, ra.right());
        assert_eq!(rc.x, ra.right());
        assert_eq!(rb.height + rc.height, 24);
        assert_eq!(rc.y, rb.bottom());
    }

    #[test]
    fn close_collapses_parent_and_returns_neighbor() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right);
        let neighbor = l.close(b);
        assert_eq!(neighbor, Some(a));
        assert_eq!(l.pane_count(), 1);
        assert_eq!(rect_of(&l, a), area()); // the survivor fills the area again
    }

    #[test]
    fn close_last_pane_returns_none() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        assert_eq!(l.close(a), None);
        assert_eq!(l.pane_count(), 1);
    }

    #[test]
    fn close_focused_pane_moves_focus_to_neighbor() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right); // focus = b
        l.close(b);
        assert_eq!(l.focus(), a);
    }

    #[test]
    fn move_pane_relocates_preserving_id() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right); // a | b
        let c = l.split(b, SplitDir::Down); // a | (b / c)
        // Move c to below a; b's column collapses back to just b.
        l.move_pane(c, a, SplitDir::Down);
        assert_eq!(l.pane_count(), 3);
        assert!(l.contains(c));
        let ra = rect_of(&l, a);
        let rc = rect_of(&l, c);
        assert_eq!(rc.y, ra.bottom());
    }

    #[test]
    fn can_split_respects_minimum_size() {
        let l = PaneLayout::new();
        let a = l.root_pane();
        // 80 wide splits into columns fine…
        assert!(l.can_split(a, SplitDir::Right, area()));
        // …but a tiny area cannot.
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 12,
            height: 2,
        };
        assert!(!l.can_split(a, SplitDir::Down, tiny));
    }

    #[test]
    fn dividers_report_geometry_and_hit_testing() {
        let mut layout = PaneLayout::new();
        let left = layout.root_pane();
        let right = layout.split(left, SplitDir::Right);
        let dividers = layout.dividers(area());
        assert_eq!(dividers.len(), 1);
        assert_eq!(
            dividers[0],
            PaneDivider {
                axis: SplitAxis::Cols,
                position: 39,
                start: 0,
                end: 24,
                before: left,
                after: right,
            }
        );
        assert!(dividers[0].contains(39, 12));
        assert!(!dividers[0].contains(40, 12));
    }

    #[test]
    fn divider_resize_changes_weights_and_clamps_to_descendant_minimums() {
        let mut layout = PaneLayout::new();
        let left = layout.root_pane();
        let right = layout.split(left, SplitDir::Right);
        let divider = layout.dividers(area())[0];

        assert!(layout.resize_divider(divider, 10, area()));
        assert_eq!(rect_of(&layout, left).width, 50);
        assert_eq!(rect_of(&layout, right).width, 30);
        assert!(layout.resize_divider(divider, i16::MAX, area()));
        assert_eq!(rect_of(&layout, left).width, 70);
        assert_eq!(rect_of(&layout, right).width, MIN_W);
        assert!(!layout.resize_divider(divider, 1, area()));
    }

    #[test]
    fn directional_resize_grows_the_focused_pane_toward_that_edge() {
        let mut layout = PaneLayout::new();
        let left = layout.root_pane();
        let right = layout.split(left, SplitDir::Right);

        assert!(layout.resize_pane(right, SplitDir::Left, 7, area()));
        assert_eq!(rect_of(&layout, left).width, 33);
        assert_eq!(rect_of(&layout, right).width, 47);
        assert!(!layout.resize_pane(right, SplitDir::Right, 7, area()));
    }

    #[test]
    fn nested_split_minimum_reserves_space_for_every_leaf() {
        let mut layout = PaneLayout::new();
        let left = layout.root_pane();
        let right = layout.split(left, SplitDir::Right);
        let lower_right = layout.split(right, SplitDir::Down);
        let _fourth = layout.split(lower_right, SplitDir::Right);
        let outer = layout
            .dividers(area())
            .into_iter()
            .find(|divider| divider.before == left)
            .unwrap_or(PaneDivider {
                axis: SplitAxis::Cols,
                position: 0,
                start: 0,
                end: 0,
                before: left,
                after: right,
            });

        assert!(layout.resize_divider(outer, i16::MAX, area()));
        let rects = layout.layout(area());
        assert!(rects.iter().all(|(_, rect)| rect.width >= MIN_W));
        assert_eq!(rect_of(&layout, left).width, 60);
    }

    #[test]
    fn drop_zone_classifies_center_and_edges() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };
        assert_eq!(drop_zone(r, 20, 10), DropZone::Center);
        assert_eq!(drop_zone(r, 0, 10), DropZone::Left);
        assert_eq!(drop_zone(r, 39, 10), DropZone::Right);
        assert_eq!(drop_zone(r, 20, 0), DropZone::Top);
        assert_eq!(drop_zone(r, 20, 19), DropZone::Bottom);
    }

    #[test]
    fn drop_preview_halves_for_edges_and_fills_for_center() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 20,
        };
        assert_eq!(drop_preview_rect(r, DropZone::Center), r);
        assert_eq!(drop_preview_rect(r, DropZone::Left).width, 20);
        let right = drop_preview_rect(r, DropZone::Right);
        assert_eq!(right.x, 20);
        assert_eq!(right.width, 20);
        assert_eq!(drop_preview_rect(r, DropZone::Top).height, 10);
        assert_eq!(drop_preview_rect(r, DropZone::Bottom).y, 10);
    }

    #[test]
    fn layout_tiles_without_overlap_for_many_panes() {
        let mut l = PaneLayout::new();
        let a = l.root_pane();
        let b = l.split(a, SplitDir::Right);
        let _c = l.split(b, SplitDir::Down);
        let _d = l.split(a, SplitDir::Down);
        let rects = l.layout(area());
        // Every pane has a non-empty rect, and total cell area equals the frame.
        let total: u32 = rects
            .iter()
            .map(|(_, r)| u32::from(r.width) * u32::from(r.height))
            .sum();
        assert_eq!(total, 80 * 24);
        assert_eq!(rects.len(), l.pane_count());
    }
}

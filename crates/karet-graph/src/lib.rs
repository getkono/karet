//! DAG lane layout and a ratatui rail renderer for karet visualizations.
//!
//! The headless [`assign_lanes`] turns an ordered list of nodes-with-parents (a
//! commit history newest-first, or any DAG walked from its tips) into one [`RailRow`]
//! per input row: a compact glyph gutter (`● │ ├ ╮ ╯ ─ …`) plus, for each glyph, the
//! lane colour it belongs to. One row per input keeps a 1:1 mapping with the caller's
//! own columns (a commit's hash / summary / age), so the SCM panel draws the rails to
//! the left of the existing commit text.
//!
//! With the **`view`** feature, [`render_rail`](view::render_rail) turns a [`RailRow`]
//! into a themed ratatui [`Line`](ratatui::text::Line).

#[cfg(feature = "view")]
pub mod view;

/// One input node for [`assign_lanes`]: its id, its parent ids (first parent first),
/// and whether it is the current tip (`HEAD`) — which the renderer marks specially.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaneInput {
    /// Stable node id (a commit hash).
    pub id: String,
    /// Parent ids, first-parent first.
    pub parents: Vec<String>,
    /// Whether this node is `HEAD` (rendered with a distinct node glyph).
    pub head: bool,
}

impl LaneInput {
    /// A node with the given id and parents (not `HEAD`).
    #[must_use]
    pub fn new(id: impl Into<String>, parents: Vec<String>) -> Self {
        Self {
            id: id.into(),
            parents,
            head: false,
        }
    }
}

/// The laid-out rail gutter for one input row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailRow {
    /// The rendered glyph gutter (each `char` is one terminal cell).
    pub gutter: String,
    /// The lane colour index for each `char` in [`gutter`](RailRow::gutter) (same
    /// length, in `char`s). Renderers map the index onto a cycling palette.
    pub colors: Vec<u8>,
    /// The `char` column of this row's node glyph within the gutter.
    pub node_col: usize,
    /// Whether this node is a merge (two or more parents).
    pub merge: bool,
    /// Whether this node is `HEAD`.
    pub head: bool,
}

// Glyphs. Kept together so the visual language is easy to see and tweak.
const NODE: char = '\u{25CF}'; // ● commit
const NODE_HEAD: char = '\u{25C9}'; // ◉ HEAD
const NODE_MERGE: char = '\u{25C6}'; // ◆ merge
const RAIL: char = '\u{2502}'; // │ vertical rail
const DASH: char = '\u{2500}'; // ─ horizontal
const TL: char = '\u{256D}'; // ╭ down-and-right
const TR: char = '\u{256E}'; // ╮ down-and-left
const BL: char = '\u{2570}'; // ╰ up-and-right
const BR: char = '\u{256F}'; // ╯ up-and-left
const SPACE: char = ' ';

/// Assign lanes to `rows` (in display order — for a commit history, newest first) and
/// return one [`RailRow`] per input. A node takes the leftmost lane already expecting
/// it (else a new lane); its first parent continues that lane, additional parents open
/// or merge into lanes to the right, and lanes that expected this node fold into it.
#[must_use]
pub fn assign_lanes(rows: &[LaneInput]) -> Vec<RailRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<u8> = Vec::new();
    let mut next_color: u8 = 0;
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        // 1. Find (or open) the lane this node sits in.
        let node_lane = match lanes.iter().position(|l| l.as_deref() == Some(&row.id)) {
            Some(idx) => idx,
            None => alloc_lane(&mut lanes, &mut colors, &mut next_color),
        };

        // 2. Lanes to the left/right that also expected this node fold into node_lane.
        let converging: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(j, l)| *j != node_lane && l.as_deref() == Some(&row.id))
            .map(|(j, _)| j)
            .collect();

        // 3. Route each parent to a lane. The first parent continues on `node_lane`
        //    unless it is already tracked elsewhere (then `node_lane` folds into it);
        //    additional parents merge into an existing lane or open a new one. These
        //    target lanes all continue *below* this row (top corners).
        lanes[node_lane] = None; // cleared; the first parent may re-claim it below
        let mut parent_connectors: Vec<usize> = Vec::new();
        for (k, parent) in row.parents.iter().enumerate() {
            match lanes.iter().position(|l| l.as_deref() == Some(parent)) {
                Some(existing) => {
                    if existing != node_lane {
                        parent_connectors.push(existing);
                    }
                },
                None if k == 0 => lanes[node_lane] = Some(parent.clone()),
                None => {
                    let idx = alloc_lane(&mut lanes, &mut colors, &mut next_color);
                    lanes[idx] = Some(parent.clone());
                    parent_connectors.push(idx);
                },
            }
        }

        // 4. Fold the converged children closed (their lanes ended *above*: bottom
        //    corners), then render from the post-update lane picture.
        for &j in &converging {
            lanes[j] = None;
        }
        let width = lanes.len().max(node_lane + 1);
        let (gutter, colcolors, node_col) = render_row(
            node_lane,
            row,
            &lanes,
            &colors,
            &parent_connectors,
            &converging,
            width,
        );

        out.push(RailRow {
            gutter,
            colors: colcolors,
            node_col,
            merge: row.parents.len() >= 2,
            head: row.head,
        });

        trim_trailing(&mut lanes, &mut colors);
    }

    out
}

/// Allocate a new lane (reusing a freed slot when possible) and give it a fresh colour.
fn alloc_lane(lanes: &mut Vec<Option<String>>, colors: &mut Vec<u8>, next: &mut u8) -> usize {
    let color = *next;
    *next = next.wrapping_add(1);
    if let Some(idx) = lanes.iter().position(Option::is_none) {
        colors[idx] = color;
        idx
    } else {
        lanes.push(None);
        colors.push(color);
        lanes.len() - 1
    }
}

/// Drop trailing empty lanes so the gutter stays narrow.
fn trim_trailing(lanes: &mut Vec<Option<String>>, colors: &mut Vec<u8>) {
    while matches!(lanes.last(), Some(None)) {
        lanes.pop();
        colors.pop();
    }
}

/// Render one row into a glyph gutter. Uses a cell grid of `2*width - 1` columns —
/// lanes at even indices, inter-lane connectors at odd indices — then trims trailing
/// blanks. `lanes`/`colors` are the *post-update* lane picture (what continues below).
#[allow(clippy::too_many_arguments)] // a self-contained renderer; splitting hurts clarity
fn render_row(
    node_lane: usize,
    row: &LaneInput,
    lanes: &[Option<String>],
    colors: &[u8],
    parent_targets: &[usize],
    fold_targets: &[usize],
    width: usize,
) -> (String, Vec<u8>, usize) {
    let cells = 2 * width.max(1) - 1;
    let mut grid = vec![SPACE; cells];
    let mut gcol = vec![0u8; cells];
    let color_at = |lane: usize| colors.get(lane).copied().unwrap_or(0);

    // Vertical rails for every lane that continues below this row.
    for (lane, slot) in lanes.iter().enumerate() {
        if slot.is_some() && lane != node_lane {
            grid[2 * lane] = RAIL;
            gcol[2 * lane] = color_at(lane);
        }
    }

    // Connectors to parent lanes continue *below* (top corners ╮ ╭); connectors to
    // converging child lanes came from *above* (bottom corners ╯ ╰).
    for &target in parent_targets {
        connect(
            &mut grid,
            &mut gcol,
            node_lane,
            target,
            color_at(target),
            false,
        );
    }
    for &target in fold_targets {
        connect(
            &mut grid,
            &mut gcol,
            node_lane,
            target,
            color_at(target),
            true,
        );
    }

    // The node itself, drawn last so it always wins its own cell.
    let glyph = if row.head {
        NODE_HEAD
    } else if row.parents.len() >= 2 {
        NODE_MERGE
    } else {
        NODE
    };
    let node_col = 2 * node_lane;
    grid[node_col] = glyph;
    gcol[node_col] = color_at(node_lane);

    // Trim trailing blanks.
    let mut end = grid.len();
    while end > node_col + 1 && grid[end - 1] == SPACE {
        end -= 1;
    }
    let gutter: String = grid[..end].iter().collect();
    let colors_out = gcol[..end].to_vec();
    (gutter, colors_out, node_col)
}

/// Draw a horizontal connector between the node lane and a `target` lane, with a
/// rounded corner at the target end. `fold` picks a bottom corner (the target lane
/// ends *above*, folding into the node) rather than a top corner (the target lane
/// continues *below*, a branch or merge).
fn connect(
    grid: &mut [char],
    gcol: &mut [u8],
    node_lane: usize,
    target: usize,
    color: u8,
    fold: bool,
) {
    if target == node_lane {
        return;
    }
    let node_col = 2 * node_lane;
    let target_col = 2 * target;
    let (lo, hi) = (node_col.min(target_col), node_col.max(target_col));
    // The horizontal run between the two columns.
    for col in (lo + 1)..hi {
        if matches!(grid[col], SPACE | RAIL) {
            grid[col] = DASH;
            gcol[col] = color;
        }
    }
    // The corner at the target lane: top corners (╮ ╭) for a lane continuing below, or
    // bottom corners (╯ ╰) for a lane that folds in from above. Left/right picks which
    // way the horizontal leaves the corner.
    let corner = match (fold, target_col > node_col) {
        (false, true) => TR,  // ╮ branch to the right
        (false, false) => TL, // ╭ branch to the left
        (true, true) => BR,   // ╯ fold from the right
        (true, false) => BL,  // ╰ fold from the left
    };
    if matches!(grid[target_col], SPACE | RAIL) {
        grid[target_col] = corner;
        gcol[target_col] = color;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear() -> Vec<LaneInput> {
        vec![
            LaneInput {
                id: "c".into(),
                parents: vec!["b".into()],
                head: true,
            },
            LaneInput::new("b", vec!["a".into()]),
            LaneInput::new("a", vec![]),
        ]
    }

    #[test]
    fn linear_history_is_a_single_column() {
        let rows = assign_lanes(&linear());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].gutter, "\u{25C9}"); // ◉ HEAD
        assert_eq!(rows[1].gutter, "\u{25CF}"); // ●
        assert_eq!(rows[2].gutter, "\u{25CF}"); // ● root
        for row in &rows {
            assert_eq!(row.node_col, 0);
            assert!(!row.merge);
        }
    }

    #[test]
    fn merge_opens_a_second_lane_that_folds_back() {
        // d (merge of c, b); c and b both parent a.
        //   d           parents [c, b]  → lane0=c, lane1=b, connector ●─╮
        //   c           parents [a]     → lane0=a, lane1 rail │
        //   b           parents [a]     → node lane1, folds into lane0 (a already there)
        //   a           root
        let rows = assign_lanes(&[
            LaneInput::new("d", vec!["c".into(), "b".into()]),
            LaneInput::new("c", vec!["a".into()]),
            LaneInput::new("b", vec!["a".into()]),
            LaneInput::new("a", vec![]),
        ]);
        assert_eq!(rows.len(), 4);
        // Merge row: node is ◆ and a second lane is opened to the right.
        assert!(rows[0].merge);
        assert_eq!(rows[0].node_col, 0);
        assert!(
            rows[0].gutter.starts_with('\u{25C6}'),
            "merge node is ◆, got {:?}",
            rows[0].gutter
        );
        assert!(
            rows[0].gutter.contains('\u{2500}'),
            "merge draws a horizontal connector, got {:?}",
            rows[0].gutter
        );
        // While the side branch is live, the main-lane commit shows a parallel rail.
        assert!(
            rows[1].gutter.contains('\u{2502}'),
            "a parallel rail is shown while the branch is open, got {:?}",
            rows[1].gutter
        );
        // The side-branch commit sits in lane 1.
        assert_eq!(rows[2].node_col, 2, "side-branch node in the second lane");
        // Back to a single column at the root.
        assert_eq!(rows[3].gutter, "\u{25CF}");
    }

    #[test]
    fn colors_len_matches_gutter_char_count() {
        for row in assign_lanes(&linear()) {
            assert_eq!(row.gutter.chars().count(), row.colors.len());
        }
    }

    #[test]
    fn unknown_parents_do_not_panic_and_terminate_lanes() {
        // A tip whose parent is outside the provided window: the lane simply carries
        // the pending parent id and is never resolved — must not panic.
        let rows = assign_lanes(&[LaneInput::new("x", vec!["missing".into()])]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node_col, 0);
    }
}

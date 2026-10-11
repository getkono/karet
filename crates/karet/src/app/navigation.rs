//! Navigation history: Go Back / Go Forward across every jump.
//!
//! Each pane keeps its own history, browser-shaped: a back stack, a forward stack,
//! and the place the pane is at now. Two things move it:
//!
//! - An explicit jump ([`App::jump_to_location`]: Go to Definition, a references or
//!   symbol pick, a stack-frame click, the Seam view) always leaves an entry,
//!   however near the target is.
//! - Everything else is *observed*: after each input or backend event the pane's
//!   location is compared with the last one seen, and a move to another file or one
//!   of at least [`NAV_LINE_THRESHOLD`] lines leaves an entry too. That one rule
//!   covers search hits, go-to-line, opening or switching files, Ctrl+End and a far
//!   click, without every caller having to remember to record.
//!
//! A change of the buffer version is an edit, not a move, so typing or pasting a
//! page of text records nothing. Entries are positioned against the text they were
//! recorded in and move with it as it is edited (see [`App::shift_navigation`]).

use karet_core::TextEdit;
use karet_session::api::DocumentId;

use super::inlay::map_position;
use super::*;

/// How many entries each direction keeps. Deep enough to walk back through a long
/// exploration; bounded so a long session cannot grow it without limit.
const MAX_NAV_HISTORY: usize = 64;

/// How far the caret has to move within one file, in lines, before the place it
/// left is worth a step back. Arrowing and small clicks stay below it.
pub(crate) const NAV_LINE_THRESHOLD: u32 = 10;

/// One place in the history.
///
/// The position is stored against a path rather than a [`ViewId`] on purpose: a
/// view id dies with its tab, while a path can simply be reopened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavPoint {
    pub(crate) path: PathBuf,
    pub(crate) position: LineCol,
    /// The buffer version `position` is valid in; `None` for a tab with no text.
    pub(crate) version: Option<u64>,
    /// Whether the tab was the pane's reusable preview slot.
    pub(crate) preview: bool,
}

impl NavPoint {
    /// Whether going from one to the other is a step a user would go back over.
    fn far_from(&self, other: &Self) -> bool {
        self.path != other.path
            || self.position.line.abs_diff(other.position.line) >= NAV_LINE_THRESHOLD
    }

    /// Whether returning from `self` to `other` would change nothing visible.
    fn same_line(&self, other: &Self) -> bool {
        self.path == other.path && self.position.line == other.position.line
    }
}

/// One pane's back/forward history.
#[derive(Debug, Default)]
pub(crate) struct NavHistory {
    /// Places to go back to, most recent last.
    back: VecDeque<NavPoint>,
    /// Places Go Back left, to go forward to again, most recent last.
    forward: Vec<NavPoint>,
    /// Where the pane was when last looked at.
    current: Option<NavPoint>,
    /// Whether an edit has moved `current` since it was last observed. Shifting
    /// `current` keeps its version in step with the buffer, so without this the
    /// next observation could not tell the edit's caret jump from a navigation.
    edited: bool,
}

impl NavHistory {
    /// Take `now` as the pane's location, leaving an entry for where it was when
    /// the move was large enough to be worth one.
    pub(crate) fn observe(&mut self, now: NavPoint) {
        let shifted = std::mem::take(&mut self.edited);
        if let Some(current) = self.current.take() {
            // Same file, new text: an edit moved the caret, not a navigation.
            let edited = current.path == now.path && (shifted || current.version != now.version);
            // Arrowing through the explorer swaps the preview slot file by file;
            // that is browsing, and only the place it started from is a step. A
            // move within one preview file is not browsing.
            let browsing = current.preview && now.preview && current.path != now.path;
            if !edited && !browsing && current.far_from(&now) {
                self.leave(current);
            }
        }
        self.current = Some(now);
    }

    /// Record an explicit jump from `now` to `line` of `target`, before it happens.
    ///
    /// Any distance counts, except a jump that stays on the line it started from.
    pub(crate) fn jump(&mut self, now: Option<NavPoint>, target: &Path, line: u32) {
        if let Some(now) = now {
            self.observe(now);
        }
        let Some(current) = self.current.clone() else {
            return;
        };
        if current.path != target || current.position.line != line {
            self.leave(current);
        }
    }

    /// Adopt `now` as the pane's location without recording anything: the history
    /// itself just moved there.
    pub(crate) fn settle(&mut self, now: Option<NavPoint>) {
        if now.is_some() {
            self.current = now;
            self.edited = false;
        }
    }

    /// Step back: the place to go to, with the current one kept for Go Forward.
    ///
    /// Entries `live` rejects (a deleted file) and entries on the line the pane is
    /// already at are dropped on the way, so one keystroke always lands somewhere.
    pub(crate) fn back(&mut self, live: impl Fn(&Path) -> bool) -> Option<NavPoint> {
        while let Some(point) = self.back.pop_back() {
            if !self.worth_visiting(&point, &live) {
                continue;
            }
            if let Some(current) = self.current.take() {
                self.forward.push(current);
            }
            return Some(point);
        }
        None
    }

    /// Step forward again after [`back`](Self::back), with the same skipping rules.
    pub(crate) fn forward(&mut self, live: impl Fn(&Path) -> bool) -> Option<NavPoint> {
        while let Some(point) = self.forward.pop() {
            if !self.worth_visiting(&point, &live) {
                continue;
            }
            if let Some(current) = self.current.take() {
                self.push_back(current);
            }
            return Some(point);
        }
        None
    }

    /// Move every entry in `path` positioned against version `from` across
    /// `edits`, which took that text to version `to`.
    ///
    /// Only `from` moves, so a document shown in two panes, which hears about one
    /// edit along two paths, is shifted once.
    pub(crate) fn shift(&mut self, path: &Path, from: u64, to: u64, edits: &[TextEdit]) {
        let moves = |point: &mut NavPoint| {
            let hit = point.path == path && point.version == Some(from);
            if hit {
                point.position = map_position(point.position, edits);
                point.version = Some(to);
            }
            hit
        };
        self.back
            .iter_mut()
            .chain(self.forward.iter_mut())
            .for_each(|point| {
                moves(point);
            });
        if self.current.as_mut().is_some_and(moves) {
            self.edited = true;
        }
    }

    /// Whether any entry in `path` is positioned against version `version`.
    pub(crate) fn holds(&self, path: &Path, version: u64) -> bool {
        self.back
            .iter()
            .chain(&self.forward)
            .chain(&self.current)
            .any(|point| point.path == path && point.version == Some(version))
    }

    /// How many entries each direction holds, `(back, forward)`.
    #[cfg(test)]
    pub(crate) fn depth(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }

    fn worth_visiting(&self, point: &NavPoint, live: &impl Fn(&Path) -> bool) -> bool {
        live(&point.path)
            && !self
                .current
                .as_ref()
                .is_some_and(|current| current.same_line(point))
    }

    /// A new place left behind: it goes on the back stack and the forward stack is
    /// spent, as in a browser.
    fn leave(&mut self, point: NavPoint) {
        self.forward.clear();
        self.push_back(point);
    }

    fn push_back(&mut self, point: NavPoint) {
        if self.back.back().is_some_and(|last| last.same_line(&point)) {
            return;
        }
        if self.back.len() == MAX_NAV_HISTORY {
            self.back.pop_front();
        }
        self.back.push_back(point);
    }
}

impl App {
    /// The focused pane's location: its active tab's file and caret.
    fn nav_point(&self) -> Option<NavPoint> {
        let tab = self.tabs.get(self.active)?;
        let path = tab.path()?.to_path_buf();
        let version = match &tab.kind {
            TabKind::Code { buffer, .. } => Some(buffer.version()),
            _ => None,
        };
        Some(NavPoint {
            path,
            position: tab.editor.cursor(),
            version,
            preview: tab.is_preview,
        })
    }

    /// Note where the focused pane is now, recording the place it left when it
    /// moved far enough. Run after every input and backend event.
    pub(crate) fn observe_navigation(&mut self) {
        let layout = &self.layout;
        self.nav_history.retain(|pane, _| layout.contains(*pane));
        let Some(now) = self.nav_point() else {
            return;
        };
        let pane = self.focus_pane();
        self.nav_history.entry(pane).or_default().observe(now);
    }

    /// Adopt the focused pane's location as current without recording a move.
    fn settle_navigation(&mut self) {
        let now = self.nav_point();
        let pane = self.focus_pane();
        self.nav_history.entry(pane).or_default().settle(now);
    }

    /// Move to `position` in `path`, recording where the jump started.
    ///
    /// The caret lands on the target, collapsed to a bare caret with no selection.
    /// For a definition from a server that answers with a `LocationLink` that is the
    /// first character of the definition's *name*; a plain `Location` is whatever the
    /// server calls the start, often the `pub`/`fn` before it.
    pub(crate) fn jump_to_location(&mut self, path: &Path, position: LineCol) {
        let now = self.nav_point();
        let pane = self.focus_pane();
        self.nav_history
            .entry(pane)
            .or_default()
            .jump(now, path, position.line);
        self.focus_by_file_line(path, position, true);
        self.settle_navigation();
    }

    /// Go Back: return to the place the focused pane most recently left.
    pub(crate) fn jump_back(&mut self) {
        self.walk_history(true);
    }

    /// Go Forward: undo the last Go Back.
    pub(crate) fn jump_forward(&mut self) {
        self.walk_history(false);
    }

    fn walk_history(&mut self, back: bool) {
        // A move not yet observed is still the place to come back to.
        self.observe_navigation();
        let pane = self.focus_pane();
        let history = self.nav_history.entry(pane).or_default();
        // Never resurrect a file deleted since: `open_file` would happily produce
        // an empty phantom buffer for it.
        let target = if back {
            history.back(Path::exists)
        } else {
            history.forward(Path::exists)
        };
        let Some(target) = target else {
            let message = if back {
                "nothing to go back to"
            } else {
                "nothing to go forward to"
            };
            self.notify(Report::Refusal, NotificationKind::System, message);
            return;
        };
        // Deliberately not `jump_to_location`: that would record this step as a
        // new jump and spend the forward history.
        self.focus_by_file_line(&target.path, target.position, true);
        self.settle_navigation();
    }

    /// Carry every pane's history entries in `doc` across `edits`, which took its
    /// text from version `from` to `to`.
    pub(crate) fn shift_navigation(
        &mut self,
        doc: DocumentId,
        from: u64,
        to: u64,
        edits: &[TextEdit],
    ) {
        let Some(path) = self.doc_path(doc) else {
            return;
        };
        for history in self.nav_history.values_mut() {
            history.shift(&path, from, to, edits);
        }
    }

    /// Whether any history entry in `doc` is positioned against `version`, so a
    /// snapshot from it is worth recovering the edit for.
    pub(crate) fn navigation_holds(&self, doc: DocumentId, version: u64) -> bool {
        self.doc_path(doc).is_some_and(|path| {
            self.nav_history
                .values()
                .any(|history| history.holds(&path, version))
        })
    }

    /// The file `doc` is open from, wherever it is shown.
    pub(super) fn doc_path(&self, doc: DocumentId) -> Option<PathBuf> {
        self.all_tabs()
            .find(|tab| matches!(tab.kind, TabKind::Code { doc: Some(d), .. } if d == doc))
            .and_then(Tab::path)
            .map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: u32, version: u64) -> NavPoint {
        NavPoint {
            path: PathBuf::from(path),
            position: LineCol::new(line, 0),
            version: Some(version),
            preview: false,
        }
    }

    fn preview(path: &str) -> NavPoint {
        NavPoint {
            preview: true,
            ..at(path, 0, 1)
        }
    }

    fn always(_: &Path) -> bool {
        true
    }

    #[test]
    fn browsing_the_preview_slot_records_only_where_it_started() {
        let mut history = NavHistory::default();
        history.observe(at("main", 0, 1));
        history.observe(preview("a"));
        history.observe(preview("b"));
        history.observe(preview("c"));
        assert_eq!(history.depth(), (1, 0));
        let back = history.back(always);
        assert_eq!(back.map(|p| p.path), Some(PathBuf::from("main")));
    }

    #[test]
    fn a_large_move_within_one_preview_file_is_a_step() {
        let mut history = NavHistory::default();
        history.observe(preview("a"));
        history.observe(NavPoint {
            position: LineCol::new(2 * NAV_LINE_THRESHOLD, 0),
            ..preview("a")
        });
        assert_eq!(history.depth(), (1, 0));
        let back = history.back(always);
        assert_eq!(back.map(|p| p.position), Some(LineCol::new(0, 0)));
    }

    #[test]
    fn a_small_move_is_not_a_step_but_a_large_one_or_a_file_change_is() {
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        history.observe(at("a", NAV_LINE_THRESHOLD - 1, 1));
        assert_eq!(history.depth(), (0, 0));
        // Measured from the last place seen, not the first: arrowing down a page
        // one line at a time is never a jump.
        history.observe(at("a", NAV_LINE_THRESHOLD + 5, 1));
        assert_eq!(history.depth(), (0, 0));
        history.observe(at("a", 2 * NAV_LINE_THRESHOLD + 5, 1));
        assert_eq!(history.depth(), (1, 0));
        history.observe(at("b", 2 * NAV_LINE_THRESHOLD + 5, 1));
        assert_eq!(history.depth(), (2, 0));
    }

    #[test]
    fn an_edit_moving_the_caret_far_is_not_a_navigation() {
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        // A paste of a hundred lines: same file, new version.
        history.observe(at("a", 100, 2));
        assert_eq!(history.depth(), (0, 0));
    }

    #[test]
    fn a_paste_over_a_selection_ahead_of_the_caret_is_not_a_navigation() {
        let mut history = NavHistory::default();
        // The caret is the selection's head, at its start.
        history.observe(at("a", 4, 1));
        let paste = [TextEdit {
            range: karet_core::Range {
                start: LineCol::new(4, 0),
                end: LineCol::new(5, 0),
            },
            new_text: "x\n".repeat(20),
        }];
        // The edit is shifted in before the move is observed, syncing `current`
        // to the new version while leaving it at the replacement's start.
        history.shift(Path::new("a"), 1, 2, &paste);
        history.observe(at("a", 24, 2));
        assert_eq!(history.depth(), (0, 0));
        // The edit is spent: a real move after it is still a step.
        history.observe(at("a", 24 + NAV_LINE_THRESHOLD, 2));
        assert_eq!(history.depth(), (1, 0));
    }

    #[test]
    fn an_explicit_jump_counts_however_near_but_not_onto_its_own_line() {
        let mut history = NavHistory::default();
        history.jump(Some(at("a", 3, 1)), Path::new("a"), 3);
        assert_eq!(history.depth(), (0, 0));
        history.jump(Some(at("a", 3, 1)), Path::new("a"), 4);
        assert_eq!(history.depth(), (1, 0));
    }

    #[test]
    fn back_then_forward_walks_the_same_places() {
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        history.observe(at("b", 0, 1));
        history.observe(at("c", 0, 1));

        let back = history.back(always);
        assert_eq!(
            back.as_ref().map(|p| p.path.as_path()),
            Some(Path::new("b"))
        );
        history.settle(back);
        let back = history.back(always);
        assert_eq!(
            back.as_ref().map(|p| p.path.as_path()),
            Some(Path::new("a"))
        );
        history.settle(back);
        assert_eq!(history.back(always), None);
        assert_eq!(history.depth(), (0, 2));

        let forward = history.forward(always);
        assert_eq!(
            forward.as_ref().map(|p| p.path.as_path()),
            Some(Path::new("b"))
        );
        history.settle(forward);
        let forward = history.forward(always);
        assert_eq!(
            forward.as_ref().map(|p| p.path.as_path()),
            Some(Path::new("c"))
        );
        history.settle(forward);
        assert_eq!(history.forward(always), None);
        assert_eq!(history.depth(), (2, 0));
    }

    #[test]
    fn a_new_move_after_going_back_spends_the_forward_history() {
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        history.observe(at("b", 0, 1));
        let back = history.back(always);
        history.settle(back);
        assert_eq!(history.depth(), (0, 1));
        history.observe(at("c", 0, 1));
        assert_eq!(history.depth(), (1, 0));
    }

    #[test]
    fn going_back_skips_dead_files_and_the_line_already_shown() {
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        history.observe(at("gone", 0, 1));
        history.observe(at("b", 0, 1));
        let back = history.back(|path| path != Path::new("gone"));
        assert_eq!(back.map(|p| p.path), Some(PathBuf::from("a")));

        // Already back on the only entry's line: stepping "back" to it is a no-op,
        // so it is dropped rather than spending a keystroke.
        let mut history = NavHistory::default();
        history.observe(at("a", 0, 1));
        history.observe(at("b", 0, 1));
        history.settle(Some(at("a", 0, 1)));
        assert_eq!(history.back(always), None);
    }

    #[test]
    fn entries_move_with_edits_to_their_own_version_only() {
        let mut history = NavHistory::default();
        history.observe(at("a", 20, 1));
        history.observe(at("b", 0, 1));
        let inserted = [TextEdit {
            range: karet_core::Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 0),
            },
            new_text: "x\ny\n".to_owned(),
        }];
        // Another file, and an edit against a version the entry is not in, are
        // both left alone.
        history.shift(Path::new("c"), 1, 2, &inserted);
        history.shift(Path::new("a"), 7, 8, &inserted);
        assert!(history.holds(Path::new("a"), 1));
        history.shift(Path::new("a"), 1, 2, &inserted);
        assert!(history.holds(Path::new("a"), 2));
        assert!(!history.holds(Path::new("a"), 1));
        let back = history.back(always);
        assert_eq!(back.map(|p| p.position), Some(LineCol::new(22, 0)));
    }

    #[test]
    fn the_back_stack_stays_bounded() {
        let mut history = NavHistory::default();
        for i in 0..(MAX_NAV_HISTORY + 10) {
            history.observe(at(&format!("f{i}"), 0, 1));
        }
        assert_eq!(history.depth(), (MAX_NAV_HISTORY, 0));
    }
}

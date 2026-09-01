use super::*;

/// The responsive arrangement last used to draw a commit-like view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommitLayoutMode {
    /// Metadata, file index, and diff cards form one vertical document.
    Stacked,
    /// Metadata precedes a pinned file rail beside the diff cards.
    Wide,
}

/// View-local navigation state shared by commit and compare tabs.
#[derive(Debug, Default)]
pub(crate) struct CommitViewState {
    /// Vertical offset in the current layout's virtual document.
    pub(crate) scroll: u16,
    /// Horizontal offset for diff lines wider than the visible pane.
    pub(crate) column: u16,
    /// The layout used by the previous frame, for resize-aware anchor remapping.
    pub(crate) layout: Option<CommitLayoutMode>,
    /// Per-file card-header offsets from the previous frame.
    pub(crate) file_anchors: Vec<u16>,
    /// File cards the user has flipped away from their default collapse state.
    ///
    /// Storing the *overrides* rather than the collapsed set is what lets a
    /// machine-maintained file start folded: the effective state is derived per
    /// file where it is painted (`ui::commit::responsive::card_collapsed`), so
    /// files arriving after the tab exists need no seeding pass, and a refresh
    /// cannot silently re-fold a card the user just opened.
    pub(crate) toggled_files: BTreeSet<usize>,
    /// Directories folded in the changed-file tree.
    ///
    /// Keyed by path rather than by row index, for the reason the Search panel's
    /// fold set is: a re-fetched file list renumbers the rows, and an index key
    /// would silently transfer one directory's fold to whatever landed in its
    /// place. The key is a compacted chain's *deepest* directory — the path the
    /// row reports (`tab::changed_file_rows`).
    pub(crate) collapsed_dirs: BTreeSet<PathBuf>,
    /// The wide layout's file-rail offset.
    ///
    /// Independent of [`scroll`](Self::scroll): the wheel and the rail's scrollbar
    /// pan the index without moving the diff beside it, so a long commit's file
    /// list can be read ahead of the card on screen.
    pub(crate) rail_scroll: u16,
    /// The active file the rail last revealed.
    ///
    /// Revealing is what keeps the rail useful as the diff moves, but doing it
    /// every frame would undo the manual scroll the rail now has. Recording the
    /// file it was last done for makes it fire on a *change* of active file only.
    pub(crate) rail_revealed: Option<usize>,
    /// Rows the previous frame's document spent before its first file card.
    ///
    /// Folding a directory in the stacked layout shortens the table of contents,
    /// which moves every card anchor up by the same amount. Carrying the previous
    /// length is what lets the next frame shift `scroll` to match, instead of
    /// sliding the diff out from under the reader.
    pub(crate) prefix_rows: u16,
}

/// Human-readable title for standalone commit tabs.
#[must_use]
pub(crate) fn commit_title(short: &str) -> String {
    format!("Commit {short}")
}

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
}

/// Human-readable title for standalone commit tabs.
#[must_use]
pub(crate) fn commit_title(short: &str) -> String {
    format!("Commit {short}")
}

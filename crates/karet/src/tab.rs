//! Editor tabs: the open documents the main area can show.
//!
//! Each [`Tab`] carries a [`TabKind`] (the content + how to render it) plus an
//! [`EditorState`] used by code tabs for scroll/cursor. Diff and hex tabs keep
//! their own scroll inside the kind.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use karet_core::Decoration;
use karet_editor::EditorState;
use karet_fileview::image::Image;
use karet_fileview::viewer::FileKind;
use karet_pdf::Document as PdfDocument;
use karet_session::DocumentId;
use karet_session::ViewId;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_text::TextBuffer;

use crate::render::FileView;

/// How a diff tab is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// One column: removals then additions.
    Unified,
    /// Two columns: old on the left, new on the right.
    SideBySide,
}

/// The content of a tab and how to render it.
// The `Code` variant is intentionally the heavy one (it carries the buffer and its
// derived render state); there are only ever a handful of tabs, so boxing every
// field to equalize variant sizes would add indirection for no real benefit.
#[allow(clippy::large_enum_variant)]
pub enum TabKind {
    /// The landing page shown when nothing is open.
    Welcome,
    /// An editable code/text view.
    Code {
        /// The file path.
        path: PathBuf,
        /// The display language name.
        language: &'static str,
        /// The session document backing this view, once registered. Editing routes
        /// through the session; the fields below are the latest snapshot for render.
        doc: Option<DocumentId>,
        /// The base version for the next edit (predicted ahead of snapshot echoes so
        /// rapid typing isn't rejected as stale).
        next_version: u64,
        /// The latest snapshot's buffer (a cheap rope-sharing clone).
        buffer: TextBuffer,
        /// The source text (kept in sync with `buffer` for in-file search).
        text: String,
        /// Syntax highlight spans (empty when no grammar / disabled).
        highlights: Highlights,
        /// Foldable line regions from the latest snapshot (empty when no grammar).
        folds: FoldRegions,
        /// The set of collapsed fold header lines (per-view UI state).
        folded: BTreeSet<u32>,
        /// Find-in-file match decorations (empty when not searching).
        decos: Vec<Decoration>,
    },
    /// A raster image.
    Image {
        /// The file path.
        path: PathBuf,
        /// The decoded image.
        image: Image,
    },
    /// A rendered multi-page document (e.g. PDF): pages rasterized to images on
    /// demand and shown via the Kitty graphics protocol.
    Document {
        /// The file path.
        path: PathBuf,
        /// The parsed document; pages rasterize lazily during rendering.
        doc: PdfDocument,
        /// The total number of pages.
        page_count: usize,
        /// The current 0-based page.
        page: usize,
        /// Cache of the most recently rasterized page — `(page index, image)` — so a
        /// redraw at the same page does not re-rasterize.
        rendered: Option<(usize, Image)>,
        /// The document's navigation outline (bookmarks), extracted once at open;
        /// empty when the PDF has none. Drives the right-side outline panel.
        outline: Vec<karet_pdf::OutlineItem>,
    },
    /// A hex dump of binary content.
    Hex {
        /// The file path.
        path: PathBuf,
        /// The raw bytes.
        bytes: Vec<u8>,
        /// The first visible 16-byte row.
        scroll: usize,
    },
    /// A graceful placeholder (PDF, too-large, or undecodable image).
    Placeholder {
        /// The file path.
        path: PathBuf,
        /// Why it is not shown inline.
        kind: FileKind,
        /// Image dimensions, when known.
        dims: Option<(u32, u32)>,
        /// The file length in bytes.
        len: u64,
    },
    /// A single-file diff (opened from the Source Control panel).
    Diff {
        /// The prepared file diff.
        file: Box<FileView>,
        /// The current layout.
        view: ViewMode,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only semantic-blame view (`blameline`): consecutive lines grouped by
    /// the commit that introduced them, with full commit messages.
    Blame {
        /// The file the blame is for.
        path: PathBuf,
        /// The grouped blame entries, in line order.
        groups: Vec<blameline::BlameGroup>,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only code-visualization graph (dependency or usage), rendered as an
    /// indented tree.
    Graph {
        /// A short title for the view (workspace or symbol name).
        title: String,
        /// The neutral graph to render.
        view: karet_core::GraphView,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only, GitHub-parity commit view: the message, author/committer, parents,
    /// signature badge, changed-file list, and per-file semantic diffs.
    Commit {
        /// The commit metadata (message, author/committer, parents, signature).
        detail: Box<karet_vcs::CommitDetail>,
        /// Each changed file (vs the first parent), diffed and highlighted for display.
        files: Vec<FileView>,
        /// The forge's "Verified" verdict, once fetched (lazily, over the network).
        verification: Option<karet_session::GithubVerification>,
        /// When the signature badge was last double-clicked, if its explanatory
        /// tooltip is being revealed. The reveal auto-hides a few seconds later.
        explain_since: Option<Instant>,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only "compare" view: the diff between two points (a range), with the same
    /// summary + table-of-contents + per-file cards as the commit view, but a range
    /// header instead of commit metadata.
    Compare {
        /// The resolved "before" endpoint label (e.g. `origin/main`, or a short hash).
        base_label: String,
        /// The resolved "after" endpoint label (e.g. `HEAD`).
        head_label: String,
        /// Whether the diff was taken from the merge base (three-dot, `base...head`)
        /// rather than the two tips (two-dot, `base..head`).
        merge_base: bool,
        /// Each changed file between the two points, diffed and highlighted for display.
        files: Vec<FileView>,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// The full-screen commit graph browser: a scrollable DAG commit log on the left
    /// and the selected commit's detail on the right.
    CommitGraph {
        /// When set, the browser shows the history of this file (`git log -- <path>`)
        /// rather than the whole-repository log; paging uses the same source.
        history_path: Option<PathBuf>,
        /// The loaded commits, newest first (its own paged history).
        commits: Vec<karet_vcs::Commit>,
        /// Whether older commits remain to be paged in.
        has_more: bool,
        /// Whether a history page is currently in flight.
        loading: bool,
        /// The selected commit's index into `commits`.
        selected: usize,
        /// The selected commit's loaded detail, if the fetch has answered.
        detail: Option<Box<karet_vcs::CommitDetail>>,
        /// The selected commit's changed files, diffed for the detail pane.
        files: Vec<FileView>,
        /// The forge's verdict for the selected commit, once fetched.
        verification: Option<karet_session::GithubVerification>,
        /// A commit hash marked as the base for a two-commit comparison, if any. Set by
        /// "mark base"; the next "compare" diffs it against the current selection.
        compare_base: Option<String>,
        /// The commit-list scroll offset (first visible row).
        list_offset: u16,
    },
}

/// An open tab: a title, its content, and per-view editor state.
///
/// A tab *is* a view onto its content; [`view`](Tab::view) is its identity, which a
/// future tiled/split layout uses to let several views share one document (whose
/// edit log lives once in the session). It is `ViewId(0)` until [`App`] assigns a
/// real id when the tab is opened.
pub struct Tab {
    /// The tab title (usually a file name).
    pub title: String,
    /// The content + renderer.
    pub kind: TabKind,
    /// Code-tab scroll/cursor state.
    pub editor: EditorState,
    /// This view's identity (assigned by the app on open).
    pub view: ViewId,
    /// Whether the backing document has unsaved changes (code tabs only). Kept in
    /// sync from document snapshots and cleared on save.
    pub dirty: bool,
    /// When the in-flight save began, if a save is writing to disk. Drives the tab's
    /// saving spinner once the write exceeds a short threshold; `None` when idle.
    pub saving_since: Option<Instant>,
}

impl Tab {
    /// Build a tab from a title and content.
    #[must_use]
    pub fn new(title: impl Into<String>, kind: TabKind) -> Self {
        Self {
            title: title.into(),
            kind,
            editor: EditorState::new(),
            view: ViewId(0),
            dirty: false,
            saving_since: None,
        }
    }

    /// The welcome tab.
    #[must_use]
    pub fn welcome() -> Self {
        Self::new("Welcome", TabKind::Welcome)
    }

    /// A read-only visualization tab rendering `view` as an indented tree.
    #[must_use]
    pub fn graph(title: impl Into<String>, view: karet_core::GraphView) -> Self {
        let title = title.into();
        Self::new(
            title.clone(),
            TabKind::Graph {
                title,
                view,
                scroll: 0,
            },
        )
    }

    /// A read-only commit view for `detail` and its changed `files`.
    #[must_use]
    pub fn commit(detail: Box<karet_vcs::CommitDetail>, files: Vec<FileView>) -> Self {
        let title = format!("● {}", detail.short_hash);
        Self::new(
            title,
            TabKind::Commit {
                detail,
                files,
                verification: None,
                explain_since: None,
                scroll: 0,
            },
        )
    }

    /// An empty commit graph browser, to be filled as its history pages arrive. Pass
    /// `history_path` to scope it to one file's history; `None` browses the whole log.
    #[must_use]
    pub fn commit_graph(history_path: Option<PathBuf>, title: impl Into<String>) -> Self {
        Self::new(
            title,
            TabKind::CommitGraph {
                history_path,
                commits: Vec::new(),
                has_more: false,
                loading: true,
                selected: 0,
                detail: None,
                files: Vec::new(),
                verification: None,
                compare_base: None,
                list_offset: 0,
            },
        )
    }

    /// A read-only compare view for the diff between two points.
    #[must_use]
    pub fn compare(
        base_label: String,
        head_label: String,
        merge_base: bool,
        files: Vec<FileView>,
    ) -> Self {
        let sep = if merge_base { "\u{2026}" } else { ".." };
        let title = format!("\u{21c4} {base_label}{sep}{head_label}");
        Self::new(
            title,
            TabKind::Compare {
                base_label,
                head_label,
                merge_base,
                files,
                scroll: 0,
            },
        )
    }

    /// The file path backing this tab, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            TabKind::Code { path, .. }
            | TabKind::Image { path, .. }
            | TabKind::Document { path, .. }
            | TabKind::Hex { path, .. }
            | TabKind::Placeholder { path, .. }
            | TabKind::Blame { path, .. } => Some(path),
            TabKind::Diff { file, .. } => Some(&file.change.path),
            TabKind::Welcome
            | TabKind::Graph { .. }
            | TabKind::Commit { .. }
            | TabKind::Compare { .. }
            | TabKind::CommitGraph { .. } => None,
        }
    }

    /// Whether this is a diff tab (enables diff-specific keys).
    #[must_use]
    pub fn is_diff(&self) -> bool {
        matches!(self.kind, TabKind::Diff { .. })
    }

    /// A short language/kind label for the status bar.
    #[must_use]
    pub fn language(&self) -> &str {
        match &self.kind {
            TabKind::Code { language, .. } => language,
            TabKind::Image { .. } => "image",
            TabKind::Document { .. } => "pdf",
            TabKind::Hex { .. } => "binary",
            TabKind::Placeholder { .. } => "preview",
            TabKind::Diff { file, .. } => file.language,
            TabKind::Blame { .. } => "blame",
            TabKind::Graph { .. } => "graph",
            TabKind::Commit { .. } => "commit",
            TabKind::Compare { .. } => "compare",
            TabKind::CommitGraph { .. } => "commits",
            TabKind::Welcome => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_has_no_path() {
        let tab = Tab::welcome();
        assert!(tab.path().is_none());
        assert!(!tab.is_diff());
    }

    #[test]
    fn code_tab_reports_path_and_language() {
        let tab = Tab::new(
            "a.rs",
            TabKind::Code {
                path: PathBuf::from("/x/a.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer: TextBuffer::from_text("fn main() {}"),
                text: "fn main() {}".to_string(),
                highlights: Highlights::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
            },
        );
        assert_eq!(tab.path(), Some(Path::new("/x/a.rs")));
        assert_eq!(tab.language(), "Rust");
    }
}

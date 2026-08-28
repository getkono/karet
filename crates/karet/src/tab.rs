//! Editor tabs: the open documents the main area can show.
//!
//! Each [`Tab`] carries a [`TabKind`] (the content + how to render it) plus an
//! [`EditorState`] used by code tabs for scroll/cursor. Diff and hex tabs keep
//! their own scroll inside the kind.

mod language_servers;
mod view_state;

use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use karet_core::Decoration;
use karet_editor::EditorState;
#[cfg(any(feature = "images", feature = "pdf"))]
use karet_fileview::image::Image;
use karet_fileview::viewer::FileKind;
use karet_markdown::WrappedDocument;
#[cfg(feature = "pdf")]
use karet_pdf::Document as PdfDocument;
use karet_search::SearchQuery;
use karet_session::DocumentId;
use karet_session::LoadedConfig;
use karet_session::ViewId;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::TextBuffer;
pub(crate) use language_servers::LanguageServerAction;
pub(crate) use language_servers::LanguageServerActionHit;
pub(crate) use language_servers::LanguageServerPending;
pub(crate) use language_servers::LanguageServerPendingKind;
pub(crate) use language_servers::LanguageServersViewState;
use ratatui::layout::Rect;
pub(crate) use view_state::MarkdownPreviewState;
pub use view_state::ViewMode;

use crate::app::Pending;
use crate::render::FileView;
use crate::render::Section;

mod commit;
mod merge_conflict;
pub(crate) use commit::CommitLayoutMode;
pub(crate) use commit::CommitViewState;
pub(crate) use commit::commit_title;
pub(crate) use merge_conflict::MergeConflictState;

/// The find-in-file bar state: the query, the match cursor, and the replace field
/// (mirroring the workspace Search panel's model for a consistent UI). Lives on
/// the [`Tab`] it was opened over, so closing the find bar (but not the tab)
/// doesn't lose the query.
#[derive(Clone, Default)]
pub(crate) struct FindState {
    /// The search query.
    pub(crate) query: String,
    /// Caret and selection within [`Self::query`].
    pub(crate) query_edit: karet_widgets::textfield::TextFieldState,
    /// The replacement text.
    pub(crate) replace: String,
    /// Caret and selection within [`Self::replace`].
    pub(crate) replace_edit: karet_widgets::textfield::TextFieldState,
    /// The number of matches.
    pub(crate) count: usize,
    /// The current match (0-based).
    pub(crate) current: usize,
    /// Which field is being edited (find / replace).
    pub(crate) field: SearchField,
    /// Whether the replace field is shown (collapsible; hidden by default).
    pub(crate) replace_visible: bool,
    /// Interpret the query as a regular expression.
    pub(crate) regex: bool,
    /// Match case-sensitively.
    pub(crate) case_sensitive: bool,
    /// Match whole words only.
    pub(crate) whole_word: bool,
}

impl FindState {
    /// The [`SearchQuery`] for the current query text and option toggles.
    pub(crate) fn query_spec(&self) -> SearchQuery {
        SearchQuery {
            pattern: self.query.clone(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            ..Default::default()
        }
    }
}

/// Which field of a find/replace surface is being edited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SearchField {
    /// The find query.
    #[default]
    Find,
    /// The replacement text.
    Replace,
}

/// Two-axis scroll state shared by every read-only pager tab (diff, stash
/// patch, graph, settings inspector, loading commit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PagerState {
    /// Vertical scroll offset (display rows).
    pub scroll: u16,
    /// Horizontal scroll offset (display columns).
    pub column: u16,
}

/// The lazily-loaded changed-file block shared by the commit-family views
/// (standalone commit tab, compare tab, and the graph browser's detail pane):
/// the prepared per-file diffs plus their delayed-loading/error state and the
/// forge's lazily-fetched signature verdict.
#[derive(Default)]
pub struct CommitFiles {
    /// Each changed file, diffed and highlighted for display.
    pub files: Vec<FileView>,
    /// The in-flight changed-file extraction, if metadata is visible but the
    /// files are not yet.
    pub loading_since: Option<Pending>,
    /// A load error for the changed-file block, when metadata resolved but the
    /// diffs did not.
    pub error: Option<String>,
    /// The forge's "Verified" verdict, once fetched (lazily, over the network).
    pub verification: Option<karet_session::GithubVerification>,
}

impl CommitFiles {
    /// A fully loaded block.
    #[must_use]
    pub fn ready(files: Vec<FileView>) -> Self {
        Self {
            files,
            ..Self::default()
        }
    }

    /// An empty block whose extraction is in flight.
    #[must_use]
    pub fn loading() -> Self {
        Self {
            loading_since: Some(Pending::start()),
            ..Self::default()
        }
    }
}

/// The content of a tab and how to render it.
// The `Code` variant is intentionally the heavy one (it carries the buffer and its
// derived render state); there are only ever a handful of tabs, so boxing every
// field to equalize variant sizes would add indirection for no real benefit.
#[allow(clippy::large_enum_variant)]
pub enum TabKind {
    /// The landing page shown when nothing is open.
    Welcome,
    /// The singleton language-server inventory and lifecycle manager.
    LanguageServers(LanguageServersViewState),
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
        /// Semantic block scopes from the latest syntax snapshot.
        semantic_blocks: SemanticBlocks,
        /// Foldable line regions from the latest snapshot (empty when no grammar).
        folds: FoldRegions,
        /// The set of collapsed fold header lines (per-view UI state).
        folded: BTreeSet<u32>,
        /// Find-in-file match decorations (empty when not searching).
        decos: Vec<Decoration>,
        /// Global (workspace) search match decorations, kept separate from
        /// `decos` so closing/rerunning local find can't wipe them (or vice
        /// versa). Empty unless this tab's path is a current search result.
        search_decos: Vec<Decoration>,
        /// Inclusive line ranges covered by syntax errors, from the latest
        /// snapshot. Gates the completion auto-trigger (issue #57).
        syntax_errors: Vec<(u32, u32)>,
    },
    /// A standalone, read-only rendered Markdown document (for example converted DOCX).
    /// Editable Markdown previews are view-local state on [`Tab`], not tabs of this kind.
    MarkdownPreview {
        /// The file path.
        path: PathBuf,
        /// The converted Markdown buffer.
        buffer: TextBuffer,
        /// The parsed + wrapped render model, rebuilt only when `rendered` goes stale.
        wrapped: WrappedDocument,
        /// The `(document version, wrap width)` `wrapped` was built at, or `None` when it
        /// has never been built. A change in either rebuilds it on the next draw.
        rendered: Option<(u64, u16)>,
        /// The backend conversion producing this preview's markdown
        /// (a reserved DOCX preview), or `None` for an ordinary source preview.
        pending_since: Option<Pending>,
        /// The first visible wrapped line.
        scroll: u16,
    },
    /// A raster image.
    #[cfg(feature = "images")]
    Image {
        /// The file path.
        path: PathBuf,
        /// The decoded image.
        image: Image,
    },
    /// A rendered multi-page document (e.g. PDF): pages rasterized to images on
    /// demand and shown via the Kitty graphics protocol.
    #[cfg(feature = "pdf")]
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
    /// A LaTeX preview reserved immediately while its external compiler runs.
    LatexPreview {
        /// Editable TeX source that initiated the build.
        source: PathBuf,
        /// The in-flight build, driving the shared delayed-loading policy.
        loading_since: Pending,
        /// Compiler/startup failure, when the preview could not be produced.
        error: Option<String>,
    },
    /// A single-file diff (opened from the Source Control panel). The tab is
    /// reserved immediately with its identity; the prepared diff fills in when
    /// the backend answers (the shared delayed-loading policy hides the gap on
    /// fast paths).
    Diff {
        /// The diffed file's path (the tab's identity while the diff loads).
        path: PathBuf,
        /// The Source-Control group the diff belongs to.
        section: Section,
        /// The prepared file diff, once the backend has answered.
        file: Option<Box<FileView>>,
        /// The in-flight preparation request, if any.
        loading_since: Option<Pending>,
        /// A load error, when the diff could not be prepared.
        error: Option<String>,
        /// The current layout.
        view: ViewMode,
        /// Two-axis scroll state.
        pager: PagerState,
    },
    /// A read-only stash patch preview.
    StashPreview {
        /// Unified patch and stat output.
        patch: String,
        /// Two-axis scroll state.
        pager: PagerState,
    },
    /// A read-only code-visualization graph (dependency or usage), rendered as an
    /// indented tree.
    Graph {
        /// A short title for the view (workspace or symbol name).
        title: String,
        /// The neutral graph to render.
        view: karet_core::GraphView,
        /// Two-axis scroll state.
        pager: PagerState,
    },
    /// The full-screen Seam view: a package read by its seams rather than its files.
    ///
    /// Reserved with its identity the moment it is opened, so the pane switches
    /// immediately and the index fills in behind it.
    Seam(Box<crate::app::seam::SeamViewState>),
    /// A read-only view of the loaded settings and their provenance.
    LoadedConfig {
        /// The loaded configuration report.
        report: LoadedConfig,
        /// Two-axis scroll state.
        pager: PagerState,
    },
    /// A read-only, GitHub-parity commit view: the message, author/committer, parents,
    /// signature badge, changed-file list, and per-file semantic diffs.
    CommitLoading {
        /// The revision/hash being resolved.
        rev: String,
        /// The in-flight detail request; drives the delayed loading placeholder.
        loading_since: Pending,
        /// A load error for the revision, when metadata could not be resolved.
        error: Option<String>,
        /// Two-axis scroll state (reserved so the loading tab stays in the pager
        /// layer even while empty).
        pager: PagerState,
    },
    /// A read-only, GitHub-parity commit view: the message, author/committer, parents,
    /// signature badge, changed-file list, and per-file semantic diffs.
    Commit {
        /// The commit metadata (message, author/committer, parents, signature).
        detail: Box<karet_vcs::CommitDetail>,
        /// The changed files (vs the first parent) and their load state.
        files: CommitFiles,
        /// When the signature badge was last double-clicked, if its explanatory
        /// tooltip is being revealed. The reveal auto-hides a few seconds later.
        explain_since: Option<Instant>,
        /// Responsive scrolling, anchor, and file-rail state.
        view: CommitViewState,
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
        /// The changed files between the two points and their load state.
        files: CommitFiles,
        /// Responsive scrolling, anchor, and file-rail state.
        view: CommitViewState,
    },
    /// The full-screen commit-graph view: the DAG log across the whole pane, panned in
    /// both axes. Selecting a commit opens it as its own [`TabKind::Commit`] tab rather
    /// than an embedded pane, so a wide graph keeps the full width.
    CommitGraph {
        /// When set, the view shows the history of this file (`git log -- <path>`)
        /// rather than the whole-repository log; paging uses the same source.
        history_path: Option<PathBuf>,
        /// The loaded commits, newest first (its own paged history).
        commits: Vec<karet_vcs::Commit>,
        /// The lane layout for `commits`, cached because assignment is sequential from
        /// the tip and would otherwise be recomputed for every row on every frame.
        /// Always the same length as `commits`.
        rails: Vec<karet_graph::RailRow>,
        /// Whether older commits remain to be paged in.
        has_more: bool,
        /// Whether a history page is currently in flight.
        loading: bool,
        /// The in-flight history-page request, if any.
        loading_since: Option<Pending>,
        /// The selected commit's index into `commits`.
        selected: usize,
        /// A commit hash marked as the base for a two-commit comparison, if any. Set by
        /// "mark base"; the next "compare" diffs it against the current selection.
        compare_base: Option<String>,
        /// The vertical scroll offset (first visible row), free of the selection so the
        /// graph can be panned without moving the cursor.
        list_offset: u16,
        /// The horizontal scroll offset, for graphs wider than the pane.
        column: u16,
        /// The last painted rect of the commit rows. It sets how far ahead history is
        /// prefetched and maps a click to the commit under it. Empty until the view has
        /// been drawn once.
        list_rect: Rect,
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
    /// This tab's find-in-file query/toggles, kept for its lifetime (not reset by
    /// closing the find bar) so reopening Find over the same file restores the
    /// last search rather than starting blank. Dropped when the tab itself closes.
    pub(crate) find: Option<FindState>,
    /// Whether this is the pane's reusable "preview" tab (VS Code-style):
    /// navigating to another file replaces it in place instead of opening a new
    /// tab. Cleared permanently on the first edit (clean→dirty transition) or by
    /// double-clicking the file in the tree.
    pub(crate) is_preview: bool,
    /// Whether the path opened for this view was itself a filesystem symbolic link.
    pub(crate) is_symlink: bool,
    /// Cached merge-conflict decorations, keyed by the code buffer version.
    pub(crate) conflict_decorations: Option<(u64, Vec<Decoration>)>,
    /// Cached GFM table source-line ranges, keyed by the code buffer version.
    pub(crate) markdown_table_lines: Option<(u64, Vec<RangeInclusive<u32>>)>,
    /// A rendered Markdown preview shown inside this editor view, when enabled.
    pub(crate) markdown_preview: Option<MarkdownPreviewState>,
    /// Dedicated three-way conflict presentation for this editable code document.
    pub(crate) merge_conflict: Option<MergeConflictState>,
}

impl Tab {
    /// Build a tab from a title and content.
    #[must_use]
    pub fn new(title: impl Into<String>, kind: TabKind) -> Self {
        let is_symlink = tab_kind_path(&kind)
            .and_then(|path| std::fs::symlink_metadata(path).ok())
            .is_some_and(|metadata| metadata.file_type().is_symlink());
        Self {
            title: title.into(),
            kind,
            editor: EditorState::new(),
            view: ViewId(0),
            dirty: false,
            saving_since: None,
            find: None,
            is_preview: false,
            is_symlink,
            conflict_decorations: None,
            markdown_table_lines: None,
            markdown_preview: None,
            merge_conflict: None,
        }
    }

    /// The welcome tab.
    #[must_use]
    pub fn welcome() -> Self {
        Self::new("Welcome", TabKind::Welcome)
    }

    /// The language-server inventory and lifecycle manager.
    #[must_use]
    pub(crate) fn language_servers(pending: Option<karet_session::RequestId>) -> Self {
        Self::new(
            "Language Servers",
            TabKind::LanguageServers(LanguageServersViewState::loading(pending)),
        )
    }

    /// A rendered, read-only Markdown view of a converted document (e.g. a Word
    /// `.docx`) with no editable source tab or session document behind it. The
    /// conversion itself happens in the backend, so this is plain tab plumbing
    /// (compiled regardless of the `docx` feature).
    #[must_use]
    pub fn document_preview(path: PathBuf, markdown: &str) -> Self {
        let title = path
            .file_name()
            .map_or_else(|| path.to_string_lossy(), std::ffi::OsStr::to_string_lossy)
            .into_owned();
        Self::new(
            title,
            TabKind::MarkdownPreview {
                path,
                buffer: TextBuffer::from_text(markdown),
                wrapped: WrappedDocument::default(),
                rendered: None,
                pending_since: None,
                scroll: 0,
            },
        )
    }

    /// A standalone markdown preview reserved while the backend converts the
    /// document (DOCX) to markdown; [`Self::document_preview`]'s loading state.
    #[must_use]
    pub fn document_converting(path: PathBuf) -> Self {
        let mut tab = Self::document_preview(path, "");
        if let TabKind::MarkdownPreview { pending_since, .. } = &mut tab.kind {
            *pending_since = Some(Pending::start());
        }
        tab
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
                pager: PagerState::default(),
            },
        )
    }

    /// A read-only loaded-configuration inspector.
    #[must_use]
    pub fn loaded_config(report: LoadedConfig) -> Self {
        Self::new(
            "Loaded Settings",
            TabKind::LoadedConfig {
                report,
                pager: PagerState::default(),
            },
        )
    }

    /// A read-only stash patch preview.
    #[must_use]
    pub fn stash_preview(reference: &str, patch: String) -> Self {
        Self::new(
            format!("Stash {reference}"),
            TabKind::StashPreview {
                patch,
                pager: PagerState::default(),
            },
        )
    }

    /// A single-file diff tab. With `file` present the diff shows immediately;
    /// without it the tab is reserved in its loading state (the caller has asked
    /// the backend to prepare the diff).
    #[must_use]
    pub fn diff(
        title: String,
        path: PathBuf,
        section: Section,
        file: Option<Box<FileView>>,
        view: ViewMode,
    ) -> Self {
        let loading_since = file.is_none().then(Pending::start);
        Self::new(
            title,
            TabKind::Diff {
                path,
                section,
                file,
                loading_since,
                error: None,
                view,
                pager: PagerState::default(),
            },
        )
    }

    /// A read-only commit view for `detail` and its changed `files`.
    #[must_use]
    pub fn commit(detail: Box<karet_vcs::CommitDetail>, files: CommitFiles) -> Self {
        let title = commit_title(&detail.short_hash);
        Self::new(
            title,
            TabKind::Commit {
                detail,
                files,
                explain_since: None,
                view: CommitViewState::default(),
            },
        )
    }

    /// A commit tab opened before its full detail has loaded.
    #[must_use]
    pub fn commit_loading(rev: impl Into<String>) -> Self {
        let rev = rev.into();
        let title = commit_title(&rev.chars().take(7).collect::<String>());
        Self::new(
            title,
            TabKind::CommitLoading {
                rev,
                loading_since: Pending::start(),
                error: None,
                pager: PagerState::default(),
            },
        )
    }

    /// A pending LaTeX PDF preview.
    #[must_use]
    pub fn latex_preview(source: PathBuf) -> Self {
        let title = source.file_stem().map_or_else(
            || "LaTeX Preview".to_owned(),
            |stem| format!("{} (Preview)", stem.to_string_lossy()),
        );
        Self::new(
            title,
            TabKind::LatexPreview {
                source,
                loading_since: Pending::start(),
                error: None,
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
                rails: Vec::new(),
                has_more: false,
                loading: true,
                loading_since: Some(Pending::start()),
                selected: 0,
                compare_base: None,
                list_offset: 0,
                column: 0,
                list_rect: Rect::ZERO,
            },
        )
    }

    /// A read-only compare view for the diff between two points.
    #[must_use]
    pub fn compare(
        base_label: String,
        head_label: String,
        merge_base: bool,
        files: CommitFiles,
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
                view: CommitViewState::default(),
            },
        )
    }

    /// The file path backing this tab, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            TabKind::Code { path, .. }
            | TabKind::MarkdownPreview { path, .. }
            | TabKind::Hex { path, .. }
            | TabKind::Placeholder { path, .. } => Some(path),
            #[cfg(feature = "images")]
            TabKind::Image { path, .. } => Some(path),
            #[cfg(feature = "pdf")]
            TabKind::Document { path, .. } => Some(path),
            TabKind::Diff { path, .. } => Some(path),
            TabKind::Welcome
            | TabKind::LanguageServers(_)
            | TabKind::Seam(_)
            | TabKind::Graph { .. }
            | TabKind::LoadedConfig { .. }
            | TabKind::LatexPreview { .. }
            | TabKind::CommitLoading { .. }
            | TabKind::Commit { .. }
            | TabKind::Compare { .. }
            | TabKind::CommitGraph { .. } => None,
            TabKind::StashPreview { .. } => None,
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
            TabKind::MarkdownPreview { .. } => "markdown",
            #[cfg(feature = "images")]
            TabKind::Image { .. } => "image",
            #[cfg(feature = "pdf")]
            TabKind::Document { .. } => "pdf",
            TabKind::Hex { .. } => "binary",
            TabKind::Placeholder { .. } => "preview",
            TabKind::LatexPreview { .. } => "latex preview",
            TabKind::Diff { file, .. } => file.as_deref().map_or("diff", FileView::language),
            TabKind::StashPreview { .. } => "stash",
            TabKind::Seam(_) => "seams",
            TabKind::Graph { .. } => "graph",
            TabKind::LoadedConfig { .. } => "settings",
            TabKind::CommitLoading { .. } => "commit",
            TabKind::Commit { .. } => "commit",
            TabKind::Compare { .. } => "compare",
            TabKind::CommitGraph { .. } => "commits",
            TabKind::Welcome => "",
            TabKind::LanguageServers(_) => "language servers",
        }
    }

    /// The text encoding and line-ending label for the status bar (e.g.
    /// `"UTF-8 · LF"`, with a `"mixed EOL"` suffix when the file mixes `\n` and
    /// `\r\n`), for code tabs; `None` for anything else (images, hex dumps, …
    /// have no encoding/line-ending concept).
    #[must_use]
    pub fn encoding_label(&self) -> Option<String> {
        let TabKind::Code { buffer, .. } = &self.kind else {
            return None;
        };
        let mut label = format!("{} · {}", buffer.encoding(), buffer.eol());
        if buffer.has_mixed_eol() {
            label.push_str(" · mixed EOL");
        }
        Some(label)
    }
}

fn tab_kind_path(kind: &TabKind) -> Option<&Path> {
    match kind {
        TabKind::Code { path, .. }
        | TabKind::MarkdownPreview { path, .. }
        | TabKind::Hex { path, .. }
        | TabKind::Placeholder { path, .. } => Some(path),
        #[cfg(feature = "images")]
        TabKind::Image { path, .. } => Some(path),
        #[cfg(feature = "pdf")]
        TabKind::Document { path, .. } => Some(path),
        TabKind::Diff { path, .. } => Some(path),
        TabKind::Welcome
        | TabKind::LanguageServers(_)
        | TabKind::StashPreview { .. }
        | TabKind::Seam(_)
        | TabKind::Graph { .. }
        | TabKind::LoadedConfig { .. }
        | TabKind::LatexPreview { .. }
        | TabKind::CommitLoading { .. }
        | TabKind::Commit { .. }
        | TabKind::Compare { .. }
        | TabKind::CommitGraph { .. } => None,
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
                semantic_blocks: SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        );
        assert_eq!(tab.path(), Some(Path::new("/x/a.rs")));
        assert_eq!(tab.language(), "Rust");
    }

    #[test]
    fn encoding_label_reports_encoding_and_eol_for_code_tabs_only() {
        let buffer = TextBuffer::from_bytes(b"a\r\nb\r\n").unwrap_or_default();
        let tab = Tab::new(
            "a.rs",
            TabKind::Code {
                path: PathBuf::from("/x/a.rs"),
                language: "Rust",
                doc: None,
                next_version: 0,
                buffer,
                text: "a\nb\n".to_string(),
                highlights: Highlights::default(),
                semantic_blocks: SemanticBlocks::default(),
                folds: FoldRegions::default(),
                folded: BTreeSet::new(),
                decos: Vec::new(),
                search_decos: Vec::new(),
                syntax_errors: Vec::new(),
            },
        );
        assert_eq!(tab.encoding_label().as_deref(), Some("UTF-8 · CRLF"));
        assert_eq!(Tab::welcome().encoding_label(), None);
    }

    #[test]
    fn commit_tabs_do_not_use_the_unsaved_marker_as_their_title() {
        let identity = karet_vcs::Identity {
            name: "Tester".to_string(),
            email: "t@example.com".to_string(),
            time: 0,
            offset: 0,
        };
        let detail = karet_vcs::CommitDetail {
            hash: "a".repeat(40),
            short_hash: "aaaaaaa".to_string(),
            summary: "subject".to_string(),
            body: String::new(),
            author: identity.clone(),
            committer: identity,
            parents: Vec::new(),
            signature: None,
        };

        let loaded = Tab::commit(Box::new(detail), CommitFiles::default());
        let loading = Tab::commit_loading("bbbbbbb111");

        assert_eq!(loaded.title, "Commit aaaaaaa");
        assert_eq!(loading.title, "Commit bbbbbbb");
        assert!(!loaded.dirty);
        assert!(!loading.dirty);
    }
}

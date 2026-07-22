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
use ratatui::layout::Rect;

use crate::render::FileView;

/// The `source_view` sentinel for a [`Tab::document_preview`]: a markdown preview
/// with no source tab. Real view ids are allocated upward from 1 (`ViewId(0)` is
/// the pre-assignment placeholder), so `u64::MAX` can never collide with one and
/// every source-pairing lookup (`previews_view`, scroll sync, reveal) misses it.
#[cfg(feature = "docx")]
pub(crate) const DETACHED_SOURCE_VIEW: ViewId = ViewId(u64::MAX);

/// The find-in-file bar state: the query, the match cursor, and the replace field
/// (mirroring the workspace Search panel's model for a consistent UI). Lives on
/// the [`Tab`] it was opened over, so closing the find bar (but not the tab)
/// doesn't lose the query.
#[derive(Clone, Default)]
pub(crate) struct FindState {
    /// The search query.
    pub(crate) query: String,
    /// The replacement text.
    pub(crate) replace: String,
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

/// How a diff tab is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// One column: removals then additions.
    Unified,
    /// Two columns: old on the left, new on the right.
    SideBySide,
}

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
    /// The layout used by the previous frame, for resize-aware anchor remapping.
    pub(crate) layout: Option<CommitLayoutMode>,
    /// Per-file card-header offsets from the previous frame.
    pub(crate) file_anchors: Vec<u16>,
    /// First file shown in the wide layout's pinned rail.
    pub(crate) rail_offset: usize,
}

/// The content of a tab and how to render it.
// The `Code` variant is intentionally the heavy one (it carries the buffer and its
// derived render state); there are only ever a handful of tabs, so boxing every
// field to equalize variant sizes would add indirection for no real benefit.
#[allow(clippy::large_enum_variant)]
pub enum TabKind {
    /// The landing page shown when nothing is open.
    Welcome,
    /// A GitHub repository dashboard, detail, or creation form.
    Github(crate::app::github::GithubViewState),
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
    /// A rendered, read-only preview of a Markdown document, shown beside the
    /// [`Code`](TabKind::Code) tab it mirrors.
    ///
    /// The source of truth is the session document, not this tab: `buffer` is refreshed
    /// from every snapshot, and the render model behind it is rebuilt lazily at draw
    /// time (see `rendered`).
    MarkdownPreview {
        /// The file path.
        path: PathBuf,
        /// The session document previewed, once the source tab has registered one.
        doc: Option<DocumentId>,
        /// The [`Tab::view`] of the source tab this previews. Scrolling is synchronized
        /// with it while both are their pane's active tab.
        source_view: ViewId,
        /// The latest snapshot's buffer (a cheap rope-sharing clone).
        buffer: TextBuffer,
        /// The parsed + wrapped render model, rebuilt only when `rendered` goes stale.
        wrapped: WrappedDocument,
        /// The `(document version, wrap width)` `wrapped` was built at, or `None` when it
        /// has never been built. A change in either rebuilds it on the next draw.
        rendered: Option<(u64, u16)>,
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
    /// A single-file diff (opened from the Source Control panel).
    Diff {
        /// The prepared file diff.
        file: Box<FileView>,
        /// The current layout.
        view: ViewMode,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only stash patch preview.
    StashPreview {
        /// Stable stash selector.
        reference: String,
        /// Unified patch and stat output.
        patch: String,
        /// Vertical scroll offset.
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
    /// A read-only view of the loaded settings and their provenance.
    LoadedConfig {
        /// The loaded configuration report.
        report: LoadedConfig,
        /// Vertical scroll offset (display rows).
        scroll: u16,
    },
    /// A read-only, GitHub-parity commit view: the message, author/committer, parents,
    /// signature badge, changed-file list, and per-file semantic diffs.
    CommitLoading {
        /// The revision/hash being resolved.
        rev: String,
        /// When the detail request began; drives the delayed loading placeholder.
        loading_since: Instant,
        /// A load error for the revision, when metadata could not be resolved.
        error: Option<String>,
        /// Vertical scroll offset (reserved so the loading tab stays in the pager layer).
        scroll: u16,
    },
    /// A read-only, GitHub-parity commit view: the message, author/committer, parents,
    /// signature badge, changed-file list, and per-file semantic diffs.
    Commit {
        /// The commit metadata (message, author/committer, parents, signature).
        detail: Box<karet_vcs::CommitDetail>,
        /// Each changed file (vs the first parent), diffed and highlighted for display.
        files: Vec<FileView>,
        /// When changed-file extraction began, if metadata is visible but files are not.
        files_loading_since: Option<Instant>,
        /// A load error for the changed-file block, when metadata resolved but diffs did
        /// not.
        files_error: Option<String>,
        /// The forge's "Verified" verdict, once fetched (lazily, over the network).
        verification: Option<karet_session::GithubVerification>,
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
        /// Each changed file between the two points, diffed and highlighted for display.
        files: Vec<FileView>,
        /// Responsive scrolling, anchor, and file-rail state.
        view: CommitViewState,
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
        /// When the history-page request began, if one is in flight.
        loading_since: Option<Instant>,
        /// The selected commit's index into `commits`.
        selected: usize,
        /// When the selected commit's detail request began, if one is in flight.
        detail_loading_since: Option<Instant>,
        /// The selected commit's loaded detail, if the fetch has answered.
        detail: Option<Box<karet_vcs::CommitDetail>>,
        /// The selected commit's changed files, diffed for the detail pane.
        files: Vec<FileView>,
        /// When changed-file extraction began, if metadata is visible but files are not.
        files_loading_since: Option<Instant>,
        /// A load error for the changed-file block, when metadata resolved but diffs did
        /// not.
        files_error: Option<String>,
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
    /// This tab's find-in-file query/toggles, kept for its lifetime (not reset by
    /// closing the find bar) so reopening Find over the same file restores the
    /// last search rather than starting blank. Dropped when the tab itself closes.
    pub(crate) find: Option<FindState>,
    /// Whether this is the pane's reusable "preview" tab (VS Code-style):
    /// navigating to another file replaces it in place instead of opening a new
    /// tab. Cleared permanently on the first edit (clean→dirty transition) or by
    /// double-clicking the file in the tree.
    pub(crate) is_preview: bool,
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
            find: None,
            is_preview: false,
        }
    }

    /// The welcome tab.
    #[must_use]
    pub fn welcome() -> Self {
        Self::new("Welcome", TabKind::Welcome)
    }

    /// The singleton, permanently pinned GitHub repository dashboard.
    #[must_use]
    pub(crate) fn github_dashboard(
        repository: karet_session::GithubRepository,
        auth: karet_session::GithubAuth,
    ) -> Self {
        Self::new(
            "GitHub",
            TabKind::Github(crate::app::github::GithubViewState::dashboard(
                repository, auth,
            )),
        )
    }

    /// A lazily loaded issue detail tab.
    #[must_use]
    pub(crate) fn github_issue(
        repository: karet_session::GithubRepository,
        number: u64,
        pending: Option<karet_session::RequestId>,
    ) -> Self {
        Self::new(
            format!("Issue #{number}"),
            TabKind::Github(crate::app::github::GithubViewState::Issue {
                repository,
                number,
                issue: None,
                comments: karet_session::GithubPage {
                    items: Vec::new(),
                    page: 1,
                    next_page: None,
                    total_count: None,
                },
                pending,
                loading_since: Instant::now(),
                error: None,
                scroll: 0,
            }),
        )
    }

    /// A pull-request detail tab seeded from its search result.
    #[must_use]
    pub(crate) fn github_pull_request(
        repository: karet_session::GithubRepository,
        pull_request: karet_session::GithubPullRequest,
        can_write: bool,
        pending: Option<karet_session::RequestId>,
    ) -> Self {
        Self::new(
            format!("Pull Request #{}", pull_request.number),
            TabKind::Github(crate::app::github::GithubViewState::PullRequest(
                crate::app::github::GithubPullRequestView {
                    repository,
                    pull_request,
                    comments: karet_session::GithubPage {
                        items: Vec::new(),
                        page: 1,
                        next_page: None,
                        total_count: None,
                    },
                    commits: Vec::new(),
                    checks: Vec::new(),
                    activity: Vec::new(),
                    activity_error: None,
                    can_write,
                    section: crate::app::github::GithubPullRequestSection::Conversation,
                    pending,
                    loading_since: Instant::now(),
                    error: None,
                    scroll: 0,
                    commit_cursor: 0,
                    commit_offset: 0,
                    body_edit: None,
                    comment_edit: String::new(),
                    editor: None,
                    preview: false,
                    section_hits: Vec::new(),
                    body_rect: Rect::default(),
                    comment_rect: Rect::default(),
                    merge_rect: Rect::default(),
                    draft_rect: Rect::default(),
                    check_hits: Vec::new(),
                    commits_rect: Rect::default(),
                },
            )),
        )
    }

    /// A read-only GitHub Actions workflow-run detail tab.
    #[must_use]
    pub(crate) fn github_workflow_run(
        repository: karet_session::GithubRepository,
        workflow: Option<karet_session::GithubWorkflow>,
        run: karet_session::GithubWorkflowRun,
    ) -> Self {
        Self::new(
            format!("Actions #{}", run.run_number),
            TabKind::Github(crate::app::github::GithubViewState::WorkflowRun {
                repository,
                workflow,
                run,
                scroll: 0,
            }),
        )
    }

    /// A new-issue form tab.
    #[must_use]
    pub(crate) fn github_new_issue(
        repository: karet_session::GithubRepository,
        metadata_pending: Option<karet_session::RequestId>,
    ) -> Self {
        let form = crate::app::github::GithubIssueForm {
            metadata_pending,
            ..crate::app::github::GithubIssueForm::default()
        };
        Self::new(
            "New GitHub Issue",
            TabKind::Github(crate::app::github::GithubViewState::NewIssue { repository, form }),
        )
    }

    /// A new-pull-request form tab.
    #[must_use]
    pub(crate) fn github_new_pull_request(repository: karet_session::GithubRepository) -> Self {
        Self::new(
            "New Pull Request",
            TabKind::Github(crate::app::github::GithubViewState::NewPullRequest {
                repository,
                form: crate::app::github::GithubPullRequestForm::default(),
            }),
        )
    }

    /// A rendered, read-only markdown view of a converted document (e.g. a Word
    /// `.docx`) with **no source tab behind it**: `doc` stays `None` forever (no
    /// session document is ever registered for it) and `source_view` is the
    /// [`DETACHED_SOURCE_VIEW`] sentinel no real view id can take, so every code
    /// path that pairs a preview with its source — scroll sync in both directions,
    /// preview reveal, document binding, the close guard — finds no partner and
    /// leaves this tab alone.
    #[cfg(feature = "docx")]
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
                doc: None,
                source_view: DETACHED_SOURCE_VIEW,
                buffer: TextBuffer::from_text(markdown),
                wrapped: WrappedDocument::default(),
                rendered: None,
                scroll: 0,
            },
        )
    }

    /// A rendered preview of the Markdown document `source_view` holds.
    ///
    /// `buffer` is seeded from the source tab so the preview paints on its very first
    /// frame, before any snapshot has arrived.
    #[must_use]
    pub fn markdown_preview(
        path: PathBuf,
        doc: Option<DocumentId>,
        source_view: ViewId,
        buffer: TextBuffer,
    ) -> Self {
        let title = preview_title(&path);
        Self::new(
            title,
            TabKind::MarkdownPreview {
                path,
                doc,
                source_view,
                buffer,
                wrapped: WrappedDocument::default(),
                rendered: None,
                scroll: 0,
            },
        )
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

    /// A read-only loaded-configuration inspector.
    #[must_use]
    pub fn loaded_config(report: LoadedConfig) -> Self {
        Self::new(
            "Loaded Settings",
            TabKind::LoadedConfig { report, scroll: 0 },
        )
    }

    /// A read-only stash patch preview.
    #[must_use]
    pub fn stash_preview(reference: String, patch: String) -> Self {
        Self::new(
            format!("Stash {reference}"),
            TabKind::StashPreview {
                reference,
                patch,
                scroll: 0,
            },
        )
    }

    /// A read-only commit view for `detail` and its changed `files`.
    #[must_use]
    pub fn commit(detail: Box<karet_vcs::CommitDetail>, files: Vec<FileView>) -> Self {
        let title = commit_title(&detail.short_hash);
        Self::new(
            title,
            TabKind::Commit {
                detail,
                files,
                files_loading_since: None,
                files_error: None,
                verification: None,
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
                loading_since: Instant::now(),
                error: None,
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
                loading_since: Some(Instant::now()),
                selected: 0,
                detail_loading_since: None,
                detail: None,
                files: Vec::new(),
                files_loading_since: None,
                files_error: None,
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
            TabKind::Diff { file, .. } => Some(&file.change.path),
            TabKind::Welcome
            | TabKind::Github(_)
            | TabKind::Graph { .. }
            | TabKind::LoadedConfig { .. }
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

    /// Whether this is the uncloseable pinned GitHub dashboard.
    #[must_use]
    pub(crate) fn is_github_dashboard(&self) -> bool {
        matches!(&self.kind, TabKind::Github(view) if view.is_pinned())
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
            TabKind::Diff { file, .. } => file.language,
            TabKind::StashPreview { .. } => "stash",
            TabKind::Graph { .. } => "graph",
            TabKind::LoadedConfig { .. } => "settings",
            TabKind::CommitLoading { .. } => "commit",
            TabKind::Commit { .. } => "commit",
            TabKind::Compare { .. } => "compare",
            TabKind::CommitGraph { .. } => "commits",
            TabKind::Welcome => "",
            TabKind::Github(_) => "github",
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

/// Human-readable title for standalone commit tabs.
#[must_use]
pub(crate) fn commit_title(short: &str) -> String {
    format!("Commit {short}")
}

/// Human-readable title for a markdown preview tab.
#[must_use]
pub(crate) fn preview_title(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(|| path.to_string_lossy(), std::ffi::OsStr::to_string_lossy);
    format!("Preview {name}")
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

        let loaded = Tab::commit(Box::new(detail), Vec::new());
        let loading = Tab::commit_loading("bbbbbbb111");

        assert_eq!(loaded.title, "Commit aaaaaaa");
        assert_eq!(loading.title, "Commit bbbbbbb");
        assert!(!loaded.dirty);
        assert!(!loading.dirty);
    }
}

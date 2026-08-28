//! The in-process contract between the presentation layer and the backend: the
//! [`Command`]s a client submits and the [`Event`]s the backend emits.
//!
//! This module carries only neutral `karet-core` (plus a few engine) types, so it
//! is the designated extraction point for a future dependency-light
//! `karet-protocol` crate when the client-server split is undertaken.

use std::borrow::Cow;
use std::path::PathBuf;

use karet_core::BlameAttribution;
use karet_core::Change;
use karet_core::CompletionItem;
use karet_core::CursorState;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::NotificationKind;
use karet_core::Severity;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use karet_text::EditCause;
use karet_vcs::Branch;
use karet_vcs::BranchTarget;
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::CreateBranchOptions;
use karet_vcs::Remote;
use karet_vcs::RemoteBranch;
use karet_vcs::RepositoryState;
use karet_vcs::RepositorySummary;
use karet_vcs::StashEntry;
use karet_vcs::StashOptions;

mod debug;
mod event;
mod github;
mod seam;
mod vcs;

pub use debug::*;
pub use event::Event;
pub use github::*;
pub use seam::*;
pub use vcs::*;

/// Per-document editing and serialization behavior after application settings and
/// matching EditorConfig files have been resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DocumentSettings {
    /// Whether indentation commands insert spaces (`true`) or hard tabs (`false`).
    pub insert_spaces: bool,
    /// Display columns in one indentation level.
    pub indent_size: u16,
    /// Display columns between hard-tab stops.
    pub tab_width: u16,
    /// Remove whitespace immediately before line endings on save.
    pub trim_trailing_whitespace: bool,
    /// Ensure non-empty files end in a newline on save when enabled.
    pub insert_final_newline: bool,
    /// Explicit line-ending override, or `None` to preserve the detected style.
    pub line_ending: Option<DocumentLineEnding>,
    /// Explicit text-encoding override, or `None` to preserve the detected encoding.
    pub encoding: Option<DocumentEncoding>,
    /// Active spell-check dictionary after settings and EditorConfig resolution.
    pub spelling_language: Option<SpellingLanguage>,
}

impl Default for DocumentSettings {
    fn default() -> Self {
        Self {
            insert_spaces: true,
            indent_size: 4,
            tab_width: 4,
            trim_trailing_whitespace: true,
            insert_final_newline: true,
            line_ending: None,
            encoding: None,
            spelling_language: None,
        }
    }
}

/// A bundled spell-check behavior supported by karet.
///
/// The dictionary files themselves are discovered at runtime so the application
/// package stays small; this enum deliberately keeps the supported locale set narrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpellingLanguage {
    /// American English (`en_US`).
    EnglishUnitedStates,
    /// British English (`en_GB`).
    EnglishUnitedKingdom,
}

impl SpellingLanguage {
    /// Parse an EditorConfig/BCP-47 or Hunspell spelling locale.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().replace('_', "-").to_ascii_lowercase().as_str() {
            "en" | "en-us" => Some(Self::EnglishUnitedStates),
            "en-gb" => Some(Self::EnglishUnitedKingdom),
            _ => None,
        }
    }

    /// Hunspell dictionary basename.
    #[must_use]
    pub const fn locale(self) -> &'static str {
        match self {
            Self::EnglishUnitedStates => "en_US",
            Self::EnglishUnitedKingdom => "en_GB",
        }
    }

    /// Human-readable status-bar label.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::EnglishUnitedStates => "English (US)",
            Self::EnglishUnitedKingdom => "English (UK)",
        }
    }
}

/// One codetag comment located by a workspace scan
/// ([`Command::ScanWorkspaceTodos`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TodoHit {
    /// The absolute path of the file the tag was found in.
    pub path: std::path::PathBuf,
    /// The 0-based line the tag opens on.
    pub line: u32,
    /// The matched tag (`TODO`, `FIXME`, …), as configured.
    pub tag: String,
    /// The comment text after the tag.
    pub message: String,
}

/// The freshness state of one manifest dependency (see
/// [`Event::ManifestHints`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ManifestHintState {
    /// The constraint already reaches the newest release.
    UpToDate,
    /// A patch release within the constraint exists.
    Patch,
    /// A newer release exists outside the constraint.
    Outdated,
    /// The current/locked version has known vulnerabilities.
    Vulnerable,
    /// The registry check failed for this dependency.
    Error,
}

/// One dependency's freshness, positioned at its version value in the
/// manifest text.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManifestHint {
    /// The package name.
    pub name: String,
    /// 0-based line of the version value.
    pub line: u32,
    /// Character column where the version value starts (no quotes).
    pub col_start: u32,
    /// Character column one past the version value.
    pub col_end: u32,
    /// The constraint exactly as written.
    pub current: String,
    /// The newest release, when known.
    pub latest: Option<String>,
    /// The classified state.
    pub state: ManifestHintState,
    /// Advisory ids affecting the current/locked version.
    pub vulnerabilities: Vec<String>,
}

/// One misspelling located by a workspace spelling scan
/// ([`Command::ScanWorkspaceSpelling`]).
///
/// Deliberately *not* a [`Diagnostic`](karet_core::Diagnostic): a scan hit belongs
/// to a file rather than an open document, and carries no replacement suggestions —
/// computing them for a whole workspace dominates the scan's cost, and the fix flow
/// lives in the editor, which recomputes them for the one word being fixed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpellingHit {
    /// The absolute path of the file the word was found in.
    pub path: PathBuf,
    /// The word's span, in the file's 0-based line/column coordinates.
    pub range: karet_core::Range,
    /// The unknown word exactly as written.
    pub word: String,
    /// The whole line the word sits on, trimmed of surrounding whitespace, as
    /// one-line context for a results list.
    pub line_text: String,
}

/// A text line-ending style supported by editable karet documents.
///
/// This *is* `karet-text`'s [`Eol`](karet_text::Eol) — the buffer's own
/// vocabulary, serde-ready, carried across the seam without a mirror type.
pub type DocumentLineEnding = karet_text::Eol;

/// A text encoding supported by editable karet documents.
///
/// This *is* `karet-text`'s [`Encoding`](karet_text::Encoding).
pub type DocumentEncoding = karet_text::Encoding;

use crate::config::LoadedConfig;

/// Identifies an open document within a session.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct DocumentId(pub u64);

/// Identifies a view (editor pane) within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ViewId(pub u64);

/// Correlates a [`Command`] with the [`Event`] that answers it.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct RequestId(pub u64);

/// Stable, opaque identity for a language-server provider.
///
/// The value is string-backed rather than a closed enum so adding a provider
/// does not break exhaustive downstream matches. Built-in constants cover
/// Karet's catalog; embedders may define their own static identifiers with
/// [`Self::new`].
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct LanguageServerId(Cow<'static, str>);

// The associated constants below are named like enum variants (`RustAnalyzer`,
// `Pyright`, …) because callers treat them as a closed set of well-known ids;
// SCREAMING_SNAKE_CASE would misread as configuration keys.
#[allow(non_upper_case_globals)]
impl LanguageServerId {
    /// Construct a stable provider ID.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(Cow::Owned(key.into()))
    }

    const fn builtin(key: &'static str) -> Self {
        Self(Cow::Borrowed(key))
    }

    /// Rust language intelligence from rust-analyzer.
    pub const RustAnalyzer: Self = Self::builtin("rust-analyzer");
    /// JavaScript and TypeScript intelligence from TypeScript Language Server.
    pub const TypeScript: Self = Self::builtin("typescript-language-server");
    /// Python language intelligence from Pyright.
    pub const Pyright: Self = Self::builtin("pyright");
    /// Python linting and formatting from Ruff.
    pub const Ruff: Self = Self::builtin("ruff");
    /// TeX and LaTeX language intelligence from texlab.
    pub const Texlab: Self = Self::builtin("texlab");
    /// C and C++ intelligence from clangd.
    pub const Clangd: Self = Self::builtin("clangd");
    /// C# intelligence from Roslyn.
    pub const CSharp: Self = Self::builtin("csharp");
    /// Go intelligence from gopls.
    pub const Gopls: Self = Self::builtin("gopls");
    /// Java intelligence from Eclipse JDT LS.
    pub const Jdtls: Self = Self::builtin("jdtls");
    /// Zig intelligence from ZLS.
    pub const Zls: Self = Self::builtin("zls");
    /// Astro framework intelligence.
    pub const Astro: Self = Self::builtin("astro-language-server");
    /// Svelte framework intelligence.
    pub const Svelte: Self = Self::builtin("svelte-language-server");
    /// Vue framework intelligence.
    pub const Vue: Self = Self::builtin("vue-language-server");
    /// Biome linting and formatting.
    pub const Biome: Self = Self::builtin("biome");
    /// YAML intelligence.
    pub const Yaml: Self = Self::builtin("yaml-language-server");
    /// XML intelligence from LemMinX.
    pub const Xml: Self = Self::builtin("lemminx");

    /// Stable registry key used in on-disk paths and manifests.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.0
    }

    /// Human-readable provider name for prompts and status.
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self.0.as_ref() {
            "typescript-language-server" => "TypeScript Language Server",
            "pyright" => "Pyright",
            "ruff" => "Ruff",
            "csharp" => "C# Language Server",
            "astro-language-server" => "Astro Language Server",
            "svelte-language-server" => "Svelte Language Server",
            "vue-language-server" => "Vue Language Server",
            "yaml-language-server" => "YAML Language Server",
            "lemminx" => "Eclipse LemMinX",
            other => other,
        }
    }
}

/// Opaque identifier for an exact, explicitly checked language-server update.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerPlanId(pub u64);

/// One exact language-server change returned by an explicit update check.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerChange {
    /// Managed provider.
    pub server: LanguageServerId,
    /// Currently active version, absent for a first installation.
    pub current: Option<String>,
    /// Exact version whose download metadata is held by the plan.
    pub target: String,
    /// Expected compressed download bytes, when upstream supplied a size.
    pub download_bytes: Option<u64>,
}

/// How a language-server executable was resolved for one repository root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum LanguageServerSource {
    /// An explicit `lsp.servers` configuration entry.
    Configured,
    /// A repository-local executable such as `node_modules/.bin` or `.venv/bin`.
    ProjectLocal,
    /// An executable resolved from the process `PATH`.
    Path,
    /// A checksum-verified installation managed by Karet.
    Managed,
    /// No usable executable is currently available.
    Unavailable,
}

/// Current lifecycle state of a repository-scoped language-server connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum LanguageServerRuntimeState {
    /// No open document currently needs the provider.
    Idle,
    /// A connection is being established.
    Starting,
    /// The provider is connected and serving this session.
    Running,
    /// The provider stopped and is waiting for a bounded retry.
    Retrying,
    /// Repeated failures opened the restart circuit.
    CircuitOpen,
    /// The provider task stopped without another retry.
    Stopped,
}

/// Resolution and runtime state for one provider at one repository root.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerInstanceStatus {
    /// Repository/workspace root passed to the language server.
    pub root: PathBuf,
    /// Where the executable was resolved.
    pub source: LanguageServerSource,
    /// Resolved executable, absent when unavailable.
    pub command: Option<String>,
    /// Resolved command-line arguments.
    pub args: Vec<String>,
    /// This session's runtime state for the provider/root pair.
    pub runtime: LanguageServerRuntimeState,
    /// Number of open documents attached to the instance.
    pub open_documents: usize,
    /// Most recent concise runtime failure, when known.
    pub error: Option<String>,
}

/// Complete local status for one built-in or configured language-server provider.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerStatus {
    /// Stable provider identity.
    pub server: LanguageServerId,
    /// Language IDs that select this provider.
    pub languages: Vec<String>,
    /// Whether the global LSP setting and this provider are enabled.
    pub enabled: bool,
    /// Whether Karet owns installation lifecycle operations for this provider.
    pub managed: bool,
    /// Why this built-in provider must be installed by the user, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_install_reason: Option<String>,
    /// Active Karet-managed version, if any.
    pub installed: Option<String>,
    /// Whether an unreferenced managed payload still awaits safe cleanup.
    pub cleanup_pending: bool,
    /// Repository-scoped resolution and runtime state.
    pub instances: Vec<LanguageServerInstanceStatus>,
}

/// The spell-check dictionary settings layer a word is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DictionaryScope {
    /// The per-user settings layer.
    User,
    /// The workspace's `.karet/setting.jsonc` layer.
    Project,
}

/// A request submitted by the presentation layer to the backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Command {
    /// Cancel a safely-droppable background request.
    ///
    /// Cancellation is cooperative: a worker suppresses results and stops before
    /// the next expensive phase. Repository mutations are never cancellable.
    Cancel {
        /// The original request to cancel.
        request: RequestId,
    },
    /// Open a document.
    OpenDocument {
        /// The file path to open.
        path: PathBuf,
        /// An explicit language id, or `None` to detect from the path.
        language: Option<String>,
    },
    /// Close a document.
    CloseDocument {
        /// The document to close.
        doc: DocumentId,
    },
    /// Apply an atomic change to a document.
    ApplyChange {
        /// The target document.
        doc: DocumentId,
        /// The change to apply.
        change: Change,
        /// Why the edit happened, used for undo grouping.
        cause: EditCause,
    },
    /// Save a document to disk.
    Save {
        /// The document to save.
        doc: DocumentId,
    },
    /// Retarget an open document to a new path after a filesystem rename/move.
    RetargetDocument {
        /// The document to retarget.
        doc: DocumentId,
        /// The document's new file path.
        path: PathBuf,
    },
    /// Undo the most recent edit group on a document.
    Undo {
        /// The target document.
        doc: DocumentId,
    },
    /// Redo the most recently undone edit group on a document.
    Redo {
        /// The target document.
        doc: DocumentId,
    },
    /// Request completions at a position.
    Completion {
        /// The target document.
        doc: DocumentId,
        /// The position to complete at.
        position: LineCol,
    },
    /// Request hover information at a position.
    Hover {
        /// The target document.
        doc: DocumentId,
        /// The position to hover.
        position: LineCol,
    },
    /// Resolve the definition of the symbol at a position.
    Definition {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
    },
    /// Request the document's symbols.
    DocumentSymbols {
        /// The target document.
        doc: DocumentId,
    },
    /// Query the managed language-server registry without performing network I/O.
    LanguageServerStatus,
    /// Explicitly approve discovery and installation of one missing server.
    InstallLanguageServer {
        /// Provider to install at its latest stable version.
        server: LanguageServerId,
    },
    /// Explicitly perform network metadata checks for installed servers.
    CheckLanguageServerUpdates {
        /// One provider to check, or `None` to force-check every installed provider.
        server: Option<LanguageServerId>,
    },
    /// Apply part or all of the exact update plan previously returned by the backend.
    ApplyLanguageServerPlan {
        /// Opaque plan identifier.
        plan: LanguageServerPlanId,
        /// Providers from the plan to apply. An empty set is rejected.
        servers: Vec<LanguageServerId>,
    },
    /// Deactivate a Karet-managed provider and safely retire its payload.
    UninstallLanguageServer {
        /// Managed provider to uninstall.
        server: LanguageServerId,
    },
    /// Restart this session's processes for an already-approved active version.
    RestartLanguageServer {
        /// Provider whose running slots should restart.
        server: LanguageServerId,
    },
    /// Search workspace symbols.
    WorkspaceSymbols {
        /// The query string.
        query: String,
    },
    /// Rename the symbol at a position.
    Rename {
        /// The target document.
        doc: DocumentId,
        /// The position of the symbol.
        position: LineCol,
        /// The new name.
        new_name: String,
    },
    /// Format a document as part of saving it.
    FormatOnSave {
        /// The document to format.
        doc: DocumentId,
    },
    /// Compile the LaTeX root containing an editable TeX document and produce a PDF.
    BuildLatex {
        /// The open TeX document that initiated the build.
        doc: DocumentId,
    },
    /// Run a workspace search on the backend's search worker; answered with
    /// [`Event::SearchResults`]. A newer search supersedes an unstarted one.
    Search {
        /// The search query and options.
        query: karet_search::SearchQuery,
        /// Keep at most this many file hits.
        limit: usize,
    },
    /// Spell-check the whole workspace on the backend's scan worker; answered with
    /// a stream of [`Event::SpellingScanProgress`] batches and one final
    /// [`Event::SpellingScanFinished`].
    ///
    /// Open documents are answered from their live buffers rather than from disk,
    /// so an unsaved edit is never reported stale. A no-op when spell-checking is
    /// disabled (the finish event still arrives, with nothing scanned), and
    /// cancellable through [`Command::Cancel`].
    ScanWorkspaceSpelling {
        /// Keep at most this many misspellings; the scan stops once it is reached
        /// and reports `truncated`.
        limit: usize,
    },
    /// Re-run the dependency-freshness check for one open manifest.
    RefreshManifestHints {
        /// The manifest document.
        doc: DocumentId,
    },
    /// Scan the workspace for codetag comments (`TODO`, `FIXME`, …), streaming
    /// results; cancellable through [`Command::Cancel`]. The tag vocabulary is
    /// `editor.semanticComments.tags` — the same set the editor tints.
    ScanWorkspaceTodos {
        /// Stop after this many hits, reporting truncation.
        limit: usize,
    },
    /// Start a debug session from a `debug.configurations` entry (the first
    /// one when unnamed). Progress and outcomes arrive as unsolicited
    /// `Debug*` events.
    DebugStart {
        /// The configuration name; `None` = the first configuration.
        configuration: Option<String>,
    },
    /// End the debug session, terminating the debuggee when the adapter
    /// supports it.
    DebugStop,
    /// Resume the stopped thread.
    DebugContinue,
    /// Step over the current line.
    DebugStepOver,
    /// Step into the call at the stop location.
    DebugStepIn,
    /// Step out of the current frame.
    DebugStepOut,
    /// Pause the running debuggee.
    DebugPause,
    /// The stopped thread's call stack; answered by [`Event::DebugStack`].
    DebugStackTrace,
    /// The variable scopes of one frame; answered by [`Event::DebugScopes`].
    DebugScopes {
        /// The frame id (from [`Event::DebugStack`]).
        frame: i64,
    },
    /// The children of a variables reference (a scope handle or a structured
    /// variable's); answered by [`Event::DebugVariables`]. Fetch lazily, on
    /// expand — references can be arbitrarily deep.
    DebugVariables {
        /// The `variablesReference` handle.
        reference: i64,
    },
    /// Evaluate an expression in the debuggee (the REPL); answered by
    /// [`Event::DebugEvaluated`].
    DebugEvaluate {
        /// The expression.
        expression: String,
        /// The frame to evaluate in, when one is selected.
        frame: Option<i64>,
    },
    /// Replace the breakpoints of one file (the full set, not a delta —
    /// `setBreakpoints` is full-replace per file by design). Stored so a
    /// session started later replays them; forwarded live to a running one,
    /// answered by [`Event::DebugBreakpoints`].
    DebugSetBreakpoints {
        /// The source file.
        path: std::path::PathBuf,
        /// The 0-based breakpoint lines.
        lines: Vec<u32>,
    },
    /// Run every code cell of a notebook, top to bottom, on its kernel
    /// (started on first use; `notebook.kernel.autoStart` starts it at open).
    /// Progress arrives as unsolicited `Notebook*` events plus refreshed
    /// [`Event::DocumentConverted`] previews; an errored cell stops the run.
    NotebookRunAll {
        /// The `.ipynb` path.
        path: std::path::PathBuf,
    },
    /// Run one code cell (by its index among the notebook's cells).
    NotebookRunCell {
        /// The `.ipynb` path.
        path: std::path::PathBuf,
        /// The cell index (all cells, not only code cells).
        cell: usize,
    },
    /// Interrupt the running cell (out-of-band, on the control channel).
    NotebookInterrupt,
    /// Restart the kernel; every cell's outputs are marked stale (cleared).
    NotebookRestart,
    /// Replace across every workspace match on the search worker; answered with
    /// [`Event::SearchReplaced`]. Open buffers pick the edits up through the
    /// file watcher.
    SearchReplaceAll {
        /// The query selecting the text to replace.
        query: karet_search::SearchQuery,
        /// The replacement text.
        replacement: String,
    },
    /// Add `word` to a spell-check dictionary settings layer. The write runs on
    /// the backend (never a UI thread); answered with
    /// [`Event::DictionaryWordAdded`], or
    /// [`Event::ProjectSettingsCreationRequired`] when the project layer does not
    /// exist and `create_project` was not set.
    AddDictionaryWord {
        /// The word to accept.
        word: String,
        /// Which settings layer receives it.
        scope: DictionaryScope,
        /// Explicit confirmation to create a missing project settings tree.
        create_project: bool,
    },
    /// Persist the inline-blame toggle to the user settings layer.
    SetBlameEnabled {
        /// Whether inline blame is enabled.
        enabled: bool,
    },
    /// Resolve the repository/remote facts for one file on the VCS worker
    /// (discovery starts from the file's own directory, so nested repositories
    /// resolve correctly); answered with [`Event::RemoteFacts`].
    RemoteFacts {
        /// The file whose repository context is wanted.
        path: PathBuf,
    },
    /// Prepare one [`Event::VcsStatus`] entry's displayable diff (line diff,
    /// syntax tokens, intra-line pairs) on the VCS worker; answered with
    /// [`Event::ChangePrepared`].
    PrepareChange {
        /// The changed file's path as listed by [`Event::VcsStatus`].
        path: PathBuf,
        /// `true` for the staged entry, `false` for the working-tree entry.
        staged: bool,
    },
    /// Index a repository's seams, answering with a stream of
    /// [`Event::SeamPackageIndexed`] closed by one [`Event::SeamIndexFinished`].
    IndexSeams {
        /// The package root to index. Defaults to the first workspace root when absent.
        root: Option<PathBuf>,
        /// Whether to trust the stored index or rebuild it from source.
        mode: SeamSync,
    },
    /// Re-index one file whose text changed, keeping the rest of the tree.
    ReindexSeams {
        /// The file that changed.
        path: PathBuf,
        /// Its current text, which may be unsaved buffer content.
        text: String,
    },
    /// Evaluate a seam query, answering with [`Event::SeamQueryResult`].
    SeamQuery {
        /// The query text, exactly as typed.
        text: String,
    },
    /// Fetch one seam node's edges and source, answering with [`Event::SeamNodeDetail`].
    SeamNode {
        /// The node's identity, as its semantic path.
        path: String,
    },
    /// Switch the active configuration, answering with a re-evaluated
    /// [`Event::SeamIndexed`].
    SetSeamConfiguration {
        /// The configuration to activate.
        name: String,
    },
    /// Convert a binary document (DOCX) to markdown for a read-only preview;
    /// answered with [`Event::DocumentConverted`].
    ConvertDocument {
        /// The document to convert.
        path: PathBuf,
    },
    /// Prepare an ad-hoc diff of two provided texts for display (e.g. the
    /// client's two-file diff mode); answered with [`Event::DiffPrepared`].
    PrepareDiff {
        /// Path used for language detection and labeling (the new side's).
        path: PathBuf,
        /// The old (left) text.
        old: String,
        /// The new (right) text.
        new: String,
    },
    /// Prepare the diff of one file at a revision against its current content,
    /// on the VCS worker; answered with [`Event::DiffPrepared`]. The revision
    /// side is read from the repository; the current side is `live` when given
    /// (an unsaved buffer), the worktree file otherwise.
    DiffWithRev {
        /// The file to diff (absolute or workspace-relative).
        path: PathBuf,
        /// The revision to read the old side at (e.g. `HEAD`, a branch, a hash).
        rev: String,
        /// The current (new-side) text, when the client holds unsaved edits.
        live: Option<String>,
    },
    /// Report the client's cursor/selection state for a view.
    SetCursor {
        /// The target document.
        doc: DocumentId,
        /// The view whose cursors changed.
        view: ViewId,
        /// The new cursor state.
        cursors: CursorState,
    },
    /// Stage the given paths (add their worktree state to the index).
    Stage {
        /// Repository-relative paths to stage.
        paths: Vec<PathBuf>,
    },
    /// Unstage the given paths (reset their index entries to `HEAD`).
    Unstage {
        /// Repository-relative paths to unstage.
        paths: Vec<PathBuf>,
    },
    /// Discard the working-tree changes to the given paths (destructive).
    Discard {
        /// Repository-relative paths to discard.
        paths: Vec<PathBuf>,
    },
    /// Apply a unified-diff patch to the index only (per-hunk staging):
    /// `reverse: false` stages the patch's changes, `reverse: true` un-stages
    /// them. The worktree is untouched. Answered by a fresh
    /// [`Event::VcsStatus`], or an [`Event::Notification`] when the patch does
    /// not apply.
    ApplyIndexPatch {
        /// A unified-diff patch (typically one hunk, from
        /// `karet_diff::format_hunk_patch`).
        patch: String,
        /// Un-stage instead of stage.
        reverse: bool,
    },
    /// Stage every change in the worktree.
    StageAll,
    /// Unstage every staged change.
    UnstageAll,
    /// Commit the staged changes with the given message.
    Commit {
        /// The commit message.
        message: String,
    },
    /// Generate a commit message from the staged diff (answered asynchronously by
    /// [`Event::CommitMessageGenerated`], or an [`Event::Notification`] when nothing
    /// is staged, generation fails, or the `aicommit` feature / `git.aiCommit`
    /// setting is disabled). Honours the `git.aiCommit.*` settings.
    GenerateCommitMessage,
    /// Recompute and re-emit the source-control status.
    RefreshVcs,
    /// Load the current and incoming index stages for an unresolved merge conflict.
    MergeConflict {
        /// Repository-relative or absolute path to the conflicted file.
        path: PathBuf,
    },
    /// Load branch, remote, operation, and stash state for Source Control.
    RepositorySnapshot,
    /// Compute compact status for a nested repository shown in the explorer.
    NestedRepositoryStatus {
        /// Exact nested repository worktree directory.
        path: PathBuf,
    },
    /// Run one repository mutation on the serialized background worker.
    VcsAction {
        /// Action to run.
        action: VcsAction,
    },
    /// Fetch a page of open pull requests for one GitHub remote.
    PullRequests {
        /// Configured remote whose URL identifies the GitHub repository.
        remote: String,
        /// One-based page number.
        page: u32,
        /// Maximum entries per page, from 1 to 100.
        per_page: u8,
    },
    /// Attribute the current buffer's cursor line.
    Blame {
        /// Open document to attribute.
        doc: DocumentId,
        /// Buffer version the client currently renders.
        version: u64,
        /// Zero-based cursor line.
        line: u32,
    },
    /// Fetch a page of the commit-history log (newest first), for lazy loading.
    VcsLog {
        /// How many commits to skip from `HEAD`.
        skip: usize,
        /// The maximum number of commits to return.
        limit: usize,
    },
    /// Load the full detail of a single commit (first answered by
    /// [`Event::CommitDetailReady`], then by [`Event::CommitReady`] once changed files
    /// are computed).
    CommitDetail {
        /// The revision to resolve: a hash, a ref name, `HEAD`, `HEAD~3`, ….
        rev: String,
    },
    /// Compute the diff between two points (answered by [`Event::RangeReady`], or an
    /// [`Event::Notification`] when the range cannot be resolved — e.g. no upstream, no
    /// base branch, a bad revision, or unrelated histories).
    RangeChanges {
        /// Which comparison to compute.
        spec: RangeSpec,
    },
    /// Fetch a page of a single file's history (answered by [`Event::FileHistory`]).
    FileHistory {
        /// The file whose history to walk.
        path: PathBuf,
        /// How many matching commits to skip.
        skip: usize,
        /// The maximum number of commits to return.
        limit: usize,
    },
    /// Lazily fetch a commit's GitHub "Verified" status (answered by
    /// [`Event::CommitVerification`]). A no-op unless the backend was built with the
    /// `github` feature and the `origin` remote is a GitHub repository.
    FetchCommitVerification {
        /// The full commit hash to look up.
        hash: String,
    },
    /// Re-evaluate GitHub eligibility and authentication for the workspace root.
    GithubRefresh,
    /// Authenticate the GitHub manager for this session with a personal access token.
    /// The backend consumes the token immediately and never includes it in an event.
    GithubLogin {
        /// Personal access token entered through the presentation's masked control.
        token: GithubToken,
    },
    /// Search repository issues with GitHub query syntax.
    GithubSearchIssues {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Search repository pull requests with GitHub query syntax.
    GithubSearchPullRequests {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Load repository Actions workflows and recent runs.
    GithubActions {
        /// One-based result page.
        page: u32,
    },
    /// Load one issue and its complete conversation comments.
    GithubIssue {
        /// Repository-local issue number.
        number: u64,
    },
    /// Load one pull request's canonical primary resource.
    GithubPullRequest {
        /// Repository-local pull request number.
        number: u64,
    },
    /// Replace a pull request's Markdown description.
    GithubUpdatePullRequestBody {
        /// Repository-local pull-request number.
        number: u64,
        /// New Markdown body.
        body: String,
    },
    /// Add a Markdown comment to a pull request conversation.
    GithubCommentPullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Comment Markdown.
        body: String,
    },
    /// Merge a pull request at its currently displayed head SHA.
    GithubMergePullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Expected head SHA, preventing an unseen update from being merged.
        head_sha: String,
    },
    /// Convert a pull request to draft or mark it ready for review.
    GithubSetPullRequestDraft {
        /// GraphQL pull-request node identifier.
        node_id: String,
        /// Repository-local pull-request number, used to refresh after mutation.
        number: u64,
        /// Desired draft state.
        draft: bool,
    },
    /// Load repository-aware options for the new-issue form.
    GithubIssueMetadata,
    /// Create a repository issue.
    GithubCreateIssue {
        /// The complete primary create payload.
        issue: GithubNewIssue,
    },
    /// Create a repository pull request.
    GithubCreatePullRequest {
        /// The complete primary create payload.
        pull_request: GithubNewPullRequest,
    },
    /// Recover the crash-recovery swaps announced by [`Event::SwapsFound`]: restore
    /// each backed-up buffer as an unsaved (dirty) document.
    RecoverSwaps,
    /// Discard the crash-recovery swaps announced by [`Event::SwapsFound`] without
    /// recovering them.
    DiscardSwaps,
    /// Build the workspace package-dependency graph (answered by [`Event::GraphReady`]).
    DependencyGraph,
    /// Return the loaded settings and their in-memory provenance for this session.
    LoadedConfig,
}

/// Which visualization a [`Event::GraphReady`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum GraphKind {
    /// The package-dependency graph of the workspace.
    Dependency,
    /// The usage/call graph of a symbol.
    Usage,
}

/// A crash-recovery swap offered to the UI on startup (see [`Event::SwapsFound`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SwapInfo {
    /// The document the swap backs up.
    pub original: PathBuf,
    /// When the swap was last written (milliseconds since the Unix epoch).
    pub updated_unix_ms: u128,
    /// Whether the original file changed on disk since the swap was written —
    /// recovering would discard those on-disk changes.
    pub conflict: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam's serde-ready claim, enforced: representative [`Command`]s and
    /// [`Event`]s must round-trip through JSON. Every type a variant carries is
    /// pulled into this guarantee by the derive, so a leak that breaks
    /// serializability fails to compile and a behavioral regression fails here.
    #[test]
    fn commands_and_events_round_trip_through_serde() -> Result<(), serde_json::Error> {
        fn rt<T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug>(
            value: &T,
        ) -> Result<String, serde_json::Error> {
            let json = serde_json::to_string(value)?;
            let back: T = serde_json::from_str(&json)?;
            serde_json::to_string(&back)
        }

        let open = Command::OpenDocument {
            path: PathBuf::from("src/main.rs"),
            language: None,
        };
        assert_eq!(rt(&open)?, serde_json::to_string(&open)?);

        let apply = Command::ApplyChange {
            doc: DocumentId(3),
            change: Change::new(
                7,
                vec![TextEdit {
                    range: karet_core::Range::default(),
                    new_text: "x".into(),
                }],
            ),
            cause: EditCause::Type,
        };
        assert_eq!(rt(&apply)?, serde_json::to_string(&apply)?);

        let vcs = Command::VcsAction {
            action: VcsAction::SwitchBranch(BranchTarget::Local("main".into())),
        };
        assert_eq!(rt(&vcs)?, serde_json::to_string(&vcs)?);

        let diag = Event::DiagnosticsPublished {
            doc: DocumentId(3),
            diagnostics: vec![Diagnostic {
                range: karet_core::Range::default(),
                severity: Severity::Warning,
                message: "m".into(),
                source: None,
                code: None,
                tags: Vec::new(),
                related: Vec::new(),
            }],
        };
        assert_eq!(rt(&diag)?, serde_json::to_string(&diag)?);

        let hunk = Command::ApplyIndexPatch {
            patch: "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+b\n".into(),
            reverse: true,
        };
        assert_eq!(rt(&hunk)?, serde_json::to_string(&hunk)?);

        let prepare = Command::PrepareChange {
            path: PathBuf::from("src/main.rs"),
            staged: true,
        };
        assert_eq!(rt(&prepare)?, serde_json::to_string(&prepare)?);

        let status = Event::VcsStatus {
            staged: vec![ChangeSummary {
                path: PathBuf::from("src/main.rs"),
                old_path: None,
                status: karet_vcs::StatusKind::Modified,
                is_binary: false,
                added: 2,
                removed: 1,
            }],
            working: Vec::new(),
        };
        assert_eq!(rt(&status)?, serde_json::to_string(&status)?);

        let prepared = Event::ChangePrepared {
            path: PathBuf::from("src/main.rs"),
            staged: false,
            result: Ok(Box::new(PreparedChange {
                path: PathBuf::from("src/main.rs"),
                old_path: None,
                status: karet_vcs::StatusKind::Modified,
                language: "Rust".into(),
                diff: karet_diff::PreparedDiff::new(
                    karet_diff::diff_text("a\n", "b\n", &karet_diff::DiffOptions::default()),
                    Vec::new(),
                    Vec::new(),
                ),
            })),
        };
        assert_eq!(rt(&prepared)?, serde_json::to_string(&prepared)?);
        Ok(())
    }

    /// The one secret in the vocabulary must refuse to serialize: a transport can
    /// never move a GitHub token, by construction.
    #[test]
    fn github_token_refuses_to_serialize() {
        let command = Command::GithubLogin {
            token: GithubToken::new("github_pat_secret".into()),
        };
        let result = serde_json::to_string(&command);
        assert!(result.is_err(), "a credential must not cross the seam");
    }

    #[test]
    fn ids_and_payloads_construct() {
        assert_eq!(DocumentId(1), DocumentId(1));
        assert_ne!(RequestId(1), RequestId(2));
        let _cmd = Command::Save { doc: DocumentId(7) };
        let _cmd = Command::RetargetDocument {
            doc: DocumentId(7),
            path: PathBuf::from("new.txt"),
        };
        let _cmd = Command::BuildLatex { doc: DocumentId(7) };
        let _ev = Event::Saved { doc: DocumentId(7) };
        let _ev = Event::Retargeted {
            doc: DocumentId(7),
            path: PathBuf::from("new.txt"),
        };
        let _ev = Event::LatexBuildFinished {
            doc: DocumentId(7),
            root: PathBuf::from("main.tex"),
            pdf: Some(PathBuf::from("main.pdf")),
            diagnostics: Vec::new(),
            error: None,
        };
        let _cfg = Command::LoadedConfig;
        let server = LanguageServerId::Texlab;
        assert_eq!(server.key(), "texlab");
        assert_eq!(server.display_name(), "texlab");
        let plan = LanguageServerPlanId(9);
        let change = LanguageServerChange {
            server: server.clone(),
            current: Some("1.0.0".into()),
            target: "2.0.0".into(),
            download_bytes: Some(42),
        };
        let status = LanguageServerStatus {
            server: server.clone(),
            languages: vec!["tex".into()],
            enabled: true,
            managed: true,
            manual_install_reason: None,
            installed: Some("1.0.0".into()),
            cleanup_pending: false,
            instances: vec![LanguageServerInstanceStatus {
                root: PathBuf::from("/tmp"),
                source: LanguageServerSource::Managed,
                command: Some("texlab".into()),
                args: Vec::new(),
                runtime: LanguageServerRuntimeState::Running,
                open_documents: 1,
                error: None,
            }],
        };
        let _commands = [
            Command::InstallLanguageServer {
                server: server.clone(),
            },
            Command::ApplyLanguageServerPlan {
                plan,
                servers: vec![server.clone()],
            },
            Command::UninstallLanguageServer {
                server: server.clone(),
            },
            Command::RestartLanguageServer { server },
        ];
        let _events = [
            Event::LanguageServerUpdatePlan {
                plan,
                changes: vec![change],
            },
            Event::LanguageServerStatus {
                servers: vec![status],
            },
        ];
        assert_eq!(
            DocumentSettings::default(),
            DocumentSettings {
                insert_spaces: true,
                indent_size: 4,
                tab_width: 4,
                trim_trailing_whitespace: true,
                insert_final_newline: true,
                line_ending: None,
                encoding: None,
                spelling_language: None,
            }
        );
        assert_eq!(
            SpellingLanguage::parse("en-GB").map(SpellingLanguage::display_name),
            Some("English (UK)")
        );
        assert_eq!(
            SpellingLanguage::parse("en_US").map(SpellingLanguage::locale),
            Some("en_US")
        );
        assert!(SpellingLanguage::parse("fr_FR").is_none());
    }

    #[test]
    fn github_token_debug_never_exposes_the_secret() {
        let token = GithubToken::new("github_pat_super_secret".to_string());
        let debug = format!("{token:?}");
        assert_eq!(debug, "GithubToken(***)");
        assert!(!debug.contains("super_secret"));
    }

    #[test]
    fn pull_request_conversation_models_remain_serde_ready() -> Result<(), serde_json::Error> {
        let commit = GithubPullRequestCommit {
            sha: "bbbbbbbb".to_string(),
            summary: "Add feature".to_string(),
            author: "Octo Cat".to_string(),
            committed_unix: 2,
            parents: vec!["aaaaaaaa".to_string()],
            html_url: "https://github.com/o/r/commit/bbbbbbbb".to_string(),
        };
        let check = GithubCheckRun {
            id: 9,
            name: "CI".to_string(),
            status: "completed".to_string(),
            conclusion: Some("success".to_string()),
            html_url: "https://github.com/o/r/runs/9".to_string(),
        };
        let activity = GithubPullRequestActivity {
            id: Some(3),
            kind: "committed".to_string(),
            actor: Some("octocat".to_string()),
            commit_id: Some(commit.sha.clone()),
            before: None,
            after: None,
            created_unix: Some(2),
        };
        let commit_json = serde_json::to_string(&commit)?;
        let check_json = serde_json::to_string(&check)?;
        let activity_json = serde_json::to_string(&activity)?;
        assert_eq!(
            serde_json::from_str::<GithubPullRequestCommit>(&commit_json)?,
            commit
        );
        assert_eq!(serde_json::from_str::<GithubCheckRun>(&check_json)?, check);
        assert_eq!(
            serde_json::from_str::<GithubPullRequestActivity>(&activity_json)?,
            activity
        );
        let commands = [
            Command::GithubUpdatePullRequestBody {
                number: 12,
                body: "body".to_string(),
            },
            Command::GithubCommentPullRequest {
                number: 12,
                body: "comment".to_string(),
            },
            Command::GithubMergePullRequest {
                number: 12,
                head_sha: "bbbbbbbb".to_string(),
            },
            Command::GithubSetPullRequestDraft {
                node_id: "PR_node".to_string(),
                number: 12,
                draft: true,
            },
        ];
        assert_eq!(commands.len(), 4);
        Ok(())
    }
}

//! The in-process contract between the presentation layer and the backend: the
//! [`Command`]s a client submits and the [`Event`]s the backend emits.
//!
//! This module carries only neutral `karet-core` (plus a few engine) types, so it
//! is the designated extraction point for a future dependency-light
//! `karet-protocol` crate when the client-server split is undertaken.

use std::path::PathBuf;

use karet_core::BlameAttribution;
use karet_core::Change;
use karet_core::CodeAction;
use karet_core::CodeLens;
use karet_core::CompletionItem;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;
use karet_core::CursorState;
use karet_core::Diagnostic;
use karet_core::DocumentColor;
use karet_core::DocumentLink;
use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::Hover;
use karet_core::InlayHint;
use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::Severity;
use karet_core::SignatureHelp;
use karet_core::SignatureHelpOptions;
use karet_core::Symbol;
use karet_core::WorkspaceEdit;
use karet_core::WorkspaceSymbol;
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

mod aicommit;
mod command;
mod debug;
mod event;
mod github;
mod language_servers;
mod navigation;
mod refactor;
mod seam;
mod search;
mod server_window;
mod vcs;

pub use aicommit::*;
pub use command::*;
pub use debug::*;
pub use event::Event;
pub use github::*;
/// One occurrence of the symbol a [`LspCommand::DocumentHighlights`] asked
/// about, as [`Event::DocumentHighlights`] carries it: its range in buffer
/// columns. Re-exported from [`karet_core`], which defines it once.
pub use karet_core::DocumentHighlight;
/// How a [`DocumentHighlight`] uses its symbol; re-exported from
/// [`karet_core`].
pub use karet_core::HighlightKind;
pub use language_servers::*;
pub use navigation::*;
pub use refactor::*;
pub use seam::*;
pub use search::*;
pub use server_window::*;
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
/// ([`WorkspaceCommand::ScanTodos`]).
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
/// ([`WorkspaceCommand::ScanSpelling`]).
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

/// The spell-check dictionary settings layer a word is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DictionaryScope {
    /// The per-user settings layer.
    User,
    /// The workspace's `.karet/setting.jsonc` layer.
    Project,
}

/// What prompted a save, for the decisions that depend on it.
///
/// Only format-on-save reads this today, and it asks one question: does this
/// save record a decision to persist the file, or is it a snapshot taken while
/// the user is still editing it? Formatting rewrites the whole buffer, so on a
/// snapshot it moves text under a live caret mid-edit — the reformat is not
/// wrong, it is just badly timed, and it arrives while the user is typing.
///
/// Every cause here records a decision except [`SaveCause::AutoDelay`], which
/// fires a second after a typing pause with the caret still in the file. The
/// rest — a keystroke, a close prompt, focus leaving the document — all mean
/// the user is done with it for now, which is precisely when reformatting is
/// free.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SaveCause {
    /// The user asked, explicitly — a keybinding, a command, or a close prompt.
    #[default]
    Manual,
    /// Autosave, triggered by focus leaving the document. The user has moved on
    /// from it, so there is no caret in it left to disturb.
    FocusChange,
    /// Autosave, triggered by the inactivity timer while the document is still
    /// the one being edited. A pause in typing is not a decision to persist,
    /// and this is the one cause that does not format.
    AutoDelay,
    /// Forward-compatibility fallback: an unrecognized cause from a newer peer.
    /// Treated as [`SaveCause::Manual`], the conservative reading — a save that
    /// formats is recoverable, one that silently does not is surprising.
    #[serde(other)]
    Unknown,
}

impl SaveCause {
    /// Whether a save from this cause may run the formatter first.
    ///
    /// True for every cause that records a decision to persist the file, which
    /// is all of them but the inactivity timer.
    #[must_use]
    pub fn may_format(self) -> bool {
        !matches!(self, Self::AutoDelay)
    }
}

/// Which visualization a [`Event::GraphReady`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum GraphKind {
    /// The package-dependency graph of the workspace.
    Dependency,
    /// The usage/call graph of a symbol.
    ///
    /// Reserved; no producer yet. No [`Command`] requests it and the session never
    /// emits a [`Event::GraphReady`] carrying it.
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
mod tests;

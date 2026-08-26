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

mod command;
mod debug;
mod event;
mod github;
mod seam;
mod vcs;

pub use command::Command;
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

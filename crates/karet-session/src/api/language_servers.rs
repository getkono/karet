//! The language-server vocabulary: which provider, what state it is in, and how
//! far a refusal to install one reaches.
//!
//! Split out of [`super`] to keep that module under the workspace's per-file code
//! line ceiling; every type here is re-exported from it unchanged.

use std::borrow::Cow;
use std::path::PathBuf;

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
    /// The provider cannot start here, and karet has stopped trying.
    ///
    /// Distinct from [`Self::CircuitOpen`], which is a cooldown a provider
    /// comes back from. This is reached only for a failure no retry can fix --
    /// a binary that is absent or not executable, or a server that exits on
    /// sight and never once connected. Installing the provider, or restarting
    /// it from the Language Servers panel, clears it.
    ///
    /// There is deliberately no separate "stopped" state beside this one. A
    /// provider that stops and is not retried has no slot, and a provider with
    /// no slot is [`Self::Idle`] -- there is nowhere left to record anything
    /// else, which is the point of the slot being the only record. The one
    /// remaining meaning, "karet has given up on it", is this variant.
    Unavailable,
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
    /// Whether Karet has ever completed an install of this provider — true even
    /// after an uninstall, unlike [`installed`](Self::installed). Distinguishes a
    /// provider the user has already decided about from one never offered.
    #[serde(default)]
    pub ever_installed: bool,
    /// Whether the user declined this provider's install and has not been asked
    /// again since.
    #[serde(default)]
    pub declined: bool,
    /// Whether an unreferenced managed payload still awaits safe cleanup.
    pub cleanup_pending: bool,
    /// Repository-scoped resolution and runtime state.
    pub instances: Vec<LanguageServerInstanceStatus>,
}

impl LanguageServerInstanceStatus {
    /// Whether this session holds a process worth restarting.
    ///
    /// Meant to become the one definition. Presentation carries two
    /// byte-identical copies of this predicate today -- one deciding whether to
    /// paint the button, one deciding whether the click does anything -- and
    /// both read [`open_documents`](Self::open_documents), which no event ever
    /// corrected, so a retired provider kept offering a Restart for a process
    /// that no longer existed. The session-side half of that is fixed: the
    /// pushed inventory now corrects both fields together. Deleting the two
    /// copies in favour of this one is the client-side follow-up, which is why
    /// nothing outside tests calls this yet.
    #[must_use]
    pub fn restartable(&self) -> bool {
        self.open_documents > 0 || !matches!(self.runtime, LanguageServerRuntimeState::Idle)
    }
}

impl LanguageServerStatus {
    /// Whether any of this provider's instances is worth restarting.
    #[must_use]
    pub fn restartable(&self) -> bool {
        self.instances
            .iter()
            .any(LanguageServerInstanceStatus::restartable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(
        runtime: LanguageServerRuntimeState,
        open_documents: usize,
    ) -> LanguageServerInstanceStatus {
        LanguageServerInstanceStatus {
            root: std::path::PathBuf::from("/work/repo"),
            source: LanguageServerSource::Path,
            command: Some("rust-analyzer".to_owned()),
            args: Vec::new(),
            runtime,
            open_documents,
            error: None,
        }
    }

    #[test]
    fn a_provider_with_no_process_is_not_restartable() {
        assert!(!instance(LanguageServerRuntimeState::Idle, 0).restartable());
    }

    #[test]
    fn a_serving_provider_is_restartable() {
        assert!(instance(LanguageServerRuntimeState::Running, 1).restartable());
        assert!(instance(LanguageServerRuntimeState::Starting, 0).restartable());
    }

    /// A provider karet has given up on, or is cooling down, is still holding a
    /// slot: restarting it is how the user asks karet to try again.
    #[test]
    fn a_failed_provider_is_restartable() {
        assert!(instance(LanguageServerRuntimeState::Unavailable, 0).restartable());
        assert!(instance(LanguageServerRuntimeState::CircuitOpen, 0).restartable());
    }

    /// The stale-count case, which is the defect itself: state says idle, the
    /// document count was never corrected, and the row offered a dead Restart.
    #[test]
    fn a_stale_document_count_still_reads_as_restartable() {
        assert!(
            instance(LanguageServerRuntimeState::Idle, 2).restartable(),
            "the predicate trusts its input; keeping that input true is the \
             inventory's job, and is what the session-side change exists to do"
        );
    }
}

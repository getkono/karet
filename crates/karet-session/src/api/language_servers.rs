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

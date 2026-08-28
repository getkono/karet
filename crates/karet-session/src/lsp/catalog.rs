//! The built-in provider table: who serves a language, and how it is launched.
//!
//! This is the single source of truth for a provider's argv. It is consulted
//! both when the executable comes from the project or `PATH` and when it comes
//! from a karet-managed installation, so the two can no longer disagree. They
//! previously held independent copies of every provider's argv.
//!
//! There is deliberately **no catch-all**. Every row states its `args`, so a
//! provider that genuinely takes none records that as a decision (`args: &[]`)
//! rather than inheriting it from a fall-through arm.

use crate::api::LanguageServerId;

/// Whether a provider owns a language's intelligence or rides alongside it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// The language's default intelligence provider. Exactly one per language.
    Primary,
    /// A diagnostics or formatting companion, selected by repository markers
    /// rather than by being the language's default.
    Companion,
}

/// One built-in provider: its identity, its languages, and its launch.
pub(crate) struct BuiltinProvider {
    /// Stable provider id, matching [`LanguageServerId::key`].
    pub(crate) key: &'static str,
    /// The executable to run. Usually the key, but not always — Haskell's
    /// server is launched through its `-wrapper` binary, and Pyright's LSP
    /// entry point is a different binary from its CLI.
    pub(crate) command: &'static str,
    /// The arguments that put `command` into stdio LSP mode.
    pub(crate) args: &'static [&'static str],
    /// Language ids that resolve to this provider.
    pub(crate) languages: &'static [&'static str],
    /// Whether this is a language's default provider or a companion.
    pub(crate) role: Role,
}

/// Shorthand for a row, so the table reads as data rather than as struct syntax.
const fn provider(
    key: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    languages: &'static [&'static str],
    role: Role,
) -> BuiltinProvider {
    BuiltinProvider {
        key,
        command,
        args,
        languages,
        role,
    }
}

const BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    provider(
        "rust-analyzer",
        "rust-analyzer",
        &[],
        &["rust"],
        Role::Primary,
    ),
    provider(
        "typescript-language-server",
        "typescript-language-server",
        &["--stdio"],
        &["typescript", "javascript", "jsx", "tsx"],
        Role::Primary,
    ),
    provider(
        "pyright",
        "pyright-langserver",
        &["--stdio"],
        &["python"],
        Role::Primary,
    ),
    provider("texlab", "texlab", &[], &["tex"], Role::Primary),
    provider("clangd", "clangd", &[], &["c", "c++"], Role::Primary),
    provider(
        "csharp",
        "Microsoft.CodeAnalysis.LanguageServer",
        &["--stdio"],
        &["c#"],
        Role::Primary,
    ),
    provider("gopls", "gopls", &[], &["go"], Role::Primary),
    provider("jdtls", "jdtls", &[], &["java"], Role::Primary),
    provider("zls", "zls", &[], &["zig"], Role::Primary),
    provider(
        "astro-language-server",
        "astro-ls",
        &["--stdio"],
        &["astro"],
        Role::Primary,
    ),
    provider(
        "svelte-language-server",
        "svelteserver",
        &["--stdio"],
        &["svelte"],
        Role::Primary,
    ),
    provider(
        "vue-language-server",
        "vue-language-server",
        &["--stdio"],
        &["vue"],
        Role::Primary,
    ),
    provider(
        "yaml-language-server",
        "yaml-language-server",
        &["--stdio"],
        &["yaml"],
        Role::Primary,
    ),
    provider("lemminx", "lemminx", &[], &["xml", "svg"], Role::Primary),
    provider("ruby-lsp", "ruby-lsp", &[], &["ruby"], Role::Primary),
    provider("phpactor", "phpactor", &[], &["php"], Role::Primary),
    provider(
        "sourcekit-lsp",
        "sourcekit-lsp",
        &[],
        &["swift"],
        Role::Primary,
    ),
    provider("metals", "metals", &[], &["scala"], Role::Primary),
    provider(
        "lua-language-server",
        "lua-language-server",
        &[],
        &["lua"],
        Role::Primary,
    ),
    provider(
        "haskell-language-server",
        "haskell-language-server",
        &[],
        &["haskell"],
        Role::Primary,
    ),
    provider("ocamllsp", "ocamllsp", &[], &["ocaml"], Role::Primary),
    provider("elp", "elp", &[], &["erlang"], Role::Primary),
    provider(
        "dart-language-server",
        "dart",
        &["language-server"],
        &["dart"],
        Role::Primary,
    ),
    provider(
        "r-languageserver",
        "R",
        &["--no-echo", "-e", "languageserver::run()"],
        &["r"],
        Role::Primary,
    ),
    provider(
        "clojure-lsp",
        "clojure-lsp",
        &[],
        &["clojure"],
        Role::Primary,
    ),
    provider(
        "vscode-html-language-server",
        "vscode-html-language-server",
        &["--stdio"],
        &["html"],
        Role::Primary,
    ),
    provider(
        "vscode-css-language-server",
        "vscode-css-language-server",
        &["--stdio"],
        &["css", "sass", "less"],
        Role::Primary,
    ),
    provider(
        "vscode-json-language-server",
        "vscode-json-language-server",
        &["--stdio"],
        &["json"],
        Role::Primary,
    ),
    provider("taplo", "taplo", &[], &["toml"], Role::Primary),
    provider("pkl-lsp", "pkl-lsp", &[], &["pkl"], Role::Primary),
    provider("buf", "buf", &["beta", "lsp"], &["protobuf"], Role::Primary),
    provider(
        "graphql-lsp",
        "graphql-lsp",
        &["server", "-m", "stream"],
        &["graphql"],
        Role::Primary,
    ),
    provider(
        "bash-language-server",
        "bash-language-server",
        &["start"],
        &["shell", "bash"],
        Role::Primary,
    ),
    // PowerShell Editor Services has no standalone executable: it is a module
    // bundle entered through `Start-EditorServices.ps1`. The row keeps the
    // language routed and the manager honest; the reason text tells the user to
    // point `lsp.servers` at their own bundle.
    provider(
        "powershell-editor-services",
        "powershell-editor-services",
        &[],
        &["powershell"],
        Role::Primary,
    ),
    provider("marksman", "marksman", &[], &["markdown"], Role::Primary),
    provider(
        "esbonio",
        "esbonio",
        &[],
        &["restructuredtext"],
        Role::Primary,
    ),
    provider(
        "docker-langserver",
        "docker-langserver",
        &["--stdio"],
        &["dockerfile"],
        Role::Primary,
    ),
    provider("neocmakelsp", "neocmakelsp", &[], &["cmake"], Role::Primary),
    provider("ruff", "ruff", &["server"], &["python"], Role::Companion),
    provider(
        "biome",
        "biome",
        &["lsp-proxy"],
        &["javascript", "typescript"],
        Role::Companion,
    ),
    // Selected for a repository that configures Flake8 without Ruff.
    provider("pylsp", "pylsp", &[], &["python"], Role::Companion),
];

/// Every built-in provider, primaries and companions alike.
pub(crate) fn builtin_providers() -> &'static [BuiltinProvider] {
    BUILTIN_PROVIDERS
}

/// The row for `server`, if it is a built-in provider at all.
pub(crate) fn builtin_provider(server: &LanguageServerId) -> Option<&'static BuiltinProvider> {
    BUILTIN_PROVIDERS
        .iter()
        .find(|provider| provider.key == server.key())
}

/// The arguments a managed installation of `server` must be launched with.
///
/// Read by the registry so an installed provider and a `PATH` one are launched
/// identically.
pub(crate) fn managed_arguments(server: &str) -> &'static [&'static str] {
    builtin_provider(&LanguageServerId::new(server.to_owned()))
        .map_or(&[], |provider| provider.args)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;

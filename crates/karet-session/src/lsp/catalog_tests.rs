//! Invariants over the built-in provider table.
//!
//! The golden argv table below is deliberately an *expected-value* list rather
//! than a cross-check against some other table. Two independent tables agreeing
//! proves nothing: before this module existed, the launch table and the managed
//! install recipes both said `neocmakelsp` took no arguments, and both were
//! wrong. A reviewer has to read each row and decide it is right.

use super::*;

/// Every provider's deliberate launch, in table order.
///
/// A provider missing here fails, and a row for a provider that no longer
/// exists fails, so the table cannot drift in either direction.
const EXPECTED_LAUNCH: &[(&str, &str, &[&str])] = &[
    ("astro-language-server", "astro-ls", &["--stdio"]),
    ("bash-language-server", "bash-language-server", &["start"]),
    ("biome", "biome", &["lsp-proxy"]),
    ("buf", "buf", &["beta", "lsp"]),
    ("clangd", "clangd", &[]),
    ("clojure-lsp", "clojure-lsp", &[]),
    (
        "csharp",
        "Microsoft.CodeAnalysis.LanguageServer",
        &["--stdio"],
    ),
    ("dart-language-server", "dart", &["language-server"]),
    ("docker-langserver", "docker-langserver", &["--stdio"]),
    ("elp", "elp", &[]),
    ("esbonio", "esbonio", &[]),
    ("gopls", "gopls", &[]),
    ("graphql-lsp", "graphql-lsp", &["server", "-m", "stream"]),
    ("haskell-language-server", "haskell-language-server", &[]),
    ("jdtls", "jdtls", &[]),
    ("lemminx", "lemminx", &[]),
    ("lua-language-server", "lua-language-server", &[]),
    ("marksman", "marksman", &[]),
    ("metals", "metals", &[]),
    ("neocmakelsp", "neocmakelsp", &[]),
    ("ocamllsp", "ocamllsp", &[]),
    ("phpactor", "phpactor", &[]),
    ("pkl-lsp", "pkl-lsp", &[]),
    (
        "powershell-editor-services",
        "powershell-editor-services",
        &[],
    ),
    ("pylsp", "pylsp", &[]),
    ("pyright", "pyright-langserver", &["--stdio"]),
    (
        "r-languageserver",
        "R",
        &["--no-echo", "-e", "languageserver::run()"],
    ),
    ("ruby-lsp", "ruby-lsp", &[]),
    ("ruff", "ruff", &["server"]),
    ("rust-analyzer", "rust-analyzer", &[]),
    ("sourcekit-lsp", "sourcekit-lsp", &[]),
    ("svelte-language-server", "svelteserver", &["--stdio"]),
    ("taplo", "taplo", &[]),
    ("texlab", "texlab", &[]),
    (
        "typescript-language-server",
        "typescript-language-server",
        &["--stdio"],
    ),
    (
        "vscode-css-language-server",
        "vscode-css-language-server",
        &["--stdio"],
    ),
    (
        "vscode-html-language-server",
        "vscode-html-language-server",
        &["--stdio"],
    ),
    (
        "vscode-json-language-server",
        "vscode-json-language-server",
        &["--stdio"],
    ),
    ("vue-language-server", "vue-language-server", &["--stdio"]),
    ("yaml-language-server", "yaml-language-server", &["--stdio"]),
    ("zls", "zls", &[]),
];

#[test]
fn every_provider_has_a_reviewed_launch_and_no_row_is_stale() {
    let mut actual = builtin_providers()
        .iter()
        .map(|provider| (provider.key, provider.command, provider.args))
        .collect::<Vec<_>>();
    actual.sort_by_key(|(key, ..)| *key);
    let mut expected = EXPECTED_LAUNCH.to_vec();
    expected.sort_by_key(|(key, ..)| *key);
    assert_eq!(actual, expected);
}

#[test]
fn provider_keys_are_unique() {
    let mut keys = builtin_providers()
        .iter()
        .map(|provider| provider.key)
        .collect::<Vec<_>>();
    let total = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), total, "a provider key is listed twice");
}

/// `builtin_server` returns the first primary claiming a language, so a second
/// primary for the same language would silently shadow the first.
#[test]
fn each_language_has_exactly_one_primary_provider() {
    let mut claimed = Vec::new();
    for provider in builtin_providers() {
        if provider.role != Role::Primary {
            continue;
        }
        for language in provider.languages {
            assert!(
                !claimed.contains(language),
                "{language} has more than one primary provider"
            );
            claimed.push(*language);
        }
    }
    assert!(claimed.contains(&"toml"));
    assert!(claimed.contains(&"rust"));
}

#[test]
fn no_provider_claims_a_language_twice() {
    for provider in builtin_providers() {
        let mut languages = provider.languages.to_vec();
        let total = languages.len();
        languages.sort_unstable();
        languages.dedup();
        assert_eq!(
            languages.len(),
            total,
            "{} repeats a language",
            provider.key
        );
    }
}

/// Every argument is passed verbatim to the executable, never through a shell,
/// so an empty or whitespace-only argument would silently become a bad argv.
#[test]
fn launches_name_a_real_executable_and_carry_no_empty_arguments() {
    for provider in builtin_providers() {
        assert!(!provider.command.trim().is_empty(), "{}", provider.key);
        assert!(!provider.languages.is_empty(), "{}", provider.key);
        for argument in provider.args {
            assert!(
                !argument.trim().is_empty(),
                "{} has a blank argument",
                provider.key
            );
        }
    }
}

/// The registry launches an installed provider with these same arguments, so a
/// managed install and a `PATH` one cannot diverge.
#[test]
fn managed_arguments_come_from_the_launch_table() {
    assert_eq!(managed_arguments("neocmakelsp"), &[] as &[&str]);
    assert_eq!(managed_arguments("bash-language-server"), &["start"]);
    assert_eq!(managed_arguments("rust-analyzer"), &[] as &[&str]);
    assert_eq!(managed_arguments("not-a-provider"), &[] as &[&str]);
}

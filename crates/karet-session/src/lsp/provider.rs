use super::*;

/// One built-in provider and the language IDs that resolve to it.
pub(crate) struct ProviderDescriptor {
    pub(crate) server: LanguageServerId,
    pub(crate) languages: Vec<String>,
    pub(crate) managed: bool,
}

const BUILTIN_PROVIDERS: &[(&str, &str)] = &[
    ("rust", "rust-analyzer"),
    ("typescript", "typescript-language-server"),
    ("javascript", "typescript-language-server"),
    ("jsx", "typescript-language-server"),
    ("tsx", "typescript-language-server"),
    ("python", "pyright"),
    ("tex", "texlab"),
    ("c", "clangd"),
    ("c++", "clangd"),
    ("c#", "csharp"),
    ("go", "gopls"),
    ("java", "jdtls"),
    ("zig", "zls"),
    ("astro", "astro-language-server"),
    ("svelte", "svelte-language-server"),
    ("vue", "vue-language-server"),
    ("yaml", "yaml-language-server"),
    ("xml", "lemminx"),
    ("svg", "lemminx"),
    ("ruby", "ruby-lsp"),
    ("php", "phpactor"),
    ("swift", "sourcekit-lsp"),
    ("scala", "metals"),
    ("lua", "lua-language-server"),
    ("haskell", "haskell-language-server"),
    ("ocaml", "ocamllsp"),
    ("erlang", "elp"),
    ("dart", "dart-language-server"),
    ("r", "r-languageserver"),
    ("clojure", "clojure-lsp"),
    ("html", "vscode-html-language-server"),
    ("css", "vscode-css-language-server"),
    ("sass", "vscode-css-language-server"),
    ("less", "vscode-css-language-server"),
    ("json", "vscode-json-language-server"),
    ("toml", "taplo"),
    ("pkl", "pkl-lsp"),
    ("protobuf", "buf"),
    ("graphql", "graphql-lsp"),
    ("shell", "bash-language-server"),
    ("bash", "bash-language-server"),
    ("powershell", "powershell-editor-services"),
    ("markdown", "marksman"),
    ("restructuredtext", "esbonio"),
    ("dockerfile", "docker-langserver"),
    ("cmake", "neocmakelsp"),
];

pub(crate) fn managed_provider(server: &LanguageServerId) -> bool {
    matches!(
        server.key(),
        "rust-analyzer" | "typescript-language-server" | "pyright" | "ruff" | "texlab"
    )
}

pub(crate) fn builtin_catalog() -> Vec<ProviderDescriptor> {
    let mut providers = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (language, server) in BUILTIN_PROVIDERS {
        providers
            .entry((*server).to_owned())
            .or_default()
            .push((*language).to_owned());
    }
    // Ruff is a built-in Python diagnostics/formatting companion rather than
    // Python's primary intelligence provider, so it is not in the direct map.
    providers
        .entry("ruff".to_owned())
        .or_default()
        .push("python".to_owned());
    providers
        .entry("biome".to_owned())
        .or_default()
        .extend(["javascript".to_owned(), "typescript".to_owned()]);
    providers
        .into_iter()
        .map(|(server, languages)| {
            let server = LanguageServerId::new(server);
            ProviderDescriptor {
                managed: managed_provider(&server),
                server,
                languages,
            }
        })
        .collect()
}

/// The built-in default servers, used when `lsp.servers` has no entry for a
/// language. Keys are lowercase language names (the same keys user config uses).
pub(crate) fn builtin_server(language: &str) -> Option<LanguageServerId> {
    BUILTIN_PROVIDERS.iter().find_map(|(candidate, server)| {
        (*candidate == language).then(|| LanguageServerId::new(*server))
    })
}

pub(super) fn builtin_spec(provider: &LanguageServerId, language: &str) -> LspSpec {
    let (command, args): (&str, &[&str]) = match provider.key() {
        "typescript-language-server" => ("typescript-language-server", &["--stdio"]),
        "pyright" => ("pyright-langserver", &["--stdio"]),
        "ruff" => ("ruff", &["server"]),
        "csharp" => ("Microsoft.CodeAnalysis.LanguageServer", &["--stdio"]),
        "astro-language-server" => ("astro-ls", &["--stdio"]),
        "svelte-language-server" => ("svelteserver", &["--stdio"]),
        "vue-language-server" => ("vue-language-server", &["--stdio"]),
        "yaml-language-server" => ("yaml-language-server", &["--stdio"]),
        "vscode-html-language-server" => ("vscode-html-language-server", &["--stdio"]),
        "vscode-css-language-server" => ("vscode-css-language-server", &["--stdio"]),
        "vscode-json-language-server" => ("vscode-json-language-server", &["--stdio"]),
        "bash-language-server" => ("bash-language-server", &["start"]),
        "docker-langserver" => ("docker-langserver", &["--stdio"]),
        "biome" => ("biome", &["lsp-proxy"]),
        "buf" => ("buf", &["beta", "lsp"]),
        "dart-language-server" => ("dart", &["language-server"]),
        "r-languageserver" => ("R", &["--no-echo", "-e", "languageserver::run()"]),
        key => (key, &[]),
    };
    LspSpec {
        command: command.to_owned(),
        args: args.iter().map(|argument| (*argument).to_owned()).collect(),
        languages: vec![language.to_owned()],
    }
}

pub(super) fn executable_exists(command: &OsStr) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        let candidate = directory.join(path);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            ["exe", "cmd", "bat"]
                .iter()
                .any(|extension| candidate.with_extension(extension).is_file())
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

pub(super) fn project_local_spec(root: &Path, spec: &LspSpec) -> Option<LspSpec> {
    let command = Path::new(&spec.command);
    if command.components().count() > 1 {
        return command.is_file().then(|| spec.clone());
    }
    let candidates = [
        root.join("node_modules").join(".bin").join(command),
        root.join(".venv")
            .join(if cfg!(windows) { "Scripts" } else { "bin" })
            .join(command),
        root.join("venv")
            .join(if cfg!(windows) { "Scripts" } else { "bin" })
            .join(command),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(|command| {
            let mut resolved = spec.clone();
            resolved.command = command.to_string_lossy().into_owned();
            resolved
        })
}

pub(super) fn nearest_repository_root(path: &Path, fallback: Option<&Path>) -> PathBuf {
    let mut directory = path.parent().unwrap_or(path);
    loop {
        if directory.join(".git").exists() {
            return directory.to_path_buf();
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    fallback
        .map(Path::to_path_buf)
        .or_else(|| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn file_contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|contents| contents.contains(needle))
}

pub(super) fn python_diagnostic_provider(root: &Path) -> LanguageServerId {
    let ruff = root.join("ruff.toml").is_file()
        || root.join(".ruff.toml").is_file()
        || file_contains(&root.join("pyproject.toml"), "[tool.ruff");
    let flake8 = root.join(".flake8").is_file()
        || file_contains(&root.join("setup.cfg"), "[flake8]")
        || file_contains(&root.join("tox.ini"), "[flake8]");
    if flake8 && !ruff {
        LanguageServerId::new("pylsp")
    } else {
        LanguageServerId::Ruff
    }
}

pub(super) fn uses_biome(root: &Path) -> bool {
    ["biome.json", "biome.jsonc", "rome.json"]
        .iter()
        .any(|marker| root.join(marker).is_file())
}

/// The lookup/settings key for a document's display language (`"Rust"` →
/// `"rust"`), doubling as the LSP `languageId`.
pub(super) fn language_key(language: Option<&str>) -> Option<String> {
    language.map(str::to_ascii_lowercase)
}

/// Clamp a buffer version into LSP's `i32` version space (monotonic for any
/// realistic session; documents do not see 2³¹ edits).
pub(crate) fn version_i32(version: u64) -> i32 {
    i32::try_from(version % 2_147_483_647).unwrap_or(0)
}

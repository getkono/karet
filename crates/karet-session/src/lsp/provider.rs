use super::*;

/// The built-in default servers, used when `lsp.servers` has no entry for a
/// language. Keys are lowercase language names (the same keys user config uses).
pub(crate) fn builtin_server(language: &str) -> Option<LanguageServerId> {
    match language {
        "rust" => Some(LanguageServerId::RustAnalyzer),
        "typescript" | "javascript" | "jsx" | "tsx" => Some(LanguageServerId::TypeScript),
        "python" => Some(LanguageServerId::Pyright),
        "tex" => Some(LanguageServerId::Texlab),
        "c" | "c++" => Some(LanguageServerId::Clangd),
        "c#" => Some(LanguageServerId::CSharp),
        "go" => Some(LanguageServerId::Gopls),
        "java" => Some(LanguageServerId::Jdtls),
        "zig" => Some(LanguageServerId::Zls),
        "astro" => Some(LanguageServerId::Astro),
        "svelte" => Some(LanguageServerId::Svelte),
        "vue" => Some(LanguageServerId::Vue),
        "yaml" => Some(LanguageServerId::Yaml),
        "xml" | "svg" => Some(LanguageServerId::Xml),
        "ruby" => Some(LanguageServerId::new("ruby-lsp")),
        "php" => Some(LanguageServerId::new("phpactor")),
        "swift" => Some(LanguageServerId::new("sourcekit-lsp")),
        "scala" => Some(LanguageServerId::new("metals")),
        "lua" => Some(LanguageServerId::new("lua-language-server")),
        "haskell" => Some(LanguageServerId::new("haskell-language-server")),
        "ocaml" => Some(LanguageServerId::new("ocamllsp")),
        "erlang" => Some(LanguageServerId::new("elp")),
        "dart" => Some(LanguageServerId::new("dart-language-server")),
        "r" => Some(LanguageServerId::new("r-languageserver")),
        "clojure" => Some(LanguageServerId::new("clojure-lsp")),
        "html" => Some(LanguageServerId::new("vscode-html-language-server")),
        "css" | "sass" | "less" => Some(LanguageServerId::new("vscode-css-language-server")),
        "json" => Some(LanguageServerId::new("vscode-json-language-server")),
        "toml" => Some(LanguageServerId::new("taplo")),
        "pkl" => Some(LanguageServerId::new("pkl-lsp")),
        "protobuf" => Some(LanguageServerId::new("buf")),
        "graphql" => Some(LanguageServerId::new("graphql-lsp")),
        "shell" | "bash" => Some(LanguageServerId::new("bash-language-server")),
        "powershell" => Some(LanguageServerId::new("powershell-editor-services")),
        "markdown" => Some(LanguageServerId::new("marksman")),
        "restructuredtext" => Some(LanguageServerId::new("esbonio")),
        "dockerfile" => Some(LanguageServerId::new("docker-langserver")),
        "cmake" => Some(LanguageServerId::new("neocmakelsp")),
        _ => None,
    }
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

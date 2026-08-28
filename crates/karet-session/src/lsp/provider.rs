use super::*;

/// One built-in provider and the language IDs that resolve to it.
pub(crate) struct ProviderDescriptor {
    pub(crate) server: LanguageServerId,
    pub(crate) languages: Vec<String>,
    pub(crate) managed: bool,
    pub(crate) manual_install_reason: Option<String>,
}

pub(crate) fn managed_provider(server: &LanguageServerId) -> bool {
    crate::lsp_registry::managed_provider(server)
}

pub(crate) fn builtin_catalog() -> Vec<ProviderDescriptor> {
    let mut providers = catalog::builtin_providers()
        .iter()
        .map(|provider| {
            let server = LanguageServerId::new(provider.key);
            ProviderDescriptor {
                managed: managed_provider(&server),
                manual_install_reason: crate::lsp_registry::manual_install_reason(&server),
                server,
                languages: provider
                    .languages
                    .iter()
                    .map(|it| (*it).to_owned())
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.server.key().cmp(right.server.key()));
    providers
}

/// The built-in default server for a language, used when `lsp.servers` has no
/// entry for it. Keys are lowercase language names (the same keys user config
/// uses).
///
/// Only a [`Role::Primary`] provider answers here: `ruff`, `biome` and `pylsp`
/// also name a language, but they are companions attached per document by
/// repository markers, never a language's default.
pub(crate) fn builtin_server(language: &str) -> Option<LanguageServerId> {
    catalog::builtin_providers()
        .iter()
        .find(|provider| {
            provider.role == catalog::Role::Primary && provider.languages.contains(&language)
        })
        .map(|provider| LanguageServerId::new(provider.key))
}

/// How karet launches `provider` when the executable comes from the project or
/// `PATH`.
///
/// [`None`] for an id with no row in the launch table. Every built-in has one
/// (asserted by `catalog_tests::every_provider_has_a_reviewed_launch_and_no_row_is_stale`),
/// so a missing row means a user-configured id — and the launch table has no
/// opinion about those. Falling back to "the key, with no arguments" launched
/// a bare `mypy-ls` off `PATH` for a user whose config named
/// `/opt/tools/mypy-lsp --stdio`; the caller now consults `lsp.servers` for
/// those, or reports the id rather than guessing at it.
pub(super) fn builtin_spec(provider: &LanguageServerId, language: &str) -> Option<LspSpec> {
    let launch = catalog::builtin_provider(provider)?;
    Some(LspSpec::new(
        launch.command,
        launch
            .args
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        vec![language.to_owned()],
    ))
}

/// Whether `path` is a file this process could actually execute.
///
/// The executable bit is part of the question, not a detail: a file that is
/// present but not runnable was accepted as a resolved server and then failed
/// at exec, which is a far more confusing failure than simply looking further
/// along `PATH`. Metadata is followed through symlinks deliberately -- a
/// `node_modules/.bin` entry is one, and what matters is the target.
pub(super) fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `candidate` with `extension` **appended**, which is what probing `PATHEXT`
/// means.
///
/// [`Path::with_extension`] replaces everything after the last dot instead, and
/// several providers are named with dots: it turned the C# server's
/// `Microsoft.CodeAnalysis.LanguageServer` into `Microsoft.CodeAnalysis.EXE`,
/// so an installed Roslyn server on `PATH` was probed for under a name nothing
/// has ever been called and reported missing.
///
/// Compiled on every platform, and gated only at its call site, so the naming
/// rule stays unit-testable where the tests actually run.
#[cfg_attr(not(windows), allow(dead_code))] // only the Windows PATHEXT probe calls it
pub(super) fn with_appended_extension(candidate: &Path, extension: &str) -> PathBuf {
    let Some(name) = candidate.file_name() else {
        return candidate.to_path_buf();
    };
    let mut name = name.to_os_string();
    name.push(".");
    name.push(extension);
    candidate.with_file_name(name)
}

pub(super) fn executable_exists(command: &OsStr) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        let candidate = directory.join(path);
        if is_executable_file(&candidate) {
            return true;
        }
        #[cfg(windows)]
        {
            // Windows marks executability by extension, so PATHEXT is the
            // authority rather than the hardcoded three this used to try.
            let extensions =
                std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
            extensions.split(';').any(|extension| {
                let extension = extension.trim().trim_start_matches('.');
                !extension.is_empty()
                    && is_executable_file(&with_appended_extension(&candidate, extension))
            })
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
        return is_executable_file(command).then(|| spec.clone());
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
        .find(|candidate| is_executable_file(candidate))
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

/// The built-in provider id of the Astro language server.
pub(super) const ASTRO: &str = "astro-language-server";

/// Whether this launch is Astro's language server.
///
/// Astro refuses the handshake unless it is told where TypeScript lives, so the
/// caller has to recognise it whichever way karet resolved the executable: by
/// provider id, since a managed install runs `node` rather than `astro-ls`, and
/// by command name for anything that resolved to the `astro-ls` script itself.
/// A command the user configured never reaches here — the caller's preflight
/// diagnoses karet's own launches only.
pub(super) fn is_astro(provider: Option<&LanguageServerId>, spec: &LspSpec) -> bool {
    provider.is_some_and(|provider| provider.key() == ASTRO)
        || Path::new(&spec.command)
            .file_stem()
            .is_some_and(|stem| stem == "astro-ls")
}

/// The TypeScript SDK directory for a project-resolved Astro server.
///
/// The project's own TypeScript is the natural SDK for a project-local install
/// — an Astro repository with `@astrojs/language-server` in `node_modules` has
/// one right there — so the `node_modules` the executable came out of is tried
/// before the repository root's.
pub(super) fn project_typescript_sdk(command: &Path, root: &Path) -> Option<PathBuf> {
    command
        .ancestors()
        .filter(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "node_modules")
        })
        .map(Path::to_path_buf)
        .chain(std::iter::once(root.join("node_modules")))
        .map(|modules| modules.join("typescript").join("lib"))
        .find(|lib| lib.is_dir())
}

/// The `initializationOptions` that point a server at `tsdk` as its TypeScript
/// SDK, in the shape Astro (and every other `tsdk` consumer) reads.
pub(super) fn typescript_sdk_options(tsdk: &Path) -> serde_json::Value {
    serde_json::json!({ "typescript": { "tsdk": tsdk.to_string_lossy() } })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `with_extension` would have made this `Microsoft.CodeAnalysis.exe`.
    #[test]
    fn a_pathext_probe_appends_to_a_command_name_containing_dots() {
        let candidate = Path::new("/opt/roslyn/Microsoft.CodeAnalysis.LanguageServer");
        assert_eq!(
            with_appended_extension(candidate, "exe"),
            Path::new("/opt/roslyn/Microsoft.CodeAnalysis.LanguageServer.exe")
        );
    }

    #[test]
    fn a_pathext_probe_appends_to_an_ordinary_command_name() {
        assert_eq!(
            with_appended_extension(Path::new("/opt/bin/rust-analyzer"), "cmd"),
            Path::new("/opt/bin/rust-analyzer.cmd")
        );
    }

    #[test]
    fn a_pathext_probe_leaves_a_nameless_path_alone() {
        assert_eq!(
            with_appended_extension(Path::new("/"), "exe"),
            Path::new("/")
        );
    }

    #[test]
    fn the_typescript_sdk_next_to_the_executable_wins_over_the_repository_root()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let package = root.join("packages").join("site");
        std::fs::create_dir_all(root.join("node_modules").join("typescript").join("lib"))?;
        std::fs::create_dir_all(package.join("node_modules").join("typescript").join("lib"))?;
        let command = package.join("node_modules").join(".bin").join("astro-ls");
        assert_eq!(
            project_typescript_sdk(&command, root),
            Some(package.join("node_modules").join("typescript").join("lib"))
        );
        Ok(())
    }

    #[test]
    fn the_repository_typescript_serves_an_executable_resolved_off_path()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let lib = root.join("node_modules").join("typescript").join("lib");
        std::fs::create_dir_all(&lib)?;
        assert_eq!(
            project_typescript_sdk(Path::new("/usr/bin/astro-ls"), root),
            Some(lib)
        );
        assert_eq!(
            project_typescript_sdk(Path::new("/usr/bin/astro-ls"), Path::new("/nowhere")),
            None
        );
        Ok(())
    }

    #[test]
    fn an_unknown_provider_has_no_builtin_launch() {
        assert!(builtin_spec(&LanguageServerId::new("mypy-ls"), "python").is_none());
        let rust = builtin_spec(&LanguageServerId::RustAnalyzer, "rust");
        assert_eq!(
            rust.map(|spec| spec.command),
            Some("rust-analyzer".to_owned())
        );
    }
}

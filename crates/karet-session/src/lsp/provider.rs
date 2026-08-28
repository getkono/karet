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
/// A provider absent from the launch table falls back to its own key with no
/// arguments. That is the shape a user-configured id takes, and it is only ever
/// reached for one: every built-in has a row, asserted by
/// `catalog_tests::every_provider_has_a_reviewed_launch_and_no_row_is_stale`.
pub(super) fn builtin_spec(provider: &LanguageServerId, language: &str) -> LspSpec {
    let launch = catalog::builtin_provider(provider);
    LspSpec {
        command: launch.map_or_else(
            || provider.key().to_owned(),
            |launch| launch.command.to_owned(),
        ),
        args: launch
            .map(|launch| launch.args)
            .unwrap_or_default()
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        languages: vec![language.to_owned()],
        initialization_options: None,
    }
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
                !extension.is_empty() && is_executable_file(&candidate.with_extension(extension))
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

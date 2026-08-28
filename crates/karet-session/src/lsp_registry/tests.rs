use super::*;
use crate::api::DeclineScope;

#[test]
fn registry_tasks_can_run_concurrently() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use std::sync::Condvar;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let mut tasks = Vec::new();
    for name in ["one", "two"] {
        let gate = Arc::clone(&gate);
        let started_tx = started_tx.clone();
        let finished_tx = finished_tx.clone();
        tasks.push(spawn_registry_task(name.to_string(), move || {
            let _ = started_tx.send(name);
            let (lock, ready) = &*gate;
            if let Ok(released) = lock.lock() {
                drop(ready.wait_while(released, |released| !*released));
            }
            let _ = finished_tx.send(name);
        })?);
    }
    drop(started_tx);
    drop(finished_tx);

    let first = started_rx.recv_timeout(Duration::from_secs(1));
    let second = started_rx.recv_timeout(Duration::from_secs(1));
    let (lock, ready) = &*gate;
    if let Ok(mut released) = lock.lock() {
        *released = true;
        ready.notify_all();
    }

    assert!(first.is_ok());
    assert!(second.is_ok());
    assert!(finished_rx.recv_timeout(Duration::from_secs(1)).is_ok());
    assert!(finished_rx.recv_timeout(Duration::from_secs(1)).is_ok());
    for task in tasks {
        assert!(task.join().is_ok());
    }
    Ok(())
}

#[test]
fn activation_journal_ignores_a_torn_tail() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let provider = provider_root(dir.path(), &LanguageServerId::Texlab);
    std::fs::create_dir_all(&provider)?;
    let command = provider.join("texlab");
    std::fs::write(&command, b"test")?;
    let active = ActiveInstallation {
        version: "1.2.3".into(),
        command,
        args: Vec::new(),
    };
    let encoded = serde_json::to_string(&active)?;
    std::fs::write(provider.join("active.jsonl"), format!("{encoded}\n{{"))?;
    let resolved = read_active(dir.path(), &LanguageServerId::Texlab);
    assert_eq!(resolved.map(|item| item.version), Some("1.2.3".into()));
    Ok(())
}

#[test]
fn unsafe_versions_cannot_escape_the_provider_directory() {
    assert_eq!(safe_version("../../bad release"), ".._.._bad_release");
}

#[test]
fn named_file_discovery_ignores_matching_directories() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("node"))?;
    assert_eq!(find_file_named(dir.path(), "node"), None);

    let runtime = dir.path().join("runtime");
    std::fs::create_dir(&runtime)?;
    let executable = runtime.join("node");
    std::fs::write(&executable, b"binary")?;
    assert_eq!(find_file_named(dir.path(), "node"), Some(executable));
    Ok(())
}

#[test]
fn node_provider_identity_covers_every_managed_runtime() {
    let release = Release {
        server: LanguageServerId::TypeScript,
        version: "5.3.0".into(),
        from_version: None,
        kind: ReleaseKind::Npm {
            package: "typescript-language-server".into(),
            companion: Some(("typescript".into(), "5.9.3".into())),
            entrypoint: "lib/cli.mjs".into(),
            arguments: &["--stdio"],
            node_version: "v24.4.0".into(),
            node_url: String::new(),
            node_sha256: String::new(),
            node_archive: Archive::TarGzip,
        },
        download_bytes: None,
    };
    assert_eq!(
        release.active_version(),
        "5.3.0+typescript-5.9.3+node-24.4.0"
    );
}

#[test]
fn approved_first_install_activates_without_returning_another_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    let version = "1.2.3";
    let payload = provider_root(dir.path(), &server)
        .join("versions")
        .join(version);
    std::fs::create_dir_all(&payload)?;
    std::fs::write(payload.join(executable("texlab")), b"test")?;
    let release = Release {
        server: server.clone(),
        version: version.into(),
        from_version: None,
        kind: ReleaseKind::Standalone {
            url: String::new(),
            sha256: String::new(),
            archive: Archive::Raw,
            executable_name: executable("texlab"),
            retain_archive: false,
            arguments: &[],
        },
        download_bytes: None,
    };
    let (updates, _updates_rx) = tokio_mpsc::unbounded_channel();

    let update = install_discovered(
        dir.path(),
        None,
        &Client::new(),
        RequestId(7),
        &release,
        &updates,
    )?;

    assert!(matches!(
        update,
        RegistryUpdate::Changed {
            request: RequestId(7),
            server: changed,
            version: installed,
            was_installed: false,
        } if changed == server && installed == version
    ));
    assert_eq!(
        installed_version(Some(dir.path()), &server).as_deref(),
        Some(version)
    );
    Ok(())
}

#[test]
fn builtin_install_recipes_are_complete_for_supported_targets() {
    use super::catalog::ManagedSource;

    let mut expected = vec![
        "astro-language-server",
        "bash-language-server",
        "biome",
        "buf",
        "clangd",
        "clojure-lsp",
        "docker-langserver",
        "graphql-lsp",
        "lua-language-server",
        "marksman",
        "neocmakelsp",
        "pyright",
        "ruff",
        "rust-analyzer",
        "svelte-language-server",
        "texlab",
        "typescript-language-server",
        "vscode-css-language-server",
        "vscode-html-language-server",
        "vscode-json-language-server",
        "vue-language-server",
        "yaml-language-server",
        "zls",
    ];
    if std::env::consts::ARCH != "x86_64" {
        expected.retain(|server| *server != "clangd");
    }
    let mut actual = managed_servers()
        .into_iter()
        .map(|server| server.key().to_owned())
        .collect::<Vec<_>>();
    actual.sort();
    assert_eq!(actual, expected);
    assert!(actual.len() > 20);

    let mut manual = vec![
        "csharp",
        "dart-language-server",
        "elp",
        "esbonio",
        "gopls",
        "haskell-language-server",
        "jdtls",
        "lemminx",
        "metals",
        "ocamllsp",
        "phpactor",
        "pkl-lsp",
        "powershell-editor-services",
        "r-languageserver",
        "ruby-lsp",
        "sourcekit-lsp",
        "taplo",
    ];
    if std::env::consts::ARCH != "x86_64" {
        manual.push("clangd");
        manual.sort();
    }
    for server in &manual {
        assert!(
            manual_install_reason(&LanguageServerId::new(*server))
                .is_some_and(|reason| !reason.trim().is_empty()),
            "{server} has no manual-install reason"
        );
    }
    assert_eq!(actual.len() + manual.len(), 40);
    assert!(manual_install_reason(&LanguageServerId::new("company-lsp")).is_none());

    let targets = [
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("macos", "x86_64"),
        ("macos", "aarch64"),
        ("windows", "x86_64"),
    ];
    for recipe in catalog::managed_recipes() {
        assert!(!recipe.server.is_empty());
        match recipe.source {
            ManagedSource::Npm {
                package, binary, ..
            } => {
                assert!(!package.is_empty());
                assert!(!binary.is_empty());
            },
            ManagedSource::Github { repository } => {
                assert!(!repository.is_empty());
                for (os, arch) in targets {
                    let available = catalog::github_asset_for(
                        &LanguageServerId::new(recipe.server),
                        "1.2.3",
                        os,
                        arch,
                    )
                    .is_ok();
                    assert!(
                        available || (recipe.server == "clangd" && arch == "aarch64"),
                        "{} has no recipe for {os}-{arch}",
                        recipe.server
                    );
                }
            },
        }
    }
}

#[test]
fn uninstall_deactivates_resolution_and_reclaims_unused_payload()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    let provider = provider_root(dir.path(), &server);
    let payload = provider.join("versions/1.2.3");
    std::fs::create_dir_all(&payload)?;
    let command = payload.join(executable("texlab"));
    std::fs::write(&command, b"test")?;
    append_json_line(
        &provider.join("active.jsonl"),
        &ActiveInstallation {
            version: "1.2.3".into(),
            command,
            args: Vec::new(),
        },
    )?;

    assert!(!uninstall(dir.path(), &server)?);
    assert!(read_active(dir.path(), &server).is_none());
    assert!(!payload.exists());
    assert!(!cleanup_pending(Some(dir.path()), &server));
    Ok(())
}

#[test]
fn uninstall_defers_payload_cleanup_while_a_broker_is_live()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    let provider = provider_root(dir.path(), &server);
    let payload = provider.join("versions/1.2.3");
    std::fs::create_dir_all(&payload)?;
    let command = payload.join(executable("texlab"));
    std::fs::write(&command, b"test")?;
    append_json_line(
        &provider.join("active.jsonl"),
        &ActiveInstallation {
            version: "1.2.3".into(),
            command: command.clone(),
            args: Vec::new(),
        },
    )?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let brokers = dir.path().join("brokers");
    std::fs::create_dir_all(&brokers)?;
    let endpoint = serde_json::json!({
        "address": listener.local_addr()?,
        "token": "test",
        "pid": std::process::id(),
        "command": command,
    });
    std::fs::write(brokers.join("live.json"), serde_json::to_vec(&endpoint)?)?;

    assert!(uninstall(dir.path(), &server)?);
    assert!(read_active(dir.path(), &server).is_none());
    assert!(payload.is_dir());
    assert!(cleanup_pending(Some(dir.path()), &server));
    Ok(())
}

#[test]
fn uninstall_rejects_external_providers() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let result = uninstall(dir.path(), &LanguageServerId::Gopls);
    assert!(matches!(result, Err(message) if message.contains("not managed")));
    Ok(())
}

/// Write an activation record for `server` under `root`, as a completed install
/// would. The command file must exist for `read_active` to resolve it.
fn activate(root: &Path, server: &LanguageServerId, version: &str) -> std::io::Result<()> {
    let provider = provider_root(root, server);
    std::fs::create_dir_all(&provider)?;
    let command = provider.join("bin");
    std::fs::write(&command, b"test")?;
    let active = ActiveInstallation {
        version: version.into(),
        command,
        args: Vec::new(),
    };
    let Ok(encoded) = serde_json::to_string(&active) else {
        return Ok(());
    };
    let mut journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(provider.join("active.jsonl"))?;
    writeln!(journal, "{encoded}")
}

#[test]
fn a_provider_never_touched_was_never_installed() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    assert!(!ever_installed(Some(dir.path()), &LanguageServerId::Texlab));
    assert!(!ever_installed(None, &LanguageServerId::Texlab));
}

#[test]
fn an_uninstalled_provider_still_counts_as_ever_installed() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    activate(dir.path(), &server, "1.2.3")?;
    assert!(ever_installed(Some(dir.path()), &server));

    // Deactivating is what `read_active` replays away — and exactly the case that
    // must not read as "never offered", or the prompt returns after an uninstall.
    let provider = provider_root(dir.path(), &server);
    let deactivation = serde_json::json!({ "deactivated": true, "version": "1.2.3" });
    let mut journal = std::fs::OpenOptions::new()
        .append(true)
        .open(provider.join("active.jsonl"))?;
    writeln!(journal, "{deactivation}")?;

    assert!(
        read_active(dir.path(), &server).is_none(),
        "no longer active"
    );
    assert!(
        ever_installed(Some(dir.path()), &server),
        "but the question has been answered once already"
    );
    Ok(())
}

#[test]
fn a_declined_install_round_trips_and_can_be_cleared() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    assert!(read_declined(Some(dir.path()), &server).is_none());

    let declined = Declined::now(DeclineScope::Forever, Some("1.2.3".into()));
    write_declined(dir.path(), &server, &declined)?;
    let read = read_declined(Some(dir.path()), &server).ok_or("declined record")?;
    assert_eq!(read.scope, DeclineScope::Forever);
    assert_eq!(read.version_offered.as_deref(), Some("1.2.3"));

    clear_declined(dir.path(), &server)?;
    assert!(read_declined(Some(dir.path()), &server).is_none());
    // Clearing what is already cleared is the state the caller asked for.
    clear_declined(dir.path(), &server)?;
    Ok(())
}

#[test]
fn a_permanent_refusal_suppresses_every_version() {
    let declined = Declined::now(DeclineScope::Forever, Some("1.2.3".into()));
    assert!(declined.suppresses(None));
    assert!(declined.suppresses(Some("1.2.3")));
    assert!(declined.suppresses(Some("9.9.9")));
}

#[test]
fn a_version_refusal_is_spent_once_a_different_version_is_offered() {
    let declined = Declined::now(DeclineScope::Version, Some("1.2.3".into()));
    assert!(
        declined.suppresses(Some("1.2.3")),
        "the same offer is refused"
    );
    assert!(
        !declined.suppresses(Some("9.9.9")),
        "a new version is a new question"
    );
    // With nothing resolved there is no offer to compare, so the refusal stands
    // rather than being re-asked on every launch.
    assert!(declined.suppresses(None));
}

#[test]
fn a_version_refusal_with_no_recorded_version_suppresses() {
    let declined = Declined::now(DeclineScope::Version, None);
    assert!(declined.suppresses(Some("1.2.3")));
    assert!(declined.suppresses(None));
}

#[test]
fn a_corrupt_declined_file_reads_as_no_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let server = LanguageServerId::Texlab;
    let provider = provider_root(dir.path(), &server);
    std::fs::create_dir_all(&provider)?;
    std::fs::write(provider.join("declined.json"), b"{ not json")?;
    // Failing open re-asks once; failing closed would silently disable a provider
    // the user never refused, with no way to discover why.
    assert!(read_declined(Some(dir.path()), &server).is_none());
    Ok(())
}

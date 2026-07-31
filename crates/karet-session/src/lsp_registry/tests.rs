use super::*;

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

//! Unit tests for provider selection and inventory construction.

use super::*;

#[test]
fn builtin_registry_covers_the_documented_languages() {
    assert_eq!(builtin_server("rust"), Some(LanguageServerId::RustAnalyzer));
    for lang in ["typescript", "javascript"] {
        assert_eq!(builtin_server(lang), Some(LanguageServerId::TypeScript));
    }
    assert_eq!(builtin_server("python"), Some(LanguageServerId::Pyright));
    assert_eq!(builtin_server("tex"), Some(LanguageServerId::Texlab));
    assert!(builtin_server("cobol").is_none());
}

#[test]
fn inventory_covers_builtins_and_configured_providers() -> TestResult {
    let root = tempfile::tempdir()?;
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "company-rust".to_owned(),
        crate::config::schema::LspServer {
            command: root
                .path()
                .join("company-ls")
                .to_string_lossy()
                .into_owned(),
            args: vec!["--stdio".to_owned()],
            ..crate::config::schema::LspServer::default()
        },
    );
    settings.languages.insert(
        "rust".to_owned(),
        crate::config::schema::LspLanguage {
            servers: vec!["company-rust".to_owned()],
            ..crate::config::schema::LspLanguage::default()
        },
    );
    let (manager, _rx) = LspManager::new(settings, Some(root.path().to_path_buf()), None, None);
    let statuses = manager.inventory(Vec::<PathBuf>::new());
    assert!(
        statuses
            .iter()
            .any(|status| status.server == LanguageServerId::RustAnalyzer)
    );
    let custom = statuses
        .iter()
        .find(|status| status.server.key() == "company-rust")
        .ok_or("custom provider missing")?;
    assert_eq!(custom.languages, vec!["rust".to_owned()]);
    assert!(!custom.managed);
    assert_eq!(
        custom.instances.first().map(|instance| instance.source),
        Some(LanguageServerSource::Configured)
    );
    let mut expected_managed = vec![
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
        expected_managed.retain(|server| *server != "clangd");
    }
    assert!(
        statuses
            .iter()
            .filter(|status| status.managed)
            .map(|status| status.server.key())
            .eq(expected_managed)
    );
    Ok(())
}

#[test]
fn language_server_inventory_payload_is_serde_ready() -> TestResult {
    let status = LanguageServerStatus {
        ever_installed: false,
        declined: false,
        server: LanguageServerId::Texlab,
        languages: vec!["tex".to_owned()],
        enabled: true,
        managed: true,
        manual_install_reason: None,
        installed: Some("5.0.0".to_owned()),
        cleanup_pending: false,
        instances: vec![LanguageServerInstanceStatus {
            root: PathBuf::from("/repo"),
            source: LanguageServerSource::Managed,
            command: Some("/managed/texlab".to_owned()),
            args: Vec::new(),
            runtime: LanguageServerRuntimeState::Running,
            open_documents: 1,
            error: None,
        }],
    };
    let encoded = serde_json::to_string(&status)?;
    let decoded: LanguageServerStatus = serde_json::from_str(&encoded)?;
    assert_eq!(decoded, status);
    Ok(())
}

#[test]
fn user_config_overrides_builtins() {
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "rust".to_owned(),
        crate::config::schema::LspServer {
            command: "my-ra".to_owned(),
            args: vec!["--custom".to_owned()],
            ..crate::config::schema::LspServer::default()
        },
    );
    settings.servers.insert(
        "zig".to_owned(),
        crate::config::schema::LspServer {
            command: "zls".to_owned(),
            args: Vec::new(),
            ..crate::config::schema::LspServer::default()
        },
    );
    let (manager, _rx) = LspManager::new(settings, None, None, None);
    let rust = manager.spec_for("rust", Path::new("/tmp"));
    assert_eq!(
        rust.map(|(s, _)| (s.command, s.args)),
        Some(("my-ra".to_owned(), vec!["--custom".to_owned()]))
    );
    assert_eq!(
        manager
            .spec_for("zig", Path::new("/tmp"))
            .map(|(s, _)| s.command),
        Some("zls".to_owned())
    );
    assert_eq!(
        manager
            .spec_for("python", Path::new("/tmp"))
            .map(|(s, _)| s.command),
        Some("pyright-langserver".to_owned())
    );
}

#[test]
fn language_keys_lowercase_display_names() {
    assert_eq!(language_key(Some("Rust")), Some("rust".to_owned()));
    assert_eq!(
        language_key(Some("TypeScript")),
        Some("typescript".to_owned())
    );
    assert_eq!(language_key(None), None);
}

#[test]
fn versions_clamp_into_i32() {
    assert_eq!(version_i32(0), 0);
    assert_eq!(version_i32(41), 41);
    assert!(version_i32(u64::MAX) >= 0);
}

#[test]
fn repository_markers_select_non_overlapping_companions() -> TestResult {
    let ruff = tempfile::tempdir()?;
    std::fs::write(ruff.path().join("pyproject.toml"), "[tool.ruff]\n")?;
    assert_eq!(
        python_diagnostic_provider(ruff.path()),
        LanguageServerId::Ruff
    );

    let flake8 = tempfile::tempdir()?;
    std::fs::write(flake8.path().join(".flake8"), "[flake8]\n")?;
    assert_eq!(python_diagnostic_provider(flake8.path()).key(), "pylsp");

    let biome = tempfile::tempdir()?;
    std::fs::write(biome.path().join("biome.json"), "{}")?;
    assert!(uses_biome(biome.path()));
    Ok(())
}

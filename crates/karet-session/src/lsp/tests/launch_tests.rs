//! What karet actually launches once a provider has been chosen: the argv a
//! user configured, and the `initializationOptions` a server cannot start
//! without.
//!
//! These observe the spec handed to the connector rather than a running server,
//! because the defects they cover were entirely in the spec: a companion
//! launched as a bare `<id>` off `PATH`, and an Astro server launched with no
//! TypeScript SDK.

use serde_json::json;

use super::*;
use crate::config::schema::LspLanguage;
use crate::config::schema::LspServer;

/// How long an unexpected launch is given to show up before "nothing else
/// started" is asserted.
const SETTLE: Duration = Duration::from_millis(200);

/// A connector that records the launch it was handed and then fails as a
/// missing binary. That failure is permanent, so each server task calls it
/// exactly once and the recording stays a launch-per-server.
fn recording_connector(launches: mpsc::UnboundedSender<LspSpec>) -> Connector {
    Arc::new(move |spec: LspSpec, _root| {
        let _ = launches.send(spec.clone());
        let failure = karet_lsp::LaunchFailure::new(
            spec.command.clone(),
            spec.args.clone(),
            karet_lsp::LaunchCause::NotFound,
        );
        Box::pin(async move { Err(LspError::Launch(Box::new(failure))) })
    })
}

/// Every launch attempted: `expected` of them awaited, then a settling window
/// so a launch that should not have happened is still seen if it did.
async fn observed_launches(
    rx: &mut mpsc::UnboundedReceiver<LspSpec>,
    expected: usize,
) -> Vec<LspSpec> {
    let mut seen = Vec::new();
    while seen.len() < expected {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(spec)) => seen.push(spec),
            _ => break,
        }
    }
    while let Ok(Some(spec)) = tokio::time::timeout(SETTLE, rx.recv()).await {
        seen.push(spec);
    }
    seen
}

/// A Python project whose `diagnostics` name `mypy-ls`, with whatever
/// `lsp.servers` entry the test wants for it.
fn python_with_companion(server: Option<LspServer>) -> LspSettings {
    let mut settings = LspSettings::default();
    if let Some(server) = server {
        settings.servers.insert("mypy-ls".to_owned(), server);
    }
    settings.languages.insert(
        "python".to_owned(),
        LspLanguage {
            diagnostics: vec!["mypy-ls".to_owned()],
            ..LspLanguage::default()
        },
    );
    settings
}

fn manager_with_recorder(
    settings: LspSettings,
    root: &Path,
) -> (
    LspManager,
    mpsc::UnboundedReceiver<LspUpdate>,
    mpsc::UnboundedReceiver<LspSpec>,
) {
    let (launches, launched) = mpsc::unbounded_channel();
    let (mut manager, updates) = LspManager::new(settings, Some(root.to_path_buf()), None, None);
    manager.set_connector(recording_connector(launches));
    (manager, updates, launched)
}

fn write_executable(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

// --- user-configured diagnostics companions ------------------------------

/// The companion path never consulted `lsp.servers`, so this configuration
/// launched a bare `mypy-ls` off `PATH` with no arguments.
#[tokio::test]
async fn a_configured_companion_launches_the_command_it_was_configured_with() -> TestResult {
    let dir = tempfile::tempdir()?;
    let command = dir.path().join("mypy-lsp").to_string_lossy().into_owned();
    let settings = python_with_companion(Some(LspServer {
        command: command.clone(),
        args: vec!["--stdio".to_owned()],
        ..LspServer::default()
    }));
    let (mut manager, _updates, mut launched) = manager_with_recorder(settings, dir.path());

    manager.document_opened(
        Some("python"),
        Some("python"),
        &dir.path().join("app.py"),
        1,
        || "x = 1\n".into(),
    );

    let launches = observed_launches(&mut launched, 2).await;
    let companion = launches
        .iter()
        .find(|spec| spec.command == command)
        .ok_or("the configured companion was never launched")?;
    assert_eq!(companion.args, vec!["--stdio".to_owned()]);
    assert!(
        !launches.iter().any(|spec| spec.command == "mypy-ls"),
        "a bare `mypy-ls` was launched off PATH: {launches:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_companion_the_user_disabled_is_not_started() -> TestResult {
    let dir = tempfile::tempdir()?;
    let command = dir.path().join("mypy-lsp").to_string_lossy().into_owned();
    let settings = python_with_companion(Some(LspServer {
        enabled: false,
        command: command.clone(),
        args: vec!["--stdio".to_owned()],
    }));
    let (mut manager, _updates, mut launched) = manager_with_recorder(settings, dir.path());

    manager.document_opened(
        Some("python"),
        Some("python"),
        &dir.path().join("app.py"),
        1,
        || "x = 1\n".into(),
    );

    let launches = observed_launches(&mut launched, 1).await;
    assert!(
        !launches
            .iter()
            .any(|spec| spec.command == command || spec.command == "mypy-ls"),
        "`enabled = false` still started the companion: {launches:?}"
    );
    Ok(())
}

/// An id karet has never heard of and the user never configured has no launch
/// at all — guessing at `<id>` with no arguments was how the bare launch got in.
#[tokio::test]
async fn an_unknown_companion_id_is_explained_rather_than_launched() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut settings = LspSettings::default();
    settings.languages.insert(
        "python".to_owned(),
        LspLanguage {
            diagnostics: vec!["totally-unknown-ls".to_owned()],
            ..LspLanguage::default()
        },
    );
    let (mut manager, mut updates, mut launched) = manager_with_recorder(settings, dir.path());

    manager.document_opened(
        Some("python"),
        Some("python"),
        &dir.path().join("app.py"),
        1,
        || "x = 1\n".into(),
    );

    let launches = observed_launches(&mut launched, 1).await;
    assert!(
        !launches
            .iter()
            .any(|spec| spec.command.contains("totally-unknown-ls")),
        "an unknown companion id was launched anyway: {launches:?}"
    );
    let mut explained = false;
    while let Ok(update) = updates.try_recv() {
        if let LspUpdate::ManualInstallRequired { server, .. } = update {
            explained |= server == LanguageServerId::new("totally-unknown-ls");
        }
    }
    assert!(explained, "the unknown companion id was never reported");
    Ok(())
}

/// `lsp.enabled = false` covers a language that attaches companions, not just
/// one (like Rust) that has none.
#[tokio::test]
async fn disabling_lsp_stops_a_language_with_companions_too() -> TestResult {
    let dir = tempfile::tempdir()?;
    // A Ruff marker, so this language would otherwise attach a companion
    // without any `lsp.languages` entry naming one.
    std::fs::write(dir.path().join("ruff.toml"), "line-length = 100\n")?;
    let settings = LspSettings {
        enabled: false,
        ..LspSettings::default()
    };
    let (mut manager, _updates, mut launched) = manager_with_recorder(settings, dir.path());

    manager.document_opened(
        Some("python"),
        Some("python"),
        &dir.path().join("app.py"),
        1,
        || "x = 1\n".into(),
    );

    let launches = observed_launches(&mut launched, 0).await;
    assert!(
        launches.is_empty(),
        "disabled means no launches: {launches:?}"
    );
    Ok(())
}

// --- Astro's TypeScript SDK ----------------------------------------------

/// The ordinary way an Astro project installs its server: `node_modules`. That
/// copy wins resolution, and it used to be launched with no `typescript.tsdk`,
/// which Astro answers by refusing the handshake.
#[tokio::test]
async fn a_project_local_astro_is_given_the_project_typescript_sdk() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::create_dir_all(root.join("src"))?;
    let bin = root.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin)?;
    write_executable(&bin.join("astro-ls"), "#!/bin/sh\nexit 0\n")?;
    let tsdk = root.join("node_modules").join("typescript").join("lib");
    std::fs::create_dir_all(&tsdk)?;
    let (mut manager, _updates, mut launched) = manager_with_recorder(LspSettings::default(), root);

    manager.document_opened(
        Some("astro"),
        Some("astro"),
        &root.join("src").join("page.astro"),
        1,
        || "---\n---\n".into(),
    );

    let launches = observed_launches(&mut launched, 1).await;
    let spec = launches.first().ok_or("astro-ls was never launched")?;
    assert_eq!(spec.command, bin.join("astro-ls").to_string_lossy());
    assert_eq!(
        spec.initialization_options,
        Some(json!({ "typescript": { "tsdk": tsdk.to_string_lossy() } })),
        "a project-local Astro was launched without a TypeScript SDK"
    );
    Ok(())
}

/// Astro would refuse the handshake, and a refused handshake says nothing about
/// why, so the missing SDK is reported instead of launched into.
#[tokio::test]
async fn astro_without_any_typescript_sdk_is_reported_rather_than_launched() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::create_dir_all(root.join("src"))?;
    let bin = root.join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin)?;
    write_executable(&bin.join("astro-ls"), "#!/bin/sh\nexit 0\n")?;
    let (mut manager, mut updates, mut launched) =
        manager_with_recorder(LspSettings::default(), root);

    manager.document_opened(
        Some("astro"),
        Some("astro"),
        &root.join("src").join("page.astro"),
        1,
        || "---\n---\n".into(),
    );

    let launches = observed_launches(&mut launched, 0).await;
    assert!(
        launches.is_empty(),
        "Astro was launched without a TypeScript SDK: {launches:?}"
    );
    let mut reported = None;
    while let Ok(update) = updates.try_recv() {
        if let LspUpdate::PreflightFailed { message, .. } = update {
            reported = Some(message);
        }
    }
    let message = reported.ok_or("no preflight diagnosis was reported")?;
    assert!(message.contains("TypeScript"), "{message}");
    Ok(())
}

/// A managed installation records the SDK inside its own version directory, and
/// nothing may overwrite it with a guess.
#[test]
fn a_managed_astro_keeps_the_sdk_its_install_recorded() -> TestResult {
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    let options = json!({ "typescript": { "tsdk": "/managed/astro/typescript/lib" } });
    let mut spec = LspSpec::new(
        "node",
        vec!["/managed/astro/server.js".to_owned(), "--stdio".to_owned()],
        vec!["astro".to_owned()],
    )
    .with_initialization_options(Some(options.clone()));
    let astro = LanguageServerId::new("astro-language-server");

    assert!(manager.astro_launch_gate(&mut spec, Some(&astro), "astro", Path::new("/nowhere")));
    assert_eq!(spec.initialization_options, Some(options));
    Ok(())
}

/// A command the user named is theirs, not karet's guess: a wrapper that
/// supplies its own `tsdk` is precisely why someone configures one, so the
/// preflight has no standing to refuse it.
#[tokio::test]
async fn a_user_configured_astro_command_launches_without_a_project_typescript() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git"))?;
    std::fs::create_dir_all(root.join("src"))?;
    let command = root.join("my-wrapper").to_string_lossy().into_owned();
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "astro-language-server".to_owned(),
        LspServer {
            command: command.clone(),
            args: vec!["--stdio".to_owned()],
            ..LspServer::default()
        },
    );
    settings.languages.insert(
        "astro".to_owned(),
        LspLanguage {
            servers: vec!["astro-language-server".to_owned()],
            ..LspLanguage::default()
        },
    );
    let (mut manager, mut updates, mut launched) = manager_with_recorder(settings, root);

    manager.document_opened(
        Some("astro"),
        Some("astro"),
        &root.join("src").join("page.astro"),
        1,
        || "---\n---\n".into(),
    );

    let launches = observed_launches(&mut launched, 1).await;
    let spec = launches
        .first()
        .ok_or("the configured Astro command was never launched")?;
    assert_eq!(spec.command, command);
    assert_eq!(spec.args, vec!["--stdio".to_owned()]);
    let mut refused = None;
    while let Ok(update) = updates.try_recv() {
        if let LspUpdate::PreflightFailed { message, .. } = update {
            refused = Some(message);
        }
    }
    assert!(
        refused.is_none(),
        "a configured command was refused a launch: {refused:?}"
    );
    Ok(())
}

/// Switching a server off is a decision, not a missing install. Reporting one
/// produced "install it yourself so 'gopls' is on PATH" for a user who had just
/// said they did not want gopls -- and `managedDownloads: off` does not swallow
/// a manual-install notice the way it swallows an install offer.
#[tokio::test]
async fn a_disabled_primary_server_is_not_reported_as_missing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let mut settings = LspSettings::default();
    settings.servers.insert(
        "gopls".to_owned(),
        LspServer {
            enabled: false,
            command: "gopls".to_owned(),
            args: Vec::new(),
        },
    );
    settings.languages.insert(
        "go".to_owned(),
        LspLanguage {
            servers: vec!["gopls".to_owned()],
            ..LspLanguage::default()
        },
    );
    let (mut manager, mut updates, mut launched) = manager_with_recorder(settings, root);

    manager.document_opened(Some("go"), Some("go"), &root.join("main.go"), 1, || {
        "package main\n".into()
    });

    let launches = observed_launches(&mut launched, 0).await;
    assert!(
        launches.is_empty(),
        "`enabled = false` still started the server: {launches:?}"
    );
    let mut nagged = Vec::new();
    while let Ok(update) = updates.try_recv() {
        match update {
            LspUpdate::ManualInstallRequired { server, .. }
            | LspUpdate::InstallRequired { server, .. } => nagged.push(server),
            _ => {},
        }
    }
    assert!(
        nagged.is_empty(),
        "a server the user switched off was reported as missing: {nagged:?}"
    );
    Ok(())
}

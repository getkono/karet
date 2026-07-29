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
    let result = uninstall(dir.path(), &LanguageServerId::Clangd);
    assert!(matches!(result, Err(message) if message.contains("not managed")));
    Ok(())
}

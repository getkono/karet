use super::*;

#[derive(Deserialize, Serialize)]
pub(super) struct Deactivation {
    pub(super) deactivated: bool,
    version: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct RetiredInstallation {
    pub(super) version: String,
}

pub(super) fn retired_installations(
    root: &Path,
    server: &LanguageServerId,
) -> Vec<RetiredInstallation> {
    std::fs::read_to_string(provider_root(root, server).join("retired.jsonl"))
        .ok()
        .map(|journal| {
            journal
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn uninstall(root: &Path, server: &LanguageServerId) -> Result<bool, String> {
    if !managed_provider(server) {
        return Err(format!("{} is not managed by Karet", server.display_name()));
    }
    let provider = provider_root(root, server);
    let lock = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(provider.join("install.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    let active = read_active(root, server)
        .ok_or_else(|| format!("{} is not installed", server.display_name()))?;
    append_json_line(
        &provider.join("active.jsonl"),
        &Deactivation {
            deactivated: true,
            version: active.version.clone(),
        },
    )?;
    append_json_line(
        &provider.join("retired.jsonl"),
        &RetiredInstallation {
            version: active.version,
        },
    )?;
    cleanup_retired_provider(root, server);
    Ok(cleanup_pending(Some(root), server))
}

pub(super) fn append_json_line(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let encoded = serde_json::to_string(value).map_err(|error| error.to_string())?;
    let mut journal = File::options()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    writeln!(journal, "{encoded}").map_err(|error| error.to_string())?;
    journal.sync_all().map_err(|error| error.to_string())
}

pub(super) fn cleanup_retired_all(root: &Path) {
    for server in managed_servers() {
        cleanup_retired_provider(root, &server);
    }
}

fn cleanup_retired_provider(root: &Path, server: &LanguageServerId) {
    let versions = provider_root(root, server).join("versions");
    for retired in retired_installations(root, server) {
        let payload = versions.join(safe_version(&retired.version));
        if payload.is_dir() && !karet_supervisor::broker::managed_payload_in_use(root, &payload) {
            let _ = std::fs::remove_dir_all(payload);
        }
    }
}

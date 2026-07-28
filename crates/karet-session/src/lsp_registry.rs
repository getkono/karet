//! Machine-local language-server installations.
//!
//! The registry is shared by every karet process for the current OS user, while
//! server processes remain session-local.  The split is important: immutable
//! installations may safely be reused, but an LSP connection carries workspace
//! roots and unsaved buffer state and therefore must never be shared between
//! editor sessions.
//!
//! This module performs network I/O only while handling an explicit
//! [`RegistryJob::Install`] or [`RegistryJob::Check`] / [`RegistryJob::Apply`]
//! transaction. Merely opening a file only reads the append-only activation
//! journal. A per-provider file lock serializes changes made by concurrent karet
//! instances, and a version directory is activated only after its executable is
//! fully installed and verified.

mod catalog;

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use catalog::Archive;
use catalog::Release;
use catalog::ReleaseKind;
use catalog::discover;
use karet_lsp::LspSpec;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::LanguageServerChange;
use crate::api::LanguageServerId;
use crate::api::LanguageServerPlanId;
use crate::api::LanguageServerStatus;
use crate::api::RequestId;

const PLAN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const USER_AGENT: &str = concat!("karet/", env!("CARGO_PKG_VERSION"));
const SERVERS: [LanguageServerId; 4] = [
    LanguageServerId::RustAnalyzer,
    LanguageServerId::TypeScript,
    LanguageServerId::Pyright,
    LanguageServerId::Texlab,
];

/// Work accepted by the blocking registry worker.
pub(crate) enum RegistryJob {
    /// Discover and install one missing provider.
    Install {
        request: RequestId,
        server: LanguageServerId,
    },
    /// Discover newer versions for every installed provider.
    Check { request: RequestId },
    /// Apply a previously discovered, exact plan.
    Apply {
        request: RequestId,
        plan: LanguageServerPlanId,
    },
}

/// A result adopted by the session actor.
pub(crate) enum RegistryUpdate {
    Plan {
        request: RequestId,
        plan: LanguageServerPlanId,
        changes: Vec<LanguageServerChange>,
    },
    Changed {
        request: RequestId,
        server: LanguageServerId,
        version: String,
        was_installed: bool,
    },
    Progress {
        server: LanguageServerId,
        downloaded: u64,
        total: Option<u64>,
    },
    Complete {
        request: RequestId,
    },
    Failed {
        request: RequestId,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ActiveInstallation {
    version: String,
    command: PathBuf,
    args: Vec<String>,
}

struct StoredPlan {
    created: Instant,
    releases: Vec<Release>,
}

/// Start the registry's blocking worker.
pub(crate) fn spawn(
    root: Option<PathBuf>,
    supervisor: Option<PathBuf>,
) -> (
    mpsc::Sender<RegistryJob>,
    tokio_mpsc::UnboundedReceiver<RegistryUpdate>,
) {
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let (updates_tx, updates_rx) = tokio_mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("karet-lsp-registry".into())
        .spawn(move || run(root, supervisor, &jobs_rx, &updates_tx))
        .ok();
    (jobs_tx, updates_rx)
}

/// Resolve a built-in provider from local registry state without network I/O.
pub(crate) fn installed_spec(
    root: Option<&Path>,
    server: LanguageServerId,
    language: &str,
) -> Option<LspSpec> {
    let active = read_active(root?, server)?;
    Some(LspSpec {
        command: active.command.to_string_lossy().into_owned(),
        args: active.args,
        languages: vec![language.to_owned()],
    })
}

/// Read all local provider states without performing network I/O.
pub(crate) fn statuses(
    root: Option<&Path>,
    running: impl Fn(LanguageServerId) -> bool,
) -> Vec<LanguageServerStatus> {
    SERVERS
        .iter()
        .copied()
        .map(|server| LanguageServerStatus {
            server,
            installed: root
                .and_then(|root| read_active(root, server))
                .map(|active| active.version),
            running: running(server),
        })
        .collect()
}

fn run(
    root: Option<PathBuf>,
    supervisor: Option<PathBuf>,
    jobs: &mpsc::Receiver<RegistryJob>,
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
) {
    let Some(root) = root else {
        while let Ok(job) = jobs.recv() {
            let request = job_request(&job);
            let _ = updates.send(RegistryUpdate::Failed {
                request,
                message: "managed language-server storage is unavailable".into(),
            });
        }
        return;
    };
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build();
    let mut plans = HashMap::<LanguageServerPlanId, StoredPlan>::new();
    let mut next_plan = 1_u64;
    while let Ok(job) = jobs.recv() {
        let job_id = job_request(&job);
        plans.retain(|_, plan| plan.created.elapsed() <= PLAN_LIFETIME);
        let result = match job {
            RegistryJob::Install { request, server } => {
                if read_active(&root, server).is_some() {
                    Err(format!(
                        "{} is already installed; check for updates first",
                        server.display_name()
                    ))
                } else {
                    client
                        .as_ref()
                        .map_err(ToString::to_string)
                        .and_then(|client| discover(client, server))
                        .and_then(|release| {
                            install(
                                &root,
                                supervisor.as_deref(),
                                client.as_ref().map_err(ToString::to_string)?,
                                &release,
                                updates,
                            )
                            .map(|active| RegistryUpdate::Changed {
                                request,
                                server,
                                version: active.version,
                                was_installed: false,
                            })
                        })
                }
            },
            RegistryJob::Check { request } => {
                let result = client
                    .as_ref()
                    .map_err(ToString::to_string)
                    .and_then(|client| {
                        let mut releases = Vec::new();
                        for server in SERVERS {
                            let Some(active) = read_active(&root, server) else {
                                continue;
                            };
                            let mut release = discover(client, server)?;
                            if release.active_version() != active.version {
                                release.from_version = Some(active.version);
                                releases.push(release);
                            }
                        }
                        Ok(releases)
                    });
                result.map(|releases| {
                    let plan = LanguageServerPlanId(next_plan);
                    next_plan = next_plan.wrapping_add(1).max(1);
                    let changes = releases
                        .iter()
                        .map(|release| LanguageServerChange {
                            server: release.server,
                            current: read_active(&root, release.server)
                                .map(|active| active.version),
                            target: release.active_version(),
                            download_bytes: release.download_bytes,
                        })
                        .collect();
                    plans.insert(
                        plan,
                        StoredPlan {
                            created: Instant::now(),
                            releases,
                        },
                    );
                    RegistryUpdate::Plan {
                        request,
                        plan,
                        changes,
                    }
                })
            },
            RegistryJob::Apply { request, plan } => {
                let Some(stored) = plans.remove(&plan) else {
                    send_result(
                        updates,
                        request,
                        Err("language-server update plan expired; check again".into()),
                    );
                    continue;
                };
                client
                    .as_ref()
                    .map_err(ToString::to_string)
                    .and_then(|client| {
                        for release in &stored.releases {
                            // Exact-plan protection: another instance changing the active
                            // version invalidates this approval instead of silently
                            // applying a different transition.
                            let active = read_active(&root, release.server).ok_or_else(|| {
                                format!("{} is no longer installed", release.server.display_name())
                            })?;
                            if release.from_version.as_deref() != Some(active.version.as_str()) {
                                return Err(format!(
                                    "{} changed after this plan was checked; check again",
                                    release.server.display_name()
                                ));
                            }
                            install(&root, supervisor.as_deref(), client, release, updates)?;
                            let _ = updates.send(RegistryUpdate::Changed {
                                request,
                                server: release.server,
                                version: release.active_version(),
                                was_installed: !active.version.is_empty(),
                            });
                        }
                        Ok(RegistryUpdate::Complete { request })
                    })
            },
        };
        send_result(updates, job_id, result);
    }
}

fn job_request(job: &RegistryJob) -> RequestId {
    match job {
        RegistryJob::Install { request, .. }
        | RegistryJob::Check { request }
        | RegistryJob::Apply { request, .. } => *request,
    }
}

fn send_result(
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
    request: RequestId,
    result: Result<RegistryUpdate, String>,
) {
    let update = result.unwrap_or_else(|message| RegistryUpdate::Failed { request, message });
    let _ = updates.send(update);
}

fn read_active(root: &Path, server: LanguageServerId) -> Option<ActiveInstallation> {
    let journal = std::fs::read_to_string(provider_root(root, server).join("active.jsonl")).ok()?;
    journal
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .filter(|active: &ActiveInstallation| active.command.is_file())
}

fn provider_root(root: &Path, server: LanguageServerId) -> PathBuf {
    root.join(server.key())
}

fn install(
    root: &Path,
    supervisor: Option<&Path>,
    client: &Client,
    release: &Release,
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
) -> Result<ActiveInstallation, String> {
    let provider = provider_root(root, release.server);
    std::fs::create_dir_all(&provider).map_err(|error| error.to_string())?;
    let lock_path = provider.join("install.lock");
    let lock = File::options()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    if let Some(active) = read_active(root, release.server) {
        if active.version == release.active_version() || release.from_version.is_none() {
            // Another karet instance won the same first-install race. Adopt its
            // complete activation rather than replacing it with stale discovery.
            return Ok(active);
        }
        if release.from_version.as_deref() != Some(active.version.as_str()) {
            return Err(format!(
                "{} changed while this operation waited for its lock; check again",
                release.server.display_name()
            ));
        }
    }
    let versions = provider.join("versions");
    std::fs::create_dir_all(&versions).map_err(|error| error.to_string())?;
    let destination = versions.join(safe_version(&release.active_version()));
    if !destination.exists() {
        let staging = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&provider)
            .map_err(|error| error.to_string())?;
        install_release(client, supervisor, release, staging.path(), updates)?;
        std::fs::rename(staging.keep(), &destination).map_err(|error| error.to_string())?;
    }
    let active = activation(release, &destination)?;
    let encoded = serde_json::to_string(&active).map_err(|error| error.to_string())?;
    let mut journal = File::options()
        .create(true)
        .append(true)
        .open(provider.join("active.jsonl"))
        .map_err(|error| error.to_string())?;
    writeln!(journal, "{encoded}").map_err(|error| error.to_string())?;
    journal.sync_all().map_err(|error| error.to_string())?;
    Ok(active)
}

fn install_release(
    client: &Client,
    supervisor: Option<&Path>,
    release: &Release,
    destination: &Path,
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
) -> Result<(), String> {
    match &release.kind {
        ReleaseKind::Standalone {
            url,
            sha256,
            archive,
            executable_name,
        } => {
            let bytes = download_verified(client, url, sha256, |downloaded, total| {
                let _ = updates.send(RegistryUpdate::Progress {
                    server: release.server,
                    downloaded,
                    total,
                });
            })?;
            extract_executable(&bytes, *archive, executable_name, destination)
        },
        ReleaseKind::Npm {
            package,
            companion,
            node_version,
            node_url,
            node_sha256,
            node_archive,
        } => {
            let supervisor =
                supervisor.ok_or_else(|| "process supervisor is unavailable".to_owned())?;
            let bytes = download_verified(client, node_url, node_sha256, |downloaded, total| {
                let _ = updates.send(RegistryUpdate::Progress {
                    server: release.server,
                    downloaded,
                    total,
                });
            })?;
            let node_root = destination.join("node");
            extract_archive(&bytes, *node_archive, &node_root, true)?;
            let node = find_named(&node_root, node_executable())
                .ok_or_else(|| "downloaded Node archive contains no executable".to_owned())?;
            let npm = find_named(&node_root, npm_cli())
                .ok_or_else(|| "downloaded Node archive contains no npm CLI".to_owned())?;
            let package_root = destination.join("package");
            std::fs::create_dir_all(&package_root).map_err(|error| error.to_string())?;
            let mut args = vec![
                npm.to_string_lossy().into_owned(),
                "install".into(),
                "--global-style".into(),
                "--ignore-scripts".into(),
                "--no-audit".into(),
                "--no-fund".into(),
                "--prefix".into(),
                package_root.to_string_lossy().into_owned(),
            ];
            if let Some((companion, version)) = companion {
                args.push(format!("{companion}@{version}"));
            }
            args.push(format!("{package}@{}", release.version));
            let mut command = crate::process_supervisor::blocking_command(
                supervisor,
                node.to_string_lossy().into_owned(),
                args,
                destination,
            )
            .map_err(|error| error.to_string())?;
            command.stdout(std::process::Stdio::null());
            let mut child = command.spawn().map_err(|error| error.to_string())?;
            // This open pipe is the supervisor lease. `wait_with_output` would
            // close it before waiting and therefore (correctly) kill npm.
            let lease = child.stdin.take();
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| "npm supervisor exposed no stderr".to_owned())?;
            let reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = stderr.read_to_end(&mut bytes);
                bytes
            });
            let status = child.wait().map_err(|error| error.to_string())?;
            drop(lease);
            let errors = reader.join().unwrap_or_default();
            if !status.success() {
                return Err(format!(
                    "npm installation failed for {package}@{}: {}",
                    release.version,
                    String::from_utf8_lossy(&errors).trim()
                ));
            }
            std::fs::write(destination.join("NODE_VERSION"), node_version)
                .map_err(|error| error.to_string())
        },
    }
}

fn activation(release: &Release, destination: &Path) -> Result<ActiveInstallation, String> {
    let (command, args) = match release.server {
        LanguageServerId::RustAnalyzer => {
            (destination.join(executable("rust-analyzer")), Vec::new())
        },
        LanguageServerId::Texlab => (destination.join(executable("texlab")), Vec::new()),
        LanguageServerId::TypeScript => {
            let node = find_named(&destination.join("node"), node_executable())
                .ok_or_else(|| "installed Node executable is missing".to_owned())?;
            let cli = find_named(&destination.join("package"), "cli.mjs")
                .or_else(|| {
                    find_named(
                        &destination.join("package"),
                        "typescript-language-server.js",
                    )
                })
                .ok_or_else(|| "installed TypeScript language server is missing".to_owned())?;
            (
                node,
                vec![cli.to_string_lossy().into_owned(), "--stdio".into()],
            )
        },
        LanguageServerId::Pyright => {
            let node = find_named(&destination.join("node"), node_executable())
                .ok_or_else(|| "installed Node executable is missing".to_owned())?;
            let cli = find_named(&destination.join("package"), "langserver.index.js")
                .ok_or_else(|| "installed Pyright language server is missing".to_owned())?;
            (
                node,
                vec![cli.to_string_lossy().into_owned(), "--stdio".into()],
            )
        },
    };
    if !command.is_file() {
        return Err(format!(
            "installed {} executable is missing",
            release.server.display_name()
        ));
    }
    Ok(ActiveInstallation {
        version: release.active_version(),
        command,
        args,
    })
}

fn download_verified(
    client: &Client,
    url: &str,
    expected: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let total = response.content_length();
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut chunk)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        progress(bytes.len() as u64, total);
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("SHA-256 mismatch for {url}"));
    }
    Ok(bytes)
}

fn extract_executable(
    bytes: &[u8],
    archive: Archive,
    executable_name: &str,
    destination: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    if matches!(archive, Archive::Gzip) {
        let mut decoder = flate2::read::GzDecoder::new(bytes);
        let path = destination.join(executable_name);
        let mut file = File::create(&path).map_err(|error| error.to_string())?;
        std::io::copy(&mut decoder, &mut file).map_err(|error| error.to_string())?;
        make_executable(&path)?;
        return Ok(());
    }
    let scratch = tempfile::tempdir_in(destination).map_err(|error| error.to_string())?;
    extract_archive(bytes, archive, scratch.path(), false)?;
    let source = find_named(scratch.path(), executable_name)
        .ok_or_else(|| format!("archive contains no {executable_name}"))?;
    let target = destination.join(executable_name);
    std::fs::copy(source, &target).map_err(|error| error.to_string())?;
    make_executable(&target)
}

fn extract_archive(
    bytes: &[u8],
    archive: Archive,
    destination: &Path,
    all_files: bool,
) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    match archive {
        Archive::TarGzip => {
            let decoder = flate2::read::GzDecoder::new(bytes);
            let mut archive = tar::Archive::new(decoder);
            for entry in archive.entries().map_err(|error| error.to_string())? {
                let mut entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path().map_err(|error| error.to_string())?;
                if path.components().any(|part| {
                    matches!(
                        part,
                        std::path::Component::ParentDir | std::path::Component::RootDir
                    )
                }) {
                    return Err("archive contains an unsafe path".into());
                }
                if all_files || entry.header().entry_type().is_file() {
                    entry
                        .unpack_in(destination)
                        .map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        },
        Archive::Zip => {
            let cursor = std::io::Cursor::new(bytes);
            let mut archive = zip::ZipArchive::new(cursor).map_err(|error| error.to_string())?;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
                let Some(path) = entry.enclosed_name() else {
                    return Err("archive contains an unsafe path".into());
                };
                let output = destination.join(path);
                if entry.is_dir() {
                    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
                } else if all_files || entry.is_file() {
                    if let Some(parent) = output.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                    }
                    let mut file = File::create(output).map_err(|error| error.to_string())?;
                    std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        },
        Archive::Gzip => Err("plain gzip is not a multi-file archive".into()),
    }
}

fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|candidate| candidate == name) {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_named(&path, name)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn executable(name: &str) -> String {
    format!("{name}.exe")
}

#[cfg(not(windows))]
fn executable(name: &str) -> String {
    name.into()
}

#[cfg(windows)]
fn node_executable() -> &'static str {
    "node.exe"
}

#[cfg(not(windows))]
fn node_executable() -> &'static str {
    "node"
}

fn npm_cli() -> &'static str {
    "npm-cli.js"
}

fn safe_version(version: &str) -> String {
    version
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_journal_ignores_a_torn_tail() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let provider = provider_root(dir.path(), LanguageServerId::Texlab);
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
        let resolved = read_active(dir.path(), LanguageServerId::Texlab);
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
}

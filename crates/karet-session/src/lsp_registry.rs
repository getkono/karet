//! Machine-local language-server installations.
//!
//! The registry is shared by every karet process for the current OS user. Live
//! processes are shared separately by the authenticated [`karet_supervisor::broker`],
//! keyed by provider launch and exact repository root.
//!
//! This module performs network I/O only while handling an explicit
//! [`RegistryJob::Install`] or [`RegistryJob::Check`] / [`RegistryJob::Apply`]
//! transaction. Merely opening a file only reads the append-only activation
//! journal. A per-provider file lock serializes changes made by concurrent karet
//! instances, and a version directory is activated only after its executable is
//! fully installed and verified.

mod archive;
mod catalog;
mod declined;
mod lifecycle;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;
use std::time::Instant;

use archive::*;
#[cfg(test)]
use catalog::Archive;
use catalog::Release;
use catalog::ReleaseKind;
use catalog::discover;
use catalog::managed_recipe;
use catalog::managed_servers;
pub(crate) use declined::*;
use karet_lsp::LspSpec;
use lifecycle::*;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::LanguageServerChange;
use crate::api::LanguageServerId;
use crate::api::LanguageServerPlanId;
use crate::api::RequestId;

const PLAN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const USER_AGENT: &str = concat!("karet/", env!("CARGO_PKG_VERSION"));
/// Work accepted by the blocking registry worker.
pub(crate) enum RegistryJob {
    /// Discover and install one missing provider.
    Install {
        request: RequestId,
        server: LanguageServerId,
    },
    /// Discover newer versions for every installed provider.
    Check {
        request: RequestId,
        server: Option<LanguageServerId>,
    },
    /// Apply a previously discovered, exact plan.
    Apply {
        request: RequestId,
        plan: LanguageServerPlanId,
        servers: Vec<LanguageServerId>,
    },
    /// Deactivate one managed provider and retire its immutable payload.
    Uninstall {
        request: RequestId,
        server: LanguageServerId,
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
    Removed {
        request: RequestId,
        server: LanguageServerId,
        cleanup_pending: bool,
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
    /// `initializationOptions` this installation must be launched with.
    ///
    /// Recorded here rather than recomputed at launch because it names paths
    /// inside this immutable version directory, which only the install knew.
    /// Defaulted so journals written before this existed still replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    initialization_options: Option<serde_json::Value>,
}

#[derive(Clone)]
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
    server: &LanguageServerId,
    language: &str,
) -> Option<LspSpec> {
    let active = read_active(root?, server)?;
    Some(LspSpec {
        command: active.command.to_string_lossy().into_owned(),
        args: active.args,
        languages: vec![language.to_owned()],
        initialization_options: active.initialization_options,
    })
}

/// Read the active managed version without performing network I/O.
pub(crate) fn installed_version(root: Option<&Path>, server: &LanguageServerId) -> Option<String> {
    root.and_then(|root| read_active(root, server))
        .map(|active| active.version)
}

/// Whether safe reclamation of a deactivated payload is still pending.
pub(crate) fn cleanup_pending(root: Option<&Path>, server: &LanguageServerId) -> bool {
    let Some(root) = root else {
        return false;
    };
    retired_installations(root, server)
        .into_iter()
        .any(|retired| {
            provider_root(root, server)
                .join("versions")
                .join(safe_version(&retired.version))
                .exists()
        })
}

/// Whether Karet has a complete managed installation recipe for this provider.
pub(crate) fn managed_provider(server: &LanguageServerId) -> bool {
    managed_recipe(server).is_some()
}

/// Why karet will not install `server` itself, if it will not.
///
/// [`None`] means exactly one thing: karet owns this provider's installation on
/// this platform, so offering to install it is honest. Every other case -- a
/// provider that must come from a project toolchain, one whose publisher ships
/// no verified artifact for this `(os, arch)`, and an id karet has never heard
/// of -- yields a reason the user can act on.
///
/// Callers rely on that totality to decide between offering an install and
/// explaining one, so a new arm must never fall through to [`None`].
pub(crate) fn manual_install_reason(server: &LanguageServerId) -> Option<String> {
    if managed_provider(server) {
        return None;
    }
    let reason = match server.key() {
        "csharp" => "requires the user's .NET SDK and MSBuild installation",
        "gopls" => "official installation and analysis require the project's Go toolchain",
        "jdtls" => "requires a user-selected Java 21 runtime and project JDKs",
        "lemminx" => "requires a compatible user-installed Java runtime",
        "ruby-lsp" => "must be installed in the project's Ruby and Bundler environment",
        "phpactor" => "requires the project's PHP runtime and extensions",
        "sourcekit-lsp" => "ships with and must match the Swift or Xcode toolchain",
        "metals" => "requires the project's JVM and Scala build environment",
        "haskell-language-server" => "must match the project's GHC toolchain",
        "ocamllsp" => "must be installed in the project's opam switch",
        "elp" => "release selection must match the project's Erlang and OTP toolchain",
        "dart-language-server" => "ships with the Dart or Flutter SDK",
        "r-languageserver" => "must be installed into the user's R library",
        "powershell-editor-services" => {
            "has no standalone executable; it is a PowerShell module bundle entered through \
             Start-EditorServices.ps1"
        },
        "esbonio" => "must use the project's Python and Sphinx environment",
        "pkl-lsp" => "requires compatible user-installed Java and Pkl runtimes",
        "taplo" => "current native releases have no publisher-authenticated SHA-256 digest",
        "pylsp" => "must be installed in the project's Python environment, with its Flake8 plugin",
        key if catalog::managed_recipes()
            .iter()
            .any(|recipe| recipe.server == key) =>
        {
            return Some(format!(
                "publisher provides no verified release for {}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ));
        },
        // A provider karet has never heard of, which in practice means a user
        // `lsp.servers` entry. Its installation was always the user's, and
        // saying so is better than the silence that used to imply karet could
        // fetch it.
        _ => return Some(format!("{} is not a provider karet installs", server.key())),
    };
    Some(reason.to_owned())
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
    let mut installers = Vec::new();
    loop {
        reap_registry_tasks(&mut installers);
        cleanup_retired_all(&root);
        let job = match jobs.recv_timeout(Duration::from_secs(30)) {
            Ok(job) => job,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let job_id = job_request(&job);
        plans.retain(|_, plan| plan.created.elapsed() <= PLAN_LIFETIME);
        let result = match job {
            RegistryJob::Install { request, server } => {
                // Last guard, and the one that decides what the user reads. An
                // unmanaged provider reaching here used to fail deep in
                // discovery with "has no managed installer"; the reason it is
                // manual is more useful and is known up front.
                if let Some(reason) = manual_install_reason(&server) {
                    let message = format!("{} {reason}", server.display_name());
                    send_result(updates, request, Err(message));
                    continue;
                }
                let client = match client.as_ref() {
                    Ok(client) => client,
                    Err(error) => {
                        send_result(updates, request, Err(error.to_string()));
                        continue;
                    },
                };
                match spawn_install(
                    root.clone(),
                    supervisor.clone(),
                    client.clone(),
                    request,
                    server,
                    updates.clone(),
                ) {
                    Ok(installer) => installers.push(installer),
                    Err(message) => send_result(updates, request, Err(message)),
                }
                continue;
            },
            RegistryJob::Check { request, server } => {
                let result = client
                    .as_ref()
                    .map_err(ToString::to_string)
                    .and_then(|client| {
                        let mut releases = Vec::new();
                        if server
                            .as_ref()
                            .is_some_and(|candidate| !managed_provider(candidate))
                        {
                            return Err("provider has no Karet-managed update channel".to_owned());
                        }
                        let servers = managed_servers();
                        for server in servers
                            .iter()
                            .filter(|candidate| server.as_ref().is_none_or(|id| id == *candidate))
                        {
                            let Some(active) = read_active(&root, server) else {
                                continue;
                            };
                            let mut release = discover(client, server.clone())?;
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
                            server: release.server.clone(),
                            current: read_active(&root, &release.server)
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
            RegistryJob::Apply {
                request,
                plan,
                servers,
            } => {
                let Some(stored) = plans.remove(&plan) else {
                    send_result(
                        updates,
                        request,
                        Err("language-server update plan expired; check again".into()),
                    );
                    continue;
                };
                if servers.is_empty() {
                    plans.insert(plan, stored);
                    send_result(
                        updates,
                        request,
                        Err("language-server update selection is empty".into()),
                    );
                    continue;
                }
                let selected: HashSet<_> = servers.into_iter().collect();
                if selected.iter().any(|server| {
                    !stored
                        .releases
                        .iter()
                        .any(|release| &release.server == server)
                }) {
                    plans.insert(plan, stored);
                    send_result(
                        updates,
                        request,
                        Err("language-server update selection is not in this plan".into()),
                    );
                    continue;
                }
                let backup = stored.clone();
                let remaining = stored
                    .releases
                    .iter()
                    .filter(|release| !selected.contains(&release.server))
                    .cloned()
                    .collect::<Vec<_>>();
                let result = client
                    .as_ref()
                    .map_err(ToString::to_string)
                    .and_then(|client| {
                        for release in stored
                            .releases
                            .iter()
                            .filter(|release| selected.contains(&release.server))
                        {
                            // Exact-plan protection: another instance changing the active
                            // version invalidates this approval instead of silently
                            // applying a different transition.
                            let active = read_active(&root, &release.server);
                            if release.from_version.as_deref()
                                != active
                                    .as_ref()
                                    .map(|installation| installation.version.as_str())
                            {
                                return Err(format!(
                                    "{} changed after this plan was checked; check again",
                                    release.server.display_name()
                                ));
                            }
                            install(&root, supervisor.as_deref(), client, release, updates)?;
                            let _ = updates.send(RegistryUpdate::Changed {
                                request,
                                server: release.server.clone(),
                                version: release.active_version(),
                                was_installed: active.is_some(),
                            });
                        }
                        Ok(RegistryUpdate::Complete { request })
                    });
                if result.is_err() {
                    plans.insert(plan, backup);
                } else if !remaining.is_empty() {
                    plans.insert(
                        plan,
                        StoredPlan {
                            created: stored.created,
                            releases: remaining,
                        },
                    );
                }
                result
            },
            RegistryJob::Uninstall { request, server } => {
                uninstall(&root, &server).map(|cleanup_pending| RegistryUpdate::Removed {
                    request,
                    server,
                    cleanup_pending,
                })
            },
        };
        send_result(updates, job_id, result);
    }
    for installer in installers {
        let _ = installer.join();
    }
}

fn spawn_install(
    root: PathBuf,
    supervisor: Option<PathBuf>,
    client: Client,
    request: RequestId,
    server: LanguageServerId,
    updates: tokio_mpsc::UnboundedSender<RegistryUpdate>,
) -> Result<std::thread::JoinHandle<()>, String> {
    let name = format!("karet-lsp-install-{}", server.key());
    spawn_registry_task(name, move || {
        let result = if read_active(&root, &server).is_some() {
            Err(format!(
                "{} is already installed; check for updates first",
                server.display_name()
            ))
        } else {
            discover(&client, server).and_then(|release| {
                install_discovered(
                    &root,
                    supervisor.as_deref(),
                    &client,
                    request,
                    &release,
                    &updates,
                )
            })
        };
        send_result(&updates, request, result);
    })
    .map_err(|error| format!("cannot start language-server installer: {error}"))
}

fn spawn_registry_task(
    name: String,
    task: impl FnOnce() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(name).spawn(task)
}

fn reap_registry_tasks(tasks: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut active = Vec::with_capacity(tasks.len());
    for task in tasks.drain(..) {
        if task.is_finished() {
            let _ = task.join();
        } else {
            active.push(task);
        }
    }
    *tasks = active;
}

fn job_request(job: &RegistryJob) -> RequestId {
    match job {
        RegistryJob::Install { request, .. }
        | RegistryJob::Check { request, .. }
        | RegistryJob::Apply { request, .. }
        | RegistryJob::Uninstall { request, .. } => *request,
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

fn read_active(root: &Path, server: &LanguageServerId) -> Option<ActiveInstallation> {
    let journal = std::fs::read_to_string(provider_root(root, server).join("active.jsonl")).ok()?;
    let mut active = None;
    for line in journal.lines() {
        if serde_json::from_str::<Deactivation>(line).is_ok_and(|record| record.deactivated) {
            active = None;
        } else if let Ok(candidate) = serde_json::from_str::<ActiveInstallation>(line) {
            active = Some(candidate);
        }
    }
    active.filter(|active: &ActiveInstallation| active.command.is_file())
}

fn provider_root(root: &Path, server: &LanguageServerId) -> PathBuf {
    root.join(server.key())
}

fn install(
    root: &Path,
    supervisor: Option<&Path>,
    client: &Client,
    release: &Release,
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
) -> Result<ActiveInstallation, String> {
    let provider = provider_root(root, &release.server);
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
    if let Some(active) = read_active(root, &release.server) {
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

fn install_discovered(
    root: &Path,
    supervisor: Option<&Path>,
    client: &Client,
    request: RequestId,
    release: &Release,
    updates: &tokio_mpsc::UnboundedSender<RegistryUpdate>,
) -> Result<RegistryUpdate, String> {
    let active = install(root, supervisor, client, release, updates)?;
    Ok(RegistryUpdate::Changed {
        request,
        server: release.server.clone(),
        version: active.version,
        was_installed: false,
    })
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
            retain_archive,
            ..
        } => {
            let bytes = download_verified(client, url, sha256, |downloaded, total| {
                let _ = updates.send(RegistryUpdate::Progress {
                    server: release.server.clone(),
                    downloaded,
                    total,
                });
            })?;
            if *retain_archive {
                extract_archive(&bytes, *archive, destination, true)
            } else {
                extract_executable(&bytes, *archive, executable_name, destination)
            }
        },
        ReleaseKind::Npm {
            package,
            companion,
            node_version,
            node_url,
            node_sha256,
            node_archive,
            ..
        } => {
            let supervisor =
                supervisor.ok_or_else(|| "process supervisor is unavailable".to_owned())?;
            let bytes = download_verified(client, node_url, node_sha256, |downloaded, total| {
                let _ = updates.send(RegistryUpdate::Progress {
                    server: release.server.clone(),
                    downloaded,
                    total,
                });
            })?;
            let node_root = destination.join("node");
            extract_archive(&bytes, *node_archive, &node_root, true)?;
            let node = find_file_named(&node_root, node_executable())
                .ok_or_else(|| "downloaded Node archive contains no executable".to_owned())?;
            let npm = find_file_named(&node_root, npm_cli())
                .ok_or_else(|| "downloaded Node archive contains no npm CLI".to_owned())?;
            let package_root = destination.join("package");
            std::fs::create_dir_all(&package_root).map_err(|error| error.to_string())?;
            let mut args = vec![
                npm.to_string_lossy().into_owned(),
                "install".into(),
                // `--global-style` is deprecated in favour of this since npm 9;
                // the bundled runtime is active LTS, so the newer spelling is
                // always available. Same layout: dependencies stay unhoisted
                // under the package, which is what `activation` resolves against.
                "--install-strategy=shallow".into(),
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
            let mut command = karet_supervisor::supervisor::blocking_command(
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
    let mut options = None;
    let (command, args) = match &release.kind {
        ReleaseKind::Standalone {
            executable_name,
            arguments,
            ..
        } => {
            let command = find_file_named(destination, executable_name).ok_or_else(|| {
                format!(
                    "installed {} executable is missing",
                    release.server.display_name()
                )
            })?;
            make_executable(&command)?;
            (
                command,
                arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect(),
            )
        },
        ReleaseKind::Npm {
            package,
            companion,
            entrypoint,
            arguments,
            ..
        } => {
            let node = find_file_named(&destination.join("node"), node_executable())
                .ok_or_else(|| "installed Node executable is missing".to_owned())?;
            let package_root = package.split('/').try_fold(
                destination.join("package").join("node_modules"),
                |path, component| {
                    (!component.is_empty() && component != "." && component != "..")
                        .then(|| path.join(component))
                },
            );
            let cli = package_root
                .map(|root| root.join(entrypoint))
                .filter(|path| path.is_file())
                .ok_or_else(|| {
                    format!(
                        "installed {} language server is missing",
                        release.server.display_name()
                    )
                })?;
            let mut args = vec![cli.to_string_lossy().into_owned()];
            args.extend(arguments.iter().map(|argument| (*argument).to_owned()));
            // A server given a TypeScript companion needs to be told where it
            // went. karet installs the pair into an immutable version directory
            // that is on nobody's search path, and a server like Astro's takes
            // the location as an init option rather than looking for one.
            if companion
                .as_ref()
                .is_some_and(|(name, _)| name == "typescript")
            {
                let tsdk = destination
                    .join("package")
                    .join("node_modules")
                    .join("typescript")
                    .join("lib");
                if tsdk.is_dir() {
                    options = Some(serde_json::json!({
                        "typescript": { "tsdk": tsdk.to_string_lossy() }
                    }));
                }
            }
            (node, args)
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
        initialization_options: options,
    })
}

#[cfg(all(test, windows))]
fn executable(name: &str) -> String {
    format!("{name}.exe")
}

#[cfg(all(test, not(windows)))]
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

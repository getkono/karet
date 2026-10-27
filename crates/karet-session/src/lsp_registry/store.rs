//! The registry's on-disk store, read synchronously and without network I/O.
//!
//! Each provider owns a directory under the registry root holding its
//! append-only activation journal (`active.jsonl`) and the user's recorded
//! answer to the install prompt (`declined.json`). Opening a file, listing the
//! inventory and resolving a launch only read these; the background worker in
//! the parent module is the one writer of the journal, and the session writes
//! the refusal.

mod declined;

pub(crate) use declined::Declined;
pub(crate) use declined::clear_declined;
pub(crate) use declined::ever_installed;
pub(crate) use declined::read_declined;
pub(crate) use declined::write_declined;

use super::*;

/// One activation record of the journal: the installation a provider launches.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ActiveInstallation {
    pub(super) version: String,
    pub(super) command: PathBuf,
    pub(super) args: Vec<String>,
    /// The part of `args` that belongs to this installation rather than to the
    /// provider's launch table — the entry script an npm package is run
    /// through, and nothing for a standalone binary.
    ///
    /// Recorded so the launch arguments can be re-derived from the current
    /// table at read time. Without it, `args` froze at install time: a provider
    /// installed under a recipe with the wrong argv kept launching wrongly
    /// until it was reinstalled, which is precisely what a fix to the table is
    /// supposed to prevent.
    ///
    /// Absent in journals written before this existed, and those keep replaying
    /// `args` verbatim — the behaviour they were written under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) entry: Option<Vec<String>>,
    /// `initializationOptions` this installation must be launched with.
    ///
    /// Recorded here rather than recomputed at launch because it names paths
    /// inside this immutable version directory, which only the install knew.
    /// Defaulted so journals written before this existed still replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) initialization_options: Option<serde_json::Value>,
}

impl ActiveInstallation {
    /// The arguments to launch this installation with.
    ///
    /// Composed from the recorded entry point plus the provider's *current*
    /// launch arguments, so correcting a built-in's argv takes effect on the
    /// next launch rather than the next reinstall.
    pub(super) fn launch_args(&self, server: &LanguageServerId) -> Vec<String> {
        let Some(entry) = &self.entry else {
            return self.args.clone();
        };
        entry
            .iter()
            .cloned()
            .chain(
                crate::lsp::managed_arguments(server.key())
                    .iter()
                    .map(|argument| (*argument).to_owned()),
            )
            .collect()
    }
}

/// Resolve a built-in provider from local registry state without network I/O.
pub(crate) fn installed_spec(
    root: Option<&Path>,
    server: &LanguageServerId,
    language: &str,
) -> Option<LspSpec> {
    let active = read_active(root?, server)?;
    Some(
        LspSpec::new(
            active.command.to_string_lossy().into_owned(),
            active.launch_args(server),
            vec![language.to_owned()],
        )
        .with_initialization_options(active.initialization_options),
    )
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

/// The installation `server` launches: the journal replayed last-wins, and
/// `None` once a deactivation follows it or its executable is gone.
pub(super) fn read_active(root: &Path, server: &LanguageServerId) -> Option<ActiveInstallation> {
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

/// The directory `server`'s journal, versions and refusal live in.
pub(super) fn provider_root(root: &Path, server: &LanguageServerId) -> PathBuf {
    root.join(server.key())
}

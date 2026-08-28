//! Whether the user has already been asked to install a provider, and said no.
//!
//! The install prompt costs the user bandwidth they may not want to spend, so it
//! is asked at most once. Two facts decide that, and they are deliberately
//! separate from settings: settings say what the user *wants configured*, while
//! these say what karet has already *asked and been told*. Turning a provider
//! back on should not silently re-trigger a download the user declined, and
//! declining a download should not read as disabling the provider.
//!
//! Both live beside the install journals under the provider's own directory, in
//! the same append-and-replay style: a `declined.json` the user's answer writes,
//! and `active.jsonl`, which already records every activation this provider has
//! ever had.

use super::*;

/// The user's recorded refusal of one provider's install.
///
/// Unconditional, and necessarily so: the prompt is raised *before* any
/// discovery — that is the point of `managedDownloads: "prompt"`, which promises
/// no network I/O until the user agrees — so at the moment of the refusal there
/// is no resolved version to scope it against.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Declined {
    /// When the refusal was recorded (seconds since the Unix epoch, for a human
    /// reading the file).
    pub(crate) declined_at: String,
}

impl Declined {
    /// A refusal recorded now.
    pub(crate) fn now() -> Self {
        Self {
            declined_at: unix_timestamp(),
        }
    }
}

/// Seconds since the Unix epoch, as a string. A clock that reads before the
/// epoch is not worth an error path here; the field is human context, not logic.
fn unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// The recorded refusal for `server`, if the user has declined its install.
pub(crate) fn read_declined(root: Option<&Path>, server: &LanguageServerId) -> Option<Declined> {
    let path = provider_root(root?, server).join("declined.json");
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Record that the user declined `server`'s install.
pub(crate) fn write_declined(
    root: &Path,
    server: &LanguageServerId,
    declined: &Declined,
) -> Result<(), String> {
    let provider = provider_root(root, server);
    std::fs::create_dir_all(&provider).map_err(|error| error.to_string())?;
    let body = serde_json::to_string_pretty(declined).map_err(|error| error.to_string())?;
    std::fs::write(provider.join("declined.json"), body).map_err(|error| error.to_string())
}

/// Forget a recorded refusal, so the provider may be offered again.
pub(crate) fn clear_declined(root: &Path, server: &LanguageServerId) -> Result<(), String> {
    match std::fs::remove_file(provider_root(root, server).join("declined.json")) {
        Ok(()) => Ok(()),
        // Clearing a refusal that was never recorded is the state the caller asked
        // for, not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether Karet has ever completed an install of `server`.
///
/// Distinct from [`installed_version`](super::installed_version), which replays
/// the journal last-wins and so reports `None` for a provider that was installed
/// and later uninstalled. "Have we ever asked and been told yes" needs the
/// *history*: after an uninstall, the user has answered this question once
/// already and should not be asked again.
pub(crate) fn ever_installed(root: Option<&Path>, server: &LanguageServerId) -> bool {
    let Some(root) = root else {
        return false;
    };
    if !retired_installations(root, server).is_empty() {
        return true;
    }
    let path = provider_root(root, server).join("active.jsonl");
    let Ok(journal) = std::fs::read_to_string(path) else {
        return false;
    };
    journal.lines().any(|line| {
        // A deactivation record deserializes as neither, so check it first.
        !serde_json::from_str::<Deactivation>(line).is_ok_and(|record| record.deactivated)
            && serde_json::from_str::<ActiveInstallation>(line).is_ok()
    })
}

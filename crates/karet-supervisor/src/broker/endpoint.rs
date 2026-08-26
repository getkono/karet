//! The endpoint file a running broker publishes, and the private-permission
//! helpers guarding it.
//!
//! The JSON shape is frozen: `karet-session`'s garbage-collection test writes
//! these files by hand, so field names must not change.

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use crate::broker::BrokerError;
use crate::broker::io_error;

/// Where a live broker listens, and how to authenticate to it.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Endpoint {
    pub(crate) address: SocketAddr,
    pub(crate) token: String,
    pub(crate) pid: u32,
    #[serde(default)]
    pub(crate) command: Option<PathBuf>,
}

/// Publish `endpoint` at `path` atomically, private before it is visible.
pub(crate) fn write_endpoint(path: &Path, endpoint: &Endpoint) -> Result<(), BrokerError> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes =
        serde_json::to_vec(endpoint).map_err(|error| BrokerError::Spec(error.to_string()))?;
    std::fs::write(&temporary, bytes).map_err(io_error)?;
    set_private_file(&temporary)?;
    std::fs::rename(&temporary, path).map_err(io_error)
}

/// Whether a live broker published under `directory` still runs `payload`.
pub(crate) fn payload_in_use(directory: &Path, payload: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        if entry.path().extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            return false;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            return false;
        };
        let Ok(endpoint) = serde_json::from_slice::<Endpoint>(&bytes) else {
            return false;
        };
        if std::net::TcpStream::connect_timeout(&endpoint.address, Duration::from_millis(50))
            .is_err()
        {
            return false;
        }
        endpoint
            .command
            .as_ref()
            .is_none_or(|command| command.starts_with(payload))
    })
}

#[cfg(unix)]
pub(crate) fn set_private_directory(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io_error)
}

#[cfg(not(unix))]
pub(crate) fn set_private_directory(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_private_file(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io_error)
}

#[cfg(not(unix))]
pub(crate) fn set_private_file(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

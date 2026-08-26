//! Broker identity: the stable key derived from a launch description, and the
//! bearer token derived from that key.
//!
//! The key names the `(protocol version, supervisor version, root, command,
//! arguments)` tuple a broker owns; two connectors that agree on all five reach
//! the same broker, and any difference elects a separate one.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// How a broker starts the process it fronts.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Launch {
    /// Executable to run.
    pub command: String,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Working directory the process is rooted at.
    pub root: PathBuf,
}

/// Stable identity for the broker owning `launch` under `protocol_version`.
pub(crate) fn broker_key(protocol_version: &str, launch: &Launch) -> String {
    let mut hash = Sha256::new();
    hash.update(protocol_version);
    hash.update(env!("CARGO_PKG_VERSION"));
    hash.update(launch.root.as_os_str().to_string_lossy().as_bytes());
    hash.update(&launch.command);
    for argument in &launch.args {
        hash.update([0]);
        hash.update(argument);
    }
    format!("{:x}", hash.finalize())
}

/// Unguessable bearer token for the broker identified by `key`.
pub(crate) fn broker_token(key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(key);
    hash.update(std::process::id().to_le_bytes());
    hash.update(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::Launch;
    use super::broker_key;
    use super::broker_token;

    fn launch() -> Launch {
        Launch {
            command: "rust-analyzer".to_owned(),
            args: Vec::new(),
            root: PathBuf::from("/a"),
        }
    }

    #[test]
    fn keys_separate_protocol_versions() {
        let launch = launch();
        assert_eq!(broker_key("1", &launch), broker_key("1", &launch));
        assert_ne!(broker_key("1", &launch), broker_key("2", &launch));
    }

    #[test]
    fn tokens_are_not_the_key() {
        let key = broker_key("1", &launch());
        assert_ne!(broker_token(&key), key);
    }
}

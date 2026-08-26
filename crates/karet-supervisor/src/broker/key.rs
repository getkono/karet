//! Broker identity: the stable key derived from a launch description, and the
//! bearer token derived from that key.
//!
//! The key names the `(protocol, protocol version, supervisor version, root,
//! command, arguments)` tuple a broker owns; two connectors that agree on all
//! six reach the same broker, and any difference elects a separate one.

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
///
/// `prelude` is the protocol's authentication greeting
/// (`BrokerProtocol::PRELUDE`), folded in as the protocol's domain separator:
/// `protocol_version` alone is a bare counter each protocol picks for itself,
/// so two protocols landing on the same one would derive identical
/// `{key}.json`/`{key}.lock` names and fight over each other's files. The
/// prelude is reused rather than a key-only constant added because it is
/// already the protocol's on-the-wire identity — a second protocol that copied
/// it would also greet the first protocol's brokers indistinguishably — so it
/// carries a real pressure to stay unique that a hash-only constant would not.
///
/// Every field is hashed behind a `[0]` separator, so no two different field
/// splits can hash the same bytes.
///
/// Adding an input re-derives every key, and that is deliberate, not a
/// regression to undo: `CARGO_PKG_VERSION` is already hashed here, so every
/// karet release re-partitions brokers anyway, and a broker nobody reaches
/// idle-retires within 30 seconds.
pub(crate) fn broker_key(prelude: &str, protocol_version: &str, launch: &Launch) -> String {
    let mut hash = Sha256::new();
    for field in [prelude, protocol_version, env!("CARGO_PKG_VERSION")] {
        hash.update([0]);
        hash.update(field);
    }
    hash.update([0]);
    hash.update(launch.root.as_os_str().to_string_lossy().as_bytes());
    hash.update([0]);
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

    const PRELUDE: &str = "KARET-TEST-BROKER ";

    #[test]
    fn keys_separate_protocol_versions() {
        let launch = launch();
        assert_eq!(
            broker_key(PRELUDE, "1", &launch),
            broker_key(PRELUDE, "1", &launch)
        );
        assert_ne!(
            broker_key(PRELUDE, "1", &launch),
            broker_key(PRELUDE, "2", &launch)
        );
    }

    #[test]
    fn keys_separate_protocols_that_share_a_protocol_version() {
        let launch = launch();
        assert_ne!(
            broker_key("KARET-LSP-BROKER ", "1", &launch),
            broker_key("KARET-DAP-BROKER ", "1", &launch)
        );
    }

    #[test]
    fn keys_do_not_alias_across_field_boundaries() {
        let launch = launch();
        assert_ne!(
            broker_key("KARET-BROKER ", "1", &launch),
            broker_key("KARET-BROKER 1", "", &launch)
        );
    }

    #[test]
    fn tokens_are_not_the_key() {
        let key = broker_key(PRELUDE, "1", &launch());
        assert_ne!(broker_token(&key), key);
    }
}

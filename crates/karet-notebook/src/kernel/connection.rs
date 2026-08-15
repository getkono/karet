//! Connection files: the five loopback ports and hmac key a kernel is told
//! about at spawn, in Jupyter's on-disk JSON shape.

use std::hash::BuildHasher;
use std::hash::Hasher;
use std::path::PathBuf;

use jupyter_protocol::ConnectionInfo;

/// Mint a loopback [`ConnectionInfo`]: five OS-assigned free ports and a
/// fresh signing key (`hmac-sha256`).
///
/// The ports are bound and released, the standard race-tolerant approach —
/// the kernel binds them right after.
///
/// # Errors
/// Propagates the bind failure (exotic: no loopback at all).
pub fn local_connection() -> std::io::Result<ConnectionInfo> {
    let port = || -> std::io::Result<u16> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        Ok(listener.local_addr()?.port())
    };
    Ok(ConnectionInfo {
        ip: "127.0.0.1".to_owned(),
        transport: jupyter_protocol::connection_info::Transport::TCP,
        shell_port: port()?,
        iopub_port: port()?,
        stdin_port: port()?,
        control_port: port()?,
        hb_port: port()?,
        key: random_key(),
        signature_scheme: "hmac-sha256".to_owned(),
        kernel_name: None,
    })
}

/// Write `info` as a `kernel-<pid>-<nonce>.json` connection file in the OS
/// temp directory, returning its path (the caller passes it to the kernel's
/// argv and deletes it when the kernel dies).
///
/// # Errors
/// Propagates serialization and write failures.
pub fn write_connection_file(info: &ConnectionInfo) -> std::io::Result<PathBuf> {
    let json = serde_json::to_string_pretty(info).map_err(std::io::Error::other)?;
    let nonce: String = info.key.chars().take(8).collect();
    let path =
        std::env::temp_dir().join(format!("karet-kernel-{}-{nonce}.json", std::process::id()));
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Replace every `{connection_file}` occurrence in a kernelspec argv.
#[must_use]
pub fn substitute_argv(argv: &[String], connection_file: &str) -> Vec<String> {
    argv.iter()
        .map(|arg| arg.replace("{connection_file}", connection_file))
        .collect()
}

/// A 32-hex-char signing key from OS entropy, without a `rand` dependency:
/// each `RandomState` seeds from the OS, and finishing two of them yields
/// 128 unpredictable bits. The key guards loopback sockets on this machine.
fn random_key() -> String {
    let mut key = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(0);
        key.push_str(&format!("{:016x}", hasher.finish()));
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_are_distinct_and_write_jupyters_shape() -> Result<(), std::io::Error> {
        let a = local_connection()?;
        let b = local_connection()?;
        assert_ne!(a.key, b.key);
        assert_eq!(a.key.len(), 32);
        assert_eq!(a.signature_scheme, "hmac-sha256");
        let path = write_connection_file(&a)?;
        let text = std::fs::read_to_string(&path)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        assert_eq!(value.get("transport"), Some(&serde_json::json!("tcp")));
        assert_eq!(
            value.get("shell_port").and_then(serde_json::Value::as_u64),
            Some(u64::from(a.shell_port))
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn argv_substitution_replaces_every_occurrence() {
        let argv = vec![
            "python".to_owned(),
            "-f".to_owned(),
            "{connection_file}".to_owned(),
            "--log={connection_file}.log".to_owned(),
        ];
        assert_eq!(
            substitute_argv(&argv, "/tmp/k.json"),
            ["python", "-f", "/tmp/k.json", "--log=/tmp/k.json.log"]
        );
    }
}

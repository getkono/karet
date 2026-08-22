//! Connection files: the five loopback ports and hmac key a kernel is told
//! about at spawn, in Jupyter's on-disk JSON shape.

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

/// A 32-hex-char signing key, 128 bits straight from the OS CSPRNG.
///
/// This key is what stops another local process injecting an
/// `execute_request` into the kernel, so it is taken from `getrandom` rather
/// than from `std`'s `RandomState`: that seeds once per *thread* and then
/// increments a counter, so every key in a process would derive from one seed
/// through SipHash-1-3 — a reduced-round construction the standard library
/// documents as not cryptographically secure and reserves the right to
/// change. `getrandom` is pure Rust and already in this crate's tree through
/// `zeromq`, so the dependency footprint is unchanged.
fn random_key() -> String {
    let mut bytes = [0u8; 16];
    // A failure here means the OS has no entropy source; there is no weaker
    // fallback worth having, so surface it as an unusable connection.
    if getrandom::fill(&mut bytes).is_err() {
        return String::new();
    }
    bytes
        .iter()
        .fold(String::with_capacity(32), |mut key, byte| {
            use std::fmt::Write as _;
            let _ = write!(key, "{byte:02x}");
            key
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_key_is_full_length_hex_and_never_repeats() {
        // The key is the only thing standing between a local process and an
        // `execute_request` on the kernel, so it must be full-width, real hex,
        // and never a constant.
        let keys: Vec<String> = (0..64).map(|_| random_key()).collect();
        for key in &keys {
            assert_eq!(key.len(), 32, "expected 128 bits of hex: {key:?}");
            assert!(
                key.bytes().all(|b| b.is_ascii_hexdigit()),
                "not hex: {key:?}"
            );
            assert!(key.bytes().any(|b| b != b'0'), "all-zero key: {key:?}");
        }
        let distinct: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!(distinct.len(), keys.len(), "a signing key repeated");
    }

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

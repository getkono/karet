//! Connection files: the five loopback ports and hmac key a kernel is told
//! about at spawn, in Jupyter's on-disk JSON shape.

use std::path::PathBuf;

use jupyter_protocol::ConnectionInfo;

/// Where one kernel listens and the key that signs its messages.
///
/// Opaque on purpose: the ports and the hmac key are what a kernel process is
/// told about through [`write_connection_file`], and what
/// [`ZmqTransport::connect`](super::ZmqTransport::connect) and
/// [`KernelControl::connect`](super::KernelControl::connect) dial. Nothing
/// else needs to read them, and the key in particular never leaves this crate
/// except into that owner-only file (its `Debug` output redacts it).
#[derive(Clone)]
pub struct Connection(ConnectionInfo);

impl Connection {
    /// Wrap a hand-built description (the loopback transport tests bind their
    /// own ports and key).
    #[cfg(test)]
    pub(crate) fn from_info(info: ConnectionInfo) -> Self {
        Self(info)
    }

    /// The wire-level description the transports dial.
    pub(crate) fn info(&self) -> &ConnectionInfo {
        &self.0
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info = &self.0;
        f.debug_struct("Connection")
            .field("ip", &info.ip)
            .field("shell_port", &info.shell_port)
            .field("iopub_port", &info.iopub_port)
            .field("stdin_port", &info.stdin_port)
            .field("control_port", &info.control_port)
            .field("hb_port", &info.hb_port)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Mint a loopback [`Connection`]: five OS-assigned free ports and a
/// fresh signing key (`hmac-sha256`).
///
/// The ports are bound and released, the standard race-tolerant approach —
/// the kernel binds them right after.
///
/// # Errors
/// Fails when the OS has no entropy source for the signing key (an empty
/// key would make the kernel skip message signing, so there is no fallback),
/// or propagates the bind failure (exotic: no loopback at all).
pub fn local_connection() -> std::io::Result<Connection> {
    local_connection_from(getrandom::fill)
}

/// The OS entropy source, injectable so the no-entropy path is testable.
type Fill = fn(&mut [u8]) -> Result<(), getrandom::Error>;

/// [`local_connection`] with its signing key drawn from `fill`.
fn local_connection_from(fill: Fill) -> std::io::Result<Connection> {
    let key = random_hex::<16>(fill)?;
    let port = || -> std::io::Result<u16> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        Ok(listener.local_addr()?.port())
    };
    Ok(Connection(ConnectionInfo {
        ip: "127.0.0.1".to_owned(),
        transport: jupyter_protocol::connection_info::Transport::TCP,
        shell_port: port()?,
        iopub_port: port()?,
        stdin_port: port()?,
        control_port: port()?,
        hb_port: port()?,
        key,
        signature_scheme: "hmac-sha256".to_owned(),
        kernel_name: None,
    }))
}

/// Write `info` as a `karet-kernel-<pid>-<nonce>.json` connection file in the
/// OS temp directory, returning its path (the caller passes it to the
/// kernel's argv and deletes it when the kernel dies).
///
/// The file holds the signing key and the temp directory is usually shared by
/// every local user, so the file is created owner-only (`0o600` on unix) and
/// exclusively: a file or symlink already planted at the path is an error,
/// never something written through. The nonce is its own draw from OS
/// entropy, so the name reveals nothing about the key.
///
/// # Errors
/// Fails when no nonce can be drawn or the path already exists, and
/// propagates serialization and write failures.
pub fn write_connection_file(info: &Connection) -> std::io::Result<PathBuf> {
    let json = serde_json::to_string_pretty(&info.0).map_err(std::io::Error::other)?;
    let nonce = random_hex::<8>(getrandom::fill)?;
    let path =
        std::env::temp_dir().join(format!("karet-kernel-{}-{nonce}.json", std::process::id()));
    create_owner_only(&path, json.as_bytes())?;
    Ok(path)
}

/// Create `path` exclusively (owner-only on unix) and write `bytes` to it,
/// removing the partial file when the write fails.
fn create_owner_only(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if let Err(error) = std::io::Write::write_all(&mut file, bytes) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// Replace every `{connection_file}` occurrence in a kernelspec argv.
#[must_use]
pub fn substitute_argv(argv: &[String], connection_file: &str) -> Vec<String> {
    argv.iter()
        .map(|arg| arg.replace("{connection_file}", connection_file))
        .collect()
}

/// `N` bytes straight from the OS CSPRNG (`fill`) as `2 * N` lowercase hex
/// chars: 16 bytes make the 128-bit signing key, 8 the file-name nonce.
///
/// This key is what stops another local process injecting an
/// `execute_request` into the kernel, so it is taken from `getrandom` rather
/// than from `std`'s `RandomState`: that seeds once per *thread* and then
/// increments a counter, so every key in a process would derive from one seed
/// through SipHash-1-3 — a reduced-round construction the standard library
/// documents as not cryptographically secure and reserves the right to
/// change. `getrandom` is pure Rust and already in this crate's tree through
/// `zeromq`, so the dependency footprint is unchanged.
///
/// # Errors
/// A `fill` failure means the OS has no entropy source; there is no weaker
/// fallback worth having, so it is returned rather than papered over.
fn random_hex<const N: usize>(fill: Fill) -> std::io::Result<String> {
    let mut bytes = [0u8; N];
    fill(&mut bytes).map_err(std::io::Error::other)?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(2 * N), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signing_key_is_full_length_hex_and_never_repeats() -> std::io::Result<()> {
        // The key is the only thing standing between a local process and an
        // `execute_request` on the kernel, so it must be full-width, real hex,
        // and never a constant.
        let keys = (0..64)
            .map(|_| random_hex::<16>(getrandom::fill))
            .collect::<std::io::Result<Vec<String>>>()?;
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
        Ok(())
    }

    #[test]
    fn no_entropy_fails_the_connection_instead_of_minting_an_empty_key() {
        // An empty key makes the kernel skip signing entirely, so a missing
        // entropy source must refuse the connection, not degrade it.
        fn no_entropy(_: &mut [u8]) -> Result<(), getrandom::Error> {
            Err(getrandom::Error::UNSUPPORTED)
        }
        assert!(random_hex::<16>(no_entropy).is_err());
        assert!(local_connection_from(no_entropy).is_err());
    }

    #[test]
    fn the_file_name_is_a_fresh_nonce_that_reveals_nothing_of_the_key() -> std::io::Result<()> {
        let info = local_connection()?;
        // Same info twice: the old key-derived name would collide here (and
        // now `create_new` would refuse it), so distinct paths prove the nonce
        // is its own draw.
        let a = write_connection_file(&info)?;
        let b = write_connection_file(&info)?;
        let names: Vec<String> = [&a, &b]
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
        assert_eq!(names.len(), 2);
        assert_ne!(a, b, "two files shared a name");
        let prefix = format!("karet-kernel-{}-", std::process::id());
        for name in &names {
            let nonce = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".json"))
                .unwrap_or_default();
            assert_eq!(nonce.len(), 16, "unexpected name {name:?}");
            assert!(nonce.bytes().all(|b| b.is_ascii_hexdigit()), "{name:?}");
            for window in info.0.key.as_bytes().windows(8) {
                assert!(
                    !nonce.as_bytes().windows(8).any(|w| w == window),
                    "name {name:?} carries part of the key"
                );
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn the_connection_file_is_owner_only() -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let info = local_connection()?;
        let path = write_connection_file(&info)?;
        let mode = std::fs::metadata(&path)?.permissions().mode();
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
        Ok(())
    }

    /// A fresh scratch directory under the temp dir (this crate carries no
    /// `tempfile` dev-dep); the caller removes it.
    fn scratch_dir() -> std::io::Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!(
            "karet-connection-test-{}",
            random_hex::<8>(getrandom::fill)?
        ));
        std::fs::create_dir(&dir)?;
        Ok(dir)
    }

    #[test]
    fn a_planted_file_is_refused_not_overwritten() -> std::io::Result<()> {
        let dir = scratch_dir()?;
        let path = dir.join("k.json");
        std::fs::write(&path, "planted")?;
        let refused = create_owner_only(&path, b"secret").map_err(|e| e.kind());
        let content = std::fs::read_to_string(&path);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(refused, Err(std::io::ErrorKind::AlreadyExists));
        assert_eq!(content?, "planted");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_is_refused_not_followed() -> std::io::Result<()> {
        let dir = scratch_dir()?;
        let target = dir.join("victim");
        let link = dir.join("k.json");
        std::os::unix::fs::symlink(&target, &link)?;
        let refused = create_owner_only(&link, b"secret").is_err();
        let written_through = target.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(refused);
        assert!(!written_through, "the key was written through the symlink");
        Ok(())
    }

    #[test]
    fn connections_are_distinct_and_write_jupyters_shape() -> Result<(), std::io::Error> {
        let a = local_connection()?;
        let b = local_connection()?;
        assert_ne!(a.0.key, b.0.key);
        assert_eq!(a.0.key.len(), 32);
        assert_eq!(a.0.signature_scheme, "hmac-sha256");
        let path = write_connection_file(&a)?;
        let text = std::fs::read_to_string(&path)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(std::io::Error::other)?;
        assert_eq!(value.get("transport"), Some(&serde_json::json!("tcp")));
        assert_eq!(
            value.get("shell_port").and_then(serde_json::Value::as_u64),
            Some(u64::from(a.0.shell_port))
        );
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    /// The signing key never shows up in a connection's debug output.
    #[test]
    fn debug_output_redacts_the_signing_key() -> std::io::Result<()> {
        let connection = local_connection()?;
        let shown = format!("{connection:?}");
        assert!(!shown.contains(&connection.0.key), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(
            shown.contains(&connection.0.shell_port.to_string()),
            "{shown}"
        );
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

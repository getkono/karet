//! Asking a terminal multiplexer to host the client half.
//!
//! When karet runs in a [kmux](https://github.com/getkono/kmux) pane, the pane's
//! program is on the workspace host and the person looking at it may be somewhere
//! else entirely. kmux normally forwards that pane's screen. A *split-app pane*
//! forwards a data channel instead and runs `karet --client` on the machine with
//! the display, so the editor redraws at local speed.
//!
//! # Talking to kmux without linking kmux
//!
//! Everything here goes through the `kmux` command-line tool and the documented
//! `KMUX_*` environment variables. That is deliberate and not negotiable: kmux is
//! `AGPL-3.0-only OR LicenseRef-Commercial` with every crate unpublished, karet is
//! `MIT OR Apache-2.0`, and `xtask publish-closure` gates releases on the
//! difference. Shelling out also decouples the two release cadences — kmux pins an
//! exact protocol version between its own halves, and karet should not inherit
//! that.
//!
//! # Degrading
//!
//! Every failure here means "run locally": no kmux, an older kmux with no
//! split-app support, a kmux that declined, a malformed answer. None of them are
//! errors a user should have to read about, because the fallback is exactly the
//! editor they would have got anyway.
//!
//! The kmux side of this is tracked at
//! <https://github.com/getkono/kmux/issues/201>; until it ships, `request_split`
//! finds no support and karet runs locally.

use std::path::Path;
use std::process::Command;
use std::process::Stdio;

/// The environment variable kmux exports into every pane it spawns.
const PANE: &str = "KMUX_PANE";

/// The protocol identifier karet declares. kmux does not interpret it; it is
/// echoed to the client half so the two ends can refuse a mismatch themselves.
const PROTOCOL: &str = "karet/1";

/// How long to wait for kmux to answer before giving up and running locally.
///
/// Short on purpose: this is on the startup path, and the fallback is a perfectly
/// good editor. Waiting seconds to find out a multiplexer cannot help would be a
/// worse outcome than not asking.
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// A forwarded channel to a client the multiplexer is hosting.
pub(crate) struct Channel {
    /// The socket kmux is forwarding, to be connected by the backend half.
    pub(crate) endpoint: std::path::PathBuf,
}

/// Ask kmux to host `karet --client` on the user's machine and forward a channel.
///
/// `None` whenever a split is not available, which is the common case today and
/// always a fine one.
pub(crate) fn request_split(root: &Path) -> Option<Channel> {
    let pane = std::env::var(PANE).ok()?;
    if !supports_split_app() {
        tracing::debug!("kmux is present but has no split-app support; running locally");
        return None;
    }
    let endpoint = run_kmux(&[
        "split-app",
        "--pane",
        &pane,
        "--protocol",
        PROTOCOL,
        "--",
        "karet",
        "--client",
        &root.display().to_string(),
    ])?;
    let endpoint = std::path::PathBuf::from(endpoint.trim());
    if endpoint.as_os_str().is_empty() {
        return None;
    }
    Some(Channel { endpoint })
}

/// Whether the `kmux` on `PATH` understands split-app panes.
///
/// Asked by inspecting its help rather than its version: a version comparison
/// would need updating the moment kmux renumbers, while the presence of the
/// subcommand is the fact actually being tested.
fn supports_split_app() -> bool {
    run_kmux(&["help"]).is_some_and(|help| help.contains("split-app"))
}

/// Run `kmux` with `args`, returning its stdout when it succeeds.
///
/// Any failure — missing binary, non-zero exit, timeout — is `None`. A
/// multiplexer that cannot answer is indistinguishable from one that is not
/// there, and both mean the same thing here.
fn run_kmux(args: &[&str]) -> Option<String> {
    let mut child = Command::new("kmux")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            },
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            },
            // A multiplexer that will not answer promptly is one we run without.
            Ok(None) => {
                let _ = child.kill();
                return None;
            },
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    String::from_utf8(output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outside a pane there is no multiplexer to ask, and karet must not spend
    /// startup time discovering that.
    #[test]
    fn no_pane_means_no_split() {
        // SAFETY: single-threaded test, and the variable is read only by this
        // module's own lookups.
        unsafe { std::env::remove_var(PANE) };

        assert!(request_split(Path::new("/tmp")).is_none());
    }

    /// A pane whose multiplexer cannot host a client must still run the editor,
    /// not fail. The fallback is exactly what the user would have got anyway.
    #[test]
    fn a_pane_without_split_support_falls_back_to_local() {
        // SAFETY: single-threaded test; the variable is read only by this module.
        unsafe { std::env::set_var(PANE, "test-pane/0") };

        // No kmux on PATH in the test environment, so support detection fails and
        // the answer must be "run locally" rather than an error.
        let split = request_split(Path::new("/tmp"));

        // SAFETY: as above.
        unsafe { std::env::remove_var(PANE) };
        assert!(split.is_none());
    }
}

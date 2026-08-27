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
//! # Asking only when kmux has said what it implements
//!
//! The split is offered only to a kmux that *declares* a split-app contract this
//! karet speaks, by exporting [`SPLIT_APP`] with a revision matching
//! [`SPLIT_APP_REVISION`]. Nothing infers support — not from a version number,
//! and not from what `kmux help` happens to print, which would engage a guessed
//! calling convention against whatever ships and fail in a way the fallback is
//! designed never to be: visible.
//!
//! So this is **inert until kmux#201 lands**. No kmux exports the variable today,
//! so nothing here runs, nothing is spawned on the startup path, and karet runs
//! locally. When the contract is settled, kmux declares its revision and the two
//! sides agree on a number rather than on a guess.
//!
//! # Degrading
//!
//! Every failure here means "run locally": no kmux, a kmux that declares nothing
//! or declares a revision this build does not speak, one that declined, a
//! malformed answer. None of them are errors a user should have to read about,
//! because the fallback is exactly the editor they would have got anyway.
//!
//! The kmux side of this is tracked at
//! <https://github.com/getkono/kmux/issues/201>.

use std::path::Path;
use std::process::Command;
use std::process::Stdio;

/// The environment variable kmux exports into every pane it spawns.
const PANE: &str = "KMUX_PANE";

/// The environment variable through which kmux declares which revision of the
/// split-app contract it implements.
const SPLIT_APP: &str = "KMUX_SPLIT_APP";

/// The split-app contract revision this karet speaks.
///
/// An exact match, not a floor: the contract covers how the client half is
/// spawned and how it is handed its endpoint, and a kmux implementing a
/// different revision of that is one to run locally beside, not to guess at.
const SPLIT_APP_REVISION: u32 = 1;

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
    if !declares_split_app(std::env::var(SPLIT_APP).ok().as_deref()) {
        tracing::debug!("kmux declares no split-app contract this build speaks; running locally");
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

/// Whether `declared` names a split-app contract revision this build speaks.
///
/// A parameter rather than an ambient lookup so the decision is testable without
/// mutating the process environment — which `#[test]` functions share.
fn declares_split_app(declared: Option<&str>) -> bool {
    declared.is_some_and(|revision| {
        revision
            .trim()
            .parse::<u32>()
            .is_ok_and(|revision| revision == SPLIT_APP_REVISION)
    })
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

    /// The state of the world today, and the one that matters most: no kmux has
    /// declared a split-app contract, so karet must not act on a guess about how
    /// one would be spawned. Running locally is exactly the editor the user would
    /// have had.
    #[test]
    fn a_multiplexer_that_declares_nothing_gets_no_split() {
        assert!(!declares_split_app(None));
        assert!(!declares_split_app(Some("")));
    }

    /// A revision this build does not implement is a kmux to run beside, not to
    /// negotiate with: the contract covers how the client half is spawned, and
    /// half-speaking it would fail after the editor had already given up its
    /// local session.
    #[test]
    fn a_contract_revision_this_build_does_not_speak_gets_no_split() {
        assert!(!declares_split_app(Some(
            &(SPLIT_APP_REVISION + 1).to_string()
        )));
        assert!(!declares_split_app(Some("yes")));
        assert!(!declares_split_app(Some("1.0")));
    }

    /// The matching revision, including the whitespace a shell export picks up.
    #[test]
    fn the_declared_revision_this_build_speaks_is_accepted() {
        let declared = SPLIT_APP_REVISION.to_string();

        assert!(declares_split_app(Some(&declared)));
        assert!(declares_split_app(Some(&format!(" {declared}\n"))));
    }

    /// Outside a pane there is no multiplexer to ask, and karet must not spend
    /// startup time discovering that. Read through the real environment because
    /// that lookup — not the revision check — is the first gate.
    #[test]
    fn no_pane_means_no_split() {
        if std::env::var_os(PANE).is_some() {
            return; // running inside a real kmux pane; nothing to assert
        }

        assert!(request_split(Path::new("/tmp")).is_none());
    }
}

//! Split sessions: running the editor and its workspace on different machines.
//!
//! karet is normally one process. It can also be two — a backend holding the
//! documents, git, language servers and the files themselves, and a client that
//! renders. The point is latency. A terminal multiplexer forwarding a remote
//! pane's *screen* makes every keystroke wait for a round trip; forwarding the
//! *session* instead lets the editor redraw locally and reconcile as the backend
//! answers.
//!
//! # How a split starts
//!
//! Three ways, all landing in the same two halves:
//!
//! - **Automatically, inside a multiplexer that supports it.** `karet` in a pane
//!   asks the multiplexer to host a client on the machine the user is looking at
//!   and to forward a channel back. Nothing to configure; see [`kmux`].
//! - **By hand**, with `--serve` and `--client` joined by anything that connects
//!   two pipes — `ssh host 'karet --serve repo' | karet --client`.
//! - **Not at all.** Outside a multiplexer, against one too old to help, or with
//!   `--no-split`, karet runs as a single local process exactly as it always has.
//!
//! That last case is the important one: every path here degrades to today's
//! behaviour rather than failing. A split is an optimization, never a requirement.

use std::path::Path;
use std::path::PathBuf;

pub(crate) mod client;
pub(crate) mod kmux;
pub(crate) mod serve;

/// How this process should run.
pub(crate) enum Mode {
    /// One process: the editor and its workspace on this machine.
    Local,
    /// The backend half, speaking the protocol on stdin/stdout.
    Serve,
    /// The rendering half, reaching a backend over a Unix socket.
    ClientSocket(PathBuf),
    /// The rendering half, reaching a backend through a command it runs.
    ClientExec(String),
    /// The backend half, speaking the protocol over a channel the multiplexer
    /// forwards to a client it hosts on the user's machine.
    Forwarded(kmux::Channel),
}

/// The session configuration a backend serves `root` with.
///
/// Deliberately the same constructor the local shell uses: a workspace served to a
/// remote client and one edited in place must behave identically, and the way to
/// guarantee that is to configure them from one place.
pub(crate) fn serve_config(
    root: &std::path::Path,
    loaded_config: &karet_session::config::LoadedConfig,
    syntax: bool,
) -> karet_session::session::SessionConfig {
    crate::app::runtime::session_config_for(
        root.to_path_buf(),
        loaded_config.clone(),
        syntax,
        // A served session persists crash-recovery swaps like any other: the
        // buffers at risk are on this machine, and a client that vanishes is
        // exactly the case backups exist for.
        karet_session::backup::default_swap_dir(),
    )
}

/// Decide how to run, given the flags and the surrounding environment.
///
/// Explicit flags win, then a multiplexer that can host a client, then local.
pub(crate) fn resolve(cli: &crate::cli::Cli, root: &Path) -> Mode {
    if cli.serve {
        return Mode::Serve;
    }
    if let Some(socket) = &cli.client {
        return Mode::ClientSocket(socket.clone());
    }
    if let Some(command) = &cli.client_exec {
        return Mode::ClientExec(command.clone());
    }
    if cli.no_split {
        return Mode::Local;
    }
    // A capture or a one-shot query has no user watching a screen, so there is
    // nothing for a split to make faster.
    if cli.capture || cli.doctor || cli.seam_query.is_some() {
        return Mode::Local;
    }
    match kmux::request_split(root) {
        Some(channel) => Mode::Forwarded(channel),
        None => Mode::Local,
    }
}

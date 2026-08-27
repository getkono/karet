//! The rendering half: draw a workspace that lives somewhere else.
//!
//! Nothing about the editor changes here. The client builds the same
//! [`App`](crate::app::App) the local shell does and hands it a
//! [`Backend`](karet_session::backend::Backend) that happens to reach across a
//! connection — the seam the composition root was written against from the start.

use std::sync::Arc;

use color_eyre::eyre::eyre;
use karet_session::backend::Backend;

use crate::app;

/// How to reach the backend.
pub(crate) enum Endpoint {
    /// A Unix socket, typically one a terminal multiplexer forwards.
    Socket(std::path::PathBuf),
    /// A command whose stdin and stdout carry the protocol.
    ///
    /// The command supplies the connection and karet supplies none: `ssh`,
    /// `podman exec`, a wrapper script — anything that forwards two pipes.
    Exec(String),
}

/// Build the shell's backend source for `endpoint`.
///
/// The connection is opened inside the shell's runtime rather than here, because
/// the backend and the task pumping it are tied to the runtime that spawned them.
pub(crate) fn source(endpoint: Endpoint) -> app::runtime::Source {
    app::runtime::Source::Remote(Box::new(move || Box::pin(open(endpoint))))
}

async fn open(
    endpoint: Endpoint,
) -> color_eyre::Result<(Arc<dyn Backend>, karet_session::local::SnapshotRx)> {
    match endpoint {
        Endpoint::Socket(path) => {
            let stream = tokio::net::UnixStream::connect(&path)
                .await
                .map_err(|error| {
                    eyre!(
                        "connecting to the karet backend at {}: {error}",
                        path.display()
                    )
                })?;
            let (reader, writer) = tokio::io::split(stream);
            connect(tokio::io::BufReader::new(reader), writer).await
        },
        Endpoint::Exec(command) => {
            let mut child = spawn(&command)?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| eyre!("the backend command produced no stdout"))?;
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| eyre!("the backend command accepted no stdin"))?;
            // The child must outlive this call but die with the editor, so it is
            // parked on a task that holds it until the process ends. Forgetting it
            // would leak the handle and defeat `kill_on_drop`, leaving an orphaned
            // `ssh` behind every session.
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
            connect(tokio::io::BufReader::new(stdout), stdin).await
        },
    }
}

/// Run `command` through the platform shell, with its stdio piped.
///
/// A shell rather than a bare argv because the value is a command line a person
/// typed — `ssh dev-box karet --serve /srv/repo` has quoting in it, and reproducing
/// the shell's own splitting rules badly is worse than using the shell.
fn spawn(command: &str) -> color_eyre::Result<tokio::process::Child> {
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    tokio::process::Command::new(program)
        .arg(flag)
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // Let the command's diagnostics reach the user's terminal: an `ssh`
        // asking for a passphrase, or refusing a host key, must be visible.
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| eyre!("starting the backend command: {error}"))
}

async fn connect<R, W>(
    reader: R,
    writer: W,
) -> color_eyre::Result<(Arc<dyn Backend>, karet_session::local::SnapshotRx)>
where
    R: tokio::io::AsyncBufRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // A fresh client has seen nothing, so it resumes from sequence zero and the
    // backend describes every open document from scratch.
    let (backend, snapshots) = karet_session::remote::connect(reader, writer, 0)
        .await
        .map_err(|error| eyre!("karet backend handshake: {error}"))?;
    Ok((Arc::new(backend), snapshots))
}

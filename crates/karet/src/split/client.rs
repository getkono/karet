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
) -> color_eyre::Result<(Arc<dyn Backend>, karet_session::SnapshotRx)> {
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
            let stderr = child.stderr.take().map(capture_stderr);
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
            connect(tokio::io::BufReader::new(stdout), stdin)
                .await
                .map_err(|error| match stderr.map(|tail| tail.text()) {
                    Some(said) if !said.is_empty() => {
                        eyre!("{error}\nthe backend command said:\n{said}")
                    },
                    _ => error,
                })
        },
    }
}

/// The most recent lines a backend command wrote to stderr.
#[derive(Clone, Default)]
struct StderrTail(Arc<std::sync::Mutex<std::collections::VecDeque<String>>>);

impl StderrTail {
    /// How many lines are kept: enough for an `ssh` refusal and its reason.
    const LINES: usize = 20;

    fn push(&self, line: String) {
        let mut lines = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lines.len() == Self::LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    fn text(&self) -> String {
        let lines = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lines.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}

/// Read the command's stderr off the screen: the editor owns the terminal in
/// raw mode, so anything written there would paint over the frame. Each line
/// goes to the log, and the last few are kept for a handshake failure's message,
/// which is printed once the terminal is restored.
fn capture_stderr(stderr: tokio::process::ChildStderr) -> StderrTail {
    use tokio::io::AsyncBufReadExt;

    let tail = StderrTail::default();
    let sink = tail.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!(target: "karet::split", "backend command: {line}");
            sink.push(line);
        }
    });
    tail
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
        // Captured rather than inherited: the terminal is in raw mode under the
        // editor's frame by now. An `ssh` refusing a host key is still reported,
        // in the handshake error printed once the terminal is restored.
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| eyre!("starting the backend command: {error}"))
}

async fn connect<R, W>(
    reader: R,
    writer: W,
) -> color_eyre::Result<(Arc<dyn Backend>, karet_session::SnapshotRx)>
where
    R: tokio::io::AsyncBufRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    // A fresh client has seen nothing, so it resumes from sequence zero and the
    // backend describes every open document from scratch. Bounded: a backend
    // that never greets would otherwise leave a blank screen nothing can quit,
    // because input is not read until the handshake returns.
    let (backend, snapshots) = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        karet_session::remote::connect(reader, writer, 0),
    )
    .await
    .map_err(|_| {
        eyre!(
            "karet backend handshake: no greeting within {}s",
            HANDSHAKE_TIMEOUT.as_secs()
        )
    })?
    .map_err(|error| eyre!("karet backend handshake: {error}"))?;
    Ok((Arc::new(backend), snapshots))
}

/// How long a backend has to greet before the client gives up on it. Long
/// enough for an `ssh` connection to a slow host to come up and start karet.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

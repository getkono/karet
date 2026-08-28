//! Crash-safe ownership for long-running external process trees.
//!
//! karet never launches an LSP server directly. The application starts a hidden
//! copy of itself in supervisor mode and connects the LSP transport to that
//! process's standard input and output. The supervisor is the sole owner of the
//! real server process group (a Job Object on Windows). If the application exits
//! without running destructors, the supervisor observes EOF on its input, kills
//! the whole group, waits for it, and exits.
//!
//! This two-process arrangement is deliberate: `Child::kill_on_drop` covers Rust
//! unwinding and task cancellation, but no destructor runs after `SIGKILL`,
//! `abort`, or an equivalent forced termination.

use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use command_group::CommandGroup;
use serde::Deserialize;
use serde::Serialize;

/// Environment flag selecting the hidden supervisor entry point.
pub const MODE_ENV: &str = "KARET_INTERNAL_PROCESS_SUPERVISOR";
const SPEC_ENV: &str = "KARET_INTERNAL_PROCESS_SPEC";

/// Errors produced while preparing or running a supervised process.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SupervisorError {
    /// The launch description could not be serialized or decoded.
    #[error("invalid supervisor launch description: {0}")]
    Spec(String),
    /// The external process or its process group could not be managed.
    #[error("supervised process failed: {0}")]
    Io(String),
}

#[derive(Debug, Deserialize, Serialize)]
struct SupervisorSpec {
    command: String,
    args: Vec<String>,
    current_dir: PathBuf,
}

/// Whether this invocation is the hidden process-supervisor child.
#[must_use]
pub fn requested() -> bool {
    std::env::var_os(MODE_ENV).is_some()
}

/// Build a command that starts `supervisor` in hidden mode and owns `command`.
///
/// The returned process speaks the owned child's protocol on stdin/stdout. Its
/// stderr is the child's drained stderr stream. No shell parses either command
/// or argument.
///
/// # Errors
/// Returns [`SupervisorError::Spec`] when the launch description cannot be
/// serialized.
pub fn command(
    supervisor: &Path,
    command: String,
    args: Vec<String>,
    current_dir: &Path,
) -> Result<tokio::process::Command, SupervisorError> {
    let spec = SupervisorSpec {
        command,
        args,
        current_dir: current_dir.to_path_buf(),
    };
    let encoded =
        serde_json::to_string(&spec).map_err(|error| SupervisorError::Spec(error.to_string()))?;
    let mut child = tokio::process::Command::new(supervisor);
    child
        .env(MODE_ENV, "1")
        .env(SPEC_ENV, encoded)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(child)
}

/// Build the blocking equivalent of [`command`] for supervised helper tools.
///
/// Callers must keep the returned child's piped stdin open until it exits; that
/// pipe is the lifetime lease whose EOF triggers process-tree cleanup.
///
/// # Errors
/// Returns [`SupervisorError::Spec`] when the launch description cannot be
/// serialized.
pub fn blocking_command(
    supervisor: &Path,
    command: String,
    args: Vec<String>,
    current_dir: &Path,
) -> Result<std::process::Command, SupervisorError> {
    let spec = SupervisorSpec {
        command,
        args,
        current_dir: current_dir.to_path_buf(),
    };
    let encoded =
        serde_json::to_string(&spec).map_err(|error| SupervisorError::Spec(error.to_string()))?;
    let mut child = std::process::Command::new(supervisor);
    child
        .env(MODE_ENV, "1")
        .env(SPEC_ENV, encoded)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(child)
}

/// Run the hidden supervisor until its child exits or the parent connection dies.
///
/// This must be called before normal argument parsing and before the TUI starts.
/// It returns an exit code suitable for `std::process::exit`.
#[must_use]
pub fn run_from_env() -> i32 {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("karet process supervisor: {error}");
            1
        },
    }
}

fn run() -> Result<i32, SupervisorError> {
    let encoded = std::env::var(SPEC_ENV)
        .map_err(|error| SupervisorError::Spec(format!("missing launch description: {error}")))?;
    // Remove the description before spawning the real process so descendants do
    // not inherit either the hidden-mode flag or another executable's argv.
    unsafe {
        // SAFETY: supervisor mode is single-threaded here, before any worker is
        // created, so mutating the process environment cannot race another thread.
        std::env::remove_var(MODE_ENV);
        std::env::remove_var(SPEC_ENV);
    }
    let spec: SupervisorSpec =
        serde_json::from_str(&encoded).map_err(|error| SupervisorError::Spec(error.to_string()))?;
    supervise(spec, io::stdin(), io::stdout(), io::stderr())
}

/// Forward everything from `from` to `to`, flushing after each chunk.
///
/// Deliberately not [`io::copy`]: the destination here is the supervisor's own
/// stdout, and Rust wraps that in a `LineWriter`. The protocols carried across
/// this pipe — LSP and DAP — frame messages as a `Content-Length` header
/// followed by a raw JSON body with **no trailing newline**, so a `LineWriter`
/// writes the header (which ends `\r\n\r\n`) and holds the body back until some
/// later message happens to supply a newline. That delivers every message one
/// message late: the DAP handshake waits for an `initialized` event that is
/// already sitting in the buffer, and a quiet language server's reply does not
/// arrive until it says something else. Flushing each chunk keeps the pipe
/// honest.
fn pump(from: &mut impl Read, to: &mut impl Write) {
    let mut buf = [0u8; 8192];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buf[..read]).is_err() || to.flush().is_err() {
                    return;
                }
            },
        }
    }
}

fn supervise(
    spec: SupervisorSpec,
    mut parent_input: impl Read + Send + 'static,
    mut parent_output: impl Write + Send + 'static,
    mut parent_error: impl Write + Send + 'static,
) -> Result<i32, SupervisorError> {
    let mut command = std::process::Command::new(&spec.command);
    command
        .args(&spec.args)
        .current_dir(&spec.current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut group = command
        .group()
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| SupervisorError::Io(error.to_string()))?;
    let mut child_stdin = group
        .inner()
        .stdin
        .take()
        .ok_or_else(|| SupervisorError::Io("child stdin was unavailable".to_owned()))?;
    let mut child_stdout = group
        .inner()
        .stdout
        .take()
        .ok_or_else(|| SupervisorError::Io("child stdout was unavailable".to_owned()))?;
    let mut child_stderr = group
        .inner()
        .stderr
        .take()
        .ok_or_else(|| SupervisorError::Io("child stderr was unavailable".to_owned()))?;
    let (parent_gone_tx, parent_gone_rx) = mpsc::sync_channel(1);

    std::thread::spawn(move || {
        let result = io::copy(&mut parent_input, &mut child_stdin);
        drop(child_stdin);
        let _ = parent_gone_tx.send(result.map(|_| ()));
    });
    // Bounded rather than joined. The pumps end on their own when the child's
    // pipes reach EOF, but a grandchild that inherited them holds the write end
    // open after the child is reaped, and an unbounded join would hang the
    // supervisor forever on exactly the process trees it exists to outlive.
    let (pumps_done_tx, pumps_done_rx) = mpsc::sync_channel(2);
    let stdout_done = pumps_done_tx.clone();
    std::thread::spawn(move || {
        pump(&mut child_stdout, &mut parent_output);
        let _ = stdout_done.send(());
    });
    std::thread::spawn(move || {
        pump(&mut child_stderr, &mut parent_error);
        let _ = pumps_done_tx.send(());
    });

    loop {
        // This fires when the copy into the child's stdin ends, which is not
        // only the parent going away: a child that exits at once breaks that
        // pipe itself, so a failed launch reaches this arm too and must report
        // what the child said and how it ended. Killing an already-dead group
        // is harmless, and its `wait` still yields the child's own status.
        if parent_gone_rx.try_recv().is_ok() {
            let _ = group.kill();
            let status = group.wait().ok().and_then(|status| status.code());
            drain_pumps(&pumps_done_rx);
            return Ok(status.unwrap_or(0));
        }
        match group
            .try_wait()
            .map_err(|error| SupervisorError::Io(error.to_string()))?
        {
            // Let the pumps finish before returning: the caller exits the
            // process on this value, and a child that failed on startup says
            // why on its way out. Returning the moment `try_wait` reports the
            // child gone discarded those last words perhaps a quarter of the
            // time, leaving a launch failure with an exit code and no reason.
            Some(status) => {
                drain_pumps(&pumps_done_rx);
                return Ok(status.code().unwrap_or(1));
            },
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// How long the output pumps get to drain once the child has been reaped.
///
/// Generous for the work involved -- the child is gone, so each pump has at
/// most one pipe buffer left to forward -- and bounded so a surviving
/// grandchild holding the write end costs a fixed delay rather than a hang.
const PUMP_DRAIN_GRACE: Duration = Duration::from_millis(500);

/// Wait for both output pumps to report they are done, up to a shared deadline.
///
/// Gives up quietly on timeout: losing a trailing line is a worse report, while
/// blocking here would be a stuck editor.
fn drain_pumps(done: &mpsc::Receiver<()>) {
    let deadline = std::time::Instant::now() + PUMP_DRAIN_GRACE;
    for _ in 0..2 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if done.recv_timeout(remaining).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_command_carries_hidden_mode_without_a_shell() -> Result<(), SupervisorError> {
        let command = command(
            Path::new("/tmp/karet"),
            "texlab".to_owned(),
            vec!["--version".to_owned()],
            Path::new("/tmp/work"),
        )?;
        let command = command.as_std();
        assert_eq!(command.get_program(), "/tmp/karet");
        assert!(command.get_args().next().is_none());
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == MODE_ENV && value.is_some())
        );
        Ok(())
    }

    #[cfg(unix)]
    /// A reader that holds the parent's lease open for `for_` before reporting
    /// EOF. `io::empty()` would report it at once, and EOF is what tells the
    /// supervisor to kill the child.
    struct HoldOpen {
        for_: Duration,
    }

    impl Read for HoldOpen {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(self.for_);
            Ok(0)
        }
    }

    /// A writer that only reveals bytes once they are flushed, standing in for
    /// the `LineWriter` Rust wraps the real stdout in.
    #[derive(Clone, Default)]
    struct FlushedOnly {
        pending: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        visible: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl Write for FlushedOnly {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Ok(mut pending) = self.pending.lock() {
                pending.extend_from_slice(buf);
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let (Ok(mut pending), Ok(mut visible)) = (self.pending.lock(), self.visible.lock())
            else {
                return Ok(());
            };
            visible.append(&mut pending);
            Ok(())
        }
    }

    #[test]
    fn a_message_without_a_trailing_newline_still_reaches_the_parent() -> Result<(), SupervisorError>
    {
        // LSP and DAP frame a `Content-Length` header — which ends in a
        // newline — followed by a JSON body that does not. Forwarding with
        // `io::copy` left that body in the real stdout's `LineWriter` until a
        // later message supplied a newline, delivering every message one
        // message late and stalling the DAP handshake outright.
        let out = FlushedOnly::default();
        // The header carries the only newlines; the body must arrive anyway.
        // The trailing sleep keeps the child alive long enough for the
        // forwarding thread to run, so the assertion is not a race.
        let script = concat!(
            r"printf 'Content-Length: 17

'; ",
            r#"printf '%s' '{"seq":1,"a":"b"}'; "#,
            "sleep 0.2"
        );
        let code = supervise(
            SupervisorSpec {
                command: "sh".into(),
                args: vec!["-c".into(), script.to_owned()],
                current_dir: std::env::temp_dir(),
            },
            HoldOpen {
                for_: Duration::from_secs(3),
            },
            out.clone(),
            io::sink(),
        )?;
        assert_eq!(code, 0);

        // The forwarding threads outlive `supervise`, so give the last chunk a
        // moment rather than racing it.
        let mut text = String::new();
        for _ in 0..200 {
            let visible = out.visible.lock().map(|v| v.clone()).unwrap_or_default();
            text = String::from_utf8_lossy(&visible).into_owned();
            if text.ends_with(r#"{"seq":1,"a":"b"}"#) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            text.ends_with(r#"{"seq":1,"a":"b"}"#),
            "the newline-free body never made it across: {text:?}"
        );
        Ok(())
    }

    #[test]
    fn parent_eof_terminates_a_running_process_group() -> Result<(), SupervisorError> {
        let started = std::time::Instant::now();
        let code = supervise(
            SupervisorSpec {
                command: "sh".into(),
                args: vec!["-c".into(), "sleep 30 & wait".into()],
                current_dir: std::env::temp_dir(),
            },
            io::empty(),
            io::sink(),
            io::sink(),
        )?;
        assert_eq!(code, 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the owned process group survived its parent's lease"
        );
        Ok(())
    }
}

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
    std::thread::spawn(move || {
        let _ = io::copy(&mut child_stdout, &mut parent_output);
    });
    std::thread::spawn(move || {
        let _ = io::copy(&mut child_stderr, &mut parent_error);
    });

    loop {
        if parent_gone_rx.try_recv().is_ok() {
            let _ = group.kill();
            let _ = group.wait();
            return Ok(0);
        }
        match group
            .try_wait()
            .map_err(|error| SupervisorError::Io(error.to_string()))?
        {
            Some(status) => return Ok(status.code().unwrap_or(1)),
            None => std::thread::sleep(Duration::from_millis(20)),
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

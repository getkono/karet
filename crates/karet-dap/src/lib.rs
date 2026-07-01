//! `karet-dap` — an async Debug Adapter Protocol client for karet.
//!
//! Headless: drives a debug adapter and exposes debugging state (breakpoints,
//! variables, call stack), emitting breakpoint markers as neutral `karet-core`
//! [`Decoration`]s. (The ratatui debug panels live behind the `view` feature.)
//!
//! This is the implementation *skeleton*: the public joints are defined; the DAP
//! protocol/session logic is filled in separately.

use std::path::Path;

use karet_core::Decoration;
use tokio::sync::broadcast;

/// Errors produced by the DAP client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DapError {
    /// The debug adapter could not be launched.
    #[error("failed to launch debug adapter")]
    Launch,
    /// The adapter responded with an error.
    #[error("debug adapter error: {0}")]
    Adapter(String),
}

/// How to launch a debug adapter.
#[derive(Clone, Debug)]
pub struct DapSpec {
    /// The adapter executable.
    pub command: String,
    /// Command-line arguments.
    pub args: Vec<String>,
}

/// A breakpoint as acknowledged by the adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    /// The 0-based line.
    pub line: u32,
    /// Whether the adapter verified (bound) the breakpoint.
    pub verified: bool,
}

/// One frame of a call stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    /// The frame name (usually a function).
    pub name: String,
    /// The 0-based line the frame is stopped at.
    pub line: u32,
}

/// A variable within a scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    /// The variable name.
    pub name: String,
    /// Its rendered value.
    pub value: String,
}

/// A handle to a variables scope (DAP `variablesReference`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VarRef(pub u64);

/// An event pushed by the debug adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugEvent {
    /// Execution stopped (breakpoint, step, or exception).
    Stopped,
    /// Execution resumed.
    Continued,
    /// The debuggee exited with a status code.
    Exited(i32),
    /// Output was produced on a stream.
    Output(String),
}

/// An async client for a debug adapter.
pub struct DapClient {}

impl DapClient {
    /// Launch the adapter described by `spec`.
    ///
    /// # Errors
    /// Returns [`DapError::Launch`] if the adapter cannot start.
    pub async fn launch(spec: DapSpec) -> Result<Self, DapError> {
        let _ = spec;
        todo!()
    }

    /// Set breakpoints for `path` at the given `lines`.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn set_breakpoints(
        &self,
        path: &Path,
        lines: &[u32],
    ) -> Result<Vec<Breakpoint>, DapError> {
        let _ = (path, lines);
        todo!()
    }

    /// Breakpoint and current-line markers as editor decorations.
    #[must_use]
    pub fn decorations(&self) -> Vec<Decoration> {
        todo!()
    }

    /// The current call stack.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn stack_trace(&self) -> Result<Vec<StackFrame>, DapError> {
        todo!()
    }

    /// The variables in `scope`.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn variables(&self, scope: VarRef) -> Result<Vec<Variable>, DapError> {
        let _ = scope;
        todo!()
    }

    /// Subscribe to adapter events (stopped/continued/output/exited).
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<DebugEvent> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoint_equality() {
        assert_eq!(
            Breakpoint {
                line: 3,
                verified: true
            },
            Breakpoint {
                line: 3,
                verified: true
            }
        );
        assert_eq!(DebugEvent::Exited(0), DebugEvent::Exited(0));
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            DapError::Launch.to_string(),
            "failed to launch debug adapter"
        );
    }
}

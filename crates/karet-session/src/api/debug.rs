//! Debugger vocabulary shared by `Debug*` commands and events.

/// One acknowledged breakpoint of a file (see [`Event::DebugBreakpoints`]).
///
/// [`Event::DebugBreakpoints`]: super::Event::DebugBreakpoints
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugBreakpoint {
    /// The 0-based line the breakpoint bound to.
    pub line: u32,
    /// Whether the adapter verified (bound) it. Unverified breakpoints may
    /// still verify later; a fresh event follows.
    pub verified: bool,
}

/// The debug session's lifecycle, for chrome (status segment, key gating).
/// `#[non_exhaustive]`: richer states (e.g. attaching) may follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum DebugSessionState {
    /// No session.
    #[default]
    Idle,
    /// The adapter is launching/configuring.
    Starting,
    /// The debuggee is running.
    Running,
    /// The debuggee is stopped (breakpoint, step, pause, exception).
    Stopped,
}

/// One call-stack frame (see [`Event::DebugStack`](super::Event::DebugStack)).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugFrame {
    /// The frame id `DebugScopes`/`DebugEvaluate` take.
    pub id: i64,
    /// The frame name (usually a function).
    pub name: String,
    /// The 0-based stopped line.
    pub line: u32,
    /// The 0-based column.
    pub column: u32,
    /// The source file, when the adapter reports a path.
    pub path: Option<std::path::PathBuf>,
}

/// One variables scope of a frame (Locals, Registers, …).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugScope {
    /// The scope name.
    pub name: String,
    /// The handle `DebugVariables` takes.
    pub reference: i64,
    /// Whether fetching this scope is expensive (fetched only on expand).
    pub expensive: bool,
}

/// One variable row (see [`Event::DebugVariables`](super::Event::DebugVariables)).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugVariable {
    /// The variable name.
    pub name: String,
    /// Its rendered value.
    pub value: String,
    /// The type, when reported.
    pub ty: Option<String>,
    /// Non-zero when the variable has children (fetch via `DebugVariables`).
    pub reference: i64,
}

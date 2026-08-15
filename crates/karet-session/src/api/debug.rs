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

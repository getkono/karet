//! The Debug Adapter Protocol vocabulary: capabilities, inspection models,
//! and the adapter-pushed [`DebugEvent`] stream.
//!
//! Hand-rolled over serde on the subset karet drives (the `dap` crate is
//! alpha and server-oriented). Wire messages are camelCase and 1-based; the
//! public surface is 0-based like the rest of the toolkit, converted at the
//! client edge.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

/// The adapter capabilities the client gates on, answered by `initialize`.
///
/// Every field defaults to "unsupported": an adapter that omits a capability
/// must not be sent the corresponding request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Capabilities {
    /// Whether `configurationDone` ends the configuration phase.
    pub supports_configuration_done_request: bool,
    /// Whether `disconnect` accepts `terminateDebuggee`.
    pub support_terminate_debuggee: bool,
    /// The exception filters `setExceptionBreakpoints` may name; empty means
    /// the request must not be sent at all.
    pub exception_breakpoint_filters: Vec<ExceptionFilter>,
}

/// One exception-breakpoint filter offered by the adapter.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ExceptionFilter {
    /// The identifier sent back in `setExceptionBreakpoints`.
    pub filter: String,
    /// The human-readable name.
    pub label: String,
    /// Whether the filter should start enabled.
    pub default: bool,
}

/// A breakpoint as acknowledged by the adapter (initially or via a later
/// `breakpoint` event once it binds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    /// The adapter's id for this breakpoint, correlating later change events.
    pub id: Option<i64>,
    /// The 0-based line the breakpoint actually bound to, when known.
    pub line: Option<u32>,
    /// Whether the adapter verified (bound) the breakpoint.
    pub verified: bool,
    /// The adapter's explanation when unverified.
    pub message: Option<String>,
    /// The source file, when the adapter attaches one (late `breakpoint`
    /// change events usually do; `setBreakpoints` answers rarely need to).
    pub source_path: Option<PathBuf>,
}

/// A thread reported by `threads`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thread {
    /// The thread id other requests take.
    pub id: i64,
    /// The thread name.
    pub name: String,
}

/// One frame of a stopped thread's call stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackFrame {
    /// The frame id `scopes`/`evaluate` take.
    pub id: i64,
    /// The frame name (usually a function).
    pub name: String,
    /// The 0-based line the frame is stopped at.
    pub line: u32,
    /// The 0-based column.
    pub column: u32,
    /// The source file, when the adapter reports a path.
    pub source_path: Option<PathBuf>,
}

/// A variables scope of one frame (Locals, Registers, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    /// The scope name.
    pub name: String,
    /// The handle `variables` takes.
    pub variables_reference: i64,
    /// Whether fetching the scope is expensive (fetch lazily).
    pub expensive: bool,
}

/// A variable within a scope or structured parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    /// The variable name.
    pub name: String,
    /// Its rendered value.
    pub value: String,
    /// The type, when the adapter reports one.
    pub ty: Option<String>,
    /// Non-zero when the variable has children fetchable via `variables`.
    pub variables_reference: i64,
}

/// The result of an `evaluate` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    /// The rendered result.
    pub result: String,
    /// Non-zero when the result has children fetchable via `variables`.
    pub variables_reference: i64,
}

/// An event pushed by the debug adapter.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DebugEvent {
    /// The adapter is ready for configuration (breakpoints may be set).
    Initialized,
    /// Execution stopped (breakpoint, step, exception, pause).
    Stopped {
        /// The adapter's reason (`"breakpoint"`, `"step"`, `"exception"`, …).
        reason: String,
        /// The stopped thread, when reported.
        thread_id: Option<i64>,
        /// Whether every thread stopped.
        all_threads_stopped: bool,
    },
    /// Execution resumed.
    Continued {
        /// The resumed thread.
        thread_id: Option<i64>,
    },
    /// The adapter or debuggee produced output.
    Output {
        /// The stream (`"console"`, `"stdout"`, `"stderr"`, …).
        category: String,
        /// The text, ANSI escapes and all.
        output: String,
    },
    /// A breakpoint changed after the fact (typically late verification).
    BreakpointChanged(Breakpoint),
    /// A thread started or exited.
    Thread {
        /// `"started"` or `"exited"`.
        reason: String,
        /// The thread in question.
        thread_id: i64,
    },
    /// The debuggee exited with a status code.
    Exited {
        /// The exit code.
        exit_code: i64,
    },
    /// The debug session ended (the adapter is done).
    Terminated,
}

/// Decode one adapter event; `None` for event kinds karet does not model
/// (they are logged and dropped by the connection).
#[must_use]
pub(crate) fn event_from(name: &str, body: &Value) -> Option<DebugEvent> {
    Some(match name {
        "initialized" => DebugEvent::Initialized,
        "stopped" => DebugEvent::Stopped {
            reason: str_field(body, "reason"),
            thread_id: body.get("threadId").and_then(Value::as_i64),
            all_threads_stopped: body
                .get("allThreadsStopped")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "continued" => DebugEvent::Continued {
            thread_id: body.get("threadId").and_then(Value::as_i64),
        },
        "output" => DebugEvent::Output {
            category: body
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("console")
                .to_owned(),
            output: str_field(body, "output"),
        },
        "breakpoint" => DebugEvent::BreakpointChanged(breakpoint_from(body.get("breakpoint")?)),
        "thread" => DebugEvent::Thread {
            reason: str_field(body, "reason"),
            thread_id: body.get("threadId").and_then(Value::as_i64)?,
        },
        "exited" => DebugEvent::Exited {
            exit_code: body.get("exitCode").and_then(Value::as_i64).unwrap_or(0),
        },
        "terminated" => DebugEvent::Terminated,
        _ => return None,
    })
}

/// Decode one wire breakpoint (1-based line → 0-based).
pub(crate) fn breakpoint_from(value: &Value) -> Breakpoint {
    Breakpoint {
        id: value.get("id").and_then(Value::as_i64),
        line: value
            .get("line")
            .and_then(Value::as_u64)
            .map(|line| u32::try_from(line.saturating_sub(1)).unwrap_or(u32::MAX)),
        verified: value
            .get("verified")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source_path: value
            .get("source")
            .and_then(|source| source.get("path"))
            .and_then(Value::as_str)
            .map(PathBuf::from),
    }
}

/// A `str` field of an event body, empty when absent.
fn str_field(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn capabilities_default_to_unsupported() {
        let caps: Capabilities = serde_json::from_value(json!({})).unwrap_or_default();
        assert!(!caps.supports_configuration_done_request);
        assert!(!caps.support_terminate_debuggee);
        assert!(caps.exception_breakpoint_filters.is_empty());
    }

    #[test]
    fn capabilities_decode_the_gated_fields() {
        let caps: Capabilities = serde_json::from_value(json!({
            "supportsConfigurationDoneRequest": true,
            "supportTerminateDebuggee": true,
            "exceptionBreakpointFilters": [
                {"filter": "cpp_throw", "label": "C++: on throw", "default": true},
                {"filter": "cpp_catch", "label": "C++: on catch"}
            ],
            "supportsStepBack": false
        }))
        .unwrap_or_default();
        assert!(caps.supports_configuration_done_request);
        assert!(caps.support_terminate_debuggee);
        assert_eq!(caps.exception_breakpoint_filters.len(), 2);
        assert!(caps.exception_breakpoint_filters[0].default);
        assert!(!caps.exception_breakpoint_filters[1].default);
    }

    #[test]
    fn stopped_event_decodes() {
        let got = event_from(
            "stopped",
            &json!({"reason": "breakpoint", "threadId": 1, "allThreadsStopped": true}),
        );
        assert_eq!(
            got,
            Some(DebugEvent::Stopped {
                reason: "breakpoint".to_owned(),
                thread_id: Some(1),
                all_threads_stopped: true,
            })
        );
    }

    #[test]
    fn breakpoint_event_converts_to_zero_based() {
        let got = event_from(
            "breakpoint",
            &json!({"reason": "changed", "breakpoint": {"id": 7, "line": 12, "verified": true}}),
        );
        assert_eq!(
            got,
            Some(DebugEvent::BreakpointChanged(Breakpoint {
                id: Some(7),
                line: Some(11),
                verified: true,
                message: None,
                source_path: None,
            }))
        );
    }

    #[test]
    fn unknown_events_are_none() {
        assert_eq!(event_from("customTelemetry", &json!({})), None);
        assert_eq!(event_from("thread", &json!({"reason": "started"})), None);
    }

    #[test]
    fn output_defaults_to_console() {
        let got = event_from("output", &json!({"output": "hi\n"}));
        assert_eq!(
            got,
            Some(DebugEvent::Output {
                category: "console".to_owned(),
                output: "hi\n".to_owned(),
            })
        );
    }
}

//! `karet-dap` — an async Debug Adapter Protocol client for karet.
//!
//! Headless: drives one debug adapter over the Content-Length framing shared
//! with `karet-lsp` ([`karet_lsp::codec`]), exposing typed requests and a
//! broadcast of adapter events. Presentation and orchestration live with the
//! consumer (the karet backend); this crate speaks protocol only.
//!
//! The lifecycle is the DAP-mandated dance, capability-gated throughout:
//!
//! 1. [`DapClient::spawn`] (stdio or spawn-then-TCP) or [`DapClient::connect`];
//! 2. [`DapClient::initialize`] → the adapter's [`Capabilities`];
//! 3. [`DapClient::start`] sends `launch`/`attach` *without awaiting it*,
//!    waits for the `initialized` event, replays breakpoints per file, sets
//!    exception breakpoints (only when the adapter offers filters), sends
//!    `configurationDone` (only when supported), then awaits the original
//!    launch response;
//! 4. run controls / inspection ([`DapClient::stack_trace`],
//!    [`DapClient::variables`], …) between `stopped` and `continued` events;
//! 5. [`DapClient::disconnect`], terminating the debuggee only when the
//!    adapter says it can.
//!
//! Breakpoints may verify *late*: adapters answer `setBreakpoints` eagerly
//! and follow up with `breakpoint` change events once code is loaded —
//! surfaced as [`DebugEvent::BreakpointChanged`].

mod conn;
mod types;

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use serde_json::json;
use tokio::sync::broadcast;

pub use crate::types::Breakpoint;
pub use crate::types::Capabilities;
pub use crate::types::DebugEvent;
pub use crate::types::Evaluation;
pub use crate::types::ExceptionFilter;
pub use crate::types::Scope;
pub use crate::types::StackFrame;
pub use crate::types::Thread;
pub use crate::types::Variable;

/// How long [`DapClient::start`] waits for the `initialized` event before
/// concluding the adapter is stuck.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long spawn-then-TCP retries connecting to the adapter's socket.
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The delay between TCP connection attempts while the adapter boots.
const TCP_RETRY_DELAY: Duration = Duration::from_millis(50);

/// Errors produced by the DAP client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DapError {
    /// The debug adapter could not be launched.
    #[error("failed to launch the debug adapter: {0}")]
    Launch(String),
    /// The adapter rejected a request (a `success: false` response).
    #[error("debug adapter error: {0}")]
    Adapter(String),
    /// The connection is gone; the request cannot be delivered or answered.
    #[error("the debug-adapter connection is closed")]
    Closed,
    /// The adapter did not answer within the deadline.
    #[error("the debug adapter did not respond in time")]
    Timeout,
    /// A message could not be encoded or decoded.
    #[error("debug-adapter protocol error: {0}")]
    Protocol(String),
}

/// How the adapter process exposes its DAP endpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DapTransport {
    /// The protocol flows over the adapter's stdin/stdout.
    #[default]
    Stdio,
    /// The adapter listens on a TCP port passed via a `${port}` argument
    /// (codelldb's `--port ${port}` style); the client picks a free port,
    /// substitutes it, spawns, and connects with retries.
    Tcp,
}

/// How to launch a debug adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DapSpec {
    /// The adapter executable.
    pub command: String,
    /// Command-line arguments; under [`DapTransport::Tcp`], every `${port}`
    /// occurrence is replaced with the chosen port.
    pub args: Vec<String>,
    /// The transport the adapter speaks.
    pub transport: DapTransport,
}

/// The `launch`/`attach` half of [`DapClient::start`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartConfig {
    /// `attach` to a running debuggee instead of `launch`ing one.
    pub attach: bool,
    /// The adapter-specific arguments (`program`, `args`, `cwd`, `pid`, …),
    /// passed through verbatim — there is no cross-adapter schema to model.
    pub arguments: Value,
}

/// The breakpoints of one file, replayed wholesale on every change
/// (`setBreakpoints` is full-replace per file by design).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileBreakpoints {
    /// The source file.
    pub path: PathBuf,
    /// The 0-based breakpoint lines.
    pub lines: Vec<u32>,
}

/// An async client for one debug adapter process or socket.
pub struct DapClient {
    conn: conn::Connection,
    capabilities: Capabilities,
    /// The spawned adapter, held so it dies with the client.
    _child: Option<tokio::process::Child>,
}

impl DapClient {
    /// Take over an arbitrary I/O pair speaking DAP (tests use an in-memory
    /// duplex; a remote adapter is a connected TCP stream split in two).
    #[must_use]
    pub fn connect<R, W>(read: R, write: W) -> Self
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        Self {
            conn: conn::Connection::start(read, write),
            capabilities: Capabilities::default(),
            _child: None,
        }
    }

    /// Launch the adapter described by `spec` and connect over its transport.
    ///
    /// # Errors
    /// Returns [`DapError::Launch`] if the process cannot start or (under
    /// [`DapTransport::Tcp`]) its socket never accepts.
    pub async fn spawn(spec: &DapSpec) -> Result<Self, DapError> {
        match spec.transport {
            DapTransport::Stdio => Self::spawn_stdio(spec),
            DapTransport::Tcp => Self::spawn_then_tcp(spec).await,
        }
    }

    fn spawn_stdio(spec: &DapSpec) -> Result<Self, DapError> {
        let mut command = tokio::process::Command::new(&spec.command);
        command.args(&spec.args);
        Self::spawn_command(command)
    }

    /// Launch a *prepared* command (e.g. one wrapped by a process supervisor)
    /// whose child speaks DAP on stdin/stdout.
    ///
    /// # Errors
    /// Returns [`DapError::Launch`] if the process cannot start.
    pub fn spawn_command(mut command: tokio::process::Command) -> Result<Self, DapError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DapError::Launch(e.to_string()))?;
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(DapError::Launch("no piped standard I/O".to_owned()));
        };
        Ok(Self {
            conn: conn::Connection::start(stdout, stdin),
            capabilities: Capabilities::default(),
            _child: Some(child),
        })
    }

    /// Launch a prepared command whose child listens on `port` (already
    /// substituted into its arguments — see [`substitute_port`]), then
    /// connect with retries while it boots.
    ///
    /// # Errors
    /// Returns [`DapError::Launch`] if the process cannot start or its socket
    /// never accepts.
    pub async fn spawn_command_tcp(
        mut command: tokio::process::Command,
        port: u16,
    ) -> Result<Self, DapError> {
        let child = command
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| DapError::Launch(e.to_string()))?;
        let stream = connect_with_retries(port).await?;
        let (read, write) = stream.into_split();
        Ok(Self {
            conn: conn::Connection::start(read, write),
            capabilities: Capabilities::default(),
            _child: Some(child),
        })
    }

    async fn spawn_then_tcp(spec: &DapSpec) -> Result<Self, DapError> {
        let port = free_port().map_err(|e| DapError::Launch(format!("no free port: {e}")))?;
        let mut command = tokio::process::Command::new(&spec.command);
        command
            .args(substitute_port(&spec.args, port))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Self::spawn_command_tcp(command, port).await
    }

    /// Perform the `initialize` handshake, storing and returning the
    /// adapter's capabilities (all later gating reads the stored copy).
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`]/[`DapError::Closed`]/[`DapError::Timeout`]
    /// like any request.
    pub async fn initialize(&mut self, adapter_id: &str) -> Result<&Capabilities, DapError> {
        let body = self
            .conn
            .request(
                "initialize",
                json!({
                    "clientID": "karet",
                    "clientName": "karet",
                    "adapterID": adapter_id,
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "pathFormat": "path",
                    "supportsVariableType": true,
                    "locale": "en",
                }),
            )
            .await?;
        self.capabilities = serde_json::from_value(body)
            .map_err(|e| DapError::Protocol(format!("malformed capabilities: {e}")))?;
        Ok(&self.capabilities)
    }

    /// The capabilities stored by [`initialize`](Self::initialize).
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Run the configuration sequence (see the crate docs): `launch`/`attach`
    /// un-awaited, `initialized` event, per-file breakpoints, gated exception
    /// breakpoints, gated `configurationDone`, then the launch response.
    ///
    /// # Errors
    /// Any step's failure aborts the sequence; [`DapError::Timeout`] if the
    /// adapter never announces `initialized`.
    pub async fn start(
        &self,
        config: StartConfig,
        breakpoints: &[FileBreakpoints],
    ) -> Result<(), DapError> {
        let command = if config.attach { "attach" } else { "launch" };
        // On the wire immediately; the response typically arrives only after
        // the configuration phase below (and must not be awaited before it).
        let launch = self
            .conn
            .request_deferred(command, config.arguments)
            .await?;
        tokio::pin!(launch);
        // The adapter may answer launch before or after `initialized`; both
        // orders are legal and both must configure exactly once.
        let mut launch_result: Option<Value> = None;
        {
            let initialized = self.conn.initialized.wait();
            tokio::pin!(initialized);
            let deadline = tokio::time::sleep(HANDSHAKE_TIMEOUT);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    () = &mut initialized => break,
                    result = &mut launch, if launch_result.is_none() => {
                        launch_result = Some(result?);
                    },
                    () = &mut deadline => return Err(DapError::Timeout),
                }
            }
        }
        if self.conn.is_closed() {
            return Err(DapError::Closed);
        }
        for file in breakpoints {
            self.set_breakpoints(&file.path, &file.lines).await?;
        }
        let filters: Vec<&str> = self
            .capabilities
            .exception_breakpoint_filters
            .iter()
            .filter(|f| f.default)
            .map(|f| f.filter.as_str())
            .collect();
        if !self.capabilities.exception_breakpoint_filters.is_empty() {
            self.conn
                .request("setExceptionBreakpoints", json!({ "filters": filters }))
                .await?;
        }
        if self.capabilities.supports_configuration_done_request {
            self.conn.request("configurationDone", json!({})).await?;
        }
        if launch_result.is_none() {
            launch.await?;
        }
        Ok(())
    }

    /// Replace the breakpoints of `path` with `lines` (0-based), returning the
    /// adapter's acknowledgement in the same order. Verification may still
    /// change later via [`DebugEvent::BreakpointChanged`].
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn set_breakpoints(
        &self,
        path: &Path,
        lines: &[u32],
    ) -> Result<Vec<Breakpoint>, DapError> {
        let body = self
            .conn
            .request(
                "setBreakpoints",
                json!({
                    "source": { "path": path.to_string_lossy() },
                    "breakpoints": lines
                        .iter()
                        .map(|&line| json!({ "line": line.saturating_add(1) }))
                        .collect::<Vec<_>>(),
                }),
            )
            .await?;
        Ok(body
            .get("breakpoints")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(types::breakpoint_from).collect())
            .unwrap_or_default())
    }

    /// Resume `thread` (and typically every thread).
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn resume(&self, thread_id: i64) -> Result<(), DapError> {
        self.run_control("continue", thread_id).await
    }

    /// Step over the current line.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn step_over(&self, thread_id: i64) -> Result<(), DapError> {
        self.run_control("next", thread_id).await
    }

    /// Step into the call under the caret.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn step_in(&self, thread_id: i64) -> Result<(), DapError> {
        self.run_control("stepIn", thread_id).await
    }

    /// Step out of the current frame.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn step_out(&self, thread_id: i64) -> Result<(), DapError> {
        self.run_control("stepOut", thread_id).await
    }

    /// Pause a running thread.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn pause(&self, thread_id: i64) -> Result<(), DapError> {
        self.run_control("pause", thread_id).await
    }

    async fn run_control(&self, command: &str, thread_id: i64) -> Result<(), DapError> {
        self.conn
            .request(command, json!({ "threadId": thread_id }))
            .await
            .map(|_| ())
    }

    /// The debuggee's threads.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn threads(&self) -> Result<Vec<Thread>, DapError> {
        let body = self.conn.request("threads", Value::Null).await?;
        Ok(body
            .get("threads")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|t| {
                        Some(Thread {
                            id: t.get("id").and_then(Value::as_i64)?,
                            name: t
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The call stack of a stopped thread (0-based positions).
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn stack_trace(&self, thread_id: i64) -> Result<Vec<StackFrame>, DapError> {
        let body = self
            .conn
            .request("stackTrace", json!({ "threadId": thread_id }))
            .await?;
        Ok(body
            .get("stackFrames")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|f| {
                        Some(StackFrame {
                            id: f.get("id").and_then(Value::as_i64)?,
                            name: f
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                            line: zero_based(f.get("line")),
                            column: zero_based(f.get("column")),
                            source_path: f
                                .get("source")
                                .and_then(|s| s.get("path"))
                                .and_then(Value::as_str)
                                .map(PathBuf::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The variable scopes of one frame.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn scopes(&self, frame_id: i64) -> Result<Vec<Scope>, DapError> {
        let body = self
            .conn
            .request("scopes", json!({ "frameId": frame_id }))
            .await?;
        Ok(body
            .get("scopes")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|s| Scope {
                        name: s
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        variables_reference: s
                            .get("variablesReference")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                        expensive: s.get("expensive").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The children of a `variablesReference` (a scope or structured value) —
    /// the lazy leg of the threads → stack → scopes → variables waterfall.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure.
    pub async fn variables(&self, variables_reference: i64) -> Result<Vec<Variable>, DapError> {
        let body = self
            .conn
            .request(
                "variables",
                json!({ "variablesReference": variables_reference }),
            )
            .await?;
        Ok(body
            .get("variables")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|v| Variable {
                        name: v
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        value: v
                            .get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        ty: v.get("type").and_then(Value::as_str).map(str::to_owned),
                        variables_reference: v
                            .get("variablesReference")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Evaluate `expression` (the REPL), in `frame_id`'s context when given.
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] on failure (e.g. an invalid expression).
    pub async fn evaluate(
        &self,
        expression: &str,
        frame_id: Option<i64>,
    ) -> Result<Evaluation, DapError> {
        let mut arguments = json!({ "expression": expression, "context": "repl" });
        if let (Some(frame_id), Some(object)) = (frame_id, arguments.as_object_mut()) {
            object.insert("frameId".to_owned(), json!(frame_id));
        }
        let body = self.conn.request("evaluate", arguments).await?;
        Ok(Evaluation {
            result: body
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            variables_reference: body
                .get("variablesReference")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        })
    }

    /// End the session. `terminate_debuggee` is honored only when the adapter
    /// advertised `supportTerminateDebuggee`; a dead connection is a success
    /// (the session is equally over).
    ///
    /// # Errors
    /// Returns [`DapError::Adapter`] if the adapter rejects the disconnect.
    pub async fn disconnect(&self, terminate_debuggee: bool) -> Result<(), DapError> {
        let mut arguments = json!({});
        if self.capabilities.support_terminate_debuggee
            && let Some(object) = arguments.as_object_mut()
        {
            object.insert("terminateDebuggee".to_owned(), json!(terminate_debuggee));
        }
        match self.conn.request("disconnect", arguments).await {
            Ok(_) | Err(DapError::Closed) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Subscribe to adapter events. On stream end a final
    /// [`DebugEvent::Terminated`] is synthesized (possibly duplicating the
    /// adapter's own), so consumers must treat it idempotently.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<DebugEvent> {
        self.conn.events()
    }
}

/// A wire 1-based position as 0-based; absent/zero stays 0.
fn zero_based(value: Option<&Value>) -> u32 {
    value
        .and_then(Value::as_u64)
        .map(|n| u32::try_from(n.saturating_sub(1)).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// Replace every `${port}` occurrence in `args` (see [`DapTransport::Tcp`]).
#[must_use]
pub fn substitute_port(args: &[String], port: u16) -> Vec<String> {
    args.iter()
        .map(|arg| arg.replace("${port}", &port.to_string()))
        .collect()
}

/// A free loopback port, chosen by binding port 0 and dropping the listener
/// (the standard race-tolerant approach; the adapter binds it right after).
///
/// # Errors
/// Propagates the bind failure (exotic: no loopback at all).
pub fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Connect to the adapter's socket, retrying while it boots.
async fn connect_with_retries(port: u16) -> Result<tokio::net::TcpStream, DapError> {
    let deadline = tokio::time::Instant::now() + TCP_CONNECT_TIMEOUT;
    loop {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => return Ok(stream),
            Err(e) if tokio::time::Instant::now() >= deadline => {
                return Err(DapError::Launch(format!(
                    "adapter socket on port {port} never accepted: {e}"
                )));
            },
            Err(_) => tokio::time::sleep(TCP_RETRY_DELAY).await,
        }
    }
}

#[cfg(test)]
mod tests;

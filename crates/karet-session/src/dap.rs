//! Debugger orchestration: one active DAP session behind the `Debug*`
//! commands, mirroring the LSP manager's shape (connector seam for tests,
//! supervisor-owned processes, events answered on the shared session stream).
//!
//! The manager owns the durable state — the per-file breakpoint sets and the
//! adapter/configuration settings — while the async work (spawn, handshake,
//! requests) runs on detached tasks sharing an `Arc` handle, so the session
//! actor never blocks on an adapter.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use karet_core::NotificationKind;
use karet_core::Severity;
use karet_dap::DapClient;
use karet_dap::DapError;
use karet_dap::DapSpec;
use karet_dap::DapTransport;
use karet_dap::DebugEvent as AdapterEvent;
use karet_dap::FileBreakpoints;
use karet_dap::StartConfig;
use tokio::sync::mpsc;

use crate::api::DebugBreakpoint;
use crate::api::DebugSessionState;
use crate::api::Event;
use crate::api::RequestId;
use crate::config::schema::DebugConfiguration;
use crate::config::schema::Debugger;

/// How the manager establishes a client — supervisor-wrapped spawn in
/// production; tests inject an in-memory duplex connection instead.
pub(crate) type Connector = Arc<
    dyn Fn(AdapterLaunch) -> Pin<Box<dyn Future<Output = Result<DapClient, DapError>> + Send>>
        + Send
        + Sync,
>;

/// Everything a connector needs to launch one adapter.
pub(crate) struct AdapterLaunch {
    /// The resolved launch spec.
    pub spec: DapSpec,
    /// The supervisor executable (production requires one — fail closed).
    pub supervisor: Option<PathBuf>,
    /// The working directory (the workspace root).
    pub root: PathBuf,
}

/// The sender the manager answers on.
type Events = mpsc::UnboundedSender<(Option<RequestId>, Event)>;

/// The state shared between the actor-owned manager and its detached tasks.
#[derive(Default)]
struct Shared {
    /// The live client, present between a successful connect and stop/end.
    client: Mutex<Option<Arc<DapClient>>>,
    /// The stopped thread the run controls act on (`-1` = not stopped).
    thread: AtomicI64,
}

impl Shared {
    fn client(&self) -> Option<Arc<DapClient>> {
        self.client.lock().ok().and_then(|slot| slot.clone())
    }

    fn take_client(&self) -> Option<Arc<DapClient>> {
        self.client.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// Debug-session orchestration owned by the session actor.
pub(crate) struct DebugManager {
    connector: Connector,
    settings: Debugger,
    supervisor: Option<PathBuf>,
    root: Option<PathBuf>,
    /// The durable per-file breakpoint sets, replayed at session start.
    breakpoints: HashMap<PathBuf, Vec<u32>>,
    shared: Arc<Shared>,
    events: Events,
}

impl DebugManager {
    /// Create a manager answering on `events`.
    pub(crate) fn new(
        settings: Debugger,
        root: Option<PathBuf>,
        supervisor: Option<PathBuf>,
        events: Events,
    ) -> Self {
        Self {
            connector: production_connector(),
            settings,
            supervisor,
            root,
            breakpoints: HashMap::new(),
            shared: Arc::default(),
            events,
        }
    }

    /// Replace the connector (tests inject an in-memory adapter here).
    #[cfg(test)]
    pub(crate) fn set_connector(&mut self, connector: Connector) {
        self.connector = connector;
    }

    /// Apply new settings (adapters/configurations only affect later starts).
    pub(crate) fn reconfigure(&mut self, settings: Debugger) {
        self.settings = settings;
    }

    /// Start a session from `configuration` (the first entry when `None`).
    pub(crate) fn start(&mut self, configuration: Option<&str>) {
        if self.shared.client().is_some() {
            self.notify(Severity::Warning, "a debug session is already running");
            return;
        }
        let Some(config) = pick_configuration(&self.settings, configuration).cloned() else {
            self.notify(
                Severity::Warning,
                "no matching debug configuration (define debug.configurations in settings)",
            );
            return;
        };
        let Some(spec) = resolve_adapter(&self.settings, &config.adapter) else {
            self.notify(
                Severity::Warning,
                &format!(
                    "unknown debug adapter '{}' (define it under debug.adapters)",
                    config.adapter
                ),
            );
            return;
        };
        self.emit_state(DebugSessionState::Starting, &config.name);
        let launch = AdapterLaunch {
            spec,
            supervisor: self.supervisor.clone(),
            root: self.root.clone().unwrap_or_else(|| PathBuf::from(".")),
        };
        let connector = Arc::clone(&self.connector);
        let shared = Arc::clone(&self.shared);
        let events = self.events.clone();
        let breakpoints: Vec<FileBreakpoints> = self
            .breakpoints
            .iter()
            .map(|(path, lines)| FileBreakpoints {
                path: path.clone(),
                lines: lines.clone(),
            })
            .collect();
        tokio::spawn(async move {
            let mut client = match connector(launch).await {
                Ok(client) => client,
                Err(error) => {
                    fail_start(&events, &error);
                    return;
                },
            };
            if let Err(error) = client.initialize(&config.adapter).await {
                fail_start(&events, &error);
                return;
            }
            let client = Arc::new(client);
            if let Ok(mut slot) = shared.client.lock() {
                *slot = Some(Arc::clone(&client));
            }
            tokio::spawn(forward_events(
                Arc::clone(&client),
                Arc::clone(&shared),
                events.clone(),
            ));
            let start = StartConfig {
                attach: config.attach,
                arguments: config.arguments.clone(),
            };
            match client.start(start, &breakpoints).await {
                Ok(()) => {
                    let _ = events.send((
                        None,
                        Event::DebugState {
                            state: DebugSessionState::Running,
                            detail: config.name.clone(),
                        },
                    ));
                },
                Err(error) => {
                    if let Some(client) = shared.take_client() {
                        let _ = client.disconnect(true).await;
                    }
                    fail_start(&events, &error);
                },
            }
        });
    }

    /// End the session, terminating the debuggee when the adapter can.
    pub(crate) fn stop(&mut self) {
        let Some(client) = self.shared.take_client() else {
            self.notify(Severity::Information, "no debug session to stop");
            return;
        };
        self.shared.thread.store(-1, Ordering::SeqCst);
        let events = self.events.clone();
        tokio::spawn(async move {
            let _ = client.disconnect(true).await;
            let _ = events.send((
                None,
                Event::DebugState {
                    state: DebugSessionState::Idle,
                    detail: "stopped".to_owned(),
                },
            ));
        });
    }

    /// A run control against the stopped thread (`continue`, steps, `pause`).
    pub(crate) fn run_control(&self, control: RunControl) {
        let Some(client) = self.shared.client() else {
            self.notify(Severity::Information, "no debug session");
            return;
        };
        let thread = self.shared.thread.load(Ordering::SeqCst);
        // Pause targets a *running* session; everything else needs a stop.
        if thread < 0 && control != RunControl::Pause {
            self.notify(Severity::Information, "the debuggee is not stopped");
            return;
        }
        let thread = thread.max(1);
        let events = self.events.clone();
        tokio::spawn(async move {
            let outcome = match control {
                RunControl::Continue => client.resume(thread).await,
                RunControl::StepOver => client.step_over(thread).await,
                RunControl::StepIn => client.step_in(thread).await,
                RunControl::StepOut => client.step_out(thread).await,
                RunControl::Pause => client.pause(thread).await,
            };
            if let Err(error) = outcome {
                let _ = events.send((
                    None,
                    Event::Notification {
                        severity: Severity::Warning,
                        kind: NotificationKind::System,
                        message: format!("debugger: {error}"),
                    },
                ));
            }
        });
    }

    /// Replace one file's breakpoints: stored for future sessions, forwarded
    /// to a running one (full-replace per file, per protocol).
    pub(crate) fn set_breakpoints(&mut self, path: PathBuf, lines: Vec<u32>) {
        if lines.is_empty() {
            self.breakpoints.remove(&path);
        } else {
            self.breakpoints.insert(path.clone(), lines.clone());
        }
        let Some(client) = self.shared.client() else {
            // No adapter to verify them yet: echo the set back unverified so
            // the UI stays authoritative about what is armed.
            let breakpoints = lines
                .iter()
                .map(|&line| DebugBreakpoint {
                    line,
                    verified: false,
                })
                .collect();
            let _ = self
                .events
                .send((None, Event::DebugBreakpoints { path, breakpoints }));
            return;
        };
        let events = self.events.clone();
        tokio::spawn(async move {
            match client.set_breakpoints(&path, &lines).await {
                Ok(acked) => {
                    let breakpoints = acked
                        .iter()
                        .zip(&lines)
                        .map(|(bp, &requested)| DebugBreakpoint {
                            line: bp.line.unwrap_or(requested),
                            verified: bp.verified,
                        })
                        .collect();
                    let _ = events.send((None, Event::DebugBreakpoints { path, breakpoints }));
                },
                Err(error) => {
                    let _ = events.send((
                        None,
                        Event::Notification {
                            severity: Severity::Warning,
                            kind: NotificationKind::System,
                            message: format!("debugger: {error}"),
                        },
                    ));
                },
            }
        });
    }

    fn emit_state(&self, state: DebugSessionState, detail: &str) {
        let _ = self.events.send((
            None,
            Event::DebugState {
                state,
                detail: detail.to_owned(),
            },
        ));
    }

    fn notify(&self, severity: Severity, message: &str) {
        let _ = self.events.send((
            None,
            Event::Notification {
                severity,
                kind: NotificationKind::System,
                message: message.to_owned(),
            },
        ));
    }
}

/// The run controls [`DebugManager::run_control`] multiplexes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunControl {
    /// Resume execution.
    Continue,
    /// Step over the current line.
    StepOver,
    /// Step into the call.
    StepIn,
    /// Step out of the frame.
    StepOut,
    /// Pause a running debuggee.
    Pause,
}

/// A failed start: back to idle with the error as the detail.
fn fail_start(events: &Events, error: &DapError) {
    let _ = events.send((
        None,
        Event::DebugState {
            state: DebugSessionState::Idle,
            detail: error.to_string(),
        },
    ));
}

/// The configuration `name` names, or the first one.
fn pick_configuration<'a>(
    settings: &'a Debugger,
    name: Option<&str>,
) -> Option<&'a DebugConfiguration> {
    match name {
        Some(name) => settings.configurations.iter().find(|c| c.name == name),
        None => settings.configurations.first(),
    }
}

/// Resolve an adapter name: the user's `debug.adapters` entry first, then the
/// built-in fallbacks for the common adapters (found on `PATH` at spawn time;
/// karet does not download debug adapters).
fn resolve_adapter(settings: &Debugger, name: &str) -> Option<DapSpec> {
    if let Some(adapter) = settings.adapters.get(name) {
        return Some(DapSpec {
            command: adapter.command.clone(),
            args: adapter.args.clone(),
            transport: match adapter.transport {
                crate::config::schema::DebugTransport::Tcp => DapTransport::Tcp,
                crate::config::schema::DebugTransport::Stdio => DapTransport::Stdio,
            },
        });
    }
    let spec = |command: &str, args: &[&str], transport| DapSpec {
        command: command.to_owned(),
        args: args.iter().map(|&a| a.to_owned()).collect(),
        transport,
    };
    Some(match name {
        "codelldb" => spec("codelldb", &["--port", "${port}"], DapTransport::Tcp),
        "lldb-dap" => spec("lldb-dap", &[], DapTransport::Stdio),
        "gdb" => spec("gdb", &["-i", "dap"], DapTransport::Stdio),
        "debugpy" => spec("python3", &["-m", "debugpy.adapter"], DapTransport::Stdio),
        _ => return None,
    })
}

/// Forward adapter events onto the session stream until the session ends.
async fn forward_events(client: Arc<DapClient>, shared: Arc<Shared>, events: Events) {
    let mut rx = client.events();
    loop {
        match rx.recv().await {
            Ok(AdapterEvent::Stopped {
                reason, thread_id, ..
            }) => {
                let thread = thread_id.unwrap_or(1);
                shared.thread.store(thread, Ordering::SeqCst);
                // The top frame gives the jump-to location; a failure here
                // (adapter already resumed) degrades to a location-less stop.
                let top = client
                    .stack_trace(thread)
                    .await
                    .ok()
                    .and_then(|frames| frames.into_iter().next());
                let _ = events.send((
                    None,
                    Event::DebugStopped {
                        reason: reason.clone(),
                        thread,
                        path: top.as_ref().and_then(|f| f.source_path.clone()),
                        line: top.as_ref().map(|f| f.line),
                    },
                ));
                let _ = events.send((
                    None,
                    Event::DebugState {
                        state: DebugSessionState::Stopped,
                        detail: reason,
                    },
                ));
            },
            Ok(AdapterEvent::Continued { .. }) => {
                shared.thread.store(-1, Ordering::SeqCst);
                let _ = events.send((None, Event::DebugContinued));
                let _ = events.send((
                    None,
                    Event::DebugState {
                        state: DebugSessionState::Running,
                        detail: String::new(),
                    },
                ));
            },
            Ok(AdapterEvent::Output { category, output }) => {
                let _ = events.send((
                    None,
                    Event::DebugOutput {
                        category,
                        text: output,
                    },
                ));
            },
            Ok(AdapterEvent::BreakpointChanged(bp)) => {
                // Late verification: only mappable when the adapter names the
                // file; the app merges the single entry by line.
                if let (Some(path), Some(line)) = (bp.source_path.clone(), bp.line) {
                    let _ = events.send((
                        None,
                        Event::DebugBreakpoints {
                            path,
                            breakpoints: vec![DebugBreakpoint {
                                line,
                                verified: bp.verified,
                            }],
                        },
                    ));
                }
            },
            Ok(AdapterEvent::Exited { exit_code }) => {
                let _ = events.send((
                    None,
                    Event::DebugOutput {
                        category: "console".to_owned(),
                        text: format!("debuggee exited with code {exit_code}\n"),
                    },
                ));
            },
            Ok(AdapterEvent::Terminated) => {
                shared.thread.store(-1, Ordering::SeqCst);
                // Stop() already emitted Idle if it took the client first;
                // the take guard keeps this to one Idle per session.
                if shared.take_client().is_some() {
                    let _ = events.send((
                        None,
                        Event::DebugState {
                            state: DebugSessionState::Idle,
                            detail: "session ended".to_owned(),
                        },
                    ));
                }
                return;
            },
            Ok(_) => {},
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {},
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Run the adapter through karet's crash-safe process supervisor. A headless
/// host that supplied no supervisor fails closed, like the LSP connector.
fn production_connector() -> Connector {
    Arc::new(|launch| {
        Box::pin(async move {
            let Some(supervisor) = launch.supervisor else {
                return Err(DapError::Launch("no process supervisor".to_owned()));
            };
            match launch.spec.transport {
                DapTransport::Tcp => {
                    let port = karet_dap::free_port()
                        .map_err(|e| DapError::Launch(format!("no free port: {e}")))?;
                    let args = karet_dap::substitute_port(&launch.spec.args, port);
                    let command = karet_supervisor::supervisor::command(
                        &supervisor,
                        launch.spec.command.clone(),
                        args,
                        &launch.root,
                    )
                    .map_err(|e| DapError::Launch(e.to_string()))?;
                    DapClient::spawn_command_tcp(command, port).await
                },
                // Stdio, and (non-exhaustive) any future transport this build
                // does not know: launch over pipes, the protocol's default.
                _ => {
                    let command = karet_supervisor::supervisor::command(
                        &supervisor,
                        launch.spec.command.clone(),
                        launch.spec.args.clone(),
                        &launch.root,
                    )
                    .map_err(|e| DapError::Launch(e.to_string()))?;
                    DapClient::spawn_command(command)
                },
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use karet_lsp::codec;
    use serde_json::Value;
    use serde_json::json;
    use tokio::io::AsyncWrite;
    use tokio::io::BufReader;

    use super::*;
    use crate::config::schema::DebugConfiguration;

    fn settings_with_config() -> Debugger {
        Debugger {
            configurations: vec![DebugConfiguration {
                name: "Run".to_owned(),
                adapter: "lldb-dap".to_owned(),
                attach: false,
                arguments: json!({"program": "/w/a.out"}),
            }],
            ..Debugger::default()
        }
    }

    fn fake_connector() -> Connector {
        Arc::new(|_launch| {
            Box::pin(async move {
                let (client_end, server_end) = tokio::io::duplex(1 << 20);
                let (server_read, server_write) = tokio::io::split(server_end);
                tokio::spawn(fake_adapter(BufReader::new(server_read), server_write));
                let (read, write) = tokio::io::split(client_end);
                Ok(DapClient::connect(read, write))
            })
        })
    }

    async fn write_msg<W: AsyncWrite + Unpin>(write: &mut W, msg: &Value) {
        if let Ok(bytes) = serde_json::to_vec(msg) {
            let _ = codec::write_frame(write, &bytes).await;
        }
    }

    async fn fake_adapter<R, W>(mut read: BufReader<R>, mut write: W)
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let mut seq = 1000_i64;
        while let Ok(Some(bytes)) = codec::read_frame(&mut read).await {
            let Ok(msg): Result<Value, _> = serde_json::from_slice(&bytes) else {
                break;
            };
            let command = msg
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let mut respond = |body: Value| {
                let mut response = json!({
                    "seq": seq,
                    "type": "response",
                    "request_seq": msg.get("seq").cloned().unwrap_or(Value::Null),
                    "command": command.clone(),
                    "success": true,
                });
                if !body.is_null()
                    && let Some(object) = response.as_object_mut()
                {
                    object.insert("body".to_owned(), body);
                }
                seq += 1;
                response
            };
            match command.as_str() {
                "initialize" => {
                    let frame = respond(json!({"supportsConfigurationDoneRequest": true}));
                    write_msg(&mut write, &frame).await;
                    let event = json!({"seq": seq, "type": "event", "event": "initialized"});
                    write_msg(&mut write, &event).await;
                },
                "setBreakpoints" => {
                    let acked: Vec<Value> = msg
                        .get("arguments")
                        .and_then(|a| a.get("breakpoints"))
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .map(|bp| {
                                    json!({"line": bp.get("line").cloned().unwrap_or(Value::Null),
                                           "verified": true})
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let frame = respond(json!({ "breakpoints": acked }));
                    write_msg(&mut write, &frame).await;
                },
                "configurationDone" => {
                    let frame = respond(Value::Null);
                    write_msg(&mut write, &frame).await;
                    let stopped = json!({"seq": seq, "type": "event", "event": "stopped",
                        "body": {"reason": "breakpoint", "threadId": 7}});
                    write_msg(&mut write, &stopped).await;
                },
                "stackTrace" => {
                    let frame = respond(json!({"stackFrames": [
                        {"id": 1, "name": "main", "line": 5, "column": 1,
                         "source": {"path": "/w/main.rs"}}]}));
                    write_msg(&mut write, &frame).await;
                },
                "continue" => {
                    let frame = respond(Value::Null);
                    write_msg(&mut write, &frame).await;
                    let continued = json!({"seq": seq, "type": "event", "event": "continued",
                        "body": {"threadId": 7}});
                    write_msg(&mut write, &continued).await;
                },
                "disconnect" => {
                    let frame = respond(Value::Null);
                    write_msg(&mut write, &frame).await;
                    break;
                },
                _ => {
                    let frame = respond(Value::Null);
                    write_msg(&mut write, &frame).await;
                },
            }
        }
    }

    /// Drain events until `pick` accepts one, bounded by a deadline.
    async fn wait_for<T>(
        rx: &mut mpsc::UnboundedReceiver<(Option<RequestId>, Event)>,
        mut pick: impl FnMut(&Event) -> Option<T>,
    ) -> Option<T> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let event = tokio::time::timeout_at(deadline, rx.recv()).await.ok()??;
            if let Some(found) = pick(&event.1) {
                return Some(found);
            }
        }
    }

    #[tokio::test]
    async fn the_command_event_contract_round_trips() {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let mut manager = DebugManager::new(settings_with_config(), None, None, events_tx);
        manager.set_connector(fake_connector());
        // Breakpoints set before any session: stored, echoed unverified.
        manager.set_breakpoints(PathBuf::from("/w/main.rs"), vec![3]);
        let echoed = wait_for(&mut events_rx, |event| match event {
            Event::DebugBreakpoints { breakpoints, .. } => Some(breakpoints.clone()),
            _ => None,
        })
        .await;
        assert_eq!(
            echoed,
            Some(vec![DebugBreakpoint {
                line: 3,
                verified: false
            }])
        );

        manager.start(None);
        let starting = wait_for(&mut events_rx, |event| match event {
            Event::DebugState { state, .. } => Some(*state),
            _ => None,
        })
        .await;
        assert_eq!(starting, Some(DebugSessionState::Starting));
        // The fake stops at a breakpoint right after configuration; the stop
        // carries the top frame's location, 0-based.
        let stopped = wait_for(&mut events_rx, |event| match event {
            Event::DebugStopped {
                reason,
                thread,
                path,
                line,
            } => Some((reason.clone(), *thread, path.clone(), *line)),
            _ => None,
        })
        .await;
        assert_eq!(
            stopped,
            Some((
                "breakpoint".to_owned(),
                7,
                Some(PathBuf::from("/w/main.rs")),
                Some(4)
            ))
        );

        // Live breakpoint replace: verified by the adapter this time.
        manager.set_breakpoints(PathBuf::from("/w/main.rs"), vec![3, 9]);
        let verified = wait_for(&mut events_rx, |event| match event {
            Event::DebugBreakpoints { breakpoints, .. } => Some(breakpoints.clone()),
            _ => None,
        })
        .await
        .unwrap_or_default();
        assert!(verified.iter().all(|bp| bp.verified), "{verified:?}");

        manager.run_control(RunControl::Continue);
        let continued = wait_for(&mut events_rx, |event| {
            matches!(event, Event::DebugContinued).then_some(())
        })
        .await;
        assert_eq!(continued, Some(()));

        manager.stop();
        let idle = wait_for(&mut events_rx, |event| match event {
            Event::DebugState {
                state: DebugSessionState::Idle,
                detail,
            } => Some(detail.clone()),
            _ => None,
        })
        .await;
        assert_eq!(idle, Some("stopped".to_owned()));
    }

    #[tokio::test]
    async fn start_without_configurations_notifies() {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let mut manager = DebugManager::new(Debugger::default(), None, None, events_tx);
        manager.set_connector(fake_connector());
        manager.start(None);
        let message = wait_for(&mut events_rx, |event| match event {
            Event::Notification { message, .. } => Some(message.clone()),
            _ => None,
        })
        .await
        .unwrap_or_default();
        assert!(
            message.contains("no matching debug configuration"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn run_controls_without_a_session_notify() {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let manager = DebugManager::new(Debugger::default(), None, None, events_tx);
        manager.run_control(RunControl::StepOver);
        let message = wait_for(&mut events_rx, |event| match event {
            Event::Notification { message, .. } => Some(message.clone()),
            _ => None,
        })
        .await
        .unwrap_or_default();
        assert_eq!(message, "no debug session");
    }

    #[test]
    fn adapter_resolution_prefers_user_entries_over_builtins() {
        let mut settings = Debugger::default();
        settings.adapters.insert(
            "gdb".to_owned(),
            crate::config::schema::DebugAdapter {
                command: "/opt/gdb".to_owned(),
                args: vec!["--custom".to_owned()],
                transport: crate::config::schema::DebugTransport::Stdio,
            },
        );
        let user = resolve_adapter(&settings, "gdb");
        assert_eq!(user.as_ref().map(|s| s.command.as_str()), Some("/opt/gdb"));
        let builtin = resolve_adapter(&Debugger::default(), "codelldb");
        assert_eq!(
            builtin.map(|s| (s.command, s.transport)),
            Some(("codelldb".to_owned(), DapTransport::Tcp))
        );
        assert!(resolve_adapter(&Debugger::default(), "made-up").is_none());
    }
}

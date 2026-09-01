//! Contract tests for the debugger orchestration, driven by a scripted
//! in-memory adapter behind the connector seam.
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
            "scopes" => {
                let frame = respond(json!({"scopes": [
                    {"name": "Locals", "variablesReference": 11, "expensive": false}]}));
                write_msg(&mut write, &frame).await;
            },
            "variables" => {
                let frame = respond(json!({"variables": [
                    {"name": "answer", "value": "42", "type": "i32",
                     "variablesReference": 0},
                    {"name": "nested", "value": "{…}", "variablesReference": 12}]}));
                write_msg(&mut write, &frame).await;
            },
            "evaluate" => {
                let frame = respond(json!({"result": "43", "variablesReference": 0}));
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
    // A session that reached Running is a start that worked, and the app tiers
    // that card straight off this severity — nothing downstream re-decides it.
    let running = wait_for(&mut events_rx, |event| match event {
        Event::DebugState {
            state: DebugSessionState::Running,
            severity,
            detail,
        } => Some((*severity, detail.clone())),
        _ => None,
    })
    .await;
    assert_eq!(running, Some((Severity::Hint, "Run".to_owned())));
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
            severity,
            detail,
        } => Some((*severity, detail.clone())),
        _ => None,
    })
    .await;
    // An end the user asked for is an ordinary outcome, and says so itself. A
    // consumer that had to read "stopped" to know that would re-tier the moment
    // this wording changed.
    assert_eq!(idle, Some((Severity::Hint, "stopped".to_owned())));
}

/// A connector that never yields a client, so `start` takes the `fail_start` path.
fn failing_connector() -> Connector {
    Arc::new(|_launch| Box::pin(async move { Err(DapError::Launch("no such adapter".to_owned())) }))
}

#[tokio::test]
async fn a_start_that_fails_is_the_only_debug_transition_reported_as_an_error() {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut manager = DebugManager::new(settings_with_config(), None, None, events_tx);
    manager.set_connector(failing_connector());
    manager.start(None);

    // Idle is reached by three routes — a failed start, a Shift+F5 stop, and the
    // debuggee exiting — and only this one is a failure. The severity is what
    // separates them, since all three land on the same state.
    let failed = wait_for(&mut events_rx, |event| match event {
        Event::DebugState {
            state: DebugSessionState::Idle,
            severity,
            detail,
        } => Some((*severity, detail.clone())),
        _ => None,
    })
    .await;
    // `Unknown` stands in for "nothing was reported", which fails the assert below.
    let (severity, detail) = failed.unwrap_or((Severity::Unknown, String::new()));
    assert_eq!(severity, Severity::Error, "{detail}");
    assert!(detail.contains("no such adapter"), "{detail}");
}

#[tokio::test]
async fn inspection_answers_ride_the_request_id() {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut manager = DebugManager::new(settings_with_config(), None, None, events_tx);
    manager.set_connector(fake_connector());
    manager.start(None);
    let stopped = wait_for(&mut events_rx, |event| {
        matches!(event, Event::DebugStopped { .. }).then_some(())
    })
    .await;
    assert_eq!(stopped, Some(()));

    manager.stack_trace(RequestId(41));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let frames = loop {
        let Ok(Some((id, event))) = tokio::time::timeout_at(deadline, events_rx.recv()).await
        else {
            break None;
        };
        if let Event::DebugStack { frames } = event {
            assert_eq!(id, Some(RequestId(41)));
            break Some(frames);
        }
    };
    let frames = frames.unwrap_or_default();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].line, 4, "wire 1-based, api 0-based");

    manager.scopes(RequestId(42), frames[0].id);
    let scopes = wait_for(&mut events_rx, |event| match event {
        Event::DebugScopes { scopes, .. } => Some(scopes.clone()),
        _ => None,
    })
    .await
    .unwrap_or_default();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].reference, 11);

    manager.variables(RequestId(43), scopes[0].reference);
    let variables = wait_for(&mut events_rx, |event| match event {
        Event::DebugVariables { variables, .. } => Some(variables.clone()),
        _ => None,
    })
    .await
    .unwrap_or_default();
    assert_eq!(variables.len(), 2);
    assert_eq!(variables[0].value, "42");
    assert_eq!(
        variables[1].reference, 12,
        "expandable child keeps its handle"
    );

    manager.evaluate(RequestId(44), "1 + 42".to_owned(), Some(frames[0].id));
    let result = wait_for(&mut events_rx, |event| match event {
        Event::DebugEvaluated { result, .. } => Some(result.clone()),
        _ => None,
    })
    .await;
    assert_eq!(result, Some("43".to_owned()));
}

#[tokio::test]
async fn inspection_without_a_stop_answers_empty() {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let manager = DebugManager::new(Debugger::default(), None, None, events_tx);
    manager.stack_trace(RequestId(9));
    let frames = wait_for(&mut events_rx, |event| match event {
        Event::DebugStack { frames } => Some(frames.clone()),
        _ => None,
    })
    .await;
    assert_eq!(frames, Some(Vec::new()));
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

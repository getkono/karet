//! A scripted fake adapter over an in-memory duplex drives the full client:
//! the handshake test *is* the sequencing spec (order, gating, un-awaited
//! launch), and the negatives pin the capability gates shut.

use std::sync::Arc;
use std::sync::Mutex;

use karet_jsonrpc::framing::content_length as codec;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// What the fake adapter advertises and how it behaves.
#[derive(Clone, Default)]
struct AdapterScript {
    supports_configuration_done: bool,
    support_terminate_debuggee: bool,
    exception_filters: bool,
    /// Answer `launch` only after `configurationDone` arrives (the common
    /// adapter behavior; exercises the un-awaited launch leg).
    launch_after_configuration_done: bool,
}

/// The requests the fake adapter received, in order: each command with its
/// `arguments` (`Value::Null` when the request carried none).
type Log = Arc<Mutex<Vec<(String, Value)>>>;

fn spawn_fake_adapter(script: AdapterScript) -> (DapClient, Log) {
    let (client_end, server_end) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_end);
    let log: Log = Arc::default();
    tokio::spawn(run_adapter(
        BufReader::new(server_read),
        server_write,
        script,
        Arc::clone(&log),
    ));
    let (read, write) = tokio::io::split(client_end);
    (DapClient::connect(read, write), log)
}

async fn run_adapter<R, W>(mut read: BufReader<R>, mut write: W, script: AdapterScript, log: Log)
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut seq = 1000_i64;
    let mut pending_launch: Option<Value> = None;
    while let Ok(Some(bytes)) = codec::read_frame(&mut read).await {
        let Ok(msg): Result<Value, _> = serde_json::from_slice(&bytes) else {
            break;
        };
        let command = msg
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if let Ok(mut log) = log.lock() {
            let arguments = msg.get("arguments").cloned().unwrap_or(Value::Null);
            log.push((command.clone(), arguments));
        }
        let respond = |seq: &mut i64, body: Value| {
            let mut response = json!({
                "seq": *seq,
                "type": "response",
                "request_seq": msg.get("seq").cloned().unwrap_or(Value::Null),
                "command": command,
                "success": true,
            });
            if !body.is_null()
                && let Some(object) = response.as_object_mut()
            {
                object.insert("body".to_owned(), body);
            }
            *seq += 1;
            response
        };
        match command.as_str() {
            "initialize" => {
                let mut caps = json!({});
                if let Some(object) = caps.as_object_mut() {
                    if script.supports_configuration_done {
                        object.insert("supportsConfigurationDoneRequest".to_owned(), json!(true));
                    }
                    if script.support_terminate_debuggee {
                        object.insert("supportTerminateDebuggee".to_owned(), json!(true));
                    }
                    if script.exception_filters {
                        object.insert(
                            "exceptionBreakpointFilters".to_owned(),
                            json!([
                                {"filter": "throw", "label": "on throw", "default": true},
                                {"filter": "catch", "label": "on catch", "default": false},
                            ]),
                        );
                    }
                }
                let frame = respond(&mut seq, caps);
                write_msg(&mut write, &frame).await;
                // Capabilities first, then the readiness announcement.
                let event = json!({"seq": seq, "type": "event", "event": "initialized"});
                seq += 1;
                write_msg(&mut write, &event).await;
            },
            "launch" | "attach" => {
                if script.launch_after_configuration_done {
                    pending_launch = Some(msg.clone());
                } else {
                    let frame = respond(&mut seq, Value::Null);
                    write_msg(&mut write, &frame).await;
                }
            },
            "setBreakpoints" => {
                let acked: Vec<Value> = msg
                    .get("arguments")
                    .and_then(|a| a.get("breakpoints"))
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .enumerate()
                            .map(|(i, bp)| {
                                json!({
                                    "id": i,
                                    "line": bp.get("line").cloned().unwrap_or(Value::Null),
                                    // The first breakpoint binds immediately;
                                    // the rest verify late via events.
                                    "verified": i == 0,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let frame = respond(&mut seq, json!({ "breakpoints": acked }));
                write_msg(&mut write, &frame).await;
            },
            "configurationDone" => {
                let frame = respond(&mut seq, Value::Null);
                write_msg(&mut write, &frame).await;
                if let Some(launch) = pending_launch.take() {
                    let response = json!({
                        "seq": seq,
                        "type": "response",
                        "request_seq": launch.get("seq").cloned().unwrap_or(Value::Null),
                        "command": launch.get("command").cloned().unwrap_or(Value::Null),
                        "success": true,
                    });
                    seq += 1;
                    write_msg(&mut write, &response).await;
                }
            },
            "stackTrace" => {
                let frame = respond(
                    &mut seq,
                    json!({"stackFrames": [
                        {"id": 41, "name": "main", "line": 10, "column": 5,
                         "source": {"path": "/w/main.rs"}},
                    ]}),
                );
                write_msg(&mut write, &frame).await;
            },
            // Each inspection reply mixes a complete entry with one missing
            // every optional field, so the client's defaulting is pinned.
            "threads" => {
                let frame = respond(
                    &mut seq,
                    json!({"threads": [
                        {"id": 1, "name": "main"},
                        {"id": 2},
                        {"name": "no id: dropped"},
                    ]}),
                );
                write_msg(&mut write, &frame).await;
            },
            "scopes" => {
                let frame = respond(
                    &mut seq,
                    json!({"scopes": [
                        {"name": "Locals", "variablesReference": 7, "expensive": false},
                        {"name": "Globals", "variablesReference": 8, "expensive": true},
                        {},
                    ]}),
                );
                write_msg(&mut write, &frame).await;
            },
            "variables" => {
                let frame = respond(
                    &mut seq,
                    json!({"variables": [
                        {"name": "x", "value": "1", "type": "i32", "variablesReference": 0},
                        {"name": "v", "value": "Vec(2)", "variablesReference": 9},
                        {},
                    ]}),
                );
                write_msg(&mut write, &frame).await;
            },
            "evaluate" => {
                let bad = msg
                    .get("arguments")
                    .and_then(|a| a.get("expression"))
                    .and_then(Value::as_str)
                    == Some("boom");
                if bad {
                    let refusal = json!({
                        "seq": seq,
                        "type": "response",
                        "request_seq": msg.get("seq").cloned().unwrap_or(Value::Null),
                        "command": "evaluate",
                        "success": false,
                        "message": "invalid expression",
                    });
                    seq += 1;
                    write_msg(&mut write, &refusal).await;
                } else {
                    let frame = respond(&mut seq, json!({"result": "42", "variablesReference": 0}));
                    write_msg(&mut write, &frame).await;
                }
            },
            "disconnect" => {
                let frame = respond(&mut seq, Value::Null);
                write_msg(&mut write, &frame).await;
                break;
            },
            "emit-stopped" => {
                // A test-only trigger: push a stopped + late-verification pair.
                let frame = respond(&mut seq, Value::Null);
                write_msg(&mut write, &frame).await;
                let stopped = json!({"seq": seq, "type": "event", "event": "stopped",
                    "body": {"reason": "breakpoint", "threadId": 1, "allThreadsStopped": true}});
                seq += 1;
                write_msg(&mut write, &stopped).await;
                let verified = json!({"seq": seq, "type": "event", "event": "breakpoint",
                    "body": {"reason": "changed",
                             "breakpoint": {"id": 1, "line": 8, "verified": true}}});
                seq += 1;
                write_msg(&mut write, &verified).await;
            },
            _ => {
                let frame = respond(&mut seq, Value::Null);
                write_msg(&mut write, &frame).await;
            },
        }
    }
}

async fn write_msg<W: AsyncWrite + Unpin>(write: &mut W, msg: &Value) {
    if let Ok(bytes) = serde_json::to_vec(msg) {
        let _ = codec::write_frame(write, &bytes).await;
    }
}

fn commands(log: &Log) -> Vec<String> {
    requests(log)
        .into_iter()
        .map(|(command, _)| command)
        .collect()
}

fn requests(log: &Log) -> Vec<(String, Value)> {
    log.lock().map(|log| log.clone()).unwrap_or_default()
}

/// The arguments of the last `command` the fake adapter received.
fn last_arguments(log: &Log, command: &str) -> Option<Value> {
    requests(log)
        .into_iter()
        .rev()
        .find(|(c, _)| c == command)
        .map(|(_, arguments)| arguments)
}

fn full_script() -> AdapterScript {
    AdapterScript {
        supports_configuration_done: true,
        support_terminate_debuggee: true,
        exception_filters: true,
        launch_after_configuration_done: true,
    }
}

#[tokio::test]
async fn the_handshake_runs_in_the_mandated_order() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    let caps = client.initialize("fake").await?;
    assert!(caps.supports_configuration_done_request);
    client
        .start(
            StartConfig {
                attach: false,
                arguments: json!({"program": "/w/a.out"}),
            },
            &[FileBreakpoints {
                path: "/w/main.rs".into(),
                lines: vec![9, 19],
            }],
        )
        .await?;
    assert_eq!(
        commands(&log),
        [
            "initialize",
            "launch",
            "setBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone",
        ]
    );
    Ok(())
}

#[tokio::test]
async fn gating_skips_unsupported_configuration_requests() -> TestResult {
    // A bare adapter: no configurationDone, no exception filters — neither
    // request may be sent, and launch answers immediately.
    let (mut client, log) = spawn_fake_adapter(AdapterScript::default());
    client.initialize("fake").await?;
    client.start(StartConfig::default(), &[]).await?;
    assert_eq!(commands(&log), ["initialize", "launch"]);
    Ok(())
}

#[tokio::test]
async fn set_breakpoints_round_trips_zero_based_lines() -> TestResult {
    let (mut client, _log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    client.start(StartConfig::default(), &[]).await?;
    let acked = client
        .set_breakpoints(Path::new("/w/main.rs"), &[7, 21])
        .await?;
    assert_eq!(acked.len(), 2);
    assert_eq!(acked[0].line, Some(7), "wire 1-based, api 0-based");
    assert!(acked[0].verified);
    assert!(!acked[1].verified, "the fake defers later breakpoints");
    Ok(())
}

#[tokio::test]
async fn stopped_and_late_verification_events_arrive() -> TestResult {
    let (mut client, _log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    client.start(StartConfig::default(), &[]).await?;
    let mut events = client.events();
    client.conn.request("emit-stopped", Value::Null).await?;
    let mut stopped = None;
    let mut verified = None;
    for _ in 0..4 {
        match tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await {
            Ok(Ok(DebugEvent::Stopped { reason, .. })) => stopped = Some(reason),
            Ok(Ok(DebugEvent::BreakpointChanged(bp))) => verified = Some(bp),
            Ok(Ok(_)) => {},
            _ => break,
        }
        if stopped.is_some() && verified.is_some() {
            break;
        }
    }
    assert_eq!(stopped.as_deref(), Some("breakpoint"));
    let bp = verified.ok_or("no late verification event")?;
    assert_eq!(bp.line, Some(7), "wire line 8 → 0-based 7");
    assert!(bp.verified);
    Ok(())
}

#[tokio::test]
async fn inspection_converts_positions_and_rejections_surface() -> TestResult {
    let (mut client, _log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    client.start(StartConfig::default(), &[]).await?;
    let stack = client.stack_trace(1).await?;
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].line, 9, "wire line 10 → 0-based 9");
    assert_eq!(
        stack[0].source_path.as_deref(),
        Some(Path::new("/w/main.rs"))
    );
    let ok = client.evaluate("1 + 41", Some(41)).await?;
    assert_eq!(ok.result, "42");
    let err = client.evaluate("boom", None).await;
    assert!(
        matches!(err, Err(DapError::Adapter(ref m)) if m.contains("invalid expression")),
        "{err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn disconnect_gates_terminate_debuggee_on_the_capability() -> TestResult {
    // Advertised: the caller's choice goes on the wire verbatim, either way.
    for terminate in [true, false] {
        let (mut client, log) = spawn_fake_adapter(full_script());
        client.initialize("fake").await?;
        client.disconnect(terminate).await?;
        assert_eq!(
            last_arguments(&log, "disconnect"),
            Some(json!({ "terminateDebuggee": terminate })),
        );
    }

    // Not advertised: disconnect is still sent, but without the flag, even
    // when the caller asked to terminate.
    let (mut client, log) = spawn_fake_adapter(AdapterScript::default());
    client.initialize("fake").await?;
    client.disconnect(true).await?;
    assert_eq!(last_arguments(&log, "disconnect"), Some(json!({})));
    Ok(())
}

#[tokio::test]
async fn run_controls_send_their_command_for_the_given_thread() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    client.resume(1).await?;
    client.step_over(2).await?;
    client.step_in(3).await?;
    client.step_out(4).await?;
    client.pause(5).await?;
    let sent: Vec<(String, Value)> = requests(&log).into_iter().skip(1).collect();
    let expected: Vec<(String, Value)> = [
        ("continue", 1),
        ("next", 2),
        ("stepIn", 3),
        ("stepOut", 4),
        ("pause", 5),
    ]
    .into_iter()
    .map(|(command, thread)| (command.to_owned(), json!({ "threadId": thread })))
    .collect();
    assert_eq!(sent, expected);
    Ok(())
}

#[tokio::test]
async fn threads_drop_entries_without_an_id_and_default_the_name() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    let threads = client.threads().await?;
    assert_eq!(
        threads,
        [
            Thread {
                id: 1,
                name: "main".to_owned(),
            },
            Thread {
                id: 2,
                name: String::new(),
            },
        ]
    );
    assert_eq!(
        last_arguments(&log, "threads"),
        Some(Value::Null),
        "threads takes no arguments"
    );
    Ok(())
}

#[tokio::test]
async fn scopes_request_the_frame_and_default_missing_fields() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    let scopes = client.scopes(41).await?;
    assert_eq!(last_arguments(&log, "scopes"), Some(json!({"frameId": 41})));
    let got: Vec<(&str, i64, bool)> = scopes
        .iter()
        .map(|s| (s.name.as_str(), s.variables_reference, s.expensive))
        .collect();
    assert_eq!(
        got,
        [("Locals", 7, false), ("Globals", 8, true), ("", 0, false)]
    );
    Ok(())
}

#[tokio::test]
async fn variables_request_the_reference_and_default_missing_fields() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    let variables = client.variables(7).await?;
    assert_eq!(
        last_arguments(&log, "variables"),
        Some(json!({"variablesReference": 7}))
    );
    let field = |name: &str, value: &str, ty: Option<&str>, reference| Variable {
        name: name.to_owned(),
        value: value.to_owned(),
        ty: ty.map(str::to_owned),
        variables_reference: reference,
    };
    assert_eq!(
        variables,
        [
            field("x", "1", Some("i32"), 0),
            field("v", "Vec(2)", None, 9),
            field("", "", None, 0),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn evaluate_sends_the_frame_only_when_given() -> TestResult {
    let (mut client, log) = spawn_fake_adapter(full_script());
    client.initialize("fake").await?;
    client.evaluate("a", Some(41)).await?;
    assert_eq!(
        last_arguments(&log, "evaluate"),
        Some(json!({"expression": "a", "context": "repl", "frameId": 41}))
    );
    client.evaluate("b", None).await?;
    assert_eq!(
        last_arguments(&log, "evaluate"),
        Some(json!({"expression": "b", "context": "repl"}))
    );
    Ok(())
}

#[tokio::test]
async fn a_dead_adapter_fails_requests_and_synthesizes_terminated() -> TestResult {
    let (client_end, server_end) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(client_end);
    let client = DapClient::connect(read, write);
    let mut events = client.events();
    drop(server_end); // the adapter dies before answering anything
    let got = client.conn.request("threads", Value::Null).await;
    assert!(matches!(got, Err(DapError::Closed)), "{got:?}");
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await;
    assert!(matches!(event, Ok(Ok(DebugEvent::Terminated))), "{event:?}");
    Ok(())
}

#[test]
fn port_substitution_replaces_every_occurrence() {
    let args: Vec<String> = ["--port", "${port}", "--x=${port}:${port}", "plain"]
        .map(str::to_owned)
        .into();
    assert_eq!(
        substitute_port(&args, 4711),
        ["--port", "4711", "--x=4711:4711", "plain"]
    );
    assert!(substitute_port(&[], 4711).is_empty());
}

#[test]
fn free_port_is_a_bindable_loopback_port() -> TestResult {
    let port = free_port()?;
    assert_ne!(port, 0);
    // Dropped, so it is free again for the adapter to bind.
    drop(std::net::TcpListener::bind(("127.0.0.1", port))?);
    Ok(())
}

#[tokio::test]
async fn a_missing_adapter_executable_is_a_launch_error() {
    let missing = "karet-dap-test-no-such-adapter";
    for transport in [DapTransport::Stdio, DapTransport::Tcp] {
        let spec = DapSpec {
            command: missing.to_owned(),
            args: vec!["--port".to_owned(), "${port}".to_owned()],
            transport,
        };
        let got = DapClient::spawn(&spec).await;
        assert!(
            matches!(got, Err(DapError::Launch(_))),
            "{transport:?}: {:?}",
            got.err()
        );
    }
    let got = DapClient::spawn_command_tcp(tokio::process::Command::new(missing), 1).await;
    assert!(matches!(got, Err(DapError::Launch(_))), "{:?}", got.err());
}

#[tokio::test]
async fn a_socket_that_never_accepts_is_a_launch_error_at_the_deadline() -> TestResult {
    let port = free_port()?; // nobody listens on it
    let started = std::time::Instant::now();
    let got = connect_with_retries(port, Duration::from_millis(200)).await;
    let elapsed = started.elapsed();
    assert!(
        matches!(&got, Err(DapError::Launch(m)) if m.contains(&port.to_string())),
        "{:?}",
        got.err()
    );
    assert!(
        elapsed >= Duration::from_millis(200),
        "gave up before the deadline"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_command_tcp_retries_until_the_adapter_listens() -> TestResult {
    // The child stands in for an adapter that is slow to boot; the socket is
    // bound here, only after the client has started retrying, and speaks DAP
    // so the connection is proven end to end.
    let port = free_port()?;
    let adapter = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let (stream, _) = listener.accept().await?;
        let (read, write) = stream.into_split();
        let log: Log = Arc::default();
        run_adapter(BufReader::new(read), write, full_script(), Arc::clone(&log)).await;
        Ok::<_, std::io::Error>(commands(&log))
    });
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", "exec sleep 30"]);
    let mut client = DapClient::spawn_command_tcp(command, port).await?;
    let caps = client.initialize("fake").await?;
    assert!(caps.support_terminate_debuggee);
    client.disconnect(false).await?;
    let seen = tokio::time::timeout(Duration::from_secs(5), adapter).await???;
    assert_eq!(seen, ["initialize", "disconnect"]);
    Ok(())
}

#[test]
fn error_displays() {
    assert_eq!(
        DapError::Closed.to_string(),
        "the debug-adapter connection is closed"
    );
    assert_eq!(
        DapError::Launch("x: gone".to_owned()).to_string(),
        "failed to launch the debug adapter: x: gone"
    );
}

#[tokio::test]
async fn a_reverse_request_is_refused_rather_than_ignored() -> TestResult {
    // karet runs adapters headless, so `runInTerminal` cannot be honoured --
    // but an adapter blocks on its reverse request, so it must be *answered*.
    let (client_end, server_end) = tokio::io::duplex(1 << 20);
    let (server_read, mut server_write) = tokio::io::split(server_end);
    let (read, write) = tokio::io::split(client_end);
    let _client = DapClient::connect(read, write);
    let mut server_read = BufReader::new(server_read);

    let request = json!({
        "seq": 42,
        "type": "request",
        "command": "runInTerminal",
        "arguments": {"args": ["echo", "hi"]},
    });
    codec::write_frame(&mut server_write, &serde_json::to_vec(&request)?).await?;

    let bytes = codec::read_frame(&mut server_read)
        .await?
        .ok_or("the adapter was never answered")?;
    let refusal: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(refusal["type"], json!("response"));
    assert_eq!(refusal["request_seq"], json!(42));
    assert_eq!(refusal["command"], json!("runInTerminal"));
    assert_eq!(refusal["success"], json!(false));
    Ok(())
}

#[tokio::test]
async fn every_reverse_request_is_refused_even_once_the_queue_fills() -> TestResult {
    // A pipe too narrow to absorb the refusals, so the writer stalls and the
    // bounded outbound queue fills behind it. `try_send` discarded whatever
    // arrived then, and the adapter waited on those forever -- during session
    // start, which is exactly when `runInTerminal` is issued.
    const BURST: i64 = 400;

    let (client_end, server_end) = tokio::io::duplex(128);
    let (server_read, mut server_write) = tokio::io::split(server_end);
    let (read, write) = tokio::io::split(client_end);
    let _client = DapClient::connect(read, write);
    let mut server_read = BufReader::new(server_read);

    let sender = tokio::spawn(async move {
        for seq in 1..=BURST {
            let request = json!({
                "seq": seq,
                "type": "request",
                "command": "runInTerminal",
            });
            let Ok(bytes) = serde_json::to_vec(&request) else {
                return;
            };
            if codec::write_frame(&mut server_write, &bytes).await.is_err() {
                return;
            }
        }
    });

    let mut answered = Vec::new();
    for _ in 1..=BURST {
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            codec::read_frame(&mut server_read),
        )
        .await
        .map_err(|_| "a refusal was dropped once the outbound queue filled")??
        .ok_or("the adapter stream ended early")?;
        let refusal: Value = serde_json::from_slice(&bytes)?;
        answered.push(refusal["request_seq"].as_i64().unwrap_or_default());
    }
    sender.await?;

    // Every request being answered is the point. Order is kept too: deferred
    // refusals share one FIFO drainer and nothing overtakes them.
    assert_eq!(answered, (1..=BURST).collect::<Vec<_>>());
    Ok(())
}

#[tokio::test]
async fn deferred_refusals_share_one_drainer_task() -> TestResult {
    // An adapter that floods reverse requests while never reading its stdin.
    // Each refusal that met the full outbound queue used to get a detached
    // task of its own, so the task count grew with the flood.
    const BURST: i64 = 400;

    let (client_end, server_end) = tokio::io::duplex(128);
    let (server_read, mut server_write) = tokio::io::split(server_end);
    let (read, write) = tokio::io::split(client_end);
    let _client = DapClient::connect(read, write);
    let mut server_read = BufReader::new(server_read);

    // Into a 128-byte pipe, so this completes only once the client has read
    // (and refused) nearly the whole flood -- while nothing drains a refusal.
    let flood = tokio::spawn(async move {
        for seq in 1..=BURST {
            let request = json!({"seq": seq, "type": "request", "command": "runInTerminal"});
            let Ok(bytes) = serde_json::to_vec(&request) else {
                return;
            };
            if codec::write_frame(&mut server_write, &bytes).await.is_err() {
                return;
            }
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), flood)
        .await
        .map_err(|_| "the client stopped reading the adapter")??;

    let alive = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    assert!(
        alive <= 3,
        "{alive} tasks alive with ~{BURST} refusals deferred"
    );

    let mut answered = Vec::new();
    for _ in 1..=BURST {
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            codec::read_frame(&mut server_read),
        )
        .await
        .map_err(|_| "a deferred refusal was dropped")??
        .ok_or("the adapter stream ended early")?;
        let refusal: Value = serde_json::from_slice(&bytes)?;
        answered.push(refusal["request_seq"].as_i64().unwrap_or_default());
    }
    assert_eq!(answered, (1..=BURST).collect::<Vec<_>>());
    Ok(())
}

#[tokio::test]
async fn parked_refusals_are_still_written_after_the_adapter_stdout_ends() -> TestResult {
    // An adapter that floods reverse requests and then closes its stdout
    // while still reading its stdin. The reader hits EOF with most refusals
    // parked; they must still be written, or the adapter -- alive, and
    // blocked on its reverse requests -- waits on them forever.
    use tokio::io::AsyncWriteExt;

    // Far more refusals than a 128-byte pipe plus the outbound queue hold.
    const BURST: i64 = 400;

    let (client_end, server_end) = tokio::io::duplex(128);
    let (server_read, mut server_write) = tokio::io::split(server_end);
    let (read, write) = tokio::io::split(client_end);
    let client = DapClient::connect(read, write);
    let mut events = client.events();
    let mut server_read = BufReader::new(server_read);

    let flood = tokio::spawn(async move {
        for seq in 1..=BURST {
            let request = json!({"seq": seq, "type": "request", "command": "runInTerminal"});
            let Ok(bytes) = serde_json::to_vec(&request) else {
                return;
            };
            if codec::write_frame(&mut server_write, &bytes).await.is_err() {
                return;
            }
        }
        // EOF on the client's reader; the other direction stays open.
        let _ = server_write.shutdown().await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), flood)
        .await
        .map_err(|_| "the client stopped reading the adapter")??;

    // The synthesized `Terminated` is sent only once the reader has exited.
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv()).await;
    assert!(matches!(event, Ok(Ok(DebugEvent::Terminated))), "{event:?}");
    assert!(client.conn.is_closed(), "the reader should have seen EOF");

    let mut answered = Vec::new();
    for _ in 1..=BURST {
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            codec::read_frame(&mut server_read),
        )
        .await
        .map_err(|_| "a refusal parked before EOF was never written")??
        .ok_or("the client stopped writing at EOF")?;
        let refusal: Value = serde_json::from_slice(&bytes)?;
        answered.push(refusal["request_seq"].as_i64().unwrap_or_default());
    }
    assert_eq!(answered, (1..=BURST).collect::<Vec<_>>());
    Ok(())
}

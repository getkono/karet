//! The TCP connect leg on real loopback sockets: the free-port pick, the
//! retry deadline, the self-connect guard, and the relaunch when another
//! process takes the chosen port.

use super::*;

#[test]
fn free_port_is_a_bindable_loopback_port() -> TestResult {
    // Dropped, so it is free again for the adapter to bind. Between the drop
    // and the bind, a concurrent test's connect can take the port as its
    // ephemeral source port, so one refusal proves nothing; a `free_port`
    // that kept its listener would be refused on every attempt.
    let mut last = None;
    for _ in 0..8 {
        let port = free_port()?;
        assert_ne!(port, 0);
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                return Ok(());
            },
            Err(e) => last = Some(e),
        }
    }
    Err(format!("no port from free_port was bindable: {last:?}").into())
}

#[tokio::test]
async fn a_socket_that_never_accepts_is_a_launch_error_at_the_deadline() -> TestResult {
    // Hold the port with a socket that is bound but never listens: connects to
    // it are refused, and for as long as it lives no concurrent test can bind
    // the port and no connect can pick it as its own source port (a loopback
    // self-connect). A `free_port()` that is dropped at once leaves both open,
    // and the test then flakes by connecting successfully.
    let holder = tokio::net::TcpSocket::new_v4()?;
    holder.bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let port = holder.local_addr()?.port();
    let started = std::time::Instant::now();
    let got = connect_with_retries(port, Duration::from_millis(200), || Readiness::Try).await;
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
    drop(holder);
    Ok(())
}

#[tokio::test]
async fn a_connection_to_a_listener_is_not_a_self_connect() -> TestResult {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let accept = tokio::spawn(async move { listener.accept().await });
    let TcpConnect::Connected(got) =
        connect_with_retries(port, Duration::from_millis(200), || Readiness::Try).await?
    else {
        return Err("reported an exit nobody signalled".into());
    };
    assert!(!is_self_connect(&got));
    accept.await??;
    Ok(())
}

#[tokio::test]
async fn an_exited_adapter_stops_the_retries_before_any_connect() -> TestResult {
    // Something listens on the port, but the adapter is already gone: that
    // listener is not the adapter, so the client must not connect to it, and
    // must not wait out the deadline either.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let started = std::time::Instant::now();
    let got = connect_with_retries(port, Duration::from_secs(5), || {
        Readiness::Lost("exit 3".to_owned())
    })
    .await?;
    assert!(matches!(got, TcpConnect::Lost(ref why) if why == "exit 3"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "waited out the deadline"
    );
    drop(listener);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_launch_whose_port_is_taken_relaunches_on_a_fresh_port() -> TestResult {
    // Another process listens on the first port while the adapter is still
    // up, so no exit gives the loss away; only the socket table does. The
    // client must never connect there and must relaunch on a fresh port; the
    // handshake then proves it reached the real adapter.
    let ports = tcp_launch_scenario(true).await?;
    assert!(ports.len() >= 2, "{ports:?}");
    assert_ne!(ports[0], ports[1]);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_live_adapter_whose_port_is_always_taken_never_connects() -> TestResult {
    // Every launch finds a foreign listener on its port, and every adapter
    // stays up, however long it boots. The launch must fail clearly well
    // before the connect deadline, and no foreign listener may ever see a
    // connection.
    let spec = DapSpec {
        command: "sh".to_owned(),
        args: vec!["--port".to_owned(), "${port}".to_owned()],
        transport: DapTransport::Tcp,
    };
    let mut foreign = Vec::new();
    let started = std::time::Instant::now();
    let got = DapClient::spawn_with(&spec, |spec| {
        let port: u16 = spec.args[1]
            .parse()
            .map_err(|e| DapError::Launch(format!("unsubstituted port: {e}")))?;
        let mut command = tokio::process::Command::new(&spec.command);
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            foreign.push(listener);
            command.args(["-c", "exec cat >/dev/null"]);
        } else {
            // A concurrent test holds the port, perhaps without listening:
            // the adapter cannot bind it either, and exits as one would.
            command.args(["-c", "exit 3"]);
        }
        Ok(command)
    })
    .await;
    assert!(
        matches!(&got, Err(DapError::Launch(m)) if m.contains("could not serve its port")),
        "{:?}",
        got.err()
    );
    assert!(
        started.elapsed() < TCP_CONNECT_TIMEOUT,
        "waited out the deadline"
    );
    assert!(!foreign.is_empty(), "no foreign listener was ever bound");
    for listener in &foreign {
        listener.set_nonblocking(true)?;
        let accepted = listener.accept();
        assert!(
            matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "connected to a foreign listener: {accepted:?}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_adapter_that_loses_its_port_every_time_is_a_launch_error() -> TestResult {
    // Every launch finds a foreign listener on its port and an adapter that
    // exits: after the bounded attempts the launch fails clearly, and never
    // hands back a client connected to a foreign listener.
    let spec = DapSpec {
        command: "sh".to_owned(),
        args: vec!["--port".to_owned(), "${port}".to_owned()],
        transport: DapTransport::Tcp,
    };
    let mut foreign = Vec::new();
    let mut launches = 0;
    let got = DapClient::spawn_with(&spec, |spec| {
        launches += 1;
        let port: u16 = spec.args[1]
            .parse()
            .map_err(|e| DapError::Launch(format!("unsubstituted port: {e}")))?;
        // A port someone else already took is just as foreign.
        if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
            foreign.push(listener);
        }
        let mut command = tokio::process::Command::new(&spec.command);
        command.args(["-c", "exit 3"]);
        Ok(command)
    })
    .await;
    assert!(
        matches!(&got, Err(DapError::Launch(m)) if m.contains("could not serve its port")),
        "{:?}",
        got.err()
    );
    assert_eq!(launches, TCP_LAUNCH_ATTEMPTS);
    assert!(!foreign.is_empty(), "no foreign listener was ever bound");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_loopback_self_connect_is_recognised() -> TestResult {
    // Connecting a socket to its own bound address is a TCP simultaneous open
    // with itself: Linux completes it, which is exactly the phantom
    // "adapter" `connect_with_retries` must not hand back.
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let addr = socket.local_addr()?;
    let stream = socket.connect(addr).await?;
    assert!(is_self_connect(&stream));
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_discarded_self_connect_leaves_the_port_bindable() -> TestResult {
    // A normal close of a self-connection leaves its port in TIME_WAIT, and
    // the adapter's listener could not bind it before the connect deadline.
    // The bind below also races concurrent tests for the freed port, but only
    // for the few milliseconds the teardown takes.
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let addr = socket.local_addr()?;
    let stream = socket.connect(addr).await?;
    // The abortive close tears the socket down asynchronously (the RST goes
    // through loopback), so the port frees within milliseconds rather than at
    // once; a TIME_WAIT would hold it for about a minute, far past this bound.
    discard_self_connect(stream);
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                drop(listener);
                return Ok(());
            },
            Err(e) if std::time::Instant::now() >= deadline => return Err(e.into()),
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_with_tcp_retries_until_the_adapter_listens_and_holds_stdin_open() -> TestResult {
    // The child is an adapter that is slow to boot: it listens on the port
    // `spawn_with` substituted into its arguments only after the client has
    // started retrying, and speaks DAP so the connection is proven end to end.
    // It exits when its stdin reaches EOF, as a supervisor wrapper does, so it
    // is still running after the handshake only if the client holds stdin
    // open. A concurrent test can take the port first (as a connect's source
    // port); the adapter then cannot bind it and is relaunched, so only a run
    // that relaunches every time is a live adapter relaunched.
    let mut ports = Vec::new();
    for _ in 0..8 {
        ports = tcp_launch_scenario(false).await?;
        if ports.len() == 1 {
            return Ok(());
        }
    }
    Err(format!("relaunched a live adapter: {ports:?}").into())
}

/// The variable that turns this test binary into the adapter process
/// [`tcp_launch_scenario`] spawns; its value is the port to serve.
#[cfg(unix)]
const ADAPTER_PORT: &str = "KARET_DAP_TEST_ADAPTER_PORT";

/// The adapter process [`tcp_launch_scenario`] spawns by running this test
/// binary again with [`ADAPTER_PORT`] set, so its listener lives in the
/// adapter's process tree, where a real adapter's does. It boots for 300ms,
/// exits 3 if it cannot bind the port (as an adapter that lost it does),
/// serves one DAP connection, and exits once its stdin reaches EOF.
#[cfg(unix)]
#[test]
#[ignore = "the adapter process the TCP launch tests spawn; does nothing on its own"]
fn tcp_adapter_process() -> TestResult {
    let Ok(port) = std::env::var(ADAPTER_PORT) else {
        return Ok(());
    };
    let port: u16 = port.parse()?;
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        std::process::exit(0);
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await else {
            std::process::exit(3);
        };
        let (stream, _) = listener.accept().await?;
        let (read, write) = stream.into_split();
        run_adapter(BufReader::new(read), write, full_script(), Arc::default()).await;
        Ok(())
    })
}

/// One run of the slow-booting TCP adapter scenario, returning every port
/// `prepare` was handed.
///
/// With `foreign_first`, the first launch loses its port while its adapter
/// stays up: `prepare` binds a listener on the port in this process, outside
/// the adapter's process tree, and launches an adapter that never exits and
/// never listens. Only the relaunch gets the real adapter, and the foreign
/// listener must never see a connection.
#[cfg(unix)]
async fn tcp_launch_scenario(
    foreign_first: bool,
) -> Result<Vec<u16>, Box<dyn std::error::Error + Send + Sync>> {
    let spec = DapSpec {
        command: "unused".to_owned(),
        args: vec!["--port".to_owned(), "${port}".to_owned()],
        transport: DapTransport::Tcp,
    };
    let mut ports = Vec::new();
    let mut foreign = None;
    let launched = DapClient::spawn_with(&spec, |spec| {
        let port: u16 = spec.args[1]
            .parse()
            .map_err(|e| DapError::Launch(format!("unsubstituted port: {e}")))?;
        ports.push(port);
        if foreign_first && ports.len() == 1 {
            foreign = std::net::TcpListener::bind(("127.0.0.1", port)).ok();
            let mut command = tokio::process::Command::new("sh");
            // A concurrent test holding the port, perhaps without listening,
            // keeps the adapter from binding it too: it exits as one would.
            let script = if foreign.is_some() {
                "exec cat >/dev/null"
            } else {
                "exit 3"
            };
            command.args(["-c", script]);
            return Ok(command);
        }
        let exe = std::env::current_exe().map_err(|e| DapError::Launch(e.to_string()))?;
        let mut command = tokio::process::Command::new(exe);
        command
            .args([
                "--ignored",
                "--exact",
                "tests::connect::tcp_adapter_process",
            ])
            .env(ADAPTER_PORT, port.to_string());
        Ok(command)
    })
    .await;
    let mut client = launched?;
    let caps = client.initialize("fake").await?;
    assert!(caps.support_terminate_debuggee);
    let child = client._child.as_mut().ok_or("no child held")?;
    assert!(child.try_wait()?.is_none(), "stdin reached EOF early");
    client.disconnect(false).await?;
    if let Some(foreign) = foreign {
        foreign.set_nonblocking(true)?;
        let accepted = foreign.accept();
        assert!(
            matches!(&accepted, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "connected to the foreign listener: {accepted:?}"
        );
    }
    Ok(ports)
}

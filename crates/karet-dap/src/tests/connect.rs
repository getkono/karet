//! The TCP connect leg on real loopback sockets: the free-port pick, the
//! retry deadline, and the self-connect guard.

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
    drop(holder);
    Ok(())
}

#[tokio::test]
async fn a_connection_to_a_listener_is_not_a_self_connect() -> TestResult {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let accept = tokio::spawn(async move { listener.accept().await });
    let got = connect_with_retries(port, Duration::from_millis(200)).await?;
    assert!(!is_self_connect(&got));
    accept.await??;
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

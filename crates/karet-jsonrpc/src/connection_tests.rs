//! Tests for the connection actor, over a scripted in-memory peer.

use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use super::*;
use crate::METHOD_NOT_FOUND;
use crate::framing::content_length;
use crate::framing::content_length::ContentLength;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A handler that broadcasts every notification and answers `test/answer`.
struct TestHandler;

impl Handler for TestHandler {
    type Framing = ContentLength;
    type Push = (String, Value);

    const PEER: &'static str = "test peer";

    fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
        (method != "test/private").then(|| (method.to_owned(), params.clone()))
    }

    fn answer(&self, method: &str, params: &Value) -> Result<Value, ResponseError> {
        match method {
            "test/answer" => Ok(json!({"echoed": params.clone()})),
            _ => Err(ResponseError::method_not_found(method)),
        }
    }
}

/// The scripted fake peer side of an in-memory connection.
struct FakePeer {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl FakePeer {
    /// Read one message, or `Null` on EOF/parse failure.
    async fn recv(&mut self) -> Value {
        match content_length::read_frame(&mut self.reader).await {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    async fn send(&mut self, message: &Value) {
        let bytes = serde_json::to_vec(message).unwrap_or_default();
        let _ = content_length::write_frame(&mut self.writer, &bytes).await;
    }

    async fn respond(&mut self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }
}

/// An in-memory wire: the client's `(read, write)` halves plus the fake peer
/// holding the other end.
fn wire() -> ((ReadHalf<DuplexStream>, WriteHalf<DuplexStream>), FakePeer) {
    let (client_end, peer_end) = tokio::io::duplex(1 << 20);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (peer_read, peer_write) = tokio::io::split(peer_end);
    (
        (client_read, client_write),
        FakePeer {
            reader: BufReader::new(peer_read),
            writer: peer_write,
        },
    )
}

#[tokio::test]
async fn responses_correlate_out_of_order() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let peer_task = tokio::spawn(async move {
        let first = peer.recv().await;
        let second = peer.recv().await;
        assert_eq!(first["method"], "test/one");
        assert_eq!(second["method"], "test/two");
        // Ids we allocate are numbers, starting at 1.
        assert_eq!(first["id"], json!(1));
        assert_eq!(second["id"], json!(2));
        let second_id = second["id"].clone();
        let first_id = first["id"].clone();
        peer.respond(&second_id, json!("two")).await;
        peer.respond(&first_id, json!("one")).await;
    });

    let (one, two) = tokio::join!(
        connection.request::<_, String>("test/one", Value::Null),
        connection.request::<_, String>("test/two", Value::Null),
    );
    assert_eq!(one?, "one");
    assert_eq!(two?, "two");
    peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn error_responses_map_to_peer_errors() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let peer_task = tokio::spawn(async move {
        let req = peer.recv().await;
        peer.send(&json!({"jsonrpc": "2.0", "id": req["id"],
                          "error": {"code": -32000, "message": "boom"}}))
            .await;
    });
    let err = connection
        .request::<_, Value>("test/fails", Value::Null)
        .await;
    let Err(RpcError::Peer { method, error }) = err else {
        return Err("expected a peer error".into());
    };
    assert_eq!(method, "test/fails");
    assert_eq!((error.code, error.message.as_str()), (-32000, "boom"));
    peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn malformed_results_decode_fail() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let peer_task = tokio::spawn(async move {
        let req = peer.recv().await;
        let id = req["id"].clone();
        peer.respond(&id, json!({"not": "a string"})).await;
    });
    let err = connection
        .request::<_, String>("test/typed", Value::Null)
        .await;
    assert!(matches!(err, Err(RpcError::Decode { .. })), "got {err:?}");
    peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn unanswered_requests_time_out() -> TestResult {
    let ((read, write), peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    // Keep the peer end alive but silent, so the failure is a timeout, not a
    // closed connection.
    let err = connection
        .request_with::<_, Value>("test/silence", Value::Null, Duration::from_millis(50))
        .await;
    assert!(matches!(err, Err(RpcError::Timeout)));
    drop(peer);
    Ok(())
}

#[tokio::test]
async fn eof_fails_in_flight_requests_with_closed() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let peer_task = tokio::spawn(async move {
        let req = peer.recv().await;
        assert_eq!(req["method"], "test/doomed");
        drop(peer); // hang up without answering
    });
    let err = connection
        .request::<_, Value>("test/doomed", Value::Null)
        .await;
    assert!(matches!(err, Err(RpcError::Closed)));
    peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn requests_after_eof_fail_fast_with_closed() -> TestResult {
    let ((read, write), peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    drop(peer); // the peer is gone before any request is issued
    tokio::task::yield_now().await;
    // A generous deadline proves we do NOT wait it out.
    let started = std::time::Instant::now();
    let err = connection
        .request_with::<_, Value>("test/late", Value::Null, Duration::from_secs(30))
        .await;
    assert!(matches!(err, Err(RpcError::Closed)), "got {err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn notifications_reach_the_wire() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    connection.notify("test/note", json!({"n": 1}))?;
    let note = peer.recv().await;
    assert_eq!(
        note,
        json!({"jsonrpc": "2.0", "method": "test/note", "params": {"n": 1}})
    );
    Ok(())
}

#[tokio::test]
async fn peer_requests_are_answered_with_a_verbatim_id() -> TestResult {
    let ((read, write), mut peer) = wire();
    let _connection = Connection::start(TestHandler, read, write);
    peer.send(&json!({"jsonrpc": "2.0", "id": "peer-1",
                      "method": "test/answer", "params": {"a": 1}}))
        .await;
    let answered = peer.recv().await;
    assert_eq!(
        answered,
        json!({"jsonrpc": "2.0", "id": "peer-1", "result": {"echoed": {"a": 1}}})
    );

    peer.send(&json!({"jsonrpc": "2.0", "id": 9, "method": "test/unknown"}))
        .await;
    let refused = peer.recv().await;
    assert_eq!(refused["id"], json!(9));
    assert_eq!(refused["error"]["code"], json!(METHOD_NOT_FOUND));
    Ok(())
}

#[tokio::test]
async fn push_payloads_fan_out_and_can_be_dropped() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let mut pushes = connection.subscribe();
    // Suppressed by `push_payload` returning `None`.
    peer.send(&json!({"jsonrpc": "2.0", "method": "test/private", "params": 1}))
        .await;
    peer.send(&json!({"jsonrpc": "2.0", "method": "test/public", "params": {"x": 2}}))
        .await;
    let (method, params) = pushes.recv().await?;
    assert_eq!(method, "test/public");
    assert_eq!(params, json!({"x": 2}));
    Ok(())
}

#[tokio::test]
async fn junk_frames_do_not_kill_the_connection() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let peer_task = tokio::spawn(async move {
        let req = peer.recv().await;
        // A well-framed but non-JSON body …
        let _ = content_length::write_frame(&mut peer.writer, b"this is not json").await;
        // … a JSON body with no JSON-RPC shape …
        peer.send(&json!(["still", "not", "jsonrpc"])).await;
        // … and a response whose id matches nothing we issued.
        peer.respond(&json!("no-such-id"), json!("ignored")).await;
        let id = req["id"].clone();
        peer.respond(&id, json!("survived")).await;
    });
    let result: String = connection.request("test/resilient", Value::Null).await?;
    assert_eq!(result, "survived");
    peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn close_drains_queued_frames() -> TestResult {
    let ((read, write), mut peer) = wire();
    let mut connection = Connection::start(TestHandler, read, write);
    connection.notify("test/first", Value::Null)?;
    connection.notify("test/second", Value::Null)?;
    connection.close().await;
    assert_eq!(peer.recv().await["method"], "test/first");
    assert_eq!(peer.recv().await["method"], "test/second");
    Ok(())
}

#[tokio::test]
async fn line_delimited_framing_drives_the_same_actor() -> TestResult {
    /// The same handler, over newline-delimited JSON instead.
    struct LineHandler;
    impl Handler for LineHandler {
        type Framing = crate::framing::line_delimited::LineDelimited;
        type Push = (String, Value);

        fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
            Some((method.to_owned(), params.clone()))
        }
    }

    let (client_end, peer_end) = tokio::io::duplex(1 << 16);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (peer_read, peer_write) = tokio::io::split(peer_end);
    let connection = Connection::start(LineHandler, client_read, client_write);
    let peer_task = tokio::spawn(async move {
        let mut reader = BufReader::new(peer_read);
        let mut writer = peer_write;
        let Ok(Some(bytes)) = crate::framing::line_delimited::read_frame(&mut reader).await else {
            return;
        };
        let request: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": "lined"});
        let body = serde_json::to_vec(&reply).unwrap_or_default();
        let _ = writer.write_all(&body).await;
        let _ = writer.write_all(b"\n").await;
        let _ = writer.flush().await;
    });
    let result: String = connection.request("test/lined", Value::Null).await?;
    assert_eq!(result, "lined");
    peer_task.await?;
    Ok(())
}

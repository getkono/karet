//! Tests for the connection actor, over a scripted in-memory peer.

use std::sync::atomic::AtomicUsize;

use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;

use super::*;
use crate::INTERNAL_ERROR;
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

#[tokio::test]
async fn zero_declared_capacities_are_clamped_not_panicked() -> TestResult {
    /// A handler declaring the capacities `tokio`'s channels panic on.
    struct ZeroCapacityHandler;

    impl Handler for ZeroCapacityHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        const PUSH_CHANNEL_CAPACITY: usize = 0;
        const OUTBOUND_CHANNEL_CAPACITY: usize = 0;

        fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
            Some((method.to_owned(), params.clone()))
        }
    }

    let ((read, write), mut peer) = wire();
    // Starting at all is half the assertion: unclamped, both channel
    // constructors would panic on a zero capacity.
    let connection = Connection::start(ZeroCapacityHandler, read, write);
    let mut pushes = connection.subscribe();
    let peer_task = tokio::spawn(async move {
        let req = peer.recv().await;
        let id = req["id"].clone();
        peer.respond(&id, json!("clamped")).await;
        peer.send(&json!({"jsonrpc": "2.0", "method": "test/pushed", "params": {"n": 1}}))
            .await;
        peer
    });
    let result: String = connection.request("test/clamped", Value::Null).await?;
    assert_eq!(result, "clamped");
    let (method, params) = pushes.recv().await?;
    assert_eq!(method, "test/pushed");
    assert_eq!(params, json!({"n": 1}));
    let _peer = peer_task.await?;
    Ok(())
}

#[tokio::test]
async fn default_answer_refuses_and_default_on_notification_absorbs() -> TestResult {
    /// A handler overriding neither `answer` nor `on_notification`.
    struct DefaultsHandler;

    impl Handler for DefaultsHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
            Some((method.to_owned(), params.clone()))
        }
    }

    let ((read, write), mut peer) = wire();
    let _connection = Connection::start(DefaultsHandler, read, write);

    // The default `answer` refuses every method, naming it.
    peer.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "test/anything", "params": {"a": 1}}))
        .await;
    let refused = peer.recv().await;
    assert_eq!(refused["id"], json!(1));
    assert_eq!(refused["error"]["code"], json!(METHOD_NOT_FOUND));
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("test/anything"),
        "got {refused}"
    );

    // The default `on_notification` absorbs the notification rather than
    // killing the reader: the request behind it is still answered.
    peer.send(&json!({"jsonrpc": "2.0", "method": "test/ignored", "params": {"b": 2}}))
        .await;
    peer.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "test/again"}))
        .await;
    let second = peer.recv().await;
    assert_eq!(second["id"], json!(2));
    assert_eq!(second["error"]["code"], json!(METHOD_NOT_FOUND));
    Ok(())
}

#[tokio::test]
async fn handler_exposes_the_handler_it_was_started_with() -> TestResult {
    /// A handler carrying state the caller can observe through `handler()`.
    struct LabelledHandler {
        label: &'static str,
        notifications: AtomicUsize,
    }

    impl Handler for LabelledHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push> {
            Some((method.to_owned(), params.clone()))
        }

        fn on_notification(&self, _method: &str, _params: Value) {
            self.notifications.fetch_add(1, Ordering::SeqCst);
        }
    }

    let ((read, write), mut peer) = wire();
    let connection = Connection::start(
        LabelledHandler {
            label: "marker",
            notifications: AtomicUsize::new(0),
        },
        read,
        write,
    );
    assert_eq!(connection.handler().label, "marker");
    assert_eq!(connection.handler().notifications.load(Ordering::SeqCst), 0);

    peer.send(&json!({"jsonrpc": "2.0", "method": "test/counted"}))
        .await;
    // The reader routes frames in order, so an answered request behind the
    // notification proves the notification already reached the handler.
    peer.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "test/sync"}))
        .await;
    assert_eq!(peer.recv().await["id"], json!(1));
    assert_eq!(connection.handler().notifications.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn closed_resolves_when_the_peer_hangs_up_with_nothing_in_flight() -> TestResult {
    // The defect this covers: loss detection used to be demand-driven. A consumer
    // parked on its own input, with no request outstanding, never learned the peer
    // had gone -- it found out only when it next tried to talk.
    let ((read, write), peer) = wire();
    let connection = Connection::start(TestHandler, read, write);

    // Nothing has been sent, and nothing is pending.
    drop(peer);

    tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .map_err(|_| "closed() did not resolve after the peer hung up")?;
    Ok(())
}

#[tokio::test]
async fn closed_resolves_immediately_for_an_already_dead_connection() -> TestResult {
    // A caller that arrives after the death must not wait for a transition it
    // already missed.
    let ((read, write), peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    drop(peer);
    connection.closed().await;

    // Second call, long after the fact, on a fresh subscription.
    tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .map_err(|_| "closed() blocked on a connection that was already gone")?;
    Ok(())
}

#[tokio::test]
async fn closed_resolves_when_framing_is_lost_rather_than_at_eof() -> TestResult {
    // A truncated frame is a lost connection, not a clean end; the liveness
    // signal has to fire for it too, or a corrupt stream reads as healthy.
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    peer.writer
        .write_all(b"Content-Length: oops\r\n\r\n")
        .await?;

    tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .map_err(|_| "closed() did not resolve after the stream lost framing")?;
    Ok(())
}

#[tokio::test]
async fn a_consumer_answers_peer_requests_with_a_verbatim_id() -> TestResult {
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let mut requests = connection
        .inbound_requests()
        .ok_or("the peer-request stream was already taken")?;

    // A second take must not hand out the same stream: a request has exactly
    // one answerer, and two receivers would race for it.
    assert!(connection.inbound_requests().is_none());

    peer.send(&json!({"jsonrpc": "2.0", "id": "peer-1",
                      "method": "workspace/applyEdit", "params": {"edit": 1}}))
        .await;
    let request = requests.recv().await.ok_or("no peer request arrived")?;
    assert_eq!(request.method, "workspace/applyEdit");
    assert_eq!(request.params, json!({"edit": 1}));
    assert_eq!(request.responder.id(), &json!("peer-1"));
    request.responder.ok(json!({"applied": true}));

    let answered = peer.recv().await;
    assert_eq!(
        answered,
        json!({"jsonrpc": "2.0", "id": "peer-1", "result": {"applied": true}})
    );
    Ok(())
}

#[tokio::test]
async fn a_dropped_responder_still_answers_the_peer() -> TestResult {
    // Discipline is not a mechanism: a consumer that forgets to answer must
    // not leave the peer blocked forever.
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let mut requests = connection
        .inbound_requests()
        .ok_or("the peer-request stream was already taken")?;

    peer.send(&json!({"jsonrpc": "2.0", "id": 7, "method": "window/showDocument"}))
        .await;
    let request = requests.recv().await.ok_or("no peer request arrived")?;
    drop(request);

    let answered = peer.recv().await;
    assert_eq!(answered["id"], json!(7));
    assert_eq!(answered["error"]["code"], json!(METHOD_NOT_FOUND));
    Ok(())
}

#[tokio::test]
async fn a_slow_consumer_does_not_block_the_rest_of_the_connection() -> TestResult {
    // The whole reason the stream exists. `Handler::answer` ran on the reader
    // task, so an answer that awaited anything stalled every other message.
    // Here a peer request is held unanswered while a client request completes.
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let mut requests = connection
        .inbound_requests()
        .ok_or("the peer-request stream was already taken")?;

    peer.send(&json!({"jsonrpc": "2.0", "id": "slow", "method": "window/showMessageRequest"}))
        .await;
    let held = requests.recv().await.ok_or("no peer request arrived")?;

    let peer_task = tokio::spawn(async move {
        let outgoing = peer.recv().await;
        let id = outgoing["id"].clone();
        peer.respond(&id, json!("answered anyway")).await;
        peer
    });

    // Completes while `held` is still unanswered, which is the assertion.
    let reply: String = tokio::time::timeout(
        Duration::from_secs(5),
        connection.request("test/while-busy", Value::Null),
    )
    .await
    .map_err(|_| "a held peer request blocked an unrelated client request")??;
    assert_eq!(reply, "answered anyway");

    let mut peer = peer_task.await?;
    held.responder.ok(json!(null));
    assert_eq!(peer.recv().await["id"], json!("slow"));
    Ok(())
}

#[tokio::test]
async fn an_overflowing_peer_request_queue_answers_rather_than_drops() -> TestResult {
    struct OneDeepHandler;

    impl Handler for OneDeepHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        const INBOUND_REQUEST_CAPACITY: usize = 1;

        fn push_payload(&self, _method: &str, _params: &Value) -> Option<Self::Push> {
            None
        }
    }

    let ((read, write), mut peer) = wire();
    let connection = Connection::start(OneDeepHandler, read, write);
    // Taken but deliberately never drained, so the queue fills and stays full.
    let _requests = connection
        .inbound_requests()
        .ok_or("the peer-request stream was already taken")?;

    for id in 1..=3 {
        peer.send(&json!({"jsonrpc": "2.0", "id": id, "method": "workspace/configuration"}))
            .await;
    }

    // The first fills the one slot; the two that overflow are answered here
    // rather than discarded. A broadcast channel would have dropped them.
    let first = peer.recv().await;
    let second = peer.recv().await;
    assert_eq!(first["id"], json!(2));
    assert_eq!(second["id"], json!(3));
    // `-32603`, not `-32601`. Back-pressure is a transient condition; a peer
    // told "method not found" concludes the client does not implement the
    // method at all and stops offering the feature.
    assert_eq!(first["error"]["code"], json!(INTERNAL_ERROR));
    assert_eq!(second["error"]["code"], json!(INTERNAL_ERROR));
    Ok(())
}

#[tokio::test]
async fn the_handler_still_answers_while_nobody_holds_the_stream() -> TestResult {
    // The compatibility guarantee: a consumer that never takes the stream sees
    // exactly the behaviour it had before the stream existed.
    let ((read, write), mut peer) = wire();
    let _connection = Connection::start(TestHandler, read, write);
    peer.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "test/answer", "params": {"a": 2}}))
        .await;
    assert_eq!(peer.recv().await["result"], json!({"echoed": {"a": 2}}));
    Ok(())
}

#[tokio::test]
async fn dropping_the_stream_falls_back_to_the_handler() -> TestResult {
    // A consumer that takes the stream and then goes away -- finished,
    // cancelled, panicked -- must not permanently disable `Handler::answer`.
    // Tracking only "was it ever taken" meant every later peer request was
    // refused, including the constant answers the handler implements.
    let ((read, write), mut peer) = wire();
    let connection = Connection::start(TestHandler, read, write);
    let requests = connection
        .inbound_requests()
        .ok_or("the peer-request stream was already taken")?;
    drop(requests);

    peer.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "test/answer", "params": {"a": 1}}))
        .await;
    let answered = peer.recv().await;
    assert_eq!(
        answered["result"],
        json!({"echoed": {"a": 1}}),
        "the handler should answer once nobody holds the stream"
    );
    Ok(())
}

#[tokio::test]
async fn a_full_outbound_queue_delays_a_reply_instead_of_dropping_it() -> TestResult {
    struct NarrowHandler;

    impl Handler for NarrowHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        const OUTBOUND_CHANNEL_CAPACITY: usize = 1;

        fn push_payload(&self, _method: &str, _params: &Value) -> Option<Self::Push> {
            None
        }

        fn answer(&self, _method: &str, _params: &Value) -> Result<Value, ResponseError> {
            Ok(json!("ok"))
        }
    }

    // A pipe too narrow to absorb the replies, so the writer stalls and the
    // one-slot outbound queue fills up behind it.
    let (client_end, peer_end) = tokio::io::duplex(64);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (peer_read, peer_write) = tokio::io::split(peer_end);
    let mut peer = FakePeer {
        reader: BufReader::new(peer_read),
        writer: peer_write,
    };
    let _connection = Connection::start(NarrowHandler, client_read, client_write);

    for id in 1..=4 {
        peer.send(&json!({"jsonrpc": "2.0", "id": id, "method": "test/x"}))
            .await;
    }

    // Every reply arrives once the peer starts draining. `try_send` alone
    // discarded the ones that found the queue full, and the peer then waited
    // on them forever.
    let mut seen = Vec::new();
    for _ in 1..=4 {
        let reply = tokio::time::timeout(Duration::from_secs(5), peer.recv())
            .await
            .map_err(|_| "a reply was dropped when the outbound queue filled")?;
        seen.push(reply["id"].clone());
    }
    seen.sort_by_key(serde_json::Value::to_string);
    assert_eq!(seen, vec![json!(1), json!(2), json!(3), json!(4)]);
    Ok(())
}

#[tokio::test]
async fn deferred_replies_share_one_drainer_and_keep_their_order() -> TestResult {
    // A peer that floods requests while never reading its stdin. Each reply
    // that met the full outbound queue used to get a detached task of its
    // own, so the task count -- and the memory behind it -- grew with the
    // flood rather than staying per-connection.
    struct NarrowHandler;

    impl Handler for NarrowHandler {
        type Framing = ContentLength;
        type Push = (String, Value);

        const OUTBOUND_CHANNEL_CAPACITY: usize = 1;

        fn push_payload(&self, _method: &str, _params: &Value) -> Option<Self::Push> {
            None
        }

        fn answer(&self, _method: &str, _params: &Value) -> Result<Value, ResponseError> {
            Ok(json!("ok"))
        }
    }

    const BURST: i64 = 200;

    let (client_end, peer_end) = tokio::io::duplex(64);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (peer_read, peer_write) = tokio::io::split(peer_end);
    let mut peer = FakePeer {
        reader: BufReader::new(peer_read),
        writer: peer_write,
    };
    let _connection = Connection::start(NarrowHandler, client_read, client_write);

    // Writes into a 64-byte pipe, so the flood completes only once the reader
    // has consumed (and answered) nearly all of it -- while nothing drains
    // the replies.
    let flood = tokio::spawn(async move {
        for id in 1..=BURST {
            peer.send(&json!({"jsonrpc": "2.0", "id": id, "method": "test/x"}))
                .await;
        }
        peer
    });
    let mut peer = tokio::time::timeout(Duration::from_secs(10), flood)
        .await
        .map_err(|_| "the reader stopped draining the peer")??;

    // Reader, writer, drainer -- and nothing per deferred reply.
    let alive = tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks();
    assert!(
        alive <= 3,
        "{alive} tasks alive with ~{BURST} replies deferred"
    );

    // One FIFO drainer, and no fast-path overtaking while anything is parked:
    // the replies arrive in the order the requests did.
    let mut seen = Vec::new();
    for _ in 1..=BURST {
        let reply = tokio::time::timeout(Duration::from_secs(5), peer.recv())
            .await
            .map_err(|_| "a deferred reply was dropped")?;
        seen.push(reply["id"].as_i64().unwrap_or_default());
    }
    assert_eq!(seen, (1..=BURST).collect::<Vec<_>>());
    Ok(())
}

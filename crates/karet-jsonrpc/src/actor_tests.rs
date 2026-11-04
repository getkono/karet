//! Tests for the envelope-generic actor, over an envelope that is deliberately
//! *not* JSON-RPC: string ids, a `re` field instead of `id`, and newline
//! framing. Everything JSON-RPC-shaped is covered through `Connection`.

use std::sync::atomic::AtomicUsize;

use serde_json::json;
use tokio::io::BufReader;
use tokio::io::DuplexStream;
use tokio::io::ReadHalf;
use tokio::io::WriteHalf;
use tokio::sync::Mutex as AsyncMutex;

use super::*;
use crate::framing::line_delimited::LineDelimited;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Responses are `{"re": id, "value": …}`; anything with a `ping` is routed
/// and answered with `{"pong": ping, "n": seq}` through the link.
#[derive(Default)]
struct Toy {
    pings: AtomicUsize,
    closes: Arc<AtomicUsize>,
}

impl Envelope for Toy {
    type Framing = LineDelimited;
    type Id = String;
    type Outcome = Value;
    type Message = Value;

    const PEER: &'static str = "toy";
    const OUTBOUND_CHANNEL_CAPACITY: usize = 1;

    fn request_id(seq: i64) -> String {
        format!("r{seq}")
    }

    fn encode_request<P: Serialize>(
        &self,
        id: &String,
        method: &str,
        params: P,
    ) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&json!({"ask": id, "method": method, "params": params}))
    }

    fn classify(&self, value: Value) -> Classified<String, Value, Value> {
        if let Some(id) = value.get("re").and_then(Value::as_str) {
            return Classified::Response {
                id: id.to_owned(),
                outcome: value.get("value").cloned().unwrap_or(Value::Null),
            };
        }
        if value.get("ping").is_some() {
            return Classified::Message(value);
        }
        Classified::Malformed("a message with no toy shape")
    }

    fn route(&self, message: Value, link: &Link) {
        self.pings.fetch_add(1, Ordering::SeqCst);
        let pong = json!({"pong": message["ping"], "n": link.next_seq()});
        link.reply(serde_json::to_vec(&pong).unwrap_or_default());
    }

    fn on_closed(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

/// The scripted peer end of an in-memory wire.
struct Peer {
    reader: AsyncMutex<BufReader<ReadHalf<DuplexStream>>>,
    writer: AsyncMutex<WriteHalf<DuplexStream>>,
}

impl Peer {
    async fn recv(&self) -> Value {
        let mut reader = self.reader.lock().await;
        match LineDelimited::read_frame(&mut *reader).await {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    async fn send(&self, message: &Value) {
        let bytes = serde_json::to_vec(message).unwrap_or_default();
        let mut writer = self.writer.lock().await;
        let _ = LineDelimited::write_frame(&mut *writer, &bytes).await;
    }
}

fn start(toy: Toy) -> (Actor<Toy>, Peer) {
    let (client_end, peer_end) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(client_end);
    let (peer_read, peer_write) = tokio::io::split(peer_end);
    (
        Actor::start(toy, read, write),
        Peer {
            reader: AsyncMutex::new(BufReader::new(peer_read)),
            writer: AsyncMutex::new(peer_write),
        },
    )
}

/// Falsified by the actor assuming JSON-RPC ids: the envelope's string ids
/// must key the pending map, and out-of-order answers must still correlate.
#[tokio::test]
async fn an_envelope_of_its_own_correlates_out_of_order() -> TestResult {
    let (actor, peer) = start(Toy::default());
    let first = actor.dispatch("one", 1, Duration::from_secs(5)).await?;
    let second = actor.dispatch("two", 2, Duration::from_secs(5)).await?;
    let sent = (peer.recv().await, peer.recv().await);
    assert_eq!(sent.0, json!({"ask": "r1", "method": "one", "params": 1}));
    assert_eq!(sent.1["ask"], "r2");
    peer.send(&json!({"re": "r2", "value": "two"})).await;
    peer.send(&json!({"re": "r1", "value": "one"})).await;
    assert_eq!(second.await?, json!("two"));
    assert_eq!(first.await?, json!("one"));
    assert_eq!(actor.in_flight(), 0);
    Ok(())
}

/// Falsified by a malformed message killing the reader, or by `route` not
/// reaching the wire through the link with a seq from the shared counter.
#[tokio::test]
async fn malformed_messages_are_dropped_and_routed_ones_reply_through_the_link() -> TestResult {
    let (actor, peer) = start(Toy::default());
    peer.send(&json!({"neither": true})).await;
    peer.send(&json!({"ping": "a"})).await;
    // Request seq 1 was never used, so the link's first allocation is 1.
    assert_eq!(peer.recv().await, json!({"pong": "a", "n": 1}));
    assert_eq!(actor.envelope().pings.load(Ordering::SeqCst), 1);
    let pending = actor
        .dispatch("after", Value::Null, Duration::from_secs(5))
        .await?;
    assert_eq!(peer.recv().await["ask"], "r2");
    peer.send(&json!({"re": "r2", "value": null})).await;
    assert_eq!(pending.await?, Value::Null);
    Ok(())
}

/// Falsified by EOF leaving a dispatched request to wait out its timeout, by
/// `on_closed` not running, or by a later request not failing fast.
#[tokio::test]
async fn eof_fails_dispatched_requests_and_runs_on_closed() -> TestResult {
    let closes = Arc::new(AtomicUsize::new(0));
    let (actor, peer) = start(Toy {
        closes: Arc::clone(&closes),
        ..Toy::default()
    });
    let pending = actor
        .dispatch("hang", Value::Null, Duration::from_secs(30))
        .await?;
    let _ = peer.recv().await;
    drop(peer);
    let outcome = tokio::time::timeout(Duration::from_secs(5), pending).await?;
    assert!(matches!(outcome, Err(RpcError::Closed)));
    tokio::time::timeout(Duration::from_secs(5), actor.closed()).await?;
    assert!(actor.is_closed());
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        actor.request("late", Value::Null).await,
        Err(RpcError::Closed)
    ));
    assert_eq!(actor.in_flight(), 0);
    Ok(())
}

/// Falsified by a timed-out request lingering in the pending map, or by
/// `try_send` blocking instead of failing fast on a full queue.
#[tokio::test]
async fn timeouts_forget_and_a_full_queue_fails_fast() -> TestResult {
    let (actor, peer) = start(Toy::default());
    let outcome = actor
        .request_with("slow", Value::Null, Duration::from_millis(20))
        .await;
    assert!(matches!(outcome, Err(RpcError::Timeout)));
    assert_eq!(actor.in_flight(), 0);
    let _ = peer.recv().await;
    // Capacity 1, and nobody drains the wire past the duplex buffer: fill it.
    let big = vec![b'x'; 1 << 17];
    let mut full = false;
    for _ in 0..8 {
        if matches!(actor.try_send(big.clone()), Err(RpcError::QueueFull)) {
            full = true;
            break;
        }
    }
    assert!(full, "a full outbound queue must refuse rather than wait");
    Ok(())
}

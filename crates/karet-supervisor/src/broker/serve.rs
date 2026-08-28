//! The protocol-agnostic broker skeleton.
//!
//! Everything here is about *processes and plumbing*: publishing the endpoint,
//! spawning the brokered process, authenticating clients, rewriting request
//! identifiers so concurrent clients never collide, remembering who owns each
//! in-flight request, fanning server messages out, and retiring when idle. What
//! any of those messages *mean* is the [`BrokerProtocol`]'s business.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;

use crate::broker::BrokerError;
use crate::broker::endpoint::Endpoint;
use crate::broker::endpoint::write_endpoint;
use crate::broker::framing::Framing;
use crate::broker::io_error;
use crate::broker::lease;
use crate::broker::lease::BrokerSpec;
use crate::broker::protocol::BrokerProtocol;
use crate::broker::protocol::ClientFlow;
use crate::broker::protocol::ClientId;
use crate::broker::protocol::ClientLink;
use crate::broker::protocol::ServerLink;
use crate::broker::protocol::ServerRoute;

const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The one message a client sees when the brokered process's stdout ended.
const UPSTREAM_CLOSED: &str = "language server connection closed";
const CLIENT_QUEUE: usize = 256;

/// A request the broker forwarded upstream and still owes an answer for.
pub(crate) struct Pending<T> {
    client: ClientId,
    original_id: Value,
    tag: T,
}

/// Everything one running broker shares between its tasks.
pub(crate) struct Core<P: BrokerProtocol> {
    pub(crate) upstream: mpsc::Sender<Value>,
    pub(crate) clients: Mutex<HashMap<ClientId, mpsc::Sender<Value>>>,
    pub(crate) pending: Mutex<HashMap<u64, Pending<P::RequestTag>>>,
    pub(crate) state: Arc<P::State>,
    pub(crate) next_request: AtomicU64,
    pub(crate) next_client: AtomicU64,
    pub(crate) active_clients: AtomicUsize,
    pub(crate) last_activity: std::sync::Mutex<Instant>,
    /// Set once the brokered process's stdout ends, for any reason.
    ///
    /// A latch, not a bare [`Notify`]: `notify_waiters` wakes only the waiters
    /// registered at the moment it is called and stores nothing for anyone who
    /// arrives later. `upstream_reader` is spawned before `accept_clients`, so
    /// a server that dies immediately -- bare `taplo` printing usage is exactly
    /// this -- signalled into an empty waiter set and the broker then sat until
    /// its 30s idle timeout, long after the client gave up at 5s. The flag is
    /// what makes the signal survive that gap; the `Notify` only wakes a
    /// waiter that is already parked.
    pub(crate) upstream_closed: AtomicBool,
    pub(crate) upstream_closed_signal: Notify,
}

impl<P: BrokerProtocol> Core<P> {
    pub(crate) fn new(upstream: mpsc::Sender<Value>) -> Arc<Self> {
        Arc::new(Self {
            upstream,
            clients: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            state: Arc::new(P::State::default()),
            next_request: AtomicU64::new(1),
            next_client: AtomicU64::new(1),
            active_clients: AtomicUsize::new(0),
            last_activity: std::sync::Mutex::new(Instant::now()),
            upstream_closed: AtomicBool::new(false),
            upstream_closed_signal: Notify::new(),
        })
    }
}

/// Publish an endpoint, run the brokered process, and serve clients until idle.
///
/// The election files are given up on every exit path, including the failures
/// before a client is ever accepted. Leaving them behind cost the next
/// connector its full `CONNECT_TIMEOUT`: it could not connect to the endpoint
/// that named a dead broker, and could not take a lock that already existed.
pub(crate) async fn run_broker<P: BrokerProtocol>(spec: BrokerSpec) -> Result<(), BrokerError> {
    let outcome = run_broker_inner::<P>(&spec).await;
    if let Err(error) = &outcome {
        // The connector cannot see any of this: the server is the broker's
        // child, and the broker's own stderr goes nowhere. Recorded before the
        // endpoint is removed, so a client still waiting finds it.
        crate::broker::failure::write(
            &spec.failure,
            &crate::broker::BrokeredLaunchFailure {
                command: spec.launch.command.clone(),
                args: spec.launch.args.clone(),
                message: error.to_string(),
                // The upstream reader only reports a closed stream for a
                // process that got far enough to have one.
                ran: matches!(error, BrokerError::Io(message) if message == UPSTREAM_CLOSED),
                // Whose verdict this is. Every broker for the key writes to
                // this one path, so without it a connector cannot tell a report
                // about its own launch from one about somebody else's.
                pid: std::process::id(),
            },
        );
    }
    release_election(&spec);
    outcome
}

/// Give up the election files, but only those that still name this broker.
///
/// Removing them unconditionally is half of how one key came to have two live
/// brokers. The other half is a connector that gave up at its deadline and
/// unlinked the lease of a broker that was merely slow; a second broker was
/// then elected over the same paths, and *this* removal — running when the
/// first broker finally exited — deleted the second's endpoint file. Every
/// later connector then found no endpoint and elected yet another broker, while
/// the abandoned one held its server process open until it idled out.
fn release_election(spec: &BrokerSpec) {
    let me = std::process::id();
    if lease::published(&spec.metadata).is_some_and(|endpoint| endpoint.pid == me) {
        let _ = std::fs::remove_file(&spec.metadata);
    }
    lease::release_lease(&spec.lock, me);
}

async fn run_broker_inner<P: BrokerProtocol>(spec: &BrokerSpec) -> Result<(), BrokerError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(io_error)?;
    let endpoint = Endpoint {
        address: listener.local_addr().map_err(io_error)?,
        token: spec.token.clone(),
        pid: std::process::id(),
        command: Some(PathBuf::from(&spec.launch.command)),
    };
    write_endpoint(&spec.metadata, &endpoint)?;

    let launch = spec.launch.clone();
    let mut command = crate::supervisor::command(
        &spec.supervisor,
        launch.command.clone(),
        launch.args,
        &launch.root,
    )
    .map_err(|error| BrokerError::Io(error.to_string()))?;
    let mut child = command.spawn().map_err(io_error)?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| BrokerError::Io("server stdin unavailable".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BrokerError::Io("server stdout unavailable".to_owned()))?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "karet_supervisor::broker", "{line}");
            }
        });
    }

    let (upstream_tx, upstream_rx) = mpsc::channel(CLIENT_QUEUE);
    let core = Core::<P>::new(upstream_tx);
    tokio::spawn(message_writer::<P::Framing, _>(stdin, upstream_rx));
    tokio::spawn(upstream_reader::<P, _>(stdout, Arc::clone(&core)));

    let outcome = accept_clients::<P>(&listener, &spec.token, Arc::clone(&core)).await;
    if outcome.is_ok() {
        P::retire(&ServerLink::new(&core)).await;
    }
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    outcome
}

async fn accept_clients<P: BrokerProtocol>(
    listener: &TcpListener,
    token: &str,
    core: Arc<Core<P>>,
) -> Result<(), BrokerError> {
    let mut idle_check = tokio::time::interval(Duration::from_secs(2));
    loop {
        // Checked before every park, so a stdout that ended before this task
        // first ran is observed rather than waited on.
        if core.upstream_closed.load(Ordering::Acquire) {
            return Err(BrokerError::Io(UPSTREAM_CLOSED.to_owned()));
        }
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(io_error)?;
                let core = Arc::clone(&core);
                let token = token.to_owned();
                tokio::spawn(async move {
                    if let Err(error) = serve_client::<P>(stream, &token, core).await {
                        tracing::debug!(error = %error, "broker client disconnected");
                    }
                });
            },
            _ = idle_check.tick() => {
                let idle = core.last_activity.lock().map_or(IDLE_TIMEOUT, |last| last.elapsed());
                if core.active_clients.load(Ordering::Acquire) == 0 && idle >= IDLE_TIMEOUT {
                    return Ok(());
                }
            },
            () = core.upstream_closed_signal.notified() => {
                return Err(BrokerError::Io(UPSTREAM_CLOSED.to_owned()));
            },
        }
    }
}

async fn serve_client<P: BrokerProtocol>(
    stream: TcpStream,
    token: &str,
    core: Arc<Core<P>>,
) -> Result<(), BrokerError> {
    let client = core.next_client.fetch_add(1, Ordering::Relaxed);
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let expected = format!("{}{token}", P::PRELUDE);
    // Cap the one unauthenticated read at the only line that could be valid,
    // plus its line ending: uncapped, any local process able to reach the
    // loopback socket could stream arbitrary bytes into the broker *before*
    // authenticating. A read cut short at the cap just fails the comparison.
    let limit = expected.len() as u64 + 2;
    let mut prelude = String::new();
    (&mut reader)
        .take(limit)
        .read_line(&mut prelude)
        .await
        .map_err(io_error)?;
    if prelude.trim_end() != expected {
        return Err(BrokerError::Io("broker authentication failed".to_owned()));
    }
    let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
    core.clients.lock().await.insert(client, tx.clone());
    core.active_clients.fetch_add(1, Ordering::AcqRel);
    touch(&core);
    let writer = tokio::spawn(message_writer::<P::Framing, _>(write, rx));

    let link = ClientLink::new(&core, client, &tx);
    // The teardown below runs whatever ended the session. Letting the pump's
    // errors escape past it — as a framing error used to — would leave this
    // client in the client map with its documents still leased, and
    // `active_clients` above zero for the broker's whole life, so it could never
    // idle-retire and would hold its child process open indefinitely.
    let outcome = client_loop::<P, _>(&mut reader, &link, &core).await;

    core.clients.lock().await.remove(&client);
    P::on_client_gone(&link).await;
    core.active_clients.fetch_sub(1, Ordering::AcqRel);
    touch(&core);
    writer.abort();
    outcome
}

/// Pump one authenticated client until it stops, disconnects, or breaks framing.
async fn client_loop<P, R>(
    reader: &mut R,
    link: &ClientLink<'_, P>,
    core: &Arc<Core<P>>,
) -> Result<(), BrokerError>
where
    P: BrokerProtocol,
    R: AsyncBufRead + Unpin + Send,
{
    while let Some(mut message) = <P::Framing as Framing>::read_message(reader)
        .await
        .map_err(io_error)?
    {
        touch(core);
        match P::on_client_message(&mut message, link).await {
            ClientFlow::Proxy(tag) => {
                // Indexing `message["id"]` below is only sound on an object, and
                // an id being present proves that; a protocol asking to proxy an
                // id-less message gets it dropped rather than a panic.
                if let Some(id) = message.get("id").cloned() {
                    proxy_request(link.client(), id, tag, &mut message, core).await;
                }
            },
            ClientFlow::Forward => {
                let _ = core.upstream.send(message).await;
            },
            ClientFlow::Drop => {},
            ClientFlow::Stop => break,
        }
    }
    Ok(())
}

async fn proxy_request<P: BrokerProtocol>(
    client: ClientId,
    original_id: Value,
    tag: P::RequestTag,
    message: &mut Value,
    core: &Arc<Core<P>>,
) {
    let id = core.next_request.fetch_add(1, Ordering::Relaxed);
    message["id"] = json!(id);
    core.pending.lock().await.insert(
        id,
        Pending {
            client,
            original_id,
            tag,
        },
    );
    let _ = core.upstream.send(message.clone()).await;
}

async fn upstream_reader<P, R>(read: R, core: Arc<Core<P>>)
where
    P: BrokerProtocol,
    R: AsyncRead + Unpin + Send,
{
    let mut reader = BufReader::new(read);
    loop {
        let Ok(message) = <P::Framing as Framing>::read_message(&mut reader).await else {
            break;
        };
        let Some(mut message) = message else {
            break;
        };
        // Taken in its own statement so the guard drops here: as a let-chain
        // scrutinee it would live to the end of the `if`, holding `pending`
        // locked across `on_response` and both sends, so a slow hook or a full
        // client queue would stall every `proxy_request` insert behind it.
        let pending = if message.get("method").is_none()
            && let Some(id) = message.get("id").and_then(Value::as_u64)
        {
            core.pending.lock().await.remove(&id)
        } else {
            None
        };
        if let Some(pending) = pending {
            P::on_response(&mut message, &pending.tag, &ServerLink::new(&core)).await;
            message["id"] = pending.original_id;
            // Same reason, and the sharper case: this guard would span
            // `tx.send`, so one client that stopped draining its `CLIENT_QUEUE`
            // would block `clients` — and with it every client's join and
            // teardown, leaving `active_clients` stuck above zero.
            let client = core.clients.lock().await.get(&pending.client).cloned();
            if let Some(tx) = client {
                let _ = tx.send(message).await;
            }
            continue;
        }
        let clients: Vec<_> = core.clients.lock().await.values().cloned().collect();
        match P::route_server_message(&message) {
            ServerRoute::SingleClient => {
                if let Some(tx) = clients.first() {
                    let _ = tx.send(message).await;
                }
            },
            ServerRoute::AllClients => {
                for tx in clients {
                    let _ = tx.send(message.clone()).await;
                }
            },
            ServerRoute::Discard => {},
        }
    }
    core.upstream_closed.store(true, Ordering::Release);
    core.upstream_closed_signal.notify_waiters();
}

async fn message_writer<F, W>(mut writer: W, mut messages: mpsc::Receiver<Value>)
where
    F: Framing,
    W: AsyncWrite + Unpin + Send,
{
    while let Some(message) = messages.recv().await {
        if F::write_message(&mut writer, &message).await.is_err() {
            break;
        }
    }
}

fn touch<P: BrokerProtocol>(core: &Core<P>) {
    if let Ok(mut activity) = core.last_activity.lock() {
        *activity = Instant::now();
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
pub(crate) mod tests;

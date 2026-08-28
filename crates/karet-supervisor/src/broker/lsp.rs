//! LSP semantics for the broker skeleton.
//!
//! This is the *only* module in `karet-supervisor` allowed to know what an LSP
//! message means, and the only one that may name `karet_lsp`. Everything the
//! skeleton needs from it arrives through [`BrokerProtocol`] and [`Framing`]:
//! `initialize` de-duplication, `didOpen`/`didClose` reference counting,
//! `shutdown`/`exit` interception, `Content-Length` framing, and the
//! server-to-client routing policy.

use std::collections::HashMap;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use karet_lsp::LspSpec;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::Notify;

use crate::broker::BrokerError;
use crate::broker::endpoint;
use crate::broker::framing::Framing;
use crate::broker::key::Launch;
use crate::broker::lease;
use crate::broker::protocol::BrokerProtocol;
use crate::broker::protocol::ClientFlow;
use crate::broker::protocol::ClientId;
use crate::broker::protocol::ClientLink;
use crate::broker::protocol::ServerLink;
use crate::broker::protocol::ServerRoute;

/// Environment flag selecting the hidden LSP-broker entry point.
pub const MODE_ENV: &str = "KARET_INTERNAL_LSP_BROKER";
const SPEC_ENV: &str = "KARET_INTERNAL_LSP_BROKER_SPEC";
const PRELUDE: &str = "KARET-LSP-BROKER ";
const PROTOCOL_VERSION: &str = "1";
const STATE_DIR: &str = "brokers";

/// The language-server protocol, as the broker skeleton sees it.
#[derive(Clone, Copy, Debug)]
pub struct LspBroker;

/// Broker-wide LSP state: who has which document open, and the shared
/// `initialize` result every later client is answered from.
#[derive(Debug, Default)]
pub struct LspState {
    documents: Mutex<HashMap<String, HashSet<ClientId>>>,
    initialize_result: Mutex<Option<Value>>,
    initialize_started: AtomicBool,
    initialize_ready: Notify,
}

/// Why the broker is holding an in-flight request.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LspRequest {
    /// An ordinary request, proxied on behalf of one client.
    #[default]
    Plain,
    /// The single `initialize` whose result every other client shares.
    Initialize,
}

/// `Content-Length` framing, delegating to `karet-lsp`'s codec.
#[derive(Clone, Copy, Debug)]
pub struct ContentLength;

impl Framing for ContentLength {
    async fn read_message<R>(reader: &mut R) -> io::Result<Option<Value>>
    where
        R: AsyncBufRead + Unpin + Send,
    {
        read_message(reader).await
    }

    async fn write_message<W>(writer: &mut W, message: &Value) -> io::Result<()>
    where
        W: AsyncWrite + Unpin + Send,
    {
        write_message(writer, message).await
    }
}

impl BrokerProtocol for LspBroker {
    type State = LspState;
    type RequestTag = LspRequest;
    type Framing = ContentLength;

    const STATE_DIR: &'static str = self::STATE_DIR;
    const PROTOCOL_VERSION: &'static str = self::PROTOCOL_VERSION;
    const MODE_ENV: &'static str = self::MODE_ENV;
    const SPEC_ENV: &'static str = self::SPEC_ENV;
    const PRELUDE: &'static str = self::PRELUDE;
    const DISPLAY_NAME: &'static str = "language-server broker";

    async fn on_client_message(
        message: &mut Value,
        link: &ClientLink<'_, Self>,
    ) -> ClientFlow<LspRequest> {
        match classify(message) {
            ClientMessage::Initialize => handle_initialize(message, link).await,
            ClientMessage::Initialized => ClientFlow::Drop,
            ClientMessage::Shutdown => {
                if let Some(id) = message.get("id").cloned() {
                    link.reply(json!({"jsonrpc": "2.0", "id": id, "result": null}))
                        .await;
                }
                ClientFlow::Drop
            },
            ClientMessage::Exit => ClientFlow::Stop,
            ClientMessage::Request => ClientFlow::Proxy(LspRequest::Plain),
            ClientMessage::Notification => {
                if should_forward_notification(link.client(), message, link.state()).await {
                    ClientFlow::Forward
                } else {
                    ClientFlow::Drop
                }
            },
            ClientMessage::Reply => ClientFlow::Forward,
        }
    }

    fn route_server_message(message: &Value) -> ServerRoute {
        if message.get("id").is_some() && message.get("method").is_some() {
            ServerRoute::SingleClient
        } else {
            ServerRoute::AllClients
        }
    }

    async fn on_response(message: &mut Value, tag: &LspRequest, link: &ServerLink<'_, Self>) {
        if *tag != LspRequest::Initialize {
            return;
        }
        let state = link.state();
        if let Some(result) = message.get("result").cloned() {
            *state.initialize_result.lock().await = Some(result);
            link.send_upstream(json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            }))
            .await;
        } else {
            // The handshake failed. Release the election so a waiting client
            // runs its own `initialize` rather than waiting on a result that
            // will never arrive; the client whose attempt failed still receives
            // the error reply under its own id.
            state.initialize_started.store(false, Ordering::Release);
        }
        // Notified either way: a waiter woken with no result loops and re-elects.
        state.initialize_ready.notify_waiters();
    }

    async fn on_client_gone(link: &ClientLink<'_, Self>) {
        close_client_documents(link).await;
    }

    async fn retire(link: &ServerLink<'_, Self>) {
        let shutdown_id = link.next_request_id();
        link.send_upstream(
            json!({"jsonrpc": "2.0", "id": shutdown_id, "method": "shutdown", "params": null}),
        )
        .await;
        link.send_upstream(json!({"jsonrpc": "2.0", "method": "exit", "params": null}))
            .await;
    }
}

/// What a client message is, settled before a hook borrows it mutably.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientMessage {
    /// The one `initialize` whose result every later client shares.
    Initialize,
    /// The `initialized` notification the broker sends once, itself.
    Initialized,
    /// A `shutdown` request the broker answers without the server.
    Shutdown,
    /// The `exit` notification ending this client's session.
    Exit,
    /// Any other request: it carries an id the broker must rewrite.
    Request,
    /// Any other notification: no id, nothing to track.
    Notification,
    /// A reply to a server-originated request; travels upstream as-is.
    Reply,
}

/// Classify a client message by the *presence* of `method`, not by `method`
/// being a string.
///
/// A reply is the only client message with no `method` key at all, so a
/// malformed non-string `method` alongside an `id` is still a request and must
/// be proxied. Forwarding it verbatim would leave the client's own id on the
/// wire, and the server's answer would then be matched against whichever other
/// client's proxied request happens to carry the same id — delivering one
/// window's answer to another.
fn classify(message: &Value) -> ClientMessage {
    let Some(method) = message.get("method") else {
        return ClientMessage::Reply;
    };
    match method.as_str() {
        Some("initialize") => ClientMessage::Initialize,
        Some("initialized") => ClientMessage::Initialized,
        Some("shutdown") => ClientMessage::Shutdown,
        Some("exit") => ClientMessage::Exit,
        _ if message.get("id").is_some() => ClientMessage::Request,
        _ => ClientMessage::Notification,
    }
}

/// The single `initialize` is proxied once; everyone else is answered from the
/// cached result, waiting for it if the first request is still in flight.
///
/// The wait enrols on `initialize_ready` **before** either check, because
/// `notify_waiters` stores no permit and a `Notified` does not join the waiter
/// list until it is polled or `enable`d. Checking first and awaiting second
/// would drop a wakeup that lands in between, and since this await sits inside
/// the client's read loop a dropped wakeup wedges the whole session rather than
/// just its `initialize`.
async fn handle_initialize(
    message: &mut Value,
    link: &ClientLink<'_, LspBroker>,
) -> ClientFlow<LspRequest> {
    let state = link.state();
    // Only a request can carry the handshake. An id-less `initialize` would win
    // the election below and then be dropped by the pump for having no id to
    // rewrite, so nothing would ever answer it, release the election or notify
    // — and every later client would wait on a result nobody was producing.
    let Some(id) = message.get("id").cloned() else {
        return ClientFlow::Drop;
    };
    loop {
        let mut ready = pin!(state.initialize_ready.notified());
        ready.as_mut().enable();

        // Cloned in its own statement so the guard drops here. Held as an
        // `if let` scrutinee it would span `reply`, and one client that had
        // stopped draining its channel would park every other client's
        // `initialize` on `initialize_result` — the same wedge by another lock.
        let cached = state.initialize_result.lock().await.clone();
        if let Some(result) = cached {
            link.reply(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                .await;
            return ClientFlow::Drop;
        }
        if state
            .initialize_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return ClientFlow::Proxy(LspRequest::Initialize);
        }
        ready.await;
    }
}

/// Only the first open and the last close of a document reach the server.
async fn should_forward_notification(
    client: ClientId,
    message: &Value,
    state: &Arc<LspState>,
) -> bool {
    let method = message.get("method").and_then(Value::as_str);
    let uri = message
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str);
    match (method, uri) {
        (Some("textDocument/didOpen"), Some(uri)) => {
            let mut documents = state.documents.lock().await;
            let owners = documents.entry(uri.to_owned()).or_default();
            let first = owners.is_empty();
            owners.insert(client);
            first
        },
        (Some("textDocument/didClose"), Some(uri)) => {
            let mut documents = state.documents.lock().await;
            let Some(owners) = documents.get_mut(uri) else {
                return false;
            };
            owners.remove(&client);
            let last = owners.is_empty();
            if last {
                documents.remove(uri);
            }
            last
        },
        _ => true,
    }
}

/// Synthesise a close for every document this client was the last owner of.
async fn close_client_documents(link: &ClientLink<'_, LspBroker>) {
    let client = link.client();
    let mut closes = Vec::new();
    {
        let mut documents = link.state().documents.lock().await;
        documents.retain(|uri, owners| {
            owners.remove(&client);
            if owners.is_empty() {
                closes.push(uri.clone());
                false
            } else {
                true
            }
        });
    }
    for uri in closes {
        link.send_upstream(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {"textDocument": {"uri": uri}}
        }))
        .await;
    }
}

/// Read one framed JSON-RPC message, delegating framing (and the shared size
/// cap) to `karet-lsp`'s codec — the one Content-Length implementation.
async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<Value>> {
    let Some(body) = karet_lsp::codec::read_frame(reader)
        .await
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    karet_lsp::codec::write_frame(writer, &body).await
}

fn launch(spec: &LspSpec, root: &Path) -> Launch {
    Launch {
        command: spec.command.clone(),
        args: spec.args.clone(),
        root: root.to_path_buf(),
    }
}

/// Whether this invocation is the hidden broker child.
#[must_use]
pub fn requested() -> bool {
    lease::requested::<LspBroker>()
}

/// Run the broker described by the hidden-mode environment.
#[must_use]
pub fn run_from_env() -> i32 {
    lease::run_from_env::<LspBroker>()
}

/// Connect to the shared broker for `spec` and `root`, starting it when absent.
///
/// The returned stream has already completed the broker authentication prelude
/// and can be passed directly to `karet_lsp::LspClient::connect`.
///
/// # Errors
/// Returns [`BrokerError`] if the state directory, hidden process, or loopback
/// connection cannot be established before the startup deadline.
pub async fn connect(
    executable: &Path,
    state_root: &Path,
    spec: &LspSpec,
    root: &Path,
) -> Result<TcpStream, BrokerError> {
    lease::connect::<LspBroker>(executable, state_root, &launch(spec, root)).await
}

/// Whether a live shared broker still references a managed immutable payload.
#[must_use]
pub fn managed_payload_in_use(state_root: &Path, payload: &Path) -> bool {
    endpoint::payload_in_use(&state_root.join(STATE_DIR), payload)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::BufReader;

    use super::*;
    use crate::broker::key;
    use crate::broker::serve::Core;

    type BoxError = Box<dyn std::error::Error>;

    /// Keeps the moved tests' call shape: identity is derived from the launch
    /// description, which for LSP is a spec plus a repository root.
    fn broker_key(spec: &LspSpec, root: &Path) -> String {
        key::broker_key(PRELUDE, PROTOCOL_VERSION, &launch(spec, root))
    }

    /// The document-lease tests only touch [`LspState`]; the channel keeps the
    /// fixture's `(state, upstream)` shape from before the skeleton split.
    fn test_shared() -> (Arc<LspState>, tokio::sync::mpsc::Receiver<Value>) {
        let (_upstream, rx) = tokio::sync::mpsc::channel(8);
        (Arc::new(LspState::default()), rx)
    }

    /// Everything a [`ClientLink`] borrows, kept alive by the caller.
    struct LinkParts {
        core: Arc<Core<LspBroker>>,
        sender: tokio::sync::mpsc::Sender<Value>,
        replies: tokio::sync::mpsc::Receiver<Value>,
        _upstream: tokio::sync::mpsc::Receiver<Value>,
    }

    fn link_parts() -> LinkParts {
        let (upstream, _upstream) = tokio::sync::mpsc::channel(8);
        let (sender, replies) = tokio::sync::mpsc::channel(8);
        LinkParts {
            core: Core::<LspBroker>::new(upstream),
            sender,
            replies,
            _upstream,
        }
    }

    /// The `initialize` reply the tests hand back for the winning request.
    fn initialize_reply() -> Value {
        json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {}}})
    }

    /// Play a client that lost the election, parked on the shared result.
    ///
    /// The link's halves are moved into the task so the handle outlives this
    /// frame, and the yield lets the task reach its wait before we return.
    async fn spawn_waiter(
        core: &Arc<Core<LspBroker>>,
        sender: &tokio::sync::mpsc::Sender<Value>,
    ) -> tokio::task::JoinHandle<ClientFlow<LspRequest>> {
        let core = Arc::clone(core);
        let sender = sender.clone();
        let waiter = tokio::spawn(async move {
            let link = ClientLink::new(&core, 2, &sender);
            let mut message = json!({"jsonrpc": "2.0", "id": 9, "method": "initialize"});
            LspBroker::on_client_message(&mut message, &link).await
        });
        tokio::task::yield_now().await;
        waiter
    }

    /// Collect a waiter's outcome, failing rather than hanging the suite if a
    /// regression leaves it parked for good.
    async fn settled(
        waiter: tokio::task::JoinHandle<ClientFlow<LspRequest>>,
    ) -> Result<ClientFlow<LspRequest>, BoxError> {
        Ok(tokio::time::timeout(Duration::from_secs(5), waiter).await??)
    }

    #[test]
    fn broker_identity_separates_roots_and_launches() {
        let spec = LspSpec {
            command: "rust-analyzer".to_owned(),
            args: Vec::new(),
            languages: vec!["rust".to_owned()],
        };
        assert_eq!(
            broker_key(&spec, Path::new("/a")),
            broker_key(&spec, Path::new("/a"))
        );
        assert_ne!(
            broker_key(&spec, Path::new("/a")),
            broker_key(&spec, Path::new("/b"))
        );
        let mut other = spec.clone();
        other.args.push("--different".to_owned());
        assert_ne!(
            broker_key(&spec, Path::new("/a")),
            broker_key(&other, Path::new("/a"))
        );
    }

    #[tokio::test]
    async fn framing_rejects_oversized_messages() -> Result<(), Box<dyn std::error::Error>> {
        let input = b"Content-Length: 70000000\r\n\r\n";
        let mut reader = BufReader::new(&input[..]);
        let error = read_message(&mut reader)
            .await
            .err()
            .ok_or("expected error")?;
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        Ok(())
    }

    #[tokio::test]
    async fn document_leases_forward_only_first_open_and_last_close() {
        let (shared, _upstream) = test_shared();
        let open = json!({
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": "file:///repo/main.rs"}}
        });
        let close = json!({
            "method": "textDocument/didClose",
            "params": {"textDocument": {"uri": "file:///repo/main.rs"}}
        });
        assert!(should_forward_notification(1, &open, &shared).await);
        assert!(!should_forward_notification(2, &open, &shared).await);
        assert!(!should_forward_notification(1, &close, &shared).await);
        assert!(should_forward_notification(2, &close, &shared).await);
    }

    #[tokio::test]
    async fn a_non_string_method_with_an_id_is_still_proxied() {
        let parts = link_parts();
        let link = ClientLink::new(&parts.core, 1, &parts.sender);
        let mut message = json!({"jsonrpc": "2.0", "id": 1, "method": 42});
        assert_eq!(
            LspBroker::on_client_message(&mut message, &link).await,
            ClientFlow::Proxy(LspRequest::Plain)
        );
    }

    #[tokio::test]
    async fn a_non_string_method_without_an_id_is_forwarded() {
        let parts = link_parts();
        let link = ClientLink::new(&parts.core, 1, &parts.sender);
        let mut message = json!({"jsonrpc": "2.0", "method": 42});
        assert_eq!(
            LspBroker::on_client_message(&mut message, &link).await,
            ClientFlow::Forward
        );
    }

    #[tokio::test]
    async fn a_reply_without_a_method_travels_upstream_as_is() {
        let parts = link_parts();
        let link = ClientLink::new(&parts.core, 1, &parts.sender);
        let mut message = json!({"jsonrpc": "2.0", "id": 1, "result": null});
        assert_eq!(
            LspBroker::on_client_message(&mut message, &link).await,
            ClientFlow::Forward
        );
    }
    #[tokio::test]
    async fn a_waiting_client_is_answered_from_the_shared_result() -> Result<(), BoxError> {
        let mut parts = link_parts();
        parts
            .core
            .state
            .initialize_started
            .store(true, Ordering::Release);
        let waiter = spawn_waiter(&parts.core, &parts.sender).await;

        let mut answered = initialize_reply();
        LspBroker::on_response(
            &mut answered,
            &LspRequest::Initialize,
            &ServerLink::new(&parts.core),
        )
        .await;

        assert_eq!(settled(waiter).await?, ClientFlow::Drop);
        let reply = parts.replies.try_recv()?;
        assert_eq!(reply.get("id"), Some(&json!(9)));
        assert_eq!(reply.get("result"), Some(&json!({"capabilities": {}})));
        Ok(())
    }

    /// A wakeup that arrives with no result yet must not be mistaken for the
    /// answer: the waiting client keeps waiting instead of returning unanswered.
    #[tokio::test]
    async fn a_spurious_wakeup_does_not_drop_a_waiting_initialize() -> Result<(), BoxError> {
        let mut parts = link_parts();
        parts
            .core
            .state
            .initialize_started
            .store(true, Ordering::Release);
        let waiter = spawn_waiter(&parts.core, &parts.sender).await;

        parts.core.state.initialize_ready.notify_waiters();
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "a woken client with no result must keep waiting, not drop its initialize"
        );

        let mut answered = initialize_reply();
        LspBroker::on_response(
            &mut answered,
            &LspRequest::Initialize,
            &ServerLink::new(&parts.core),
        )
        .await;
        assert_eq!(settled(waiter).await?, ClientFlow::Drop);
        assert_eq!(parts.replies.try_recv()?.get("id"), Some(&json!(9)));
        Ok(())
    }
    /// The reply the tests hand back for an `initialize` the server refused.
    fn initialize_failure() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32603, "message": "no workspace"}
        })
    }

    #[tokio::test]
    async fn a_failed_initialize_releases_the_election() -> Result<(), BoxError> {
        let parts = link_parts();
        parts
            .core
            .state
            .initialize_started
            .store(true, Ordering::Release);

        let mut failed = initialize_failure();
        LspBroker::on_response(
            &mut failed,
            &LspRequest::Initialize,
            &ServerLink::new(&parts.core),
        )
        .await;

        assert!(parts.core.state.initialize_result.lock().await.is_none());
        let link = ClientLink::new(&parts.core, 3, &parts.sender);
        let mut retry = json!({"jsonrpc": "2.0", "id": 4, "method": "initialize"});
        assert_eq!(
            LspBroker::on_client_message(&mut retry, &link).await,
            ClientFlow::Proxy(LspRequest::Initialize)
        );
        Ok(())
    }

    /// A client already parked on the shared result must not wait on a handshake
    /// that will never land: it wakes, wins the freed election, and retries.
    #[tokio::test]
    async fn a_waiting_client_retries_when_the_handshake_fails() -> Result<(), BoxError> {
        let parts = link_parts();
        parts
            .core
            .state
            .initialize_started
            .store(true, Ordering::Release);
        let waiter = spawn_waiter(&parts.core, &parts.sender).await;

        let mut failed = initialize_failure();
        LspBroker::on_response(
            &mut failed,
            &LspRequest::Initialize,
            &ServerLink::new(&parts.core),
        )
        .await;

        assert_eq!(
            settled(waiter).await?,
            ClientFlow::Proxy(LspRequest::Initialize)
        );
        Ok(())
    }
    /// `initialize` is a request. One arriving without an id can never be
    /// answered or proxied, so it must not claim the election either.
    #[tokio::test]
    async fn an_initialize_without_an_id_does_not_claim_the_election() -> Result<(), BoxError> {
        let parts = link_parts();
        let link = ClientLink::new(&parts.core, 1, &parts.sender);

        let mut headless = json!({"jsonrpc": "2.0", "method": "initialize"});
        assert_eq!(
            LspBroker::on_client_message(&mut headless, &link).await,
            ClientFlow::Drop
        );

        let mut proper = json!({"jsonrpc": "2.0", "id": 7, "method": "initialize"});
        assert_eq!(
            LspBroker::on_client_message(&mut proper, &link).await,
            ClientFlow::Proxy(LspRequest::Initialize)
        );
        Ok(())
    }
}

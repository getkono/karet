//! Cross-process language-server broker.
//!
//! One hidden broker owns each `(server launch, repository root, karet protocol
//! version)` tuple. Editor processes connect over an authenticated loopback socket
//! and speak ordinary LSP; the broker rewrites JSON-RPC request identifiers,
//! broadcasts server notifications, and reference-counts document opens. This
//! prevents several karet windows from multiplying expensive server processes.
//! Brokers retire after an idle grace period, and stale endpoint files are
//! replaced atomically by the next connector.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use karet_lsp::LspSpec;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::mpsc;

/// Environment flag selecting the hidden LSP-broker entry point.
pub const MODE_ENV: &str = "KARET_INTERNAL_LSP_BROKER";
const SPEC_ENV: &str = "KARET_INTERNAL_LSP_BROKER_SPEC";
const PRELUDE: &str = "KARET-LSP-BROKER ";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const CLIENT_QUEUE: usize = 256;
const PROTOCOL_VERSION: &str = "1";

/// Errors returned while locating or starting a broker.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrokerError {
    /// Broker state or transport I/O failed.
    #[error("language-server broker I/O failed: {0}")]
    Io(String),
    /// Hidden broker launch state was invalid.
    #[error("invalid language-server broker state: {0}")]
    Spec(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BrokerSpec {
    launch: LspLaunch,
    metadata: PathBuf,
    lock: PathBuf,
    token: String,
    supervisor: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LspLaunch {
    command: String,
    args: Vec<String>,
    root: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct Endpoint {
    address: SocketAddr,
    token: String,
    pid: u32,
    #[serde(default)]
    command: Option<PathBuf>,
}

struct Pending {
    client: u64,
    original_id: Value,
    initialize: bool,
}

struct Shared {
    upstream: mpsc::Sender<Value>,
    clients: Mutex<HashMap<u64, mpsc::Sender<Value>>>,
    pending: Mutex<HashMap<u64, Pending>>,
    documents: Mutex<HashMap<String, HashSet<u64>>>,
    initialize_result: Mutex<Option<Value>>,
    initialize_started: AtomicBool,
    initialize_ready: Notify,
    next_request: AtomicU64,
    next_client: AtomicU64,
    active_clients: AtomicUsize,
    last_activity: std::sync::Mutex<Instant>,
    upstream_closed: Notify,
}

/// Whether this invocation is the hidden broker child.
#[must_use]
pub fn requested() -> bool {
    std::env::var_os(MODE_ENV).is_some()
}

/// Run the broker described by the hidden-mode environment.
#[must_use]
pub fn run_from_env() -> i32 {
    match run_from_env_inner() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("karet language-server broker: {error}");
            1
        },
    }
}

fn run_from_env_inner() -> Result<(), BrokerError> {
    let encoded = std::env::var(SPEC_ENV).map_err(|error| BrokerError::Spec(error.to_string()))?;
    // Hidden mode is entered before normal startup creates worker threads.
    unsafe {
        // SAFETY: no other thread exists yet, so environment mutation cannot race.
        std::env::remove_var(MODE_ENV);
        std::env::remove_var(SPEC_ENV);
    }
    let spec: BrokerSpec =
        serde_json::from_str(&encoded).map_err(|error| BrokerError::Spec(error.to_string()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| BrokerError::Io(error.to_string()))?;
    runtime.block_on(run_broker(spec))
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
    let directory = state_root.join("brokers");
    std::fs::create_dir_all(&directory).map_err(io_error)?;
    set_private_directory(&directory)?;
    let key = broker_key(spec, root);
    let metadata = directory.join(format!("{key}.json"));
    let lock = directory.join(format!("{key}.lock"));

    if let Ok(stream) = connect_existing(&metadata).await {
        return Ok(stream);
    }

    let owns_start = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock)
        .is_ok();
    if owns_start {
        let token = broker_token(&key);
        let broker = BrokerSpec {
            launch: LspLaunch {
                command: spec.command.clone(),
                args: spec.args.clone(),
                root: root.to_path_buf(),
            },
            metadata: metadata.clone(),
            lock: lock.clone(),
            token,
            supervisor: executable.to_path_buf(),
        };
        let encoded =
            serde_json::to_string(&broker).map_err(|error| BrokerError::Spec(error.to_string()))?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env(MODE_ENV, "1")
            .env(SPEC_ENV, encoded)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map_err(io_error)?;
    }

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        if let Ok(stream) = connect_existing(&metadata).await {
            return Ok(stream);
        }
        if tokio::time::Instant::now() >= deadline {
            // A hidden process normally publishes in milliseconds. At this point
            // the create-only lease is stale; removing it lets the manager's next
            // bounded retry elect a fresh owner.
            let _ = std::fs::remove_file(&lock);
            return Err(BrokerError::Io(
                "timed out waiting for shared broker".to_owned(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn connect_existing(metadata: &Path) -> Result<TcpStream, BrokerError> {
    let bytes = std::fs::read(metadata).map_err(io_error)?;
    let endpoint: Endpoint =
        serde_json::from_slice(&bytes).map_err(|error| BrokerError::Spec(error.to_string()))?;
    let mut stream = TcpStream::connect(endpoint.address)
        .await
        .map_err(io_error)?;
    stream
        .write_all(format!("{PRELUDE}{}\n", endpoint.token).as_bytes())
        .await
        .map_err(io_error)?;
    Ok(stream)
}

async fn run_broker(spec: BrokerSpec) -> Result<(), BrokerError> {
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
    let shared = Arc::new(Shared {
        upstream: upstream_tx,
        clients: Mutex::new(HashMap::new()),
        pending: Mutex::new(HashMap::new()),
        documents: Mutex::new(HashMap::new()),
        initialize_result: Mutex::new(None),
        initialize_started: AtomicBool::new(false),
        initialize_ready: Notify::new(),
        next_request: AtomicU64::new(1),
        next_client: AtomicU64::new(1),
        active_clients: AtomicUsize::new(0),
        last_activity: std::sync::Mutex::new(Instant::now()),
        upstream_closed: Notify::new(),
    });
    tokio::spawn(upstream_writer(stdin, upstream_rx));
    tokio::spawn(upstream_reader(stdout, Arc::clone(&shared)));

    let outcome = accept_clients(&listener, &spec.token, Arc::clone(&shared)).await;
    if outcome.is_ok() {
        let shutdown_id = shared.next_request.fetch_add(1, Ordering::Relaxed);
        let _ = shared
            .upstream
            .send(
                json!({"jsonrpc": "2.0", "id": shutdown_id, "method": "shutdown", "params": null}),
            )
            .await;
        let _ = shared
            .upstream
            .send(json!({"jsonrpc": "2.0", "method": "exit", "params": null}))
            .await;
    }
    if tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let _ = std::fs::remove_file(&spec.metadata);
    let _ = std::fs::remove_file(&spec.lock);
    outcome
}

/// Whether a live shared broker still references a managed immutable payload.
pub fn managed_payload_in_use(state_root: &Path, payload: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(state_root.join("brokers")) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        if entry.path().extension().and_then(std::ffi::OsStr::to_str) != Some("json") {
            return false;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            return false;
        };
        let Ok(endpoint) = serde_json::from_slice::<Endpoint>(&bytes) else {
            return false;
        };
        if std::net::TcpStream::connect_timeout(&endpoint.address, Duration::from_millis(50))
            .is_err()
        {
            return false;
        }
        endpoint
            .command
            .as_ref()
            .is_none_or(|command| command.starts_with(payload))
    })
}

async fn accept_clients(
    listener: &TcpListener,
    token: &str,
    shared: Arc<Shared>,
) -> Result<(), BrokerError> {
    let mut idle_check = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(io_error)?;
                let shared = Arc::clone(&shared);
                let token = token.to_owned();
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, &token, shared).await {
                        tracing::debug!(error = %error, "broker client disconnected");
                    }
                });
            },
            _ = idle_check.tick() => {
                let idle = shared.last_activity.lock().map_or(IDLE_TIMEOUT, |last| last.elapsed());
                if shared.active_clients.load(Ordering::Acquire) == 0 && idle >= IDLE_TIMEOUT {
                    return Ok(());
                }
            },
            () = shared.upstream_closed.notified() => {
                return Err(BrokerError::Io("language server connection closed".to_owned()));
            },
        }
    }
}

async fn serve_client(
    stream: TcpStream,
    token: &str,
    shared: Arc<Shared>,
) -> Result<(), BrokerError> {
    let client = shared.next_client.fetch_add(1, Ordering::Relaxed);
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut prelude = String::new();
    reader.read_line(&mut prelude).await.map_err(io_error)?;
    if prelude.trim_end() != format!("{PRELUDE}{token}") {
        return Err(BrokerError::Io("broker authentication failed".to_owned()));
    }
    let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
    shared.clients.lock().await.insert(client, tx.clone());
    shared.active_clients.fetch_add(1, Ordering::AcqRel);
    touch(&shared);
    let writer = tokio::spawn(client_writer(write, rx));

    while let Some(mut message) = read_message(&mut reader).await.map_err(io_error)? {
        touch(&shared);
        if message.get("method").and_then(Value::as_str) == Some("initialize") {
            handle_initialize(client, &tx, &mut message, &shared).await;
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("initialized") {
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("shutdown") {
            if let Some(id) = message.get("id").cloned() {
                let _ = tx
                    .send(json!({"jsonrpc": "2.0", "id": id, "result": null}))
                    .await;
            }
            continue;
        }
        if message.get("method").and_then(Value::as_str) == Some("exit") {
            break;
        }
        if let Some(id) = message.get("id").cloned() {
            if message.get("method").is_some() {
                proxy_request(client, id, false, &mut message, &shared).await;
            } else {
                let _ = shared.upstream.send(message).await;
            }
            continue;
        }
        if should_forward_notification(client, &message, &shared).await {
            let _ = shared.upstream.send(message).await;
        }
    }

    shared.clients.lock().await.remove(&client);
    close_client_documents(client, &shared).await;
    shared.active_clients.fetch_sub(1, Ordering::AcqRel);
    touch(&shared);
    writer.abort();
    Ok(())
}

async fn handle_initialize(
    client: u64,
    tx: &mpsc::Sender<Value>,
    message: &mut Value,
    shared: &Arc<Shared>,
) {
    if let Some(result) = shared.initialize_result.lock().await.clone() {
        if let Some(id) = message.get("id").cloned() {
            let _ = tx
                .send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                .await;
        }
        return;
    }
    if shared
        .initialize_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        if let Some(id) = message.get("id").cloned() {
            proxy_request(client, id, true, message, shared).await;
        }
        return;
    }
    shared.initialize_ready.notified().await;
    if let (Some(id), Some(result)) = (
        message.get("id").cloned(),
        shared.initialize_result.lock().await.clone(),
    ) {
        let _ = tx
            .send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
    }
}

async fn proxy_request(
    client: u64,
    original_id: Value,
    initialize: bool,
    message: &mut Value,
    shared: &Arc<Shared>,
) {
    let id = shared.next_request.fetch_add(1, Ordering::Relaxed);
    message["id"] = json!(id);
    shared.pending.lock().await.insert(
        id,
        Pending {
            client,
            original_id,
            initialize,
        },
    );
    let _ = shared.upstream.send(message.clone()).await;
}

async fn should_forward_notification(client: u64, message: &Value, shared: &Arc<Shared>) -> bool {
    let method = message.get("method").and_then(Value::as_str);
    let uri = message
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str);
    match (method, uri) {
        (Some("textDocument/didOpen"), Some(uri)) => {
            let mut documents = shared.documents.lock().await;
            let owners = documents.entry(uri.to_owned()).or_default();
            let first = owners.is_empty();
            owners.insert(client);
            first
        },
        (Some("textDocument/didClose"), Some(uri)) => {
            let mut documents = shared.documents.lock().await;
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

async fn close_client_documents(client: u64, shared: &Arc<Shared>) {
    let mut closes = Vec::new();
    {
        let mut documents = shared.documents.lock().await;
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
        let _ = shared
            .upstream
            .send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": {"textDocument": {"uri": uri}}
            }))
            .await;
    }
}

async fn upstream_writer<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut messages: mpsc::Receiver<Value>,
) {
    while let Some(message) = messages.recv().await {
        if write_message(&mut writer, &message).await.is_err() {
            break;
        }
    }
}

async fn upstream_reader<R: AsyncRead + Unpin>(read: R, shared: Arc<Shared>) {
    let mut reader = BufReader::new(read);
    loop {
        let Ok(message) = read_message(&mut reader).await else {
            break;
        };
        let Some(mut message) = message else {
            break;
        };
        if message.get("method").is_none()
            && let Some(id) = message.get("id").and_then(Value::as_u64)
            && let Some(pending) = shared.pending.lock().await.remove(&id)
        {
            if pending.initialize
                && let Some(result) = message.get("result").cloned()
            {
                *shared.initialize_result.lock().await = Some(result);
                let _ = shared
                    .upstream
                    .send(json!({
                        "jsonrpc": "2.0",
                        "method": "initialized",
                        "params": {}
                    }))
                    .await;
                shared.initialize_ready.notify_waiters();
            }
            message["id"] = pending.original_id;
            if let Some(tx) = shared.clients.lock().await.get(&pending.client).cloned() {
                let _ = tx.send(message).await;
            }
            continue;
        }
        let clients: Vec<_> = shared.clients.lock().await.values().cloned().collect();
        if message.get("id").is_some() && message.get("method").is_some() {
            if let Some(tx) = clients.first() {
                let _ = tx.send(message).await;
            }
        } else {
            for tx in clients {
                let _ = tx.send(message.clone()).await;
            }
        }
    }
    shared.upstream_closed.notify_waiters();
}

async fn client_writer<W: AsyncWrite + Unpin>(mut writer: W, mut messages: mpsc::Receiver<Value>) {
    while let Some(message) = messages.recv().await {
        if write_message(&mut writer, &message).await.is_err() {
            break;
        }
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

fn broker_key(spec: &LspSpec, root: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(PROTOCOL_VERSION);
    hash.update(env!("CARGO_PKG_VERSION"));
    hash.update(root.as_os_str().to_string_lossy().as_bytes());
    hash.update(&spec.command);
    for argument in &spec.args {
        hash.update([0]);
        hash.update(argument);
    }
    format!("{:x}", hash.finalize())
}

fn broker_token(key: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(key);
    hash.update(std::process::id().to_le_bytes());
    hash.update(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    format!("{:x}", hash.finalize())
}

fn write_endpoint(path: &Path, endpoint: &Endpoint) -> Result<(), BrokerError> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes =
        serde_json::to_vec(endpoint).map_err(|error| BrokerError::Spec(error.to_string()))?;
    std::fs::write(&temporary, bytes).map_err(io_error)?;
    set_private_file(&temporary)?;
    std::fs::rename(&temporary, path).map_err(io_error)
}

fn touch(shared: &Shared) {
    if let Ok(mut activity) = shared.last_activity.lock() {
        *activity = Instant::now();
    }
}

fn io_error(error: impl std::fmt::Display) -> BrokerError {
    BrokerError::Io(error.to_string())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io_error)
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<(), BrokerError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(io_error)
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<(), BrokerError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_shared() -> (Arc<Shared>, mpsc::Receiver<Value>) {
        let (upstream, rx) = mpsc::channel(8);
        (
            Arc::new(Shared {
                upstream,
                clients: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                documents: Mutex::new(HashMap::new()),
                initialize_result: Mutex::new(None),
                initialize_started: AtomicBool::new(false),
                initialize_ready: Notify::new(),
                next_request: AtomicU64::new(1),
                next_client: AtomicU64::new(1),
                active_clients: AtomicUsize::new(0),
                last_activity: std::sync::Mutex::new(Instant::now()),
                upstream_closed: Notify::new(),
            }),
            rx,
        )
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
}

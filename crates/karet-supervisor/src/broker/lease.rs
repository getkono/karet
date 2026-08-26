//! Broker election and the hidden entry point.
//!
//! A connector either reaches the broker already published for a key, or wins
//! an `O_EXCL` lease and spawns a hidden copy of the executable that publishes
//! one. Everything here is protocol-generic: the only thing that varies is the
//! [`BrokerProtocol`] type parameter.

use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::broker::BrokerError;
use crate::broker::endpoint;
use crate::broker::endpoint::Endpoint;
use crate::broker::io_error;
use crate::broker::key;
use crate::broker::key::Launch;
use crate::broker::protocol::BrokerProtocol;
use crate::broker::serve;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything the hidden broker process needs, passed through the environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BrokerSpec {
    pub(crate) launch: Launch,
    pub(crate) metadata: PathBuf,
    pub(crate) lock: PathBuf,
    pub(crate) token: String,
    pub(crate) supervisor: PathBuf,
}

/// Whether this invocation is the hidden broker child for `P`.
pub(crate) fn requested<P: BrokerProtocol>() -> bool {
    std::env::var_os(P::MODE_ENV).is_some()
}

/// Run the broker described by the hidden-mode environment.
pub(crate) fn run_from_env<P: BrokerProtocol>() -> i32 {
    match run_from_env_inner::<P>() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("karet {}: {error}", P::DISPLAY_NAME);
            1
        },
    }
}

fn run_from_env_inner<P: BrokerProtocol>() -> Result<(), BrokerError> {
    let encoded =
        std::env::var(P::SPEC_ENV).map_err(|error| BrokerError::Spec(error.to_string()))?;
    // Hidden mode is entered before normal startup creates worker threads.
    unsafe {
        // SAFETY: no other thread exists yet, so environment mutation cannot race.
        std::env::remove_var(P::MODE_ENV);
        std::env::remove_var(P::SPEC_ENV);
    }
    let spec: BrokerSpec =
        serde_json::from_str(&encoded).map_err(|error| BrokerError::Spec(error.to_string()))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| BrokerError::Io(error.to_string()))?;
    runtime.block_on(serve::run_broker::<P>(spec))
}

/// Connect to the shared broker for `launch`, starting it when absent.
///
/// The returned stream has already completed the authentication prelude.
pub(crate) async fn connect<P: BrokerProtocol>(
    executable: &Path,
    state_root: &Path,
    launch: &Launch,
) -> Result<TcpStream, BrokerError> {
    let directory = state_root.join(P::STATE_DIR);
    std::fs::create_dir_all(&directory).map_err(io_error)?;
    endpoint::set_private_directory(&directory)?;
    let key = key::broker_key(P::PRELUDE, P::PROTOCOL_VERSION, launch);
    let metadata = directory.join(format!("{key}.json"));
    let lock = directory.join(format!("{key}.lock"));

    if let Ok(stream) = connect_existing::<P>(&metadata).await {
        return Ok(stream);
    }

    let owns_start = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock)
        .is_ok();
    if owns_start {
        let token = key::broker_token(&key);
        let broker = BrokerSpec {
            launch: launch.clone(),
            metadata: metadata.clone(),
            lock: lock.clone(),
            token,
            supervisor: executable.to_path_buf(),
        };
        let encoded =
            serde_json::to_string(&broker).map_err(|error| BrokerError::Spec(error.to_string()))?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env(P::MODE_ENV, "1")
            .env(P::SPEC_ENV, encoded)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().map_err(io_error)?;
    }

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        if let Ok(stream) = connect_existing::<P>(&metadata).await {
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

async fn connect_existing<P: BrokerProtocol>(metadata: &Path) -> Result<TcpStream, BrokerError> {
    let bytes = std::fs::read(metadata).map_err(io_error)?;
    let endpoint: Endpoint =
        serde_json::from_slice(&bytes).map_err(|error| BrokerError::Spec(error.to_string()))?;
    let mut stream = TcpStream::connect(endpoint.address)
        .await
        .map_err(io_error)?;
    stream
        .write_all(format!("{}{}\n", P::PRELUDE, endpoint.token).as_bytes())
        .await
        .map_err(io_error)?;
    Ok(stream)
}

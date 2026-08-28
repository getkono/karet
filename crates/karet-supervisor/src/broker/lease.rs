//! Broker election and the hidden entry point.
//!
//! A connector either reaches the broker already published for a key, or wins
//! an `O_EXCL` lease and spawns a hidden copy of the executable that publishes
//! one. Everything here is protocol-generic: the only thing that varies is the
//! [`BrokerProtocol`] type parameter.
//!
//! The lease file is not empty: it records the process id of the broker it
//! elected. That identity is what the rest of the module is built on — a
//! connector that lost the election learns from it *whose* `{key}.error` it is
//! reading, and both the connector and the broker use it to be sure the files
//! they unlink are still their own.

use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::Child;

use crate::broker::BrokerError;
use crate::broker::endpoint;
use crate::broker::endpoint::Endpoint;
use crate::broker::failure;
use crate::broker::failure::read as read_failure;
use crate::broker::io_error;
use crate::broker::key;
use crate::broker::key::Launch;
use crate::broker::protocol::BrokerProtocol;
use crate::broker::serve;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a lease whose broker cannot be *proven* gone is left alone.
///
/// The backstop for the one case no ownership check covers: the broker was
/// killed before it published anything and the connector that elected it is
/// gone too, so nobody is left who can tell the lease is dead. Well past the
/// broker's own idle retirement, because a broker that is merely slow must
/// outlive it.
const STALE_LEASE: Duration = Duration::from_secs(60);
/// Cap on one greeting exchange against a published endpoint.
///
/// Short on purpose: a broker writes its acknowledgement the instant it
/// accepts, and an expired greeting proves nothing, so the caller simply greets
/// again on its next 25ms turn. A longer cap would only make paths that are
/// already failing answer more slowly.
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);
/// Cap on the greeting a peer may write before it has said anything sensible.
const MAX_GREETING: usize = 128;

/// Everything the hidden broker process needs, passed through the environment.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BrokerSpec {
    pub(crate) launch: Launch,
    pub(crate) metadata: PathBuf,
    pub(crate) lock: PathBuf,
    /// Where a broker that gave up before serving records why.
    ///
    /// The broker owns the server process, so only it can tell "the child died"
    /// from "the broker is slow to publish". Without this the connector saw the
    /// same startup timeout either way and had to assume the optimistic one,
    /// which meant a server that could never start was retried forever.
    pub(crate) failure: PathBuf,
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

/// The endpoint published for a key, if one is there and intact.
pub(crate) fn published(metadata: &Path) -> Option<Endpoint> {
    serde_json::from_slice(&std::fs::read(metadata).ok()?).ok()
}

/// The broker a lease names, once the connector that elected it recorded one.
pub(crate) fn lease_owner(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock).ok()?.trim().parse().ok()
}

/// Release `lock`, but only while it still names `owner`.
///
/// Every removal in this module goes through here. An unconditional unlink is
/// how one key came to have two brokers: whoever removed the lease had no way
/// of knowing whether it was still the one it took.
///
/// The check and the unlink are two syscalls on a path, and POSIX has no
/// "unlink if the contents still say this" — so a lease that its owner releases
/// and a third connector immediately re-takes in the window between them is
/// removed anyway. That is left as it is, deliberately: every atomic
/// alternative available here is worse. A rename-away trades deleting a
/// stranger's lease for *stealing* it, with no safe way to put it back;
/// `flock` guards an inode, and the whole race is that the inode at this path
/// changed; a separate mutex file would have to be taken by the election too,
/// adding a wedge-until-timeout failure mode to the one path that must never
/// have one. `renameat2(RENAME_EXCHANGE)` would do it and is Linux-only, for a
/// dependency this crate does not carry. The exposure is bounded and
/// self-healing in a way none of those are: one extra broker, holding one
/// server process, until its 30-second idle retirement.
pub(crate) fn release_lease(lock: &Path, owner: u32) {
    if lease_owner(lock) == Some(owner) {
        let _ = std::fs::remove_file(lock);
    }
}

/// Record the broker just spawned, so anyone waiting knows who they wait for.
fn record_owner(lease: &mut File, pid: u32) {
    let _ = lease.write_all(pid.to_string().as_bytes());
    let _ = lease.flush();
}

/// Connect to the shared broker for `launch`, starting it when absent.
///
/// The returned stream has already completed the authentication prelude.
pub(crate) async fn connect<P: BrokerProtocol>(
    executable: &Path,
    state_root: &Path,
    launch: &Launch,
) -> Result<TcpStream, BrokerError> {
    connect_observed::<P>(executable, state_root, launch)
        .await
        .map(|(stream, _broker)| stream)
}

/// The registry directory a protocol's brokers publish under.
fn state_directory<P: BrokerProtocol>(state_root: &Path) -> PathBuf {
    state_root.join(P::STATE_DIR)
}

/// The report broker `pid` left for `launch`, if that broker left one.
///
/// The caller's evidence for a *permanent* verdict, and the reason the
/// acknowledgement carries a process id: `{key}.error` is one path shared by
/// every broker the key ever had, and `ran` is permanent, so a report credited
/// to the wrong attempt retires a server that never failed. A close on a socket
/// is not this evidence and must not be treated as if it were.
pub(crate) fn reported<P: BrokerProtocol>(
    state_root: &Path,
    launch: &Launch,
    pid: u32,
) -> Option<super::BrokeredLaunchFailure> {
    let key = key::broker_key(P::PRELUDE, P::PROTOCOL_VERSION, launch);
    let report = read_failure(&state_directory::<P>(state_root).join(format!("{key}.error")))?;
    (report.pid == pid).then_some(report)
}

/// Connect as [`connect`] does, and say which broker answered.
pub(crate) async fn connect_observed<P: BrokerProtocol>(
    executable: &Path,
    state_root: &Path,
    launch: &Launch,
) -> Result<(TcpStream, u32), BrokerError> {
    let directory = state_directory::<P>(state_root);
    std::fs::create_dir_all(&directory).map_err(io_error)?;
    endpoint::set_private_directory(&directory)?;
    let key = key::broker_key(P::PRELUDE, P::PROTOCOL_VERSION, launch);
    let metadata = directory.join(format!("{key}.json"));
    let lock = directory.join(format!("{key}.lock"));
    let failure_path = directory.join(format!("{key}.error"));

    if let Some(connected) = connect_existing::<P>(&metadata).await {
        return Ok(connected);
    }
    // Nothing else ever revisits a key whose argv the user has since edited.
    failure::sweep(&directory);

    // Held for the whole wait: for the connector that elected it, the child
    // handle is the one unambiguous answer to "is my broker still coming?".
    let mut broker = None;
    let mut owner = None;
    if let Ok(mut lease) = OpenOptions::new().create_new(true).write(true).open(&lock) {
        // Holding the lease proves no broker owns this key, so whatever a
        // previous one recorded is about an attempt that is over.
        let _ = std::fs::remove_file(&failure_path);
        let spec = BrokerSpec {
            launch: launch.clone(),
            metadata: metadata.clone(),
            lock: lock.clone(),
            failure: failure_path.clone(),
            token: key::broker_token(&key),
            supervisor: executable.to_path_buf(),
        };
        let encoded =
            serde_json::to_string(&spec).map_err(|error| BrokerError::Spec(error.to_string()))?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env(P::MODE_ENV, "1")
            .env(P::SPEC_ENV, encoded)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(spawned) => {
                owner = spawned.id();
                if let Some(pid) = owner {
                    record_owner(&mut lease, pid);
                }
                broker = Some(spawned);
            },
            Err(error) => {
                // Nothing will ever publish under this lease, and leaving it
                // would cost the next attempt its whole startup deadline.
                let _ = std::fs::remove_file(&lock);
                return Err(io_error(error));
            },
        }
    }

    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        if let Some(connected) = connect_existing::<P>(&metadata).await {
            return Ok(connected);
        }
        // Whoever won the election records the broker; until then there is no
        // one to attribute a report to.
        if owner.is_none() {
            owner = lease_owner(&lock);
        }
        // A broker that gave up says so, which both ends the wait early and
        // tells the caller whether another attempt could help. Only the elected
        // broker's own report counts: `ran` is permanent, so a report left by
        // some other broker for this key would retire a server that never
        // failed. A report that is not ours is not evidence of anything, so the
        // normal wait simply continues.
        if let Some(pid) = owner
            && let Some(reported) = read_failure(&failure_path)
            && reported.pid == pid
        {
            // The broker wrote this on its way out, so its lease is finished.
            release_lease(&lock, pid);
            return Err(BrokerError::Launch(Box::new(reported)));
        }
        if tokio::time::Instant::now() >= deadline {
            abandon_lease::<P>(&metadata, &lock, owner, broker.as_mut()).await;
            return Err(BrokerError::Io(
                "timed out waiting for shared broker".to_owned(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Give up a lease at the startup deadline — but only if its broker is gone.
///
/// A hidden process normally publishes in milliseconds, so the deadline usually
/// does mean something is wrong. It does not prove the broker is *dead*, and
/// unlinking the lease of one that is merely slow is how a key ended up with
/// two live brokers: the next connector won a fresh lease, elected a second
/// broker over the same endpoint path, and the first one's exit then removed
/// the second's endpoint file. So the lease is released only on evidence.
async fn abandon_lease<P: BrokerProtocol>(
    metadata: &Path,
    lock: &Path,
    owner: Option<u32>,
    broker: Option<&mut Child>,
) {
    if !lease_is_dead::<P>(metadata, owner, broker).await && !lease_is_abandoned(lock) {
        return;
    }
    match owner {
        Some(pid) => release_lease(lock, pid),
        // Nothing was ever recorded, so there is no owner to protect.
        None => {
            let _ = std::fs::remove_file(lock);
        },
    }
}

/// Whether the broker holding the lease can be *shown* to be gone.
async fn lease_is_dead<P: BrokerProtocol>(
    metadata: &Path,
    owner: Option<u32>,
    broker: Option<&mut Child>,
) -> bool {
    // Our own child: the handle answers exactly, with no guessing.
    if let Some(broker) = broker
        && matches!(broker.try_wait(), Ok(Some(_)))
    {
        return true;
    }
    // Someone else's: a published endpoint its broker does not answer on is
    // proof, because the listener dies with the process that opened it.
    if let Some(pid) = owner
        && let Some(endpoint) = published(metadata)
        && endpoint.pid == pid
    {
        // A bare connection used to stand in for "answered", which a stale
        // endpoint whose port the OS had recycled satisfied on someone else's
        // behalf -- wedging the key for as long as that other listener lived.
        // Only the broker's own acknowledgement counts now.
        return matches!(greet::<P>(&endpoint).await, Peer::Stranger);
    }
    false
}

/// The backstop: a lease older than any startup, whose broker left no trace.
fn lease_is_abandoned(lock: &Path) -> bool {
    std::fs::metadata(lock)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age >= STALE_LEASE)
}

/// What answered at a published endpoint's address.
enum Peer {
    /// The broker the endpoint named, on an authenticated connection.
    Broker(TcpStream, u32),
    /// Something else, and it *proved* it: the address refused the connection,
    /// closed on the prelude, or answered with something no broker for this
    /// protocol writes.
    Stranger,
    /// Something accepted and then said nothing in time. Not evidence either
    /// way -- a busy broker looks exactly like this -- so nothing is removed
    /// on the strength of it.
    Unproven,
}

/// Greet whoever holds `endpoint`, and say whether it is the broker it names.
///
/// The client used to trust the address alone. It cannot: an address outlives
/// the process that bound it, so a `{key}.json` left behind by a broker that
/// was killed points at whatever local listener the OS next hands that
/// ephemeral port to. Writing the prelude and reading nothing back made that
/// stranger indistinguishable from a healthy broker, so `initialize` went to
/// it, and the close that followed was reported as this server exiting --
/// permanently, and self-sustaining, because a connection that keeps
/// succeeding never re-elects anything.
async fn greet<P: BrokerProtocol>(endpoint: &Endpoint) -> Peer {
    let exchange = async {
        let mut stream = TcpStream::connect(endpoint.address).await?;
        stream
            .write_all(format!("{}{}\n", P::PRELUDE, endpoint.token).as_bytes())
            .await?;
        let answer = read_greeting(&mut stream).await?;
        Ok::<_, std::io::Error>((stream, answer))
    };
    match tokio::time::timeout(PROBE_TIMEOUT, exchange).await {
        Err(_elapsed) => Peer::Unproven,
        // A refusal, a reset, or a close: whatever is at that address, it is
        // not a broker holding this key's token.
        Ok(Err(_closed)) => Peer::Stranger,
        Ok(Ok((stream, answer))) => match serve::acknowledged::<P>(&answer) {
            // Only the broker that published this file knows the token, so a
            // valid greeting under a different process id means the file was
            // replaced under us -- not that this connection belongs to the
            // broker the file named.
            Some(pid) if pid == endpoint.pid => Peer::Broker(stream, pid),
            _ => Peer::Stranger,
        },
    }
}

/// Read one newline-terminated line without over-reading past it.
///
/// A byte at a time on purpose: everything after the acknowledgement belongs to
/// the protocol the caller is about to speak on this very stream, and a
/// buffered reader would swallow it into a buffer that is dropped with it.
/// Capped so a peer that never writes a newline cannot make this grow.
async fn read_greeting(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while line.len() < MAX_GREETING {
        stream.read_exact(&mut byte).await?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Connect to the broker `metadata` publishes, if that is who is really there.
///
/// A stale endpoint is invalidated by being *superseded*, never by being
/// unlinked here. Refusing to use it is what breaks the self-sustaining loop:
/// the connector falls through to the election, and the broker it elects
/// republishes this very path atomically. Unlinking would be both unnecessary
/// and worse — the published endpoint is the evidence [`lease_is_dead`] uses to
/// prove a lease's broker is gone, so removing it wedges the key for a full
/// [`STALE_LEASE`] instead of freeing it on the next attempt.
async fn connect_existing<P: BrokerProtocol>(metadata: &Path) -> Option<(TcpStream, u32)> {
    match greet::<P>(&published(metadata)?).await {
        Peer::Broker(stream, pid) => Some((stream, pid)),
        Peer::Stranger | Peer::Unproven => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs::FileTimes;
    use std::net::SocketAddr;
    use std::time::SystemTime;

    use super::*;
    use crate::broker::failure::BrokeredLaunchFailure;
    use crate::broker::failure::write as write_report;
    use crate::broker::serve::tests::TestProtocol;

    type BoxError = Box<dyn std::error::Error>;

    /// A process id that is certainly not this one, standing in for a broker
    /// elected by some earlier attempt.
    const OTHER_BROKER: u32 = u32::MAX;

    /// An executable that cannot be spawned, so no broker is ever elected: the
    /// tests below stage the registry by hand instead.
    const UNSPAWNABLE: &str = "/nonexistent/karet-broker-executable";

    fn launch() -> Launch {
        Launch {
            command: "test-server".to_owned(),
            args: vec!["--stdio".to_owned()],
            root: PathBuf::from("/workspace"),
        }
    }

    /// The registry paths `connect` derives for [`launch`].
    struct Registry {
        directory: PathBuf,
        metadata: PathBuf,
        lock: PathBuf,
        failure: PathBuf,
    }

    fn registry(state_root: &Path) -> Result<Registry, BoxError> {
        let directory = state_root.join(TestProtocol::STATE_DIR);
        std::fs::create_dir_all(&directory)?;
        let key = key::broker_key(
            TestProtocol::PRELUDE,
            TestProtocol::PROTOCOL_VERSION,
            &launch(),
        );
        Ok(Registry {
            metadata: directory.join(format!("{key}.json")),
            lock: directory.join(format!("{key}.lock")),
            failure: directory.join(format!("{key}.error")),
            directory,
        })
    }

    fn report(pid: u32, command: &str) -> BrokeredLaunchFailure {
        BrokeredLaunchFailure {
            command: command.to_owned(),
            args: Vec::new(),
            message: "server stdout ended".to_owned(),
            stderr: Vec::new(),
            ran: true,
            pid,
        }
    }

    async fn connect_to(state_root: &Path) -> Result<TcpStream, BrokerError> {
        connect::<TestProtocol>(Path::new(UNSPAWNABLE), state_root, &launch()).await
    }

    /// A listener that accepts and closes without a word.
    ///
    /// What another karet broker does with a foreign token, and the likeliest
    /// thing to find at an address a dead broker's endpoint still names once
    /// the OS has handed that ephemeral port to somebody else.
    async fn stranger() -> Result<(SocketAddr, tokio::task::JoinHandle<()>), BoxError> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let accepting = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        Ok((address, accepting))
    }

    /// Publish an endpoint by hand, as a broker that has since died would have.
    fn publish(metadata: &Path, address: SocketAddr, pid: u32) -> Result<(), BoxError> {
        std::fs::write(
            metadata,
            format!(r#"{{"address":"{address}","token":"t","pid":{pid},"command":null}}"#),
        )?;
        Ok(())
    }

    /// Backdates `path` so an age-based check sees it as long abandoned.
    fn age(path: &Path, by: Duration) -> Result<(), BoxError> {
        let file = std::fs::File::options().write(true).open(path)?;
        let when = SystemTime::now() - by;
        file.set_times(FileTimes::new().set_accessed(when).set_modified(when))?;
        Ok(())
    }

    /// The stale-endpoint defect, and the reason a broker now answers the
    /// prelude.
    ///
    /// An address outlives the process that bound it. A broker killed before it
    /// could clean up leaves a `{key}.json` whose ephemeral port the OS is free
    /// to hand to any other local listener, and the connector wrote its prelude
    /// and read nothing back -- so a stranger was indistinguishable from a
    /// healthy broker. `initialize` went into it, the stranger hung up, and that
    /// close was reported as this server exiting: permanent, "will not be
    /// retried", for a server that runs perfectly well. Self-sustaining, too,
    /// because a connection that keeps succeeding never re-elects anything.
    ///
    /// Real time rather than the paused clock the rest of these tests use: a
    /// stranger answering is a genuine round trip, and an auto-advanced clock
    /// would expire the greeting before the peer could prove anything. This
    /// costs no wall clock -- nothing here reaches the startup deadline.
    #[tokio::test]
    async fn a_stale_endpoint_a_stranger_answers_is_not_trusted() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        let (address, _accepting) = stranger().await?;
        publish(&registry.metadata, address, OTHER_BROKER)?;

        let outcome = connect_to(state.path()).await;

        assert!(
            matches!(outcome, Err(BrokerError::Io(_))),
            "a listener that cannot prove it is this key's broker must not be \
             handed a connection: {outcome:?}"
        );
        Ok(())
    }

    /// The other half: proving the stranger is not the broker is also what
    /// frees the key.
    ///
    /// A published endpoint that answers used to count as its broker being
    /// alive, whoever was really answering. With the port recycled, the lease of
    /// a broker that had been killed could never be shown dead, so the key was
    /// wedged until the 60s backstop -- every attempt in between failing.
    #[tokio::test]
    async fn a_lease_is_reclaimed_when_a_stranger_answers_its_endpoint() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        let (address, _accepting) = stranger().await?;
        publish(&registry.metadata, address, OTHER_BROKER)?;
        std::fs::write(&registry.lock, OTHER_BROKER.to_string())?;

        assert!(
            lease_is_dead::<TestProtocol>(&registry.metadata, Some(OTHER_BROKER), None).await,
            "a listener that cannot answer as this key's broker is not that broker"
        );
        Ok(())
    }

    #[test]
    fn a_lease_is_released_only_by_the_broker_it_names() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let lock = state.path().join("key.lock");
        std::fs::write(&lock, b"4242")?;

        assert_eq!(lease_owner(&lock), Some(4242));
        release_lease(&lock, 99);
        assert!(lock.exists(), "a sibling's lease is not ours to remove");
        release_lease(&lock, 4242);
        assert!(!lock.exists(), "our own lease was not released");
        Ok(())
    }

    /// The misattribution defect: `{key}.error` is one path shared by every
    /// broker for the key, and a report found there says `ran`, which is a
    /// permanent verdict. Reading one written by a *different* broker retires a
    /// server that never failed for the rest of the session.
    #[tokio::test(start_paused = true)]
    async fn a_report_from_another_broker_is_not_this_launch_s_verdict() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        // A lease taken by another connector, naming a broker still starting.
        std::fs::write(&registry.lock, std::process::id().to_string())?;
        write_report(
            &registry.failure,
            &report(OTHER_BROKER, "a-server-from-a-previous-attempt"),
        );

        let outcome = connect_to(state.path()).await;

        assert!(
            matches!(&outcome, Err(BrokerError::Io(message)) if message.contains("timed out")),
            "a foreign report must be ignored, not returned: {outcome:?}"
        );
        assert!(
            registry.failure.exists(),
            "another broker's report is not ours to consume"
        );
        Ok(())
    }

    /// The positive half: identity is checked, not merely present.
    #[tokio::test(start_paused = true)]
    async fn the_elected_broker_s_own_report_is_returned() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        std::fs::write(&registry.lock, OTHER_BROKER.to_string())?;
        write_report(&registry.failure, &report(OTHER_BROKER, "test-server"));

        let outcome = connect_to(state.path()).await;

        let Err(BrokerError::Launch(reported)) = outcome else {
            return Err(BoxError::from(
                "the elected broker's report was not returned",
            ));
        };
        assert_eq!(reported.command, "test-server");
        assert!(reported.ran);
        assert!(
            !registry.lock.exists(),
            "a broker that reported on its way out has finished with its lease"
        );
        Ok(())
    }

    /// The two-brokers-for-one-key defect. Giving up at the deadline says the
    /// broker is slow, not that it is dead; unlinking its lease let the next
    /// connector elect a second broker over the same endpoint path, and the
    /// first one's exit then removed the second's endpoint file.
    #[tokio::test(start_paused = true)]
    async fn a_deadline_does_not_evict_a_broker_that_may_still_be_starting() -> Result<(), BoxError>
    {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        // This process stands in for the elected broker: it is demonstrably alive.
        std::fs::write(&registry.lock, std::process::id().to_string())?;

        let outcome = connect_to(state.path()).await;

        assert!(matches!(outcome, Err(BrokerError::Io(_))));
        assert!(
            registry.lock.exists(),
            "the lease of a broker that never proved dead was removed"
        );
        Ok(())
    }

    /// A dead broker must never wedge its key: the endpoint it published is
    /// proof, because the listener dies with the process that opened it.
    #[tokio::test(start_paused = true)]
    async fn a_lease_whose_broker_published_and_died_is_reclaimed() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        std::fs::write(&registry.lock, OTHER_BROKER.to_string())?;
        std::fs::write(
            &registry.metadata,
            format!(
                r#"{{"address":"127.0.0.1:1","token":"t","pid":{OTHER_BROKER},"command":null}}"#
            ),
        )?;

        let outcome = connect_to(state.path()).await;

        assert!(matches!(outcome, Err(BrokerError::Io(_))));
        assert!(
            !registry.lock.exists(),
            "a lease whose broker is provably gone must be reclaimable"
        );
        Ok(())
    }

    /// The backstop for the broker that was killed before it published anything
    /// and whose connector is gone too: nobody is left to prove it dead.
    #[tokio::test(start_paused = true)]
    async fn a_long_abandoned_lease_is_reclaimed() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        std::fs::write(&registry.lock, OTHER_BROKER.to_string())?;
        age(&registry.lock, STALE_LEASE + Duration::from_secs(60))?;

        let outcome = connect_to(state.path()).await;

        assert!(matches!(outcome, Err(BrokerError::Io(_))));
        assert!(!registry.lock.exists(), "a stale lease wedged the key");
        Ok(())
    }

    /// A lease nothing can ever publish under is released at once, rather than
    /// costing the next attempt its whole startup deadline.
    #[tokio::test(start_paused = true)]
    async fn a_broker_that_cannot_be_spawned_releases_its_lease() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;

        let outcome = connect_to(state.path()).await;

        assert!(matches!(outcome, Err(BrokerError::Io(_))));
        assert!(
            !registry.lock.exists(),
            "the lease outlived the election it was taken for"
        );
        Ok(())
    }

    /// Reports are unlinked by the next connector for the *same* key, and a
    /// user who edits their server's argv never revisits the old key. Without a
    /// sweep those files accumulate in the registry directory forever.
    #[tokio::test(start_paused = true)]
    async fn connecting_sweeps_reports_no_connector_will_claim() -> Result<(), BoxError> {
        let state = tempfile::tempdir()?;
        let registry = registry(state.path())?;
        let abandoned = registry.directory.join("an-old-key.error");
        let recent = registry.directory.join("a-recent-key.error");
        std::fs::write(&abandoned, b"{}")?;
        std::fs::write(&recent, b"{}")?;
        age(&abandoned, Duration::from_secs(3 * 60 * 60))?;

        let _ = connect_to(state.path()).await;

        assert!(!abandoned.exists(), "an abandoned report was left behind");
        assert!(recent.exists(), "a report still in play was swept");
        Ok(())
    }
}

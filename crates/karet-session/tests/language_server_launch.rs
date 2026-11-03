//! End-to-end language-server launch, through the path production takes.
//!
//! Every other LSP test in this workspace injects an in-memory `Connector`, so
//! none of them executes `spawn_connector`, the hidden process supervisor, the
//! shared broker, or a child on the far end of a real pipe. That is where the
//! launch defects lived, and it is what this file covers.
//!
//! These are integration tests for one mechanical reason: `CARGO_BIN_EXE_*` is
//! set for integration tests, examples and benches, and not for a lib's own
//! `#[cfg(test)]` modules — so this is the only place a test can name the
//! process double's path. Crossing the crate boundary has a second benefit
//! here: the library is built without `cfg(test)`, so `spec_for`'s test-only
//! resolution fallback is absent and provider resolution behaves as it does for
//! a user.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use karet_core::Severity;
use karet_session::SessionConfig;
use karet_session::api::DocumentCommand;
use karet_session::api::Event;
use karet_session::api::LanguageServerRuntimeState;
use karet_session::api::LspCommand;
use karet_session::backend::Backend;
use karet_session::backend::local;
use karet_session::config::schema::LspServer;

/// The process double, built by the `testbed` feature this test requires.
const TESTBED: &str = env!("CARGO_BIN_EXE_karet-testbed");

/// Generous: these spawn real processes, and a slow machine must not flake.
const DEADLINE: Duration = Duration::from_secs(20);

/// For a wait that spans a whole cold start -- supervisor spawn, the server's
/// own startup and the LSP handshake -- under the load of a parallel workspace
/// build, where [`DEADLINE`] proved too tight. It only bounds a failure: a
/// passing run returns at the first `Running`.
const STARTUP_DEADLINE: Duration = Duration::from_secs(90);

/// The double's argv. The report path travels in argv rather than the
/// environment because these tests run in parallel threads of one process, and
/// a shared env var would let one test's report path overwrite another's.
fn testbed_args(behavior: &str, report: &Path) -> Vec<String> {
    vec![
        "--behavior".to_owned(),
        behavior.to_owned(),
        "--report".to_owned(),
        report.to_string_lossy().into_owned(),
    ]
}

struct Harness {
    backend: Box<dyn Backend>,
    events: karet_session::EventRx,
    /// Declared after the session so it drops after it: by then nothing is left
    /// to elect a replacement for the brokers it reaps.
    _brokers: BrokerReaper,
    /// Held so the temporary directories outlive the session.
    _workspace: tempfile::TempDir,
    _registry: Option<tempfile::TempDir>,
    report: PathBuf,
    file: PathBuf,
}

/// Anything that ends a test, reported rather than skipped over.
///
/// These tests used to open with `let Some(harness) = ... else { return; }`, so
/// a `TMPDIR` that could not be written -- or any other setup failure -- was a
/// silent pass: fourteen tests reporting `ok` having executed nothing. `?` on a
/// `Result` fails the test instead, which is what a broken fixture deserves.
type SetupError = Box<dyn std::error::Error>;

/// Place the process double at `target`, as a project-local server binary.
///
/// A link rather than a copy. Copying writes the file from this process, and
/// these tests run on parallel threads that keep forking supervisors: a fork
/// that lands while the copy's write descriptor is open hands the child a
/// duplicate of it, held until that child execs. A supervisor that execs the
/// copy during that window gets `ETXTBSY`, exits, and the runtime reads the
/// exit as a server that can never start -- `Unavailable` for a binary that is
/// perfectly runnable. A link opens nothing for writing, so there is no window.
/// `is_executable_file` follows the link, which is how a virtualenv's own
/// `bin/` entries usually look anyway.
fn stage_testbed(target: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(TESTBED, target)
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(TESTBED, target).map(drop)
    }
}

/// A session whose Rust provider is the process double.
///
/// `brokered` picks the production fork: with a registry directory the
/// connector goes through the shared broker, without one it goes straight to
/// the supervisor. Both are real; neither is reachable from a unit test.
fn harness(behavior: &str, brokered: bool) -> Result<Harness, SetupError> {
    let workspace = tempfile::tempdir()?;
    let file = workspace.path().join("main.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    let report = workspace.path().join("launches.jsonl");

    let registry = brokered.then(tempfile::tempdir).transpose()?;
    let mut config = SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        lsp_registry_dir: registry.as_ref().map(|dir| dir.path().to_path_buf()),
        ..SessionConfig::default()
    };
    config.loaded_config.settings.lsp.servers.insert(
        "rust".to_owned(),
        LspServer {
            command: TESTBED.to_owned(),
            args: testbed_args(behavior, &report),
            ..LspServer::default()
        },
    );

    let (backend, _snapshots) = local(config);
    let events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    Ok(Harness {
        backend: Box::new(backend),
        events,
        _brokers: BrokerReaper(registry.as_ref().map(|dir| dir.path().to_path_buf())),
        _workspace: workspace,
        _registry: registry,
        report,
        file,
    })
}

impl Harness {
    fn open(&self) -> bool {
        self.backend
            .send(
                self.backend.next_id(),
                DocumentCommand::Open {
                    path: self.file.clone(),
                    language: Some("rust".to_owned()),
                }
                .into(),
            )
            .is_ok()
    }

    /// Drain events until `matches` accepts one, or the deadline passes.
    async fn wait_for(&mut self, matches: impl Fn(&Event) -> bool) -> Option<Event> {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Ok(Some((_, event))) = tokio::time::timeout(remaining, self.events.recv()).await
            else {
                return None;
            };
            if matches(&event) {
                return Some(event);
            }
        }
        None
    }

    /// How many times the double actually started.
    ///
    /// Deliberately a count of *parsed* records rather than of lines. The
    /// report is appended to by a live process, so a poll loop that waits on a
    /// line count can be satisfied by a line that is not yet a whole record and
    /// then assert against nothing -- which is exactly how
    /// `the_launch_carries_the_intended_argv_and_working_directory` used to
    /// flake. Waiting on what the assertions actually read makes that
    /// impossible, whatever the writer does.
    fn launches(&self) -> usize {
        self.launch_records().len()
    }

    /// Every launch the double has finished recording. A line that does not
    /// parse is not a record yet, so it is not counted.
    fn launch_records(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

/// Kills, on drop, every broker published under a registry directory, together
/// with the processes beneath it.
///
/// A broker is deliberately not owned by the session that elected it: it
/// outlives every editor window until 30 seconds pass with no client. Left
/// alone, a brokered test's broker, its supervisor and its server all outlive
/// the test binary, and the broker's normal exit half a minute later writes an
/// instrumented build's coverage profile while `cargo llvm-cov` may already be
/// merging the profiles -- a half-written file that aborts the whole merge.
///
/// Killed leaves first, while each is idle: a `SIGKILL` that lands on a process
/// that has not begun to exit means no profile is written at all, where a
/// supervisor killed before its server would hand the server an EOF and race
/// its exit. Linux only, because `/proc` is how the tree is found; elsewhere
/// the brokers idle out on their own, as they always have.
struct BrokerReaper(Option<PathBuf>);

impl Drop for BrokerReaper {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(registry) = &self.0 {
            reap::brokers(&registry.join("brokers"));
        }
    }
}

#[cfg(target_os = "linux")]
mod reap {
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;
    use std::time::Instant;

    use super::TESTBED;

    /// How long a killed tree gets to disappear. Only a failure waits it out.
    const GONE_DEADLINE: Duration = Duration::from_secs(5);

    pub(super) fn brokers(directory: &Path) {
        // A session shutting down might still elect a replacement as it goes,
        // so look again after each kill; three rounds is plenty for that.
        for _ in 0..3 {
            let roots = published_brokers(directory);
            if roots.is_empty() {
                return;
            }
            let tree = with_descendants(&roots);
            let mut kill = std::process::Command::new("kill");
            kill.arg("-KILL");
            // `with_descendants` lists parents first; kill the leaves first.
            kill.args(tree.iter().rev().map(u32::to_string));
            let _ = kill.status();
            let deadline = Instant::now() + GONE_DEADLINE;
            while tree.iter().any(|pid| running(*pid)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// The live brokers whose endpoint files sit in `directory`. The process
    /// must still be the double: a stale endpoint names a pid that may by now
    /// belong to anything -- the stale-endpoint test writes one naming pid 1.
    fn published_brokers(directory: &Path) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return Vec::new();
        };
        let Ok(testbed) = std::fs::canonicalize(TESTBED) else {
            return Vec::new();
        };
        let testbed = Some(testbed);
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter_map(|endpoint| u32::try_from(endpoint.get("pid")?.as_u64()?).ok())
            .filter(|pid| {
                running(*pid) && std::fs::read_link(format!("/proc/{pid}/exe")).ok() == testbed
            })
            .collect()
    }

    /// `roots` and everything beneath them, each parent before its children.
    fn with_descendants(roots: &[u32]) -> Vec<u32> {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|pid| Some((pid, stat(pid)?.1)))
        {
            children.entry(parent).or_default().push(pid);
        }
        let mut tree = roots.to_vec();
        let mut next = 0;
        while let Some(pid) = tree.get(next).copied() {
            tree.extend(children.get(&pid).into_iter().flatten().copied());
            next += 1;
        }
        tree
    }

    /// Whether `pid` still runs. A zombie does not: it can write nothing more,
    /// and where PID 1 never reaps it would otherwise be waited on forever.
    fn running(pid: u32) -> bool {
        stat(pid).is_some_and(|(state, _)| state != 'Z' && state != 'X')
    }

    /// A process's state and parent, from `/proc/PID/stat`. The command name
    /// before them is parenthesised and may itself hold spaces or parentheses,
    /// so the fields are read after its last `)`.
    fn stat(pid: u32) -> Option<(char, u32)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
        let state = fields.next()?.chars().next()?;
        let parent = fields.next()?.parse().ok()?;
        Some((state, parent))
    }
}

fn is_lsp_warning(event: &Event) -> bool {
    matches!(
        event,
        Event::Notification {
            severity: Severity::Warning | Severity::Error,
            kind: karet_core::NotificationKind::Lsp,
            ..
        }
    )
}

fn notification_text(event: &Event) -> String {
    match event {
        Event::Notification { message, .. } => message.clone(),
        _ => String::new(),
    }
}

// --- the direct supervisor path -------------------------------------------

/// The first test in this workspace to drive a real language server: a real
/// process, over real pipes, through the real hidden supervisor.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_server_completes_a_request_over_the_supervisor() -> Result<(), SetupError> {
    let mut harness = harness("normal", false)?;
    assert!(harness.open());
    let opened = harness
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;
    assert!(opened.is_some(), "the document never opened");

    let Some(Event::Opened { doc, .. }) = opened else {
        return Err(SetupError::from("the document never opened"));
    };
    let request = harness.backend.next_id();
    let _ = harness.backend.send(
        request,
        LspCommand::Completion {
            doc,
            position: karet_core::LineCol { line: 0, col: 0 },
            trigger: karet_core::CompletionTrigger::Invoked,
        }
        .into(),
    );
    let completions = harness
        .wait_for(|event| matches!(event, Event::Completions { .. }))
        .await;
    let label = match completions {
        Some(Event::Completions { items, .. }) => items.first().map(|item| item.label.clone()),
        _ => None,
    };
    assert_eq!(
        label.as_deref(),
        Some("karet_testbed_item"),
        "the answer must come from the spawned process, not a fallback"
    );
    Ok(())
}

/// Proves the whole chain — session, `supervisor::command`, hidden supervisor,
/// child — delivered exactly the argv and working directory production intends,
/// and that the supervisor scrubbed its own hidden-mode variables so a
/// descendant cannot re-enter supervisor mode.
#[tokio::test(flavor = "multi_thread")]
async fn the_launch_carries_the_intended_argv_and_working_directory() -> Result<(), SetupError> {
    let mut harness = harness("report", false)?;
    assert!(harness.open());
    let _ = harness
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;

    // Waits on parsed records -- see `launches` -- so a line still being
    // written can never end the wait early.
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut records = harness.launch_records();
    while records.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        records = harness.launch_records();
    }
    assert!(!records.is_empty(), "the server never recorded a launch");
    let record = records.first().cloned().unwrap_or_default();
    let argv = record["argv"]
        .as_array()
        .map(|argv| {
            argv.iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(
        argv.first().copied(),
        Some(TESTBED),
        "the configured executable must reach the process"
    );
    assert_eq!(
        argv.get(1..3),
        Some(["--behavior", "report"].as_slice()),
        "the configured argv must reach the process verbatim"
    );
    assert_eq!(
        record["leaked_env"].as_array().map(Vec::len),
        Some(0),
        "the supervisor must scrub its hidden-mode variables before exec"
    );
    Ok(())
}

// --- failure shapes, through the real path ---------------------------------

/// Bare `taplo` prints usage to stdout, which is fatal to `Content-Length`
/// framing. The launch must be reported, not hung on.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_prints_a_banner_is_reported_rather_than_hanging() -> Result<(), SetupError> {
    let mut harness = harness("banner", false)?;
    assert!(harness.open());
    let reported = harness.wait_for(is_lsp_warning).await;
    assert!(
        reported.is_some(),
        "a server that writes junk to stdout must surface a failure"
    );
    Ok(())
}

/// The `node: Cannot find module` shape. The server's own last words are what
/// makes this diagnosable, and they used to be discarded at `debug` level.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_exits_reports_what_it_said_on_the_way_out() -> Result<(), SetupError> {
    let mut harness = harness("exit-stderr", false)?;
    assert!(harness.open());
    let reported = harness.wait_for(is_lsp_warning).await;
    assert!(
        reported.is_some(),
        "a server that exits must surface a failure"
    );
    let message = reported.as_ref().map(notification_text).unwrap_or_default();
    assert!(
        message.contains("Cannot find module"),
        "the failure must carry the server's own diagnosis: {message}"
    );
    assert!(
        message.contains("--behavior exit-stderr"),
        "the failure must name the configured provider launch, not the supervisor \
         re-exec: {message}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_exits_silently_is_still_reported() -> Result<(), SetupError> {
    let mut harness = harness("exit-now", false)?;
    assert!(harness.open());
    assert!(
        harness.wait_for(is_lsp_warning).await.is_some(),
        "a silent immediate exit must not look like success"
    );
    Ok(())
}

/// A missing binary can never start, so karet stops rather than respawning it
/// every five minutes for the life of the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_binary_becomes_unavailable_rather_than_retrying_forever()
-> Result<(), SetupError> {
    let workspace = tempfile::tempdir()?;
    let file = workspace.path().join("main.rs");
    std::fs::write(&file, "fn main() {}\n")?;
    let mut config = SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        ..SessionConfig::default()
    };
    config.loaded_config.settings.lsp.servers.insert(
        "rust".to_owned(),
        LspServer {
            command: "karet-testbed-no-such-binary".to_owned(),
            ..LspServer::default()
        },
    );
    let (backend, _snapshots) = local(config);
    let mut events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    let _ = backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path: file,
            language: Some("rust".to_owned()),
        }
        .into(),
    );

    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut unavailable = false;
    while !unavailable && tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            unavailable = state == LanguageServerRuntimeState::Unavailable;
        }
    }
    assert!(unavailable, "a missing binary must reach a terminal state");
    Ok(())
}

/// Headers without a `Content-Length`: the frame boundary is unknowable, so the
/// connection cannot continue and the launch has to surface.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_frames_its_output_wrongly_is_reported() -> Result<(), SetupError> {
    let mut harness = harness("no-content-length", false)?;
    assert!(harness.open());
    assert!(
        harness.wait_for(is_lsp_warning).await.is_some(),
        "a peer that loses framing must surface a failure"
    );
    Ok(())
}

/// A server that completes the handshake and then dies is the case the restart
/// circuit exists for: it has proven it can run, so karet keeps trying.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_dies_after_connecting_is_retried_not_written_off() -> Result<(), SetupError>
{
    let mut harness = harness("die-after-handshake", false)?;
    assert!(harness.open());
    let mut seen_running = false;
    // Short on purpose: this waits for something that must *not* happen, so the
    // whole window is spent every run. Two connect-and-die cycles is ample --
    // the backoff starts at 250ms -- and being written off would need five.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut wrote_off = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, harness.events.recv()).await
        else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            match state {
                LanguageServerRuntimeState::Running => seen_running = true,
                LanguageServerRuntimeState::Unavailable => {
                    wrote_off = true;
                    break;
                },
                _ => {},
            }
        }
    }
    assert!(
        seen_running,
        "the server should have connected at least once"
    );
    assert!(
        !wrote_off,
        "a server that connected before must keep its retries; only one that has \
         never started is written off"
    );
    Ok(())
}

// --- the shared broker -----------------------------------------------------

/// The broker exists so concurrent karet windows share one server process. This
/// asserts the sharing actually happens, by counting how many times the double
/// really started — code with no coverage at all before now.
#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_on_one_root_share_a_single_server_process() -> Result<(), SetupError> {
    let mut first = harness("report", true)?;
    assert!(first.open());
    let _ = first
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;

    let deadline = tokio::time::Instant::now() + DEADLINE;
    while first.launches() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(first.launches(), 1, "the first session elects one broker");

    // A second session over the same root and launch must reuse it.
    let registry = first
        ._registry
        .as_ref()
        .map(|dir| dir.path().to_path_buf())
        .unwrap_or_default();
    let mut config = SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        lsp_registry_dir: Some(registry),
        ..SessionConfig::default()
    };
    config.loaded_config.settings.lsp.servers.insert(
        "rust".to_owned(),
        LspServer {
            command: TESTBED.to_owned(),
            args: testbed_args("report", &first.report),
            ..LspServer::default()
        },
    );
    let (backend, _snapshots) = local(config);
    let mut events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    let _ = backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path: first.file.clone(),
            language: Some("rust".to_owned()),
        }
        .into(),
    );
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        if matches!(event, Event::Opened { .. }) {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        first.launches(),
        1,
        "a second session on the same root must reuse the brokered process"
    );
    Ok(())
}

/// Through the broker the server is a grandchild, so its death has to cross two
/// process boundaries to be noticed. It used to be missed entirely when the
/// server died before the broker began accepting clients, and the session then
/// waited out the broker's 30-second idle timeout.
#[tokio::test(flavor = "multi_thread")]
async fn a_brokered_server_that_dies_immediately_is_reported_promptly() -> Result<(), SetupError> {
    let mut harness = harness("exit-now", true)?;
    assert!(harness.open());
    let started = tokio::time::Instant::now();
    let reported = harness.wait_for(is_lsp_warning).await;
    assert!(reported.is_some(), "the dead server must be reported");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "reporting took {:?}, which suggests the broker idle timeout was waited out",
        started.elapsed()
    );
    Ok(())
}

/// The app always has a registry directory, so it always takes the brokered
/// fork. A failure classified only for the direct fork is therefore a fix that
/// never runs in production — which is what happened here: every brokered
/// failure was reported as a host problem, meaning "a retry might help", so a
/// server that could never start was retried forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_brokered_server_that_can_never_start_also_stops_being_retried() -> Result<(), SetupError>
{
    let mut harness = harness("exit-now", true)?;
    assert!(harness.open());
    let mut unavailable = false;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while !unavailable && tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, harness.events.recv()).await
        else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { state, .. } = event {
            unavailable = state == LanguageServerRuntimeState::Unavailable;
        }
    }
    assert!(
        unavailable,
        "a brokered server that exits on sight must reach a terminal state, not \
         retry forever"
    );
    Ok(())
}

/// A stale endpoint left by a broker that died is superseded rather than
/// trusted. Every early failure in `run_broker` used to leave one behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_broker_endpoint_does_not_block_a_new_launch() -> Result<(), SetupError> {
    let mut harness = harness("normal", true)?;
    let registry = harness
        ._registry
        .as_ref()
        .map(|dir| dir.path().to_path_buf())
        .ok_or("the brokered harness has no registry directory")?;
    let brokers = registry.join("brokers");
    std::fs::create_dir_all(&brokers)?;
    // An endpoint naming a port nothing is listening on, as a crashed broker
    // would leave. The key is not the one this launch computes, so this also
    // proves an unrelated stale file is simply ignored.
    let stale =
        brokers.join("0000000000000000000000000000000000000000000000000000000000000000.json");
    std::fs::write(
        &stale,
        br#"{"address":"127.0.0.1:1","token":"stale","pid":1,"command":null}"#,
    )?;

    assert!(harness.open());
    let opened = harness
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;
    assert!(opened.is_some(), "a stale endpoint must not block a launch");
    assert!(
        Path::new(&stale).exists(),
        "an unrelated broker's file is not this launch's to remove"
    );
    Ok(())
}

/// Diagnostics are a merged layer, not the primary provider's to grant.
///
/// `document_opened` used to return the moment a language's primary provider
/// could not be resolved, so a Python repository with Ruff present but Pyright
/// missing got no diagnostics at all — an installed, configured provider that
/// silently never ran. The live matrix found it, because there every provider
/// is installed by itself.
///
/// This has to be an integration test: `spec_for` carries a `cfg(test)`
/// fallback that resolves every built-in to its bare launch spec, so under
/// `cfg(test)` a primary is never unresolved and the branch is unreachable.
#[tokio::test(flavor = "multi_thread")]
async fn a_diagnostics_companion_attaches_even_with_no_primary_provider() -> Result<(), SetupError>
{
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    // Marks the repository as a Ruff project, which is what selects Ruff as
    // Python's diagnostics companion.
    std::fs::write(root.join("ruff.toml"), "line-length = 88\n")?;
    // Resolved by `project_local_spec`, exactly as a real virtualenv would be.
    // Pyright, Python's primary, is deliberately absent.
    let venv = root.join(".venv").join("bin");
    std::fs::create_dir_all(&venv)?;
    stage_testbed(&venv.join("ruff"))?;
    let file = root.join("main.py");
    std::fs::write(&file, "x = 1\n")?;

    let (backend, _snapshots) = local(SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    });
    let mut events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    let _ = backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path: file,
            language: Some("python".to_owned()),
        }
        .into(),
    );

    let deadline = tokio::time::Instant::now() + STARTUP_DEADLINE;
    // Each transition keeps the runtime's own failure text beside its state.
    // States alone made a past flake unreadable: `[Starting, Unavailable]`
    // says the launch was judged permanent and nothing about why, and the
    // cause (the `ETXTBSY` exec `stage_testbed` documents) had to be
    // reconstructed from the code. The error names it outright.
    let mut ruff_transitions = Vec::new();
    while ruff_transitions.last().map(|(state, _)| state)
        != Some(&LanguageServerRuntimeState::Running)
        && tokio::time::Instant::now() < deadline
    {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged {
            server,
            state,
            error,
            ..
        } = event
            && server.key() == "ruff"
        {
            ruff_transitions.push((state, error));
        }
    }
    assert_eq!(
        ruff_transitions.last().map(|(state, _)| state),
        Some(&LanguageServerRuntimeState::Running),
        "Ruff must still provide diagnostics when Python's primary provider is missing; \
         its states and errors before the stream ended or {STARTUP_DEADLINE:?} passed: \
         {ruff_transitions:?}"
    );
    Ok(())
}

/// `lsp.enabled = false` has to stop every server, not just the primary.
///
/// `ensure_server` was the only place the setting was read, so once the primary
/// stopped being required for a document to attach companions, a disabled LSP
/// still spawned Ruff or Biome. The existing unit test missed it because it
/// opens a Rust file, and Rust has no companion.
#[tokio::test(flavor = "multi_thread")]
async fn disabling_lsp_stops_companions_and_not_only_primaries() -> Result<(), SetupError> {
    let workspace = tempfile::tempdir()?;
    let root = workspace.path();
    std::fs::write(root.join("ruff.toml"), "line-length = 88\n")?;
    let venv = root.join(".venv").join("bin");
    std::fs::create_dir_all(&venv)?;
    stage_testbed(&venv.join("ruff"))?;
    let file = root.join("main.py");
    std::fs::write(&file, "x = 1\n")?;

    let mut config = SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    };
    config.loaded_config.settings.lsp.enabled = false;
    let (backend, _snapshots) = local(config);
    let mut events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    let _ = backend.send(
        backend.next_id(),
        DocumentCommand::Open {
            path: file,
            language: Some("python".to_owned()),
        }
        .into(),
    );

    // Short: this waits for something that must never happen. The companion
    // path is fast enough that a second is ample — the sibling test sees Ruff
    // reach `Running` well inside it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut started = None;
    while started.is_none() && tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { server, .. } = event {
            started = Some(server.key().to_owned());
        }
    }
    assert_eq!(started, None, "LSP is disabled, yet a server was started");
    Ok(())
}

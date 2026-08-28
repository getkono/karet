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
use karet_session::api::Command;
use karet_session::api::Event;
use karet_session::api::LanguageServerRuntimeState;
use karet_session::backend::Backend;
use karet_session::backend::local;
use karet_session::config::schema::LspServer;
use karet_session::session::SessionConfig;

/// The process double, built by the `testbed` feature this test requires.
const TESTBED: &str = env!("CARGO_BIN_EXE_karet-testbed");

/// Generous: these spawn real processes, and a slow machine must not flake.
const DEADLINE: Duration = Duration::from_secs(20);

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
    config.settings.lsp.servers.insert(
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
                Command::OpenDocument {
                    path: self.file.clone(),
                    language: Some("rust".to_owned()),
                },
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
        Command::Completion {
            doc,
            position: karet_core::LineCol { line: 0, col: 0 },
        },
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
    config.settings.lsp.servers.insert(
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
        Command::OpenDocument {
            path: file,
            language: Some("rust".to_owned()),
        },
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
    config.settings.lsp.servers.insert(
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
        Command::OpenDocument {
            path: first.file.clone(),
            language: Some("rust".to_owned()),
        },
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
    let _ = std::fs::write(
        &stale,
        br#"{"address":"127.0.0.1:1","token":"stale","pid":1,"command":null}"#,
    );

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
    std::fs::copy(TESTBED, venv.join("ruff"))?;
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
        Command::OpenDocument {
            path: file,
            language: Some("python".to_owned()),
        },
    );

    let deadline = tokio::time::Instant::now() + DEADLINE;
    let mut ruff_running = false;
    while !ruff_running && tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok(Some((_, event))) = tokio::time::timeout(remaining, events.recv()).await else {
            break;
        };
        if let Event::LanguageServerRuntimeChanged { server, state, .. } = event {
            ruff_running = server.key() == "ruff" && state == LanguageServerRuntimeState::Running;
        }
    }
    assert!(
        ruff_running,
        "Ruff must still provide diagnostics when Python's primary provider is missing"
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
    std::fs::copy(TESTBED, venv.join("ruff"))?;
    let file = root.join("main.py");
    std::fs::write(&file, "x = 1\n")?;

    let mut config = SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    };
    config.settings.lsp.enabled = false;
    let (backend, _snapshots) = local(config);
    let mut events = backend
        .take_events()
        .ok_or("the session handed out no event stream")?;
    let _ = backend.send(
        backend.next_id(),
        Command::OpenDocument {
            path: file,
            language: Some("python".to_owned()),
        },
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

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

/// A session whose Rust provider is the process double.
///
/// `brokered` picks the production fork: with a registry directory the
/// connector goes through the shared broker, without one it goes straight to
/// the supervisor. Both are real; neither is reachable from a unit test.
fn harness(behavior: &str, brokered: bool) -> Option<Harness> {
    let workspace = tempfile::tempdir().ok()?;
    let file = workspace.path().join("main.rs");
    std::fs::write(&file, "fn main() {}\n").ok()?;
    let report = workspace.path().join("launches.jsonl");

    let registry = brokered.then(tempfile::tempdir).transpose().ok()?;
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
    let events = backend.take_events()?;
    Some(Harness {
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
    fn launches(&self) -> usize {
        std::fs::read_to_string(&self.report)
            .map(|report| {
                report
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
            })
            .unwrap_or_default()
    }

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
async fn a_real_server_completes_a_request_over_the_supervisor() {
    let Some(mut harness) = harness("normal", false) else {
        return;
    };
    assert!(harness.open());
    let opened = harness
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;
    assert!(opened.is_some(), "the document never opened");

    let Some(Event::Opened { doc, .. }) = opened else {
        return;
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
}

/// Proves the whole chain — session, `supervisor::command`, hidden supervisor,
/// child — delivered exactly the argv and working directory production intends,
/// and that the supervisor scrubbed its own hidden-mode variables so a
/// descendant cannot re-enter supervisor mode.
#[tokio::test(flavor = "multi_thread")]
async fn the_launch_carries_the_intended_argv_and_working_directory() {
    let Some(mut harness) = harness("report", false) else {
        return;
    };
    assert!(harness.open());
    let _ = harness
        .wait_for(|event| matches!(event, Event::Opened { .. }))
        .await;

    let deadline = tokio::time::Instant::now() + DEADLINE;
    while harness.launches() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let records = harness.launch_records();
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
}

// --- failure shapes, through the real path ---------------------------------

/// Bare `taplo` prints usage to stdout, which is fatal to `Content-Length`
/// framing. The launch must be reported, not hung on.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_prints_a_banner_is_reported_rather_than_hanging() {
    let Some(mut harness) = harness("banner", false) else {
        return;
    };
    assert!(harness.open());
    let reported = harness.wait_for(is_lsp_warning).await;
    assert!(
        reported.is_some(),
        "a server that writes junk to stdout must surface a failure"
    );
}

/// The `node: Cannot find module` shape. The server's own last words are what
/// makes this diagnosable, and they used to be discarded at `debug` level.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_exits_reports_what_it_said_on_the_way_out() {
    let Some(mut harness) = harness("exit-stderr", false) else {
        return;
    };
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
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_exits_silently_is_still_reported() {
    let Some(mut harness) = harness("exit-now", false) else {
        return;
    };
    assert!(harness.open());
    assert!(
        harness.wait_for(is_lsp_warning).await.is_some(),
        "a silent immediate exit must not look like success"
    );
}

/// A missing binary can never start, so karet stops rather than respawning it
/// every five minutes for the life of the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_binary_becomes_unavailable_rather_than_retrying_forever() {
    let Some(workspace) = tempfile::tempdir().ok() else {
        return;
    };
    let file = workspace.path().join("main.rs");
    if std::fs::write(&file, "fn main() {}\n").is_err() {
        return;
    }
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
    let Some(mut events) = backend.take_events() else {
        return;
    };
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
}

/// Headers without a `Content-Length`: the frame boundary is unknowable, so the
/// connection cannot continue and the launch has to surface.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_frames_its_output_wrongly_is_reported() {
    let Some(mut harness) = harness("no-content-length", false) else {
        return;
    };
    assert!(harness.open());
    assert!(
        harness.wait_for(is_lsp_warning).await.is_some(),
        "a peer that loses framing must surface a failure"
    );
}

/// A server that completes the handshake and then dies is the case the restart
/// circuit exists for: it has proven it can run, so karet keeps trying.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_dies_after_connecting_is_retried_not_written_off() {
    let Some(mut harness) = harness("die-after-handshake", false) else {
        return;
    };
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
}

// --- the shared broker -----------------------------------------------------

/// The broker exists so concurrent karet windows share one server process. This
/// asserts the sharing actually happens, by counting how many times the double
/// really started — code with no coverage at all before now.
#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_on_one_root_share_a_single_server_process() {
    let Some(mut first) = harness("report", true) else {
        return;
    };
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
    let Some(mut events) = backend.take_events() else {
        return;
    };
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
}

/// Through the broker the server is a grandchild, so its death has to cross two
/// process boundaries to be noticed. It used to be missed entirely when the
/// server died before the broker began accepting clients, and the session then
/// waited out the broker's 30-second idle timeout.
#[tokio::test(flavor = "multi_thread")]
async fn a_brokered_server_that_dies_immediately_is_reported_promptly() {
    let Some(mut harness) = harness("exit-now", true) else {
        return;
    };
    assert!(harness.open());
    let started = tokio::time::Instant::now();
    let reported = harness.wait_for(is_lsp_warning).await;
    assert!(reported.is_some(), "the dead server must be reported");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "reporting took {:?}, which suggests the broker idle timeout was waited out",
        started.elapsed()
    );
}

/// A stale endpoint left by a broker that died is superseded rather than
/// trusted. Every early failure in `run_broker` used to leave one behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_broker_endpoint_does_not_block_a_new_launch() {
    let Some(mut harness) = harness("normal", true) else {
        return;
    };
    let Some(registry) = harness
        ._registry
        .as_ref()
        .map(|dir| dir.path().to_path_buf())
    else {
        return;
    };
    let brokers = registry.join("brokers");
    if std::fs::create_dir_all(&brokers).is_err() {
        return;
    }
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
async fn a_diagnostics_companion_attaches_even_with_no_primary_provider() {
    let Some(workspace) = tempfile::tempdir().ok() else {
        return;
    };
    let root = workspace.path();
    // Marks the repository as a Ruff project, which is what selects Ruff as
    // Python's diagnostics companion.
    if std::fs::write(root.join("ruff.toml"), "line-length = 88\n").is_err() {
        return;
    }
    // Resolved by `project_local_spec`, exactly as a real virtualenv would be.
    // Pyright, Python's primary, is deliberately absent.
    let venv = root.join(".venv").join("bin");
    if std::fs::create_dir_all(&venv).is_err() || std::fs::copy(TESTBED, venv.join("ruff")).is_err()
    {
        return;
    }
    let file = root.join("main.py");
    if std::fs::write(&file, "x = 1\n").is_err() {
        return;
    }

    let (backend, _snapshots) = local(SessionConfig {
        process_supervisor: Some(PathBuf::from(TESTBED)),
        roots: vec![root.to_path_buf()],
        ..SessionConfig::default()
    });
    let Some(mut events) = backend.take_events() else {
        return;
    };
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
}

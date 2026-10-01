//! The server task's failure policy, pinned to the millisecond.
//!
//! Each test drives one `server_task` directly on a paused clock and reads back
//! *when* it tried to launch and *what* it reported, so a moved constant, a
//! swapped comparison or a `+` turned `-` in the backoff shows up as a wrong
//! timestamp rather than as nothing at all. Nothing here waits in real time: the
//! clock only jumps to the next deadline the task has set.
//!
//! The expected schedules are written out as literals on purpose. Deriving them
//! from `RESTART_MIN_DELAY` and friends would pass whatever those constants
//! became, which is the defect these exist to catch.

use std::sync::Mutex;

use tokio::time::Instant;

use super::*;
use crate::lsp::runtime::FORMATTING_DEADLINE;
use crate::lsp::runtime::ServerTask;
use crate::lsp::runtime::server_task;

/// What the task reported, reduced to what the policy decides.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seen {
    State(LanguageServerRuntimeState),
    Cleared,
    SpawnFailed,
}

/// When each launch attempt began, as offsets from the test's start.
#[derive(Clone, Default)]
struct Attempts(Arc<Mutex<Vec<Duration>>>);

impl Attempts {
    fn record(&self, start: Instant) {
        if let Ok(mut log) = self.0.lock() {
            log.push(start.elapsed());
        }
    }

    fn millis(&self) -> Vec<u128> {
        self.0
            .lock()
            .map(|log| log.iter().map(Duration::as_millis).collect())
            .unwrap_or_default()
    }

    fn count(&self) -> usize {
        self.0.lock().map(|log| log.len()).unwrap_or_default()
    }
}

/// A launch that fails the way a briefly unreachable broker does: worth
/// retrying, so it spends the budget rather than ending the task.
fn transient_failure(spec: &LspSpec) -> LspError {
    LspError::Launch(Box::new(karet_lsp::LaunchFailure::host(
        spec.command.clone(),
        spec.args.clone(),
        "shared broker unreachable",
    )))
}

/// A connector that fails transiently every time, recording each attempt.
fn always_failing(attempts: &Attempts, start: Instant) -> Connector {
    let attempts = attempts.clone();
    Arc::new(move |spec, _root| {
        attempts.record(start);
        let error = transient_failure(&spec);
        Box::pin(async move { Err(error) })
    })
}

/// A connector whose server completes the handshake and then exits.
fn dying_on_arrival(attempts: &Attempts, start: Instant) -> Connector {
    let attempts = attempts.clone();
    let inner = test_connector(
        Behavior::DieAfterHandshake,
        None,
        Arc::new(AtomicUsize::new(0)),
    );
    Arc::new(move |spec, root| {
        attempts.record(start);
        inner(spec, root)
    })
}

/// A connector that fails its first launch, so a document can be opened while
/// the task is down, then connects to a healthy server every time.
fn failing_once(attempts: &Attempts, start: Instant) -> Connector {
    let attempts = attempts.clone();
    let inner = test_connector(Behavior::Normal, None, Arc::new(AtomicUsize::new(0)));
    Arc::new(move |spec, root| {
        let first = attempts.count() == 0;
        attempts.record(start);
        if first {
            let error = transient_failure(&spec);
            return Box::pin(async move { Err(error) });
        }
        inner(spec, root)
    })
}

/// Start one server task on `connector`.
fn spawn_task(
    connector: Connector,
) -> (mpsc::Sender<ServerCmd>, mpsc::UnboundedReceiver<LspUpdate>) {
    let (tx, rx) = mpsc::channel(16);
    let (updates, observed) = mpsc::unbounded_channel();
    tokio::spawn(server_task(ServerTask {
        spec: LspSpec::new("pretend-analyzer", Vec::new(), vec!["rust".to_owned()]),
        key: SlotKey::new(LanguageServerId::RustAnalyzer, "/work/repo"),
        token: SlotToken::FIRST,
        rx,
        updates,
        connector,
        generation: 0,
    }));
    (tx, observed)
}

/// Reduce an update to what the policy decided, if it decided anything.
fn seen(update: &LspUpdate) -> Option<Seen> {
    match update {
        LspUpdate::RuntimeState { state, .. } => Some(Seen::State(*state)),
        LspUpdate::DiagnosticsCleared { .. } => Some(Seen::Cleared),
        LspUpdate::SpawnFailed { .. } => Some(Seen::SpawnFailed),
        _ => None,
    }
}

/// How a test stops watching: at a point in virtual time, or once the task has
/// tried to launch more often than the schedule allows by then.
///
/// The second bound is what keeps a broken schedule from hanging the suite. A
/// deadline in the past makes the task retry without ever sleeping, and on a
/// paused clock a task that never sleeps never lets the clock reach the
/// test's own deadline either.
struct Watch {
    start: Instant,
    until: Duration,
    attempts: Attempts,
    most: usize,
}

impl Watch {
    /// The next update, or `None` once either bound is reached.
    async fn next(&self, updates: &mut mpsc::UnboundedReceiver<LspUpdate>) -> Option<LspUpdate> {
        if self.attempts.count() > self.most {
            return None;
        }
        tokio::time::timeout_at(self.start + self.until, updates.recv())
            .await
            .ok()
            .flatten()
    }
}

/// Everything the task decided while `watch` lasted, with when it did.
async fn observe(
    updates: &mut mpsc::UnboundedReceiver<LspUpdate>,
    watch: &Watch,
) -> Vec<(u128, Seen)> {
    let mut log = Vec::new();
    while let Some(update) = watch.next(updates).await {
        if let Some(decision) = seen(&update) {
            log.push((watch.start.elapsed().as_millis(), decision));
        }
    }
    log
}

/// The launch-failure schedule: 250ms doubling, five failures a minute, a
/// five-minute circuit, and a 30-second ceiling on the delay.
///
/// Falsified by any of: a different minimum delay, a factor other than two, a
/// budget other than five, a cooldown other than 300s, a window other than 60s
/// (the second circuit opens on a failure 58s after the first of its five), or
/// a missing or different ceiling (the gap after 331.75s would be 32s, not 30s).
#[tokio::test(start_paused = true)]
async fn a_failing_launch_backs_off_on_the_documented_schedule() {
    let start = Instant::now();
    let attempts = Attempts::default();
    let (_tx, mut updates) = spawn_task(always_failing(&attempts, start));
    let watch = Watch {
        start,
        until: Duration::from_secs(680),
        attempts: attempts.clone(),
        most: 11,
    };
    let log = observe(&mut updates, &watch).await;

    assert_eq!(
        attempts.millis(),
        [
            0, 250, 750, 1_750, 3_750,   // the budget, spent
            303_750, // after the five-minute circuit
            307_750, 315_750, 331_750, 361_750, // 4s, 8s, 16s, then the 30s ceiling
            661_750, // the second circuit
        ]
    );
    let states: Vec<_> = log
        .iter()
        .filter_map(|(at, seen)| match seen {
            Seen::State(state) => Some((*at, *state)),
            _ => None,
        })
        .collect();
    use LanguageServerRuntimeState::CircuitOpen as Open;
    use LanguageServerRuntimeState::Retrying as Retry;
    assert_eq!(
        states,
        [
            (0, LanguageServerRuntimeState::Starting),
            (0, Retry),
            (250, Retry),
            (750, Retry),
            (1_750, Retry),
            (3_750, Open),
            (303_750, Retry),
            (307_750, Retry),
            (315_750, Retry),
            (331_750, Retry),
            (361_750, Open),
            (661_750, Retry),
        ]
    );
    assert_eq!(
        log.iter()
            .filter(|(_, seen)| *seen == Seen::SpawnFailed)
            .count(),
        1,
        "a launch failure is reported once, not once per attempt"
    );
}

/// A server that completes its handshake and then exits restarts every 250ms --
/// each connection resets the delay -- until five deaths inside a minute open
/// the circuit. Its diagnostics survive the quick reconnects and are dropped
/// exactly one second after the death that left it down.
///
/// Falsified by: a reconnect scheduled other than `delay` after the death, a
/// grace other than one second (or measured from anywhere but the death), a
/// grace that fires at the start of the outage instead of its end, or the
/// circuit opening on a death other than the fifth.
#[tokio::test(start_paused = true)]
async fn a_server_that_dies_on_arrival_is_retried_then_circuit_broken() {
    let start = Instant::now();
    let attempts = Attempts::default();
    let (_tx, mut updates) = spawn_task(dying_on_arrival(&attempts, start));
    let watch = Watch {
        start,
        until: Duration::from_millis(301_400),
        attempts: attempts.clone(),
        most: 7,
    };
    let log = observe(&mut updates, &watch).await;

    assert_eq!(
        attempts.millis(),
        [0, 250, 500, 750, 1_000, 301_000, 301_250]
    );
    let downs: Vec<_> = log
        .iter()
        .filter(|(_, seen)| *seen != Seen::State(LanguageServerRuntimeState::Running))
        .cloned()
        .collect();
    use LanguageServerRuntimeState::CircuitOpen as Open;
    use LanguageServerRuntimeState::Retrying as Retry;
    assert_eq!(
        downs,
        [
            (0, Seen::State(LanguageServerRuntimeState::Starting)),
            (0, Seen::State(Retry)),
            (250, Seen::State(Retry)),
            (500, Seen::State(Retry)),
            (750, Seen::State(Retry)),
            (1_000, Seen::State(Open)),
            (2_000, Seen::Cleared),
            (301_000, Seen::State(Retry)),
            (301_250, Seen::State(Retry)),
        ]
    );
}

/// A connection whose document replay fails is backed off on the same doubling
/// schedule as a failed launch, continuing from where that launch left it.
///
/// Falsified by: the replay path scheduling its retry other than `delay` after
/// the attempt, or not advancing the delay.
#[tokio::test(start_paused = true)]
async fn a_failed_replay_backs_off_like_a_failed_launch() -> TestResult {
    let start = Instant::now();
    let attempts = Attempts::default();
    let (tx, mut updates) = spawn_task(failing_once(&attempts, start));
    // Something to replay, delivered while the first retry is pending. A
    // relative path has no file URI, so every replay of it fails -- over a
    // connection that is otherwise perfectly healthy.
    tx.send(ServerCmd::DidOpen {
        path: PathBuf::from("src/main.rs"),
        language: "rust".to_owned(),
        version: 1,
        text: "fn main() {}\n".to_owned(),
    })
    .await?;
    let watch = Watch {
        start,
        until: Duration::from_millis(3_800),
        attempts: attempts.clone(),
        most: 5,
    };
    let mut replays = Vec::new();
    while let Some(update) = watch.next(&mut updates).await {
        if let LspUpdate::RuntimeState {
            error: Some(error), ..
        } = update
            && error == "document replay failed"
        {
            replays.push(start.elapsed().as_millis());
        }
    }
    assert_eq!(attempts.millis(), [0, 250, 750, 1_750, 3_750]);
    assert_eq!(replays, [250, 750, 1_750, 3_750]);
    Ok(())
}

/// What a request gets while the server is down depends on why: a retry is
/// expected back shortly, so a hint request is told to try again; an open
/// circuit may last indefinitely, so it is answered empty instead.
///
/// Falsified by inverting the circuit flag the task keeps.
#[tokio::test(start_paused = true)]
async fn a_hint_request_is_answered_by_the_kind_of_outage() -> TestResult {
    let start = Instant::now();
    let attempts = Attempts::default();
    let (tx, mut updates) = spawn_task(always_failing(&attempts, start));
    let ask = |request| ServerCmd::InlayHints {
        request: RequestId(request),
        doc: DocumentId(1),
        version: 1,
        path: PathBuf::from("/work/repo/src/main.rs"),
        range: Range::default(),
    };

    let mut asked_while_retrying = false;
    let mut asked_while_open = false;
    let mut answers = Vec::new();
    // Ten seconds spans the five launches that open the circuit (by 3.75s),
    // with room for one more attempt and nothing after it.
    let watch = Watch {
        start,
        until: Duration::from_secs(10),
        attempts: attempts.clone(),
        most: 6,
    };
    while let Some(update) = watch.next(&mut updates).await {
        match update {
            LspUpdate::RuntimeState {
                state: LanguageServerRuntimeState::Retrying,
                ..
            } if !asked_while_retrying => {
                asked_while_retrying = true;
                tx.send(ask(1)).await?;
            },
            LspUpdate::RuntimeState {
                state: LanguageServerRuntimeState::CircuitOpen,
                ..
            } if !asked_while_open => {
                asked_while_open = true;
                tx.send(ask(2)).await?;
            },
            LspUpdate::InlayHintsFailed { request, .. } => answers.push((request, "retry")),
            LspUpdate::InlayHints { request, hints, .. } if hints.is_empty() => {
                answers.push((request, "empty"));
            },
            _ => {},
        }
    }
    assert_eq!(answers, [(RequestId(1), "retry"), (RequestId(2), "empty")]);
    Ok(())
}

/// The formatting wait outlasts the session's own ten-second format-on-save
/// deadline by the three seconds its sweep may lag, and no more.
///
/// Falsified by: giving up before the session's sweep (racing it and throwing
/// away an answer about to be used), or by holding the task longer.
#[test]
fn the_formatting_wait_is_the_save_deadline_plus_one_sweep() {
    assert_eq!(FORMATTING_DEADLINE, Duration::from_secs(13));
}

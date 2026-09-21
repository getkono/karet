//! When a connected language server counts as lost.
//!
//! The server task used to learn this one way only: a request it made came back
//! `Closed`. That made loss detection demand-driven, and produced two failures
//! the user saw as "the LSP just stops working".
//!
//! A server that exits while nobody is typing was invisible. The reader task had
//! already set its closed flag and the diagnostics forwarder had already
//! returned, but the server task was parked on `rx.recv()` with no liveness arm,
//! so the state stayed `Running`, the badge stayed green, and the retry clock did
//! not start until the next keystroke. [`Wake`] adds that arm.
//!
//! A server that stops answering but holds its pipe open was worse: every request
//! timed out, each timeout was logged and discarded, and nothing ever concluded
//! the connection was dead. [`FailureTally`] gives repeated timeouts a verdict.

use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::CIRCUIT_COOLDOWN;
use super::RESTART_LIMIT;
use super::RESTART_MAX_DELAY;
use super::RESTART_MIN_DELAY;
use super::RESTART_WINDOW;
use super::message::LspUpdate;
use super::message::ServerCmd;
use crate::api::LanguageServerRuntimeState;

/// How long a connection must last before it counts as having worked.
///
/// Connecting is not the same as working. A server that exits as soon as it has
/// read `didOpen` completes its handshake perfectly every time, so a budget
/// cleared on connect could never be spent and the restart circuit could never
/// open. That loop used to be capped by accident -- an idle death went unnoticed,
/// so nothing respawned until the user typed -- and noticing deaths promptly
/// removes that accidental cap. A connection shorter than this counts against the
/// budget; a longer one earns a fresh start.
pub(super) const STABLE_CONNECTION: Duration = Duration::from_secs(10);

/// Silent deaths within [`HANG_WINDOW`] before a provider goes behind the circuit.
///
/// Two, not five, because each one costs at least [`TIMEOUT_DEATH_LIMIT`] request
/// timeouts to establish -- a minute and a half of a server answering nothing.
/// Waiting for five would spend seven minutes proving what two already show.
pub(super) const HANG_LIMIT: usize = 2;

/// How long a silent death counts against a provider.
///
/// Windowed rather than counted outright, and the window is its own because the
/// 60-second failure window is too short to hold even one hang cycle. Without a
/// window the count only ever rises: a provider that goes silent once, reconnects,
/// serves perfectly for an hour and then goes silent again would be circuit-broken
/// on the second -- and would stay one hang away from a five-minute outage for the
/// rest of the session. Ten minutes comfortably spans consecutive hangs, which
/// land 90 to 200 seconds apart, while forgiving an hourly one.
pub(super) const HANG_WINDOW: Duration = Duration::from_secs(600);

/// Consecutive timeouts that together mean the connection is dead.
///
/// More than one because a single slow answer is ordinary: a cold
/// rust-analyzer or a jdtls mid-import can miss one 30-second deadline and
/// still be working. Three in a row is a server that has stopped talking.
pub(super) const TIMEOUT_DEATH_LIMIT: u32 = 3;

/// What the connected loop woke up for.
pub(super) enum Wake {
    /// A command arrived, or the channel closed (`None`).
    Command(Option<ServerCmd>),
    /// The debounce window expired with no further edit: flush the pending one.
    Quiet,
    /// The connection died.
    Lost,
}

/// Wait for whichever comes first: a command, a quiet debounce window, or the
/// connection's death.
///
/// The select is `biased` so liveness is polled first. With a dead connection
/// and queued commands, taking a command would only drive it into a peer that is
/// gone; the commands stay queued and are answered by the disconnected branch,
/// which also remembers their documents for replay.
pub(super) async fn next_wake(
    rx: &mut mpsc::Receiver<ServerCmd>,
    client: Option<&LspClient>,
    debounce: std::time::Duration,
    has_pending: bool,
) -> Wake {
    let Some(client) = client else {
        return Wake::Command(rx.recv().await);
    };
    if has_pending {
        tokio::select! {
            biased;
            () = client.closed() => Wake::Lost,
            cmd = tokio::time::timeout(debounce, rx.recv()) => match cmd {
                Ok(cmd) => Wake::Command(cmd),
                Err(_quiet) => Wake::Quiet,
            },
        }
    } else {
        tokio::select! {
            biased;
            () = client.closed() => Wake::Lost,
            cmd = rx.recv() => Wake::Command(cmd),
        }
    }
}

/// Whether a connection that has just dropped had proven itself.
///
/// `None` -- never connected at all -- is not stable: it is the launch-failure
/// path, which has its own accounting.
pub(super) fn was_stable(connected_at: Option<Instant>) -> bool {
    connected_at.is_some_and(|since| since.elapsed() >= STABLE_CONNECTION)
}

/// Charge one lost connection against the restart budget, returning how long to
/// wait and what state to report.
///
/// A connection that lasted long enough to count as real earns a fresh budget and
/// the minimum delay: it worked once, and whatever killed it may well be
/// transient. A connection that died on arrival is charged, because a server that
/// completes its handshake and then exits would otherwise loop forever -- it never
/// fails to *connect*, so nothing in the launch-failure accounting ever sees it.
pub(super) fn charge_disconnect(
    connected_at: Option<Instant>,
    hangs: &mut VecDeque<Instant>,
    hung: bool,
    failures: &mut VecDeque<Instant>,
    restart_delay: &mut Duration,
    language: &str,
) -> (Duration, LanguageServerRuntimeState) {
    // A silent connection is counted separately from the sliding failure window,
    // because it cannot be caught by it. Condemning one takes three request
    // timeouts -- at least 90 seconds -- while the window is 60, so the previous
    // charge has always expired before the next lands: the budget never reaches
    // two, `RESTART_LIMIT` is unreachable, and the kill-and-respawn loop runs
    // forever at one cycle per 90 seconds. A straight count of consecutive silent
    // deaths has no such hole.
    if hung {
        let now = Instant::now();
        while hangs
            .front()
            .is_some_and(|hang| now.duration_since(*hang) > HANG_WINDOW)
        {
            hangs.pop_front();
        }
        hangs.push_back(now);
        if hangs.len() >= HANG_LIMIT {
            tracing::warn!(
                hangs = hangs.len(),
                language,
                "language server keeps going silent; restart circuit opened"
            );
            hangs.clear();
            return (CIRCUIT_COOLDOWN, LanguageServerRuntimeState::CircuitOpen);
        }
        let delay = *restart_delay;
        *restart_delay = (*restart_delay * 2).min(RESTART_MAX_DELAY);
        return (delay, LanguageServerRuntimeState::Retrying);
    }
    hangs.clear();
    if was_stable(connected_at) {
        failures.clear();
        *restart_delay = RESTART_MIN_DELAY;
        return (*restart_delay, LanguageServerRuntimeState::Retrying);
    }
    let now = Instant::now();
    while failures
        .front()
        .is_some_and(|failure| now.duration_since(*failure) > RESTART_WINDOW)
    {
        failures.pop_front();
    }
    failures.push_back(now);
    if failures.len() >= RESTART_LIMIT {
        tracing::warn!(
            language,
            "language server keeps dying on startup; restart circuit opened"
        );
        return (CIRCUIT_COOLDOWN, LanguageServerRuntimeState::CircuitOpen);
    }
    let delay = *restart_delay;
    *restart_delay = (*restart_delay * 2).min(RESTART_MAX_DELAY);
    (delay, LanguageServerRuntimeState::Retrying)
}

/// Running verdict on a connection, from the calls made over it.
#[derive(Default)]
pub(super) struct FailureTally {
    /// Timeouts since the last answered call.
    consecutive_timeouts: u32,
    /// Whether this connection has ever answered anything.
    ///
    /// The gate on condemning a connection for timing out. "Stopped answering"
    /// presupposes having answered: a server that has not yet answered its first
    /// request is far more likely to be starting up than hung, and jdtls or
    /// rust-analyzer on a large repository can spend minutes there. Killing one
    /// mid-import restarts the import, which guarantees the next requests time out
    /// too -- an unbounded kill-and-reindex loop that never reaches a usable state.
    answered: bool,
    /// Whether the connection was condemned for silence rather than closure.
    ///
    /// Read by the restart accounting: such a connection necessarily lived long
    /// enough to look stable, so without this it would earn a fresh failure budget
    /// every time and could never open the circuit.
    hung: bool,
}

impl FailureTally {
    /// Record one call's outcome, returning whether the connection is now dead.
    ///
    /// `Closed` is immediate and obvious. `Timeout` needs a tally because one
    /// slow answer is not evidence of anything; only a run of them is. Every
    /// other error belongs to the request, not the connection -- a server that
    /// rejects `textDocument/rename` is still perfectly alive -- so it is logged
    /// and the connection keeps its clean record.
    pub(super) fn note<T>(
        &mut self,
        result: Result<T, LspError>,
        dead: &mut bool,
        updates: &mpsc::UnboundedSender<LspUpdate>,
        language: &str,
        generation: u64,
    ) {
        match result {
            // Deliberately not proof of an answer. Most callers of this are
            // notification sends -- `didOpen`, `didChange` -- and those are local
            // enqueues onto the outbound channel: `Ok` means the queue accepted a
            // frame, not that any server read it. Treating one as an answer made
            // the gate below satisfied milliseconds after connecting, since the
            // first thing a new task processes is always a `didOpen`.
            Ok(_) => {},
            Err(LspError::Closed) => self.die(dead, updates, language, generation),
            Err(LspError::Timeout) => {
                self.consecutive_timeouts = self.consecutive_timeouts.saturating_add(1);
                if self.consecutive_timeouts >= TIMEOUT_DEATH_LIMIT && self.answered {
                    tracing::warn!(
                        language,
                        timeouts = self.consecutive_timeouts,
                        "language server stopped answering; treating it as dead"
                    );
                    self.hung = true;
                    self.die(dead, updates, language, generation);
                } else {
                    tracing::warn!(
                        language,
                        timeouts = self.consecutive_timeouts,
                        answered = self.answered,
                        "language server request timed out"
                    );
                }
            },
            Err(e) => {
                self.consecutive_timeouts = 0;
                tracing::warn!(language, error = %e, "language server call failed");
            },
        }
    }

    /// Record a death *observed* on the connection itself rather than inferred
    /// from a call that failed.
    ///
    /// Both routes must report identically. Adding the liveness arm without this
    /// made the better detection quieter than the worse one: noticing the exit
    /// immediately meant no request was ever issued, so nothing went through
    /// [`Self::note`], and the death notification the user relies on vanished for
    /// exactly the crashes that are now caught soonest.
    ///
    /// Called only while a client is still held, so the death is always news and
    /// needs no de-duplication of its own.
    pub(super) fn note_lost(
        &mut self,
        updates: &mpsc::UnboundedSender<LspUpdate>,
        language: &str,
        generation: u64,
    ) {
        let mut unreported = false;
        self.die(&mut unreported, updates, language, generation);
    }

    /// Observe a *request* outcome, passing it through unchanged.
    ///
    /// This is what records that the server answered: a response to a request the
    /// editor issued after the handshake. The handshake itself does not count --
    /// every connected server answers `initialize`, so counting it would make
    /// "has answered" true for exactly the servers this distinction exists to
    /// protect, like a jdtls that completes its handshake and then imports in
    /// silence for two minutes.
    pub(super) fn observe<T>(&mut self, result: Result<T, LspError>) -> Result<T, LspError> {
        if result.is_ok() {
            self.consecutive_timeouts = 0;
            self.answered = true;
        }
        result
    }

    /// Whether this connection was condemned for going silent.
    ///
    /// Such a connection outlived [`STABLE_CONNECTION`] by construction -- it took
    /// at least three request timeouts to condemn it -- so the restart accounting
    /// must not read its age as proof that it worked.
    pub(super) fn hung(&self) -> bool {
        self.hung
    }

    /// Record that the connection is gone, reporting it at most once.
    fn die(
        &mut self,
        dead: &mut bool,
        updates: &mpsc::UnboundedSender<LspUpdate>,
        language: &str,
        generation: u64,
    ) {
        self.consecutive_timeouts = 0;
        if !*dead {
            *dead = true;
            let _ = updates.send(LspUpdate::ServerDied {
                generation,
                language: language.to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally() -> (
        FailureTally,
        mpsc::UnboundedSender<LspUpdate>,
        mpsc::UnboundedReceiver<LspUpdate>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        (FailureTally::default(), tx, rx)
    }

    /// A connect-then-die loop must be bounded.
    ///
    /// The hazard: the failure budget used to be cleared on every successful
    /// connect, and a server that exits as soon as it has read `didOpen` connects
    /// perfectly every time -- so nothing in the launch accounting ever saw it and
    /// the circuit could never open. That loop was capped only by accident, because
    /// an idle death went unnoticed until the user typed. Noticing deaths promptly
    /// removes the accident, so the bound has to be real.
    #[test]
    fn a_server_that_dies_on_arrival_eventually_opens_the_circuit() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        // Each cycle connects, then dies well inside the stability threshold.
        let cycles: Vec<_> = (0..RESTART_LIMIT)
            .map(|_| {
                charge_disconnect(
                    Some(Instant::now()),
                    &mut hangs,
                    false,
                    &mut failures,
                    &mut delay,
                    "rust",
                )
            })
            .collect();
        let opened = cycles
            .iter()
            .filter(|(_, state)| *state == LanguageServerRuntimeState::CircuitOpen)
            .count();
        assert_eq!(
            opened, 1,
            "a connect-then-die loop never opened the circuit"
        );
        assert_eq!(
            cycles.last().map(|(waited, state)| (*waited, *state)),
            Some((CIRCUIT_COOLDOWN, LanguageServerRuntimeState::CircuitOpen)),
            "the circuit opened somewhere other than the budget's last cycle"
        );
    }

    /// The counterpart: a connection that worked must not be punished for it.
    #[test]
    fn a_proven_connection_earns_a_fresh_budget() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        // Spend most of the budget on quick deaths.
        for _ in 0..RESTART_LIMIT.saturating_sub(1) {
            charge_disconnect(
                Some(Instant::now()),
                &mut hangs,
                false,
                &mut failures,
                &mut delay,
                "rust",
            );
        }
        assert!(delay > RESTART_MIN_DELAY, "the backoff never advanced");

        let proven = Instant::now().checked_sub(STABLE_CONNECTION);
        let (waited, state) =
            charge_disconnect(proven, &mut hangs, false, &mut failures, &mut delay, "rust");
        assert_eq!(state, LanguageServerRuntimeState::Retrying);
        assert_eq!(waited, RESTART_MIN_DELAY, "the backoff was not reset");
        assert!(failures.is_empty(), "the budget was not cleared");
    }

    /// Repeated silent deaths must open the circuit, and the sliding failure window
    /// cannot be what closes that loop.
    ///
    /// Condemning a silent connection takes three request timeouts -- at least 90
    /// seconds -- while the window is 60, so the previous charge has always expired
    /// before the next lands. Counted through that window the budget never reaches
    /// two, `RESTART_LIMIT` is unreachable, and the kill-and-respawn loop runs
    /// forever at one cycle per 90 seconds. Hence a straight count of consecutive
    /// silent deaths.
    #[test]
    fn repeated_silent_deaths_open_the_circuit() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        // Each one looks perfectly stable by age, and the window is emptied between
        // them -- the two properties that defeated window-based counting.
        let proven = Instant::now().checked_sub(STABLE_CONNECTION);
        let cycles: Vec<_> = (0..HANG_LIMIT)
            .map(|_| {
                failures.clear();
                charge_disconnect(proven, &mut hangs, true, &mut failures, &mut delay, "java")
            })
            .collect();
        assert_eq!(
            cycles.last().map(|(waited, state)| (*waited, *state)),
            Some((CIRCUIT_COOLDOWN, LanguageServerRuntimeState::CircuitOpen)),
            "repeated silent deaths never opened the circuit"
        );
    }

    /// A hang the user has long since recovered from must not count towards the
    /// next one.
    ///
    /// Without a window the count only rises: a provider that goes silent once,
    /// reconnects, serves perfectly for an hour and goes silent again would be
    /// circuit-broken on the second -- and would stay one hang away from a
    /// five-minute outage for the rest of the session.
    #[test]
    fn a_hang_outside_the_window_is_forgiven() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        // One hang, long ago.
        hangs.push_back(
            Instant::now()
                .checked_sub(HANG_WINDOW * 2)
                .unwrap_or_else(Instant::now),
        );
        let proven = Instant::now().checked_sub(STABLE_CONNECTION);
        let (_, state) =
            charge_disconnect(proven, &mut hangs, true, &mut failures, &mut delay, "rust");
        assert_eq!(
            state,
            LanguageServerRuntimeState::Retrying,
            "a stale hang opened the circuit"
        );
        assert_eq!(hangs.len(), 1, "the stale hang was not expired");
    }

    /// A death for any reason other than silence resets the count, so occasional
    /// hangs spread across a session never accumulate into a circuit.
    #[test]
    fn a_non_silent_death_resets_the_hang_count() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        let proven = Instant::now().checked_sub(STABLE_CONNECTION);
        charge_disconnect(proven, &mut hangs, true, &mut failures, &mut delay, "java");
        assert_eq!(hangs.len(), 1);
        charge_disconnect(proven, &mut hangs, false, &mut failures, &mut delay, "java");
        assert!(
            hangs.is_empty(),
            "a clean death did not reset the hang count"
        );
    }

    /// Failures older than the window stop counting, so a provider that misbehaves
    /// twice an hour never accumulates its way into a circuit.
    #[test]
    fn failures_outside_the_window_are_forgotten() {
        let mut failures = VecDeque::new();
        let mut delay = RESTART_MIN_DELAY;
        let mut hangs = VecDeque::new();
        for _ in 0..RESTART_LIMIT.saturating_sub(1) {
            let stale = Instant::now()
                .checked_sub(RESTART_WINDOW * 2)
                .unwrap_or_else(Instant::now);
            failures.push_back(stale);
        }
        let (_, state) = charge_disconnect(
            Some(Instant::now()),
            &mut hangs,
            false,
            &mut failures,
            &mut delay,
            "rust",
        );
        assert_eq!(
            state,
            LanguageServerRuntimeState::Retrying,
            "stale failures opened the circuit"
        );
        assert_eq!(failures.len(), 1, "stale failures were not expired");
    }

    #[test]
    fn a_closed_connection_dies_at_once() {
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        tally.note::<()>(Err(LspError::Closed), &mut dead, &tx, "rust", 1);
        assert!(dead);
        assert!(matches!(rx.try_recv(), Ok(LspUpdate::ServerDied { .. })));
    }

    #[test]
    fn one_death_is_reported_once() {
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        tally.note::<()>(Err(LspError::Closed), &mut dead, &tx, "rust", 1);
        tally.note::<()>(Err(LspError::Closed), &mut dead, &tx, "rust", 1);
        assert!(matches!(rx.try_recv(), Ok(LspUpdate::ServerDied { .. })));
        assert!(rx.try_recv().is_err(), "the second close reported again");
    }

    #[test]
    fn a_single_timeout_is_not_a_death() {
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        assert!(!dead, "one slow answer condemned the connection");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timeouts_never_condemn_a_server_that_has_not_answered_yet() {
        // The hazard this closes: "stopped answering" presupposes having answered.
        // jdtls importing a large build answers nothing for a minute or two; three
        // 30-second timeouts would condemn it, and killing it restarts the import,
        // which guarantees the next three time out too -- an unbounded
        // kill-and-reindex loop that never reaches a usable state.
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        for _ in 0..TIMEOUT_DEATH_LIMIT.saturating_mul(10) {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "java", 1);
        }
        assert!(
            !dead,
            "a server that never answered was condemned for silence"
        );
        assert!(!tally.hung());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_hung_connection_is_never_read_as_proven() {
        // A connection condemned for silence always looks old enough to be stable,
        // because condemning one takes three request timeouts. If the restart
        // accounting read that age as proof it worked, it would hand the server a
        // fresh budget every cycle and the circuit could never open.
        let (mut tally, tx, _rx) = tally();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..TIMEOUT_DEATH_LIMIT {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        }
        assert!(dead);
        assert!(tally.hung(), "a silent death was not flagged as hung");
    }

    #[test]
    fn a_closed_connection_is_not_flagged_as_hung() {
        let (mut tally, tx, _rx) = tally();
        let mut dead = false;
        tally.note::<()>(Err(LspError::Closed), &mut dead, &tx, "rust", 1);
        assert!(!tally.hung());
    }

    #[test]
    fn stability_is_measured_from_when_the_connection_opened() {
        assert!(!was_stable(None), "never connected is not stable");
        assert!(
            !was_stable(Some(Instant::now())),
            "a connection that just opened is not yet proven"
        );
        assert!(
            was_stable(Instant::now().checked_sub(STABLE_CONNECTION)),
            "a connection older than the threshold is proven"
        );
    }

    #[test]
    fn a_run_of_timeouts_is_a_death() {
        // The defect this covers: a server holding its pipe open but answering
        // nothing produced one timeout per request forever, each logged and
        // discarded, while the badge read healthy.
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        // It has to have answered something first, or silence is indistinguishable
        // from a server that is still starting up.
        let _answered = tally.observe(Ok::<(), LspError>(()));
        let _answered = rx.try_recv();
        for _ in 0..TIMEOUT_DEATH_LIMIT {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        }
        assert!(dead);
        assert!(matches!(rx.try_recv(), Ok(LspUpdate::ServerDied { .. })));
    }

    #[test]
    fn an_answered_call_clears_the_timeout_run() {
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        }
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        }
        assert!(!dead, "timeouts either side of a success were summed");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn an_unsupported_method_does_not_condemn_the_connection() {
        // A server that rejects one request is still alive; only the request
        // failed. Counting these would restart a working server for asking it
        // something it does not implement.
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
        for _ in 0..TIMEOUT_DEATH_LIMIT.saturating_mul(3) {
            tally.note::<()>(
                Err(LspError::Server("method not found".to_owned())),
                &mut dead,
                &tx,
                "rust",
                1,
            );
        }
        assert!(!dead);
        assert!(rx.try_recv().is_err());
    }
}

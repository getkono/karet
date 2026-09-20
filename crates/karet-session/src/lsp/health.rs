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

use std::time::Duration;
use std::time::Instant;

use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::message::LspUpdate;
use super::message::ServerCmd;

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

/// Running verdict on a connection, from the calls made over it.
#[derive(Default)]
pub(super) struct FailureTally {
    /// Timeouts since the last answered call.
    consecutive_timeouts: u32,
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
            Ok(_) => self.consecutive_timeouts = 0,
            Err(LspError::Closed) => self.die(dead, updates, language, generation),
            Err(LspError::Timeout) => {
                self.consecutive_timeouts = self.consecutive_timeouts.saturating_add(1);
                if self.consecutive_timeouts >= TIMEOUT_DEATH_LIMIT {
                    tracing::warn!(
                        language,
                        timeouts = self.consecutive_timeouts,
                        "language server stopped answering; treating it as dead"
                    );
                    self.die(dead, updates, language, generation);
                } else {
                    tracing::warn!(
                        language,
                        timeouts = self.consecutive_timeouts,
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
    fn a_run_of_timeouts_is_a_death() {
        // The defect this covers: a server holding its pipe open but answering
        // nothing produced one timeout per request forever, each logged and
        // discarded, while the badge read healthy.
        let (mut tally, tx, mut rx) = tally();
        let mut dead = false;
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
        for _ in 0..TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            tally.note::<()>(Err(LspError::Timeout), &mut dead, &tx, "rust", 1);
        }
        tally.note(Ok(()), &mut dead, &tx, "rust", 1);
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

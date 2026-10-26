//! The server-task half of code actions: listing them, filling in a deferred
//! edit, and running a command.
//!
//! One command and one update carry all three steps, so the task loop in
//! `runtime` spends one arm on them. The session sequences the steps -- it
//! applies an action's edit between resolving it and running its command,
//! because only the session holds the documents the edit lands in.
//!
//! An *automatic* listing -- the lightbulb probe the editor sends each time
//! the caret settles -- is the exception: it is background work nobody is
//! waiting on, so it never reaches [`serve`]. The task hands it to a
//! [`ProbeFlight`] instead, which keeps it off the serial path exactly as
//! `hint_flight` keeps inlay hints off it: deferred until the server has its
//! text rather than flushing the debounced edit, at most one running per
//! document, and only the newest waiting probe launched. [`deliver`] then
//! reports its outcome without charging the hang streak.

use std::path::PathBuf;
use std::sync::Arc;

use karet_core::CodeAction;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::commands::report_unsupported;
use super::health::FailureTally;
use super::hint_flight::Answer;
use super::hint_flight::Ask;
use super::hint_flight::Background;
use super::hint_flight::Flight;
use super::hint_flight::FlightTag;
use super::hint_flight::is_cancellation;
use super::message::LspUpdate;
use super::slot::SlotKey;
use super::slot::SlotToken;
use crate::api::AppliedWorkspaceEdit;
use crate::api::CodeActionTrigger;
use crate::api::DocumentId;
use crate::api::RequestId;

/// Why no server answered a code-action step.
pub(crate) const UNAVAILABLE: &str = "the language server is not available";

/// One code-action step for a server task.
#[derive(Debug)]
pub(crate) enum CodeActionCmd {
    /// List the actions over `range` (UTF-16 columns).
    List {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: std::path::PathBuf,
        range: Range,
        kinds: Vec<String>,
        trigger: CodeActionTrigger,
    },
    /// Fill in the edit of an action that arrived without one.
    Resolve {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        action: Box<CodeAction>,
    },
    /// Run the action's command, its edit (if any) already applied as
    /// `applied` says; that report is carried back to finish the answer.
    Execute {
        request: RequestId,
        applied: Box<AppliedWorkspaceEdit>,
        action: Box<CodeAction>,
    },
}

/// What a server task made of a [`CodeActionCmd`]: its result, or the reason
/// it has none.
#[derive(Debug)]
pub(crate) enum CodeActionAnswer {
    /// The listed actions, edits still in UTF-16 columns.
    Listed {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        actions: Result<Vec<CodeAction>, String>,
    },
    /// The action with its edit filled in (UTF-16 columns), or unchanged
    /// when there was nothing to resolve.
    Resolved {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        action: Result<CodeAction, String>,
    },
    /// The command ran, or why it did not.
    Executed {
        request: RequestId,
        applied: Box<AppliedWorkspaceEdit>,
        result: Result<(), String>,
    },
}

/// What a lightbulb probe asks besides its document: the range (UTF-16
/// columns) and the kind filter.
pub(super) struct ProbeParams {
    range: Range,
    kinds: Vec<String>,
}

/// Automatic code-action listings -- the lightbulb probe -- as a
/// [`Background`] kind.
pub(super) enum Probes {}

impl Background for Probes {
    type Output = Vec<CodeAction>;
    type Params = ProbeParams;

    const WHAT: &'static str = "code-action probe";

    async fn request(
        client: Arc<LspClient>,
        path: PathBuf,
        params: ProbeParams,
    ) -> Result<Vec<CodeAction>, LspError> {
        let trigger = karet_lsp::CodeActionTrigger::Automatic;
        client
            .code_actions(&path, params.range, &params.kinds, trigger)
            .await
    }

    fn unanswered(updates: &mpsc::UnboundedSender<LspUpdate>, tag: FlightTag, generation: u64) {
        answer_listed(updates, tag, Err(UNAVAILABLE.to_owned()), generation);
    }
}

/// The lightbulb probes of one server task.
pub(super) type ProbeFlight = Flight<Probes>;

impl CodeActionCmd {
    /// This step as a lightbulb probe, when it is one: an automatic listing,
    /// which the task keeps off its serial path. Any other step comes back
    /// unchanged.
    pub(super) fn into_probe(self) -> Result<Ask<ProbeParams>, Self> {
        match self {
            Self::List {
                request,
                doc,
                version,
                path,
                range,
                kinds,
                trigger: CodeActionTrigger::Automatic,
            } => Ok(Ask {
                tag: FlightTag {
                    request,
                    doc,
                    version,
                },
                path,
                params: ProbeParams { range, kinds },
            }),
            other => Err(other),
        }
    }

    /// The answer when no server can take this step, saying `reason`.
    pub(crate) fn unanswered(self, reason: &str) -> CodeActionAnswer {
        let reason = reason.to_owned();
        match self {
            Self::List {
                request,
                doc,
                version,
                ..
            } => CodeActionAnswer::Listed {
                request,
                doc,
                version,
                actions: Err(reason),
            },
            Self::Resolve {
                request,
                doc,
                version,
                ..
            } => CodeActionAnswer::Resolved {
                request,
                doc,
                version,
                action: Err(reason),
            },
            Self::Execute {
                request, applied, ..
            } => CodeActionAnswer::Executed {
                request,
                applied,
                result: Err(reason),
            },
        }
    }
}

/// The task state a step reports its outcome into.
pub(super) struct StepContext<'a> {
    pub(super) tally: &'a mut FailureTally,
    pub(super) dead: &'a mut bool,
    pub(super) updates: &'a mpsc::UnboundedSender<LspUpdate>,
    pub(super) key: &'a SlotKey,
    pub(super) token: SlotToken,
    pub(super) generation: u64,
}

/// A server task's connection health, as a step on its serial path reports
/// into it: whether the connection is dead, its failure tally, where updates
/// go, and the slot and incarnation it is.
pub(super) type Health<'a> = (
    &'a mut bool,
    &'a mut FailureTally,
    &'a mpsc::UnboundedSender<LspUpdate>,
    &'a SlotKey,
    SlotToken,
);

impl<'a> StepContext<'a> {
    /// The context of a step taken under `generation` on a task whose
    /// health is `health`.
    pub(super) fn of((dead, tally, updates, key, token): Health<'a>, generation: u64) -> Self {
        Self {
            tally,
            dead,
            updates,
            key,
            token,
            generation,
        }
    }
}

impl StepContext<'_> {
    /// A request's error as the user's answer. A refusal or a server's error
    /// reply belongs to the request; anything else (a timeout, a closed
    /// connection) also counts against the connection.
    ///
    /// Only for steps the user is waiting on, which is what the hang streak
    /// measures; a lightbulb probe is reported by [`deliver`] instead.
    pub(super) fn settle<T>(&mut self, result: Result<T, LspError>) -> Result<T, String> {
        match self.tally.observe(result) {
            Ok(value) => Ok(value),
            Err(LspError::Server(reason) | LspError::Protocol(reason)) => Err(reason),
            Err(error @ LspError::Unsupported { .. }) => Err(error.to_string()),
            Err(error) => {
                let reason = error.to_string();
                self.tally
                    .note::<()>(Err(error), self.dead, self.updates, self.key, self.token);
                Err(reason)
            },
        }
    }
}

/// Take one code-action step on a live connection.
pub(super) async fn serve(
    client: &LspClient,
    cmd: CodeActionCmd,
    context: &mut StepContext<'_>,
) -> CodeActionAnswer {
    match cmd {
        CodeActionCmd::List {
            request,
            doc,
            version,
            path,
            range,
            kinds,
            trigger,
        } => {
            let lsp_trigger = match trigger {
                CodeActionTrigger::Invoked => karet_lsp::CodeActionTrigger::Invoked,
                CodeActionTrigger::Automatic => karet_lsp::CodeActionTrigger::Automatic,
            };
            let result = client.code_actions(&path, range, &kinds, lsp_trigger).await;
            // Only a request the user made by hand explains a server without
            // code actions; a probe as the caret moves stays quiet.
            if trigger == CodeActionTrigger::Invoked {
                report_unsupported(
                    &result,
                    context.updates,
                    context.generation,
                    request,
                    context.key,
                    ServerFeature::CodeAction,
                );
            }
            CodeActionAnswer::Listed {
                request,
                doc,
                version,
                actions: context.settle(result),
            }
        },
        CodeActionCmd::Resolve {
            request,
            doc,
            version,
            action,
        } => {
            let result = client.resolve_code_action(&action).await;
            CodeActionAnswer::Resolved {
                request,
                doc,
                version,
                action: context.settle(result),
            }
        },
        CodeActionCmd::Execute {
            request,
            applied,
            action,
        } => {
            let result = client.run_code_action_command(&action).await;
            CodeActionAnswer::Executed {
                request,
                applied,
                result: context.settle(result),
            }
        },
    }
}

/// Report one finished lightbulb probe: note what it says about the
/// connection, then answer it.
///
/// Charged as `hint_flight::deliver` charges an inlay-hint request, and for
/// the same reason: a probe is background work, sent on every caret settle,
/// that runs beside the requests the hang streak is calibrated for. An answer
/// arms the liveness gate without resetting the streak, a timeout is not
/// counted toward it, and only a closed connection is a death. A server
/// without code actions stays quiet too -- only a listing the user asked for
/// by hand explains that.
pub(super) fn deliver(
    answer: Answer<Vec<CodeAction>>,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    key: &SlotKey,
    token: SlotToken,
    generation: u64,
) {
    let actions = match answer.result {
        Ok(actions) => {
            tally.note_background_answer();
            Ok(actions)
        },
        Err(LspError::Closed) => {
            tally.note::<()>(Err(LspError::Closed), dead, updates, key, token);
            Err(LspError::Closed.to_string())
        },
        Err(error @ (LspError::Timeout | LspError::Unsupported { .. })) => {
            tracing::debug!(language = %key, %error, "code-action probe unanswered; not charged");
            Err(error.to_string())
        },
        Err(error) if is_cancellation(&error) => {
            tracing::debug!(language = %key, %error, "code-action probe cancelled; not charged");
            Err(error.to_string())
        },
        Err(error) => {
            tracing::warn!(language = %key, %error, "code-action probe failed; not charged");
            Err(error.to_string())
        },
    };
    answer_listed(updates, answer.tag, actions, generation);
}

/// Answer the listing `tag` echoes with `actions`.
fn answer_listed(
    updates: &mpsc::UnboundedSender<LspUpdate>,
    tag: FlightTag,
    actions: Result<Vec<CodeAction>, String>,
    generation: u64,
) {
    let answer = CodeActionAnswer::Listed {
        request: tag.request,
        doc: tag.doc,
        version: tag.version,
        actions,
    };
    let _ = updates.send(LspUpdate::CodeAction { generation, answer });
}

#[cfg(test)]
mod tests {
    use super::super::health;
    use super::*;

    /// Every step asked of no server still answers, with the reason, so a
    /// client waiting on a menu or an applied action is never left hanging --
    /// and an executed step keeps the report of the edit already applied.
    #[test]
    fn every_unanswerable_step_still_answers() {
        let list = CodeActionCmd::List {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: "/w/a.rs".into(),
            range: Range::default(),
            kinds: Vec::new(),
            trigger: CodeActionTrigger::Invoked,
        };
        assert!(matches!(
            list.unanswered(UNAVAILABLE),
            CodeActionAnswer::Listed {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                actions: Err(ref reason),
            } if reason == UNAVAILABLE
        ));
        let resolve = CodeActionCmd::Resolve {
            request: RequestId(4),
            doc: DocumentId(2),
            version: 3,
            action: Box::default(),
        };
        assert!(matches!(
            resolve.unanswered("gone"),
            CodeActionAnswer::Resolved {
                request: RequestId(4),
                action: Err(ref reason),
                ..
            } if reason == "gone"
        ));
        let applied = AppliedWorkspaceEdit {
            edited: vec!["/w/a.rs".into()],
            ..AppliedWorkspaceEdit::default()
        };
        let execute = CodeActionCmd::Execute {
            request: RequestId(5),
            applied: Box::new(applied.clone()),
            action: Box::default(),
        };
        assert!(matches!(
            execute.unanswered("gone"),
            CodeActionAnswer::Executed {
                request: RequestId(5),
                applied: ref kept,
                result: Err(_),
            } if **kept == applied
        ));
    }

    fn list(trigger: CodeActionTrigger) -> CodeActionCmd {
        CodeActionCmd::List {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: "/w/a.rs".into(),
            range: Range::default(),
            kinds: vec!["quickfix".to_owned()],
            trigger,
        }
    }

    /// Only an automatic listing leaves the serial path; one the user asked
    /// for, and the resolve and execute steps, are served in line.
    #[test]
    fn only_an_automatic_listing_is_a_probe() {
        let probe = list(CodeActionTrigger::Automatic).into_probe();
        assert!(
            matches!(&probe, Ok(ask) if ask.tag.request == RequestId(1)
                && ask.tag.version == 3
                && ask.params.kinds == ["quickfix"]),
            "an automatic listing was served in line, or lost what it asked"
        );
        assert!(list(CodeActionTrigger::Invoked).into_probe().is_err());
        let resolve = CodeActionCmd::Resolve {
            request: RequestId(4),
            doc: DocumentId(2),
            version: 3,
            action: Box::default(),
        };
        assert!(resolve.into_probe().is_err());
    }

    fn key() -> SlotKey {
        SlotKey::new(crate::api::LanguageServerId::new("rust"), "/w")
    }

    /// Deliver one finished probe with `result`, as the task loop does.
    fn deliver_one(
        tally: &mut FailureTally,
        dead: &mut bool,
        tx: &mpsc::UnboundedSender<LspUpdate>,
        result: Result<Vec<CodeAction>, LspError>,
    ) {
        let tag = FlightTag {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
        };
        let answer = Answer { tag, result };
        deliver(answer, tally, dead, tx, &key(), SlotToken::FIRST, 0);
    }

    /// Charge one timed-out request the task awaited in line -- a hover, say.
    fn serial_timeout(
        tally: &mut FailureTally,
        dead: &mut bool,
        tx: &mpsc::UnboundedSender<LspUpdate>,
    ) {
        tally.note::<()>(Err(LspError::Timeout), dead, tx, &key(), SlotToken::FIRST);
    }

    /// Every listing answered so far: `Ok(count)` or the reason.
    fn listings(rx: &mut mpsc::UnboundedReceiver<LspUpdate>) -> Vec<Result<usize, String>> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|update| match update {
                LspUpdate::CodeAction {
                    answer: CodeActionAnswer::Listed { actions, .. },
                    ..
                } => Some(actions.map(|actions| actions.len())),
                _ => None,
            })
            .collect()
    }

    fn died(rx: &mut mpsc::UnboundedReceiver<LspUpdate>) -> bool {
        std::iter::from_fn(|| rx.try_recv().ok())
            .any(|update| matches!(update, LspUpdate::ServerDied { .. }))
    }

    /// Probes are sent on every caret settle and run beside the requests the
    /// hang streak measures, so their timeouts never count toward it -- and
    /// each is still answered.
    ///
    /// Falsified by: settling a probe through `StepContext::settle`, which
    /// charges its timeouts and condemns a slow server.
    #[test]
    fn probe_timeouts_never_mark_a_server_hung() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        let many = health::TIMEOUT_DEATH_LIMIT.saturating_mul(3);
        for _ in 0..many {
            deliver_one(&mut tally, &mut dead, &tx, Err(LspError::Timeout));
        }
        assert!(!dead, "probe timeouts condemned the connection");
        assert_eq!(
            listings(&mut rx).len(),
            usize::try_from(many).unwrap_or_default(),
            "a timed-out probe went unanswered"
        );
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        assert!(!dead, "probe timeouts were counted toward the streak");
        serial_timeout(&mut tally, &mut dead, &tx);
        assert!(dead, "hover and completion timeouts must still condemn it");
        assert!(died(&mut rx));
    }

    /// A server still answering probes but no longer hover and completion is
    /// hung on what the user waits for: an answered probe must not clear the
    /// streak. It does prove the server is up, so it arms the liveness gate.
    #[test]
    fn an_answered_probe_arms_the_gate_without_clearing_the_streak() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        deliver_one(&mut tally, &mut dead, &tx, Ok(vec![CodeAction::default()]));
        assert_eq!(listings(&mut rx), [Ok(1)]);
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        deliver_one(&mut tally, &mut dead, &tx, Ok(Vec::new()));
        assert!(!dead);
        serial_timeout(&mut tally, &mut dead, &tx);
        assert!(
            dead,
            "an answered probe reset the streak, or never armed the gate"
        );
        assert!(died(&mut rx));
    }

    /// A probe on a closed connection is still a death, answered with why.
    #[test]
    fn a_probe_on_a_closed_connection_is_still_a_death() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        deliver_one(&mut tally, &mut dead, &tx, Err(LspError::Closed));
        assert!(dead);
        let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            updates
                .iter()
                .any(|update| matches!(update, LspUpdate::ServerDied { .. }))
        );
        assert!(updates.iter().any(|update| matches!(
            update,
            LspUpdate::CodeAction {
                answer: CodeActionAnswer::Listed {
                    actions: Err(_),
                    ..
                },
                ..
            }
        )));
    }
}

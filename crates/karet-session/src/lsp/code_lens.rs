//! The server-task half of code lenses: listing a document's lenses and
//! resolving the visible ones, and running a lens's command.
//!
//! A listing is background work the editor asks for on its own as text
//! changes and the viewport moves, so it is kept off the serial path exactly
//! as `hint_flight` keeps inlay hints off it ([`Lenses`]): deferred until the
//! server has its text rather than flushing the debounced edit, at most one
//! running per document, only the newest waiting one launched, and its
//! outcome reported by [`deliver`] without charging the hang streak. The
//! deferred lenses on the requested lines are resolved inside the same
//! background request, concurrently, so a listing is answered once, whole.
//!
//! Running a lens is something the user clicked and waits on, so it is served
//! in line like a code action's command ([`run`]).

use std::path::PathBuf;
use std::sync::Arc;

use karet_core::CodeLens;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::code_actions::StepContext;
use super::code_actions::UNAVAILABLE;
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
use crate::api::DocumentId;
use crate::api::RequestId;

/// One code-lens step for a server task.
#[derive(Debug)]
pub(crate) enum CodeLensCmd {
    /// List `path`'s lenses, resolving the deferred ones on `lines`
    /// (inclusive).
    List {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        lines: (u32, u32),
    },
    /// Run the lens's command.
    Run {
        request: RequestId,
        lens: Box<CodeLens>,
    },
}

/// A [`CodeLensCmd::Run`], taken apart: the request and the lens to run.
pub(super) type LensRun = (RequestId, Box<CodeLens>);

/// What a server task made of a [`CodeLensCmd`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CodeLensAnswer {
    /// The document's lenses, ranges in UTF-16 columns; `None` when the
    /// request went unanswered.
    Listed {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        lenses: Option<Vec<CodeLens>>,
    },
    /// The lens's command ran, or why it did not; `title` labels the answer.
    Ran {
        request: RequestId,
        title: String,
        result: Result<(), String>,
    },
}

impl CodeLensCmd {
    /// This step as a background listing, when it is one; a run comes back
    /// as its request and lens.
    pub(super) fn into_listing(self) -> Result<Ask<(u32, u32)>, LensRun> {
        match self {
            Self::List {
                request,
                doc,
                version,
                path,
                lines,
            } => Ok(Ask {
                tag: FlightTag {
                    request,
                    doc,
                    version,
                },
                path,
                params: lines,
            }),
            Self::Run { request, lens } => Err((request, lens)),
        }
    }

    /// The answer when no server will ever take this step: a listing has no
    /// lenses, which clears what is shown, and a run fails saying `reason`.
    pub(crate) fn unserved(self, reason: &str) -> CodeLensAnswer {
        match self.unanswered(reason) {
            CodeLensAnswer::Listed {
                request,
                doc,
                version,
                ..
            } => CodeLensAnswer::Listed {
                request,
                doc,
                version,
                lenses: Some(Vec::new()),
            },
            ran @ CodeLensAnswer::Ran { .. } => ran,
        }
    }

    /// The answer when no server can take this step now, saying `reason`: a
    /// listing went unanswered, which leaves what is shown standing.
    pub(crate) fn unanswered(self, reason: &str) -> CodeLensAnswer {
        match self {
            Self::List {
                request,
                doc,
                version,
                ..
            } => CodeLensAnswer::Listed {
                request,
                doc,
                version,
                lenses: None,
            },
            Self::Run { request, lens } => CodeLensAnswer::Ran {
                request,
                title: lens.title.unwrap_or_default(),
                result: Err(reason.to_owned()),
            },
        }
    }
}

impl CodeLensAnswer {
    /// This answer as the generation fence passes it on: a listing begun
    /// under settings that are gone is reported unanswered, so its asker
    /// stops waiting and asks again, while a run's outcome stands -- the
    /// command did run.
    pub(crate) fn fenced(self) -> Self {
        match self {
            Self::Listed {
                request,
                doc,
                version,
                ..
            } => Self::Listed {
                request,
                doc,
                version,
                lenses: None,
            },
            ran @ Self::Ran { .. } => ran,
        }
    }
}

/// Code-lens listings, as a [`Background`] kind.
pub(super) enum Lenses {}

impl Background for Lenses {
    type Output = Vec<CodeLens>;
    type Params = (u32, u32);

    const WHAT: &'static str = "code-lens request";

    async fn request(
        client: Arc<LspClient>,
        path: PathBuf,
        lines: (u32, u32),
    ) -> Result<Vec<CodeLens>, LspError> {
        let mut lenses = client.code_lenses(&path).await?;
        let deferred: Vec<(usize, CodeLens)> = lenses
            .iter()
            .enumerate()
            .filter(|(_, lens)| !lens.is_resolved() && (lines.0..=lines.1).contains(&lens.line()))
            .map(|(index, lens)| (index, lens.clone()))
            .collect();
        let mut resolves = JoinSet::new();
        for (index, lens) in deferred {
            // At most `MAX_CONCURRENT_RESOLVES` in flight: a resolve is often
            // a reference search, and the asked lines can span far more than
            // a screen (one document in two far-apart panes asks for
            // everything between them).
            if resolves.len() >= MAX_CONCURRENT_RESOLVES {
                adopt_resolve(&mut lenses, resolves.join_next().await);
            }
            let client = Arc::clone(&client);
            resolves.spawn(async move { (index, client.resolve_code_lens(&lens).await) });
        }
        while let Some(joined) = resolves.join_next().await {
            adopt_resolve(&mut lenses, Some(joined));
        }
        Ok(lenses)
    }

    fn unanswered(updates: &mpsc::UnboundedSender<LspUpdate>, tag: FlightTag, generation: u64) {
        answer_listed(updates, tag, None, generation);
    }
}

/// The most `codeLens/resolve` requests one listing keeps in flight at once.
pub(super) const MAX_CONCURRENT_RESOLVES: usize = 8;

/// One finished resolve: the lens's index in the listing, and its outcome.
type Resolved = Result<(usize, Result<CodeLens, LspError>), tokio::task::JoinError>;

/// Put one finished resolve in its lens's place. A lens whose resolve failed
/// stays unresolved, so it is not shown; the next listing asks again.
fn adopt_resolve(lenses: &mut [CodeLens], joined: Option<Resolved>) {
    if let Some(Ok((index, Ok(resolved)))) = joined
        && let Some(lens) = lenses.get_mut(index)
    {
        *lens = resolved;
    }
}

/// The code-lens listings of one server task.
pub(super) type LensFlight = Flight<Lenses>;

/// Report one finished listing: note what it says about the connection, then
/// answer it.
///
/// Charged as `hint_flight::deliver` charges an inlay-hint request, for the
/// same reasons: an answer arms the liveness gate without resetting the
/// streak, a timeout or a cancellation is not counted toward it, and only a
/// closed connection is a death. A server without lenses answers an empty
/// set, which clears what is shown; every other failure leaves it standing.
pub(super) fn deliver(
    answer: Answer<Vec<CodeLens>>,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    (key, token): (&SlotKey, SlotToken),
    generation: u64,
) {
    let lenses = match answer.result {
        Ok(lenses) => {
            tally.note_background_answer();
            Some(lenses)
        },
        Err(LspError::Unsupported { .. }) => Some(Vec::new()),
        Err(LspError::Closed) => {
            tally.note::<()>(Err(LspError::Closed), dead, updates, key, token);
            None
        },
        Err(error) if matches!(error, LspError::Timeout) || is_cancellation(&error) => {
            tracing::debug!(language = %key, %error, "code-lens request unanswered; not charged");
            None
        },
        Err(error) => {
            tracing::warn!(language = %key, %error, "code-lens request failed; not charged");
            None
        },
    };
    answer_listed(updates, answer.tag, lenses, generation);
}

/// Run a lens's command on a live connection: a step the user is waiting on,
/// so its failures are charged like any other. A connection the flush before
/// it found dead answers why instead.
pub(super) async fn run(
    client: &LspClient,
    request: RequestId,
    lens: Box<CodeLens>,
    context: &mut StepContext<'_>,
) -> CodeLensAnswer {
    if *context.dead {
        return CodeLensCmd::Run { request, lens }.unanswered(UNAVAILABLE);
    }
    let result = client.run_code_lens(&lens).await;
    CodeLensAnswer::Ran {
        request,
        title: lens.title.unwrap_or_default(),
        result: context.settle(result),
    }
}

/// Answer the listing `tag` echoes with `lenses`.
fn answer_listed(
    updates: &mpsc::UnboundedSender<LspUpdate>,
    tag: FlightTag,
    lenses: Option<Vec<CodeLens>>,
    generation: u64,
) {
    let answer = CodeLensAnswer::Listed {
        request: tag.request,
        doc: tag.doc,
        version: tag.version,
        lenses,
    };
    let _ = updates.send(LspUpdate::CodeLens { generation, answer });
}

#[cfg(test)]
mod tests {
    use super::super::health;
    use super::*;

    fn key() -> SlotKey {
        SlotKey::new(crate::api::LanguageServerId::new("rust"), "/w")
    }

    fn tag() -> FlightTag {
        FlightTag {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
        }
    }

    /// Deliver one finished listing with `result`, as the task loop does, and
    /// return what it answered.
    fn deliver_one(
        tally: &mut FailureTally,
        dead: &mut bool,
        result: Result<Vec<CodeLens>, LspError>,
    ) -> Vec<LspUpdate> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let answer = Answer { tag: tag(), result };
        deliver(answer, tally, dead, &tx, (&key(), SlotToken::FIRST), 0);
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn listed(updates: &[LspUpdate]) -> Option<Option<usize>> {
        updates.iter().find_map(|update| match update {
            LspUpdate::CodeLens {
                answer: CodeLensAnswer::Listed { lenses, .. },
                ..
            } => Some(lenses.as_ref().map(Vec::len)),
            _ => None,
        })
    }

    /// Every outcome is answered: a set, an empty set for a server without
    /// lenses, and "unanswered" for anything else, so the editor keeps what
    /// it shows rather than clearing it on a cancelled request.
    #[test]
    fn every_listing_is_answered_and_only_a_refusal_clears() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let ok = deliver_one(&mut tally, &mut dead, Ok(vec![CodeLens::default()]));
        assert_eq!(listed(&ok), Some(Some(1)));
        let refused = LspError::Unsupported {
            method: "textDocument/codeLens",
        };
        assert_eq!(
            listed(&deliver_one(&mut tally, &mut dead, Err(refused))),
            Some(Some(0))
        );
        let cancelled = LspError::Server("failed with code -32801: modified".into());
        for error in [LspError::Timeout, cancelled] {
            assert_eq!(
                listed(&deliver_one(&mut tally, &mut dead, Err(error))),
                Some(None)
            );
        }
        assert!(!dead);
    }

    /// Listings are sent as text changes and run beside the requests the hang
    /// streak measures, so their timeouts never condemn a server; a closed
    /// connection still does.
    ///
    /// Falsified by: settling a listing through `StepContext::settle`, which
    /// charges its timeouts.
    #[test]
    fn listing_timeouts_never_mark_a_server_hung_but_a_close_is_a_death() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_mul(3) {
            deliver_one(&mut tally, &mut dead, Err(LspError::Timeout));
        }
        assert!(!dead, "listing timeouts condemned the connection");
        let closed = deliver_one(&mut tally, &mut dead, Err(LspError::Closed));
        assert!(dead);
        assert!(
            closed
                .iter()
                .any(|update| matches!(update, LspUpdate::ServerDied { .. }))
        );
        assert_eq!(listed(&closed), Some(None));
    }

    /// Only a listing leaves the serial path, and each step asked of no
    /// server still answers: a run with why, labelled with the lens's title.
    #[test]
    fn only_a_listing_is_background_and_every_step_answers() {
        let list = || CodeLensCmd::List {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: "/w/a.rs".into(),
            lines: (4, 9),
        };
        assert!(matches!(list().into_listing(), Ok(ask) if ask.params == (4, 9)));
        assert_eq!(
            list().unanswered("gone"),
            CodeLensAnswer::Listed {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                lenses: None,
            }
        );
        let run = || CodeLensCmd::Run {
            request: RequestId(5),
            lens: Box::new(CodeLens {
                title: Some("Run test".into()),
                ..CodeLens::default()
            }),
        };
        assert!(run().into_listing().is_err());
        assert_eq!(
            run().unanswered("gone"),
            CodeLensAnswer::Ran {
                request: RequestId(5),
                title: "Run test".into(),
                result: Err("gone".into()),
            }
        );
    }
}

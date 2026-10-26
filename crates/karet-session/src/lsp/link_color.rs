//! The server-task half of document links and colours: listing both for a
//! document, and resolving the target of a link a client follows.
//!
//! A listing is background work the editor asks for on its own as text
//! changes, so it is kept off the serial path exactly as `code_lens` keeps
//! lens listings off it ([`LinksColors`]): deferred until the server has its
//! text rather than flushing the debounced edit, at most one running per
//! document, only the newest waiting one launched, and its outcome reported
//! by [`deliver`] without charging the hang streak. Links and colours are
//! asked together, concurrently, and answered once: both describe the whole
//! document at one version, so one schedule serves both, and each half of
//! the answer stands or fails on its own.
//!
//! Resolving a link is something the user clicked and waits on, so it is
//! served in line like a lens's command ([`resolve`]).

use std::path::PathBuf;
use std::sync::Arc;

use karet_core::DocumentColor;
use karet_core::DocumentLink;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::code_actions::Health;
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

/// What a resolve that left a link without a target answers.
pub(crate) const NO_TARGET: &str = "the language server gave the link no target";

/// One document-link or document-colour step for a server task.
#[derive(Debug)]
pub(crate) enum LinkColorCmd {
    /// List `path`'s links and colours.
    List {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
    },
    /// Fill in the target of a link listed for `doc`.
    Resolve {
        request: RequestId,
        doc: DocumentId,
        link: Box<DocumentLink>,
    },
}

/// A [`LinkColorCmd::Resolve`], taken apart: the request, the document and
/// the link to resolve.
pub(super) type LinkResolve = (RequestId, DocumentId, Box<DocumentLink>);

/// What a server task made of a [`LinkColorCmd`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LinkColorAnswer {
    /// The document's links and colours, ranges in UTF-16 columns; either
    /// half is `None` when that request went unanswered.
    Listed {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        links: Option<Vec<DocumentLink>>,
        colors: Option<Vec<DocumentColor>>,
    },
    /// The link with its target filled in, its range as it was asked, or why
    /// there is none.
    Resolved {
        request: RequestId,
        doc: DocumentId,
        result: Result<DocumentLink, String>,
    },
}

/// One listing's two answers, each standing on its own.
#[derive(Debug)]
pub(super) struct Listing {
    /// The `textDocument/documentLink` answer.
    links: Result<Vec<DocumentLink>, LspError>,
    /// The `textDocument/documentColor` answer.
    colors: Result<Vec<DocumentColor>, LspError>,
}

impl LinkColorCmd {
    /// This step as a background listing, when it is one; a resolve comes
    /// back taken apart.
    pub(super) fn into_listing(self) -> Result<Ask<()>, LinkResolve> {
        match self {
            Self::List {
                request,
                doc,
                version,
                path,
            } => Ok(Ask {
                tag: FlightTag {
                    request,
                    doc,
                    version,
                },
                path,
                params: (),
            }),
            Self::Resolve { request, doc, link } => Err((request, doc, link)),
        }
    }

    /// The answer when no server will ever take this step: a listing has no
    /// links and no colours, which clears what is shown, and a resolve fails
    /// saying `reason`.
    pub(crate) fn unserved(self, reason: &str) -> LinkColorAnswer {
        match self.unanswered(reason) {
            LinkColorAnswer::Listed {
                request,
                doc,
                version,
                ..
            } => LinkColorAnswer::Listed {
                request,
                doc,
                version,
                links: Some(Vec::new()),
                colors: Some(Vec::new()),
            },
            resolved @ LinkColorAnswer::Resolved { .. } => resolved,
        }
    }

    /// The answer when no server can take this step now, saying `reason`: a
    /// listing went unanswered, which leaves what is shown standing.
    pub(crate) fn unanswered(self, reason: &str) -> LinkColorAnswer {
        match self {
            Self::List {
                request,
                doc,
                version,
                ..
            } => LinkColorAnswer::Listed {
                request,
                doc,
                version,
                links: None,
                colors: None,
            },
            Self::Resolve { request, doc, .. } => LinkColorAnswer::Resolved {
                request,
                doc,
                result: Err(reason.to_owned()),
            },
        }
    }
}

impl LinkColorAnswer {
    /// This answer as the generation fence passes it on: a listing begun
    /// under settings that are gone is reported unanswered, so its asker
    /// stops waiting and asks again, while a resolve's outcome stands -- the
    /// server did answer it.
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
                links: None,
                colors: None,
            },
            resolved @ Self::Resolved { .. } => resolved,
        }
    }
}

/// Link-and-colour listings, as a [`Background`] kind.
pub(super) enum LinksColors {}

impl Background for LinksColors {
    type Output = Listing;
    type Params = ();

    const WHAT: &'static str = "document-link and colour request";

    async fn request(client: Arc<LspClient>, path: PathBuf, (): ()) -> Result<Listing, LspError> {
        let (links, colors) =
            tokio::join!(client.document_links(&path), client.document_colors(&path));
        Ok(Listing { links, colors })
    }

    fn unanswered(updates: &mpsc::UnboundedSender<LspUpdate>, tag: FlightTag, generation: u64) {
        answer_listed(updates, tag, (None, None), generation);
    }
}

/// The link-and-colour listings of one server task.
pub(super) type LinkColorFlight = Flight<LinksColors>;

/// Report one finished listing: note what each half says about the
/// connection, then answer it.
///
/// Charged as `code_lens::deliver` charges a lens listing, for the same
/// reasons: an answer arms the liveness gate without resetting the streak, a
/// timeout or a cancellation is not counted toward it, and only a closed
/// connection is a death -- noted once, whichever half saw it. A server
/// without links or colours answers an empty half, which clears what is
/// shown; every other failure leaves that half standing.
pub(super) fn deliver(
    answer: Answer<Listing>,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    (key, token): (&SlotKey, SlotToken),
    generation: u64,
) {
    let halves = match answer.result {
        Ok(Listing { links, colors }) => {
            let mut closed = false;
            let links = judge(links, tally, &mut closed, key, "document-link");
            let colors = judge(colors, tally, &mut closed, key, "document-colour");
            if closed {
                tally.note::<()>(Err(LspError::Closed), dead, updates, key, token);
            }
            (links, colors)
        },
        // The task ended without an answer: nothing was learned.
        Err(error) => {
            tracing::warn!(language = %key, %error, "document-link and colour request failed");
            (None, None)
        },
    };
    answer_listed(updates, answer.tag, halves, generation);
}

/// One half of a listing as its answer, noting a closed connection in
/// `closed` for the caller to charge once.
fn judge<T>(
    result: Result<Vec<T>, LspError>,
    tally: &mut FailureTally,
    closed: &mut bool,
    key: &SlotKey,
    what: &str,
) -> Option<Vec<T>> {
    match result {
        Ok(items) => {
            tally.note_background_answer();
            Some(items)
        },
        Err(LspError::Unsupported { .. }) => Some(Vec::new()),
        Err(LspError::Closed) => {
            *closed = true;
            None
        },
        Err(error) if matches!(error, LspError::Timeout) || is_cancellation(&error) => {
            tracing::debug!(language = %key, %error, "{what} request unanswered; not charged");
            None
        },
        Err(error) => {
            tracing::warn!(language = %key, %error, "{what} request failed; not charged");
            None
        },
    }
}

/// Resolve a link's target on a live connection and send the answer: a step
/// the user is waiting on, so its failures are charged like any other. A
/// connection already found dead answers why instead, and a link that comes
/// back still without a target answers [`NO_TARGET`].
pub(super) async fn resolve(
    client: &LspClient,
    (request, doc, link): LinkResolve,
    health: Health<'_>,
    generation: u64,
) {
    let updates = health.2;
    let answer = if *health.0 {
        LinkColorCmd::Resolve { request, doc, link }.unanswered(UNAVAILABLE)
    } else {
        let mut context = StepContext::of(health, generation);
        let result = context
            .settle(client.resolve_document_link(&link).await)
            .and_then(|link| {
                if link.is_resolved() {
                    Ok(link)
                } else {
                    Err(NO_TARGET.to_owned())
                }
            });
        LinkColorAnswer::Resolved {
            request,
            doc,
            result,
        }
    };
    let _ = updates.send(LspUpdate::LinkColor { generation, answer });
}

/// Answer the listing `tag` echoes with its two halves.
fn answer_listed(
    updates: &mpsc::UnboundedSender<LspUpdate>,
    tag: FlightTag,
    (links, colors): (Option<Vec<DocumentLink>>, Option<Vec<DocumentColor>>),
    generation: u64,
) {
    let answer = LinkColorAnswer::Listed {
        request: tag.request,
        doc: tag.doc,
        version: tag.version,
        links,
        colors,
    };
    let _ = updates.send(LspUpdate::LinkColor { generation, answer });
}

#[cfg(test)]
mod tests {
    use super::super::health;
    use super::*;

    fn key() -> SlotKey {
        SlotKey::new(crate::api::LanguageServerId::new("css"), "/w")
    }

    fn tag() -> FlightTag {
        FlightTag {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
        }
    }

    type Halves = (Option<usize>, Option<usize>);

    /// Deliver one finished listing with these halves, as the task loop
    /// does, and return what it answered.
    fn deliver_one(
        tally: &mut FailureTally,
        dead: &mut bool,
        links: Result<Vec<DocumentLink>, LspError>,
        colors: Result<Vec<DocumentColor>, LspError>,
    ) -> Vec<LspUpdate> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let answer = Answer {
            tag: tag(),
            result: Ok(Listing { links, colors }),
        };
        deliver(answer, tally, dead, &tx, (&key(), SlotToken::FIRST), 0);
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn listed(updates: &[LspUpdate]) -> Option<Halves> {
        updates.iter().find_map(|update| match update {
            LspUpdate::LinkColor {
                answer: LinkColorAnswer::Listed { links, colors, .. },
                ..
            } => Some((links.as_ref().map(Vec::len), colors.as_ref().map(Vec::len))),
            _ => None,
        })
    }

    /// Each half is judged on its own: a set, an empty set for a server that
    /// offers none, and "unanswered" for a timeout or cancellation, so the
    /// editor keeps what it shows for that half alone.
    #[test]
    fn each_half_stands_or_fails_on_its_own() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let link = DocumentLink::default();
        let both = deliver_one(
            &mut tally,
            &mut dead,
            Ok(vec![link.clone()]),
            Ok(vec![DocumentColor::default(); 2]),
        );
        assert_eq!(listed(&both), Some((Some(1), Some(2))));
        let refused = LspError::Unsupported {
            method: "textDocument/documentColor",
        };
        let no_colours = deliver_one(&mut tally, &mut dead, Ok(vec![link]), Err(refused));
        assert_eq!(listed(&no_colours), Some((Some(1), Some(0))));
        let cancelled = LspError::Server("failed with code -32801: modified".into());
        let half = deliver_one(&mut tally, &mut dead, Err(cancelled), Ok(Vec::new()));
        assert_eq!(listed(&half), Some((None, Some(0))));
        let failed = deliver_one(
            &mut tally,
            &mut dead,
            Err(LspError::Server("boom".into())),
            Err(LspError::Timeout),
        );
        assert_eq!(listed(&failed), Some((None, None)));
        assert!(!dead);
    }

    /// Listings run beside the requests the hang streak measures, so their
    /// timeouts never condemn a server; a closed connection still does, and
    /// is noted once though both halves saw it.
    ///
    /// Falsified by: settling a listing through `StepContext::settle`, which
    /// charges its timeouts.
    #[test]
    fn listing_timeouts_never_mark_a_server_hung_but_a_close_is_one_death() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_mul(3) {
            deliver_one(
                &mut tally,
                &mut dead,
                Err(LspError::Timeout),
                Err(LspError::Timeout),
            );
        }
        assert!(!dead, "listing timeouts condemned the connection");
        let closed = deliver_one(
            &mut tally,
            &mut dead,
            Err(LspError::Closed),
            Err(LspError::Closed),
        );
        assert!(dead);
        let deaths = closed
            .iter()
            .filter(|update| matches!(update, LspUpdate::ServerDied { .. }))
            .count();
        assert_eq!(deaths, 1);
        assert_eq!(listed(&closed), Some((None, None)));
    }

    /// A listing task that ended without answering learned nothing.
    #[test]
    fn a_listing_that_never_answered_is_unanswered() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let answer = Answer {
            tag: tag(),
            result: Err(LspError::Protocol("ended".into())),
        };
        let mut dead = false;
        deliver(
            answer,
            &mut FailureTally::default(),
            &mut dead,
            &tx,
            (&key(), SlotToken::FIRST),
            0,
        );
        let updates: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(listed(&updates), Some((None, None)));
    }

    /// Only a listing leaves the serial path; a listing asked of no server
    /// clears, one no server can answer now leaves what is shown, and a
    /// resolve asked of none fails saying why. The fence passes a resolve
    /// through and turns a listing unanswered.
    #[test]
    fn only_a_listing_is_background_and_every_step_answers() {
        let list = || LinkColorCmd::List {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: "/w/a.css".into(),
        };
        assert!(list().into_listing().is_ok());
        let cleared = list().unserved("gone");
        assert_eq!(
            cleared,
            LinkColorAnswer::Listed {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                links: Some(Vec::new()),
                colors: Some(Vec::new()),
            }
        );
        let unanswered = LinkColorAnswer::Listed {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            links: None,
            colors: None,
        };
        assert_eq!(list().unanswered("gone"), unanswered);
        assert_eq!(cleared.fenced(), unanswered);
        let resolve = || LinkColorCmd::Resolve {
            request: RequestId(5),
            doc: DocumentId(2),
            link: Box::default(),
        };
        assert!(resolve().into_listing().is_err());
        let failed = LinkColorAnswer::Resolved {
            request: RequestId(5),
            doc: DocumentId(2),
            result: Err("gone".into()),
        };
        assert_eq!(resolve().unserved("gone"), failed);
        let resolved = LinkColorAnswer::Resolved {
            request: RequestId(5),
            doc: DocumentId(2),
            result: Ok(DocumentLink::default()),
        };
        assert_eq!(
            resolved.fenced(),
            LinkColorAnswer::Resolved {
                request: RequestId(5),
                doc: DocumentId(2),
                result: Ok(DocumentLink::default()),
            }
        );
    }
}

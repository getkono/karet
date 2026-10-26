//! The server-task half of inline completions: asking a server for the ghost
//! text it proposes at the caret, and telling it which item was accepted.
//!
//! An ask is background work the editor issues on its own once typing
//! pauses, so it is kept off the serial path exactly as `hint_flight` keeps
//! inlay hints off it ([`Inline`]): deferred until the server has the text it
//! was asked against rather than flushing the debounced edit, at most one
//! running per document, and only the newest waiting one launched -- an
//! older one the user has typed past is dropped unasked. Its outcome is
//! charged as an inlay-hint request's is ([`deliver`]).
//!
//! Unlike every other request, an ask goes to *every* server attached to the
//! document, not only the language's primary: inline completions come from
//! a companion (Copilot) far more often than from the language server. A
//! server that does not offer them is refused inside `karet-lsp` without a
//! request, and stays quiet, as does one with nothing to propose -- so only
//! an answer with items reaches the session, tagged with the slot it came
//! from, which is where an accept is sent back to.

use std::path::PathBuf;
use std::sync::Arc;

use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;
use karet_core::LineCol;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::health::FailureTally;
use super::hint_flight::Answer;
use super::hint_flight::Ask;
use super::hint_flight::Background;
use super::hint_flight::Flight;
use super::hint_flight::FlightTag;
use super::message::LspUpdate;
use super::slot::SlotKey;
use super::slot::SlotToken;
use crate::api::DocumentId;
use crate::api::RequestId;

/// One inline-completion step for a server task.
#[derive(Debug)]
pub(crate) enum InlineCompletionCmd {
    /// Ask for items at `position` (UTF-16 columns).
    Ask {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
        trigger: InlineCompletionTrigger,
    },
    /// Tell the server the item was accepted (boxed: it dwarfs the ask).
    Accept { item: Box<InlineCompletionItem> },
}

/// An ask's position and trigger, the part of it the server is sent besides
/// its path.
pub(super) type InlineParams = (LineCol, InlineCompletionTrigger);

impl InlineCompletionCmd {
    /// This step as a background ask, when it is one; an accept comes back as
    /// its item.
    pub(super) fn into_ask(self) -> Result<Ask<InlineParams>, Box<InlineCompletionItem>> {
        match self {
            Self::Ask {
                request,
                doc,
                version,
                path,
                position,
                trigger,
            } => Ok(Ask {
                tag: FlightTag {
                    request,
                    doc,
                    version,
                },
                path,
                params: (position, trigger),
            }),
            Self::Accept { item } => Err(item),
        }
    }
}

/// Inline-completion asks, as a [`Background`] kind.
pub(super) enum Inline {}

impl Background for Inline {
    type Output = Vec<InlineCompletionItem>;
    type Params = InlineParams;

    const WHAT: &'static str = "inline-completion request";

    async fn request(
        client: Arc<LspClient>,
        path: PathBuf,
        (position, trigger): InlineParams,
    ) -> Result<Vec<InlineCompletionItem>, LspError> {
        client.inline_completion(&path, position, trigger).await
    }

    /// Nothing: an unanswered ask leaves the editor showing no ghost text,
    /// which is what it shows while asking.
    fn unanswered(_: &mpsc::UnboundedSender<LspUpdate>, _: FlightTag, _: u64) {}
}

/// The inline-completion asks of one server task.
pub(super) type InlineFlight = Flight<Inline>;

/// Report one finished ask: note what it says about the connection, then
/// answer it when it proposes anything.
///
/// Charged as `hint_flight::deliver` charges an inlay-hint request, for the
/// same reasons: an answer arms the liveness gate without resetting the hang
/// streak, and only a closed connection is a death. A failure is logged at
/// debug level rather than warned about: the editor asks at every pause in
/// typing, and a server that refuses every ask -- Copilot before sign-in --
/// says why through its own status notifications.
pub(super) fn deliver(
    answer: Answer<Vec<InlineCompletionItem>>,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    (key, token): (&SlotKey, SlotToken),
    generation: u64,
) {
    let items = match answer.result {
        Ok(items) => {
            tally.note_background_answer();
            items
        },
        Err(LspError::Unsupported { .. }) => return,
        Err(LspError::Closed) => {
            tally.note::<()>(Err(LspError::Closed), dead, updates, key, token);
            return;
        },
        Err(error) => {
            tracing::debug!(language = %key, %error, "inline-completion request unanswered; not charged");
            return;
        },
    };
    if items.is_empty() {
        return;
    }
    let FlightTag {
        request,
        doc,
        version,
    } = answer.tag;
    let _ = updates.send(LspUpdate::InlineCompletions {
        generation,
        server: key.clone(),
        request,
        doc,
        version,
        items,
    });
}

/// Tell `key`'s server `item` was accepted. Nobody waits on it, so a failure
/// is only logged, and charged to nothing: it is a courtesy to the server,
/// not something the user asked of it, and a connection it found closed is
/// noticed by the task's own watch on the connection.
pub(super) async fn accept(client: &LspClient, item: &InlineCompletionItem, key: &SlotKey) {
    if let Err(error) = client.accept_inline_completion(item).await {
        tracing::debug!(language = %key, %error, "inline-completion accept command failed");
    }
}

#[cfg(test)]
mod tests {
    use super::super::health;
    use super::*;

    fn key() -> SlotKey {
        SlotKey::new(
            crate::api::LanguageServerId::new("copilot-language-server"),
            "/w",
        )
    }

    fn tag() -> FlightTag {
        FlightTag {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
        }
    }

    /// Deliver one finished ask with `result`, as the task loop does, and
    /// return the items it answered with, if it answered.
    fn deliver_one(
        tally: &mut FailureTally,
        dead: &mut bool,
        result: Result<Vec<InlineCompletionItem>, LspError>,
    ) -> Option<(SlotKey, Vec<InlineCompletionItem>)> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let answer = Answer { tag: tag(), result };
        deliver(answer, tally, dead, &tx, (&key(), SlotToken::FIRST), 0);
        std::iter::from_fn(|| rx.try_recv().ok()).find_map(|update| match update {
            LspUpdate::InlineCompletions {
                server,
                items,
                request,
                version,
                ..
            } => {
                assert_eq!((request, version), (RequestId(1), 3), "the tag is echoed");
                Some((server, items))
            },
            _ => None,
        })
    }

    #[test]
    fn items_are_answered_with_the_slot_that_proposed_them() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let items = vec![InlineCompletionItem::new("x", LineCol::default())];
        let answered = deliver_one(&mut tally, &mut dead, Ok(items.clone()));
        assert_eq!(answered, Some((key(), items)));
        assert!(!dead);
    }

    /// Every server attached to the document is asked, so the ones with
    /// nothing to say must say nothing, or they would race the one that has.
    #[test]
    fn no_items_a_refusal_and_a_failure_all_stay_quiet() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        for result in [
            Ok(Vec::new()),
            Err(LspError::Unsupported {
                method: "textDocument/inlineCompletion",
            }),
            Err(LspError::Timeout),
            Err(LspError::Server("not signed in".to_owned())),
        ] {
            assert_eq!(deliver_one(&mut tally, &mut dead, result), None);
        }
        assert!(!dead, "none of these is a death");
    }

    /// Asks are sent at every pause in typing and run beside the requests
    /// the hang streak measures, so their timeouts never condemn a server; a
    /// closed connection still does.
    ///
    /// Falsified by: charging a failed ask through `FailureTally::note`.
    #[test]
    fn ask_timeouts_never_mark_a_server_hung_but_a_close_is_a_death() {
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_mul(3) {
            deliver_one(&mut tally, &mut dead, Err(LspError::Timeout));
        }
        assert!(!dead, "ask timeouts condemned the connection");
        assert_eq!(
            deliver_one(&mut tally, &mut dead, Err(LspError::Closed)),
            None
        );
        assert!(dead, "a closed connection must be noticed");
    }

    #[test]
    fn an_ask_keeps_its_tag_and_an_accept_its_item() {
        let ask = InlineCompletionCmd::Ask {
            request: RequestId(1),
            doc: DocumentId(2),
            version: 3,
            path: PathBuf::from("/w/a.rs"),
            position: LineCol::new(4, 5),
            trigger: InlineCompletionTrigger::Invoked,
        }
        .into_ask();
        let ask = ask.ok();
        assert_eq!(ask.as_ref().map(|ask| ask.tag.request), Some(RequestId(1)));
        assert_eq!(
            ask.map(|ask| ask.params),
            Some((LineCol::new(4, 5), InlineCompletionTrigger::Invoked))
        );
        let item = InlineCompletionItem::new("y", LineCol::default());
        let accept = InlineCompletionCmd::Accept {
            item: Box::new(item.clone()),
        }
        .into_ask();
        assert_eq!(accept.err().map(|item| *item), Some(item));
    }
}

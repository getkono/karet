//! What the task does with a command it cannot serve right now.
//!
//! Two halves of one contract. `answer_empty` keeps a caller from waiting on a
//! connection that is not there -- every request is answered, even when the answer
//! is nothing. `remember_document` maintains the task's authoritative copy of the
//! open-document set, which is what a reconnect replays; a document missing from
//! it is one the server never learns about again.
//!
//! `report_unsupported` sits beside them because it is the other half of
//! answering: when the answer is empty because the server does not offer the
//! feature, the user who asked is told so rather than left to read "nothing".

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use karet_core::CompletionItem;
use karet_core::CompletionList;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::ServerFeature;
use karet_core::SignatureHelpOptions;
use karet_core::WorkspaceEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::code_actions::UNAVAILABLE;
use super::delta::PendingChange;
use super::delta::lines_agree;
use super::editing;
use super::health::FailureTally;
use super::message::DocumentHighlightsAnswer;
use super::message::EditingAnswer;
use super::message::HierarchyAsk;
use super::message::LspUpdate;
use super::message::RenameAnswer;
use super::message::SelectionRangesAnswer;
use super::message::ServerCmd;
use super::slot::SlotKey;
use super::slot::SlotToken;
use crate::RequestId;
use crate::api::LocationKind;

/// Tell the client a request it asked for by hand was refused because the
/// server never offered `feature` -- ahead of the empty answer that follows.
///
/// Issue #279 asked for exactly this distinction: "this server does not
/// support X" read the same as "this server found nothing", and the first is
/// a fact the user can act on (use another provider) while the second is not.
/// Only for requests a user asks for by hand; the server task calls it for
/// nothing else.
pub(super) fn report_unsupported<T>(
    result: &Result<T, LspError>,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    generation: u64,
    request: RequestId,
    key: &SlotKey,
    feature: ServerFeature,
) {
    if matches!(result, Err(LspError::Unsupported { .. })) {
        let _ = updates.send(LspUpdate::Unsupported {
            generation,
            request,
            server: key.provider.clone(),
            feature,
        });
    }
}

/// Send the pending `didChange`, if any -- what a command that must see the
/// latest text does before it is served.
///
/// A send that fails leaves the server's copy behind the replay set's, which
/// has already recorded the change's version, so the document is marked
/// [`OpenDocument::resync`]: its next change goes out as the whole text rather
/// than as ranges against text the server does not hold.
pub(super) async fn flush_pending(
    client: &LspClient,
    (pending, documents): (
        &mut Option<PendingChange>,
        &mut HashMap<PathBuf, OpenDocument>,
    ),
    dead: &mut bool,
    tally: &mut FailureTally,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    key: &SlotKey,
    token: SlotToken,
) {
    if *dead {
        *pending = None;
        return;
    }
    if let Some(change) = pending.take() {
        let path = change.path.clone();
        let result = change.send(client).await;
        if result.is_err()
            && let Some(document) = documents.get_mut(&path)
        {
            document.resync = true;
        }
        tally.note(result, dead, updates, key, token);
    }
}

/// Resolve `hint` on `client`, or hand it back unchanged when the connection
/// is dead or the request fails -- which counts against the connection's
/// health like any other failure. No flush first: what is resolved is the hint
/// as the server sent it, whatever the text has done since.
pub(super) async fn resolve_hint(
    client: &LspClient,
    hint: InlayHint,
    (dead, tally, updates, key, token): (
        &mut bool,
        &mut FailureTally,
        &mpsc::UnboundedSender<LspUpdate>,
        &SlotKey,
        SlotToken,
    ),
) -> InlayHint {
    if *dead {
        return hint;
    }
    match tally.observe(client.resolve_inlay_hint(&hint).await) {
        Ok(resolved) => resolved,
        Err(error) => {
            tally.note::<()>(Err(error), dead, updates, key, token);
            hint
        },
    }
}

/// Ask `client` for completions at `position` in `path`, with the server's
/// trigger and commit characters beside them: an empty list when the
/// connection is dead or the request fails -- which counts against the
/// connection's health like any other failure. The caller flushes first, so
/// the server completes in the latest text.
pub(super) async fn complete(
    client: &LspClient,
    (path, position, trigger): (&Path, LineCol, CompletionTrigger),
    (dead, tally, updates, key, token): (
        &mut bool,
        &mut FailureTally,
        &mpsc::UnboundedSender<LspUpdate>,
        &SlotKey,
        SlotToken,
    ),
) -> (CompletionList, CompletionOptions) {
    let options = client.capabilities().completion;
    if *dead {
        return (CompletionList::default(), options);
    }
    match tally.observe(client.completion(path, position, trigger).await) {
        Ok(list) => (list, options),
        Err(error) => {
            tally.note::<()>(Err(error), dead, updates, key, token);
            (CompletionList::default(), options)
        },
    }
}

/// Resolve `item` on `client`: `None` when there is nothing to resolve, the
/// connection is dead, or the request fails -- which counts against the
/// connection's health like any other failure. No flush first: what is
/// resolved is the item as the server sent it.
pub(super) async fn resolve_item(
    client: &LspClient,
    item: &CompletionItem,
    (dead, tally, updates, key, token): (
        &mut bool,
        &mut FailureTally,
        &mpsc::UnboundedSender<LspUpdate>,
        &SlotKey,
        SlotToken,
    ),
) -> Option<CompletionItem> {
    if *dead {
        return None;
    }
    match tally.observe(client.resolve_completion(item).await) {
        Ok(resolved) => resolved,
        Err(error) => {
            tally.note::<()>(Err(error), dead, updates, key, token);
            None
        },
    }
}

/// Serve a request about the carets -- a [`ServerCmd::Hover`] or a
/// [`ServerCmd::SignatureHelp`], read in a popup, or the
/// [`ServerCmd::SelectionRanges`] Expand Selection grows through, or the
/// [`ServerCmd::DocumentHighlights`] the occurrence highlight paints, or the
/// [`ServerCmd::Editing`] an edit makes -- and
/// answer it: with nothing when the connection is dead or the request fails,
/// which counts against the connection's health like any other failure.
/// Flushes first, so the server answers about the latest text.
///
/// A refusal is reported for a hover, which is always asked by hand, and for
/// signature help only when it was: typing `(` in a file whose server has no
/// signature help must not raise a notice every time. Selection ranges are
/// refused silently: the client grows along its own syntax tree instead.
/// Document highlights are too, answered `None`: they are asked for whenever
/// the caret rests, and the client falls back to matching the word as text.
/// Linked editing is reported, being asked for by hand; on-type formatting
/// is not, being offered after every typed character. A hierarchy request is
/// reported too: every one is asked for by hand, opening or expanding a tree.
pub(super) async fn caret_query(
    client: &LspClient,
    cmd: ServerCmd,
    (generation, pending, documents): (
        u64,
        &mut Option<PendingChange>,
        &mut HashMap<PathBuf, OpenDocument>,
    ),
    (dead, tally, updates, key, token): (
        &mut bool,
        &mut FailureTally,
        &mpsc::UnboundedSender<LspUpdate>,
        &SlotKey,
        SlotToken,
    ),
) {
    flush_pending(
        client,
        (pending, documents),
        dead,
        tally,
        updates,
        key,
        token,
    )
    .await;
    if *dead {
        // Nothing to ask: answer exactly as when there is no connection.
        return answer_empty(updates, cmd, generation);
    }
    let health = (dead, tally, updates, key, token);
    match cmd {
        ServerCmd::Hover {
            request,
            doc,
            version,
            path,
            position,
        } => {
            let result = client.hover(&path, position).await;
            let hover = settle(
                result,
                Some((request, ServerFeature::Hover)),
                generation,
                health,
            );
            let _ = updates.send(LspUpdate::Hover {
                generation,
                request,
                doc,
                version,
                hover,
            });
        },
        ServerCmd::SignatureHelp {
            request,
            doc,
            path,
            position,
            manual,
        } => {
            let result = client.signature_help(&path, position).await;
            let refused = manual.then_some((request, ServerFeature::SignatureHelp));
            let help = settle(result, refused, generation, health);
            let _ = updates.send(LspUpdate::SignatureHelp {
                generation,
                request,
                doc,
                help,
                options: client.capabilities().signature_help,
            });
        },
        ServerCmd::SelectionRanges {
            request,
            doc,
            version,
            path,
            positions,
        } => {
            let result = client.selection_ranges(&path, &positions).await;
            let chains = settle(result.map(Some), None, generation, health)
                .unwrap_or_else(|| vec![Vec::new(); positions.len()]);
            let answer = SelectionRangesAnswer {
                request,
                doc,
                version,
                chains,
            };
            let _ = updates.send(LspUpdate::SelectionRanges { generation, answer });
        },
        ServerCmd::DocumentHighlights {
            request,
            doc,
            version,
            path,
            position,
        } => {
            let result = client.document_highlights(&path, position).await;
            let highlights = settle(result.map(Some), None, generation, health);
            let answer = DocumentHighlightsAnswer {
                request,
                doc,
                version,
                highlights,
            };
            let _ = updates.send(LspUpdate::DocumentHighlights { generation, answer });
        },
        ServerCmd::Editing {
            request,
            doc,
            version,
            path,
            ask,
        } => {
            let result = editing::ask(client, &path, &ask).await;
            let refused = editing::reported(&ask).map(|feature| (request, feature));
            let reply = settle(result.map(Some), refused, generation, health)
                .unwrap_or_else(|| editing::unanswered(&ask));
            let answer = EditingAnswer {
                request,
                doc,
                version,
                reply,
            };
            let _ = updates.send(LspUpdate::Editing { generation, answer });
        },
        ServerCmd::Hierarchy {
            request,
            direction,
            ask,
        } => {
            let result = match ask {
                HierarchyAsk::Prepare { path, position } => {
                    client.prepare_hierarchy(direction, &path, position).await
                },
                HierarchyAsk::Children(item) => client.hierarchy_children(direction, &item).await,
            };
            let refused = Some((request, direction.feature()));
            let items = settle(result.map(Some), refused, generation, health).unwrap_or_default();
            let _ = updates.send(LspUpdate::Hierarchy {
                generation,
                request,
                direction,
                items,
            });
        },
        cmd => answer_empty(updates, cmd, generation),
    }
}

/// Settle a [`caret_query`] answer: report a refusal the user asked to hear
/// about (`refused`), then keep the answer, or charge the failure and answer
/// nothing.
fn settle<T>(
    result: Result<Option<T>, LspError>,
    refused: Option<(RequestId, ServerFeature)>,
    generation: u64,
    (dead, tally, updates, key, token): (
        &mut bool,
        &mut FailureTally,
        &mpsc::UnboundedSender<LspUpdate>,
        &SlotKey,
        SlotToken,
    ),
) -> Option<T> {
    if let Some((request, feature)) = refused {
        report_unsupported(&result, updates, generation, request, key, feature);
    }
    tally.observe(result).unwrap_or_else(|error| {
        tally.note::<()>(Err(error), dead, updates, key, token);
        None
    })
}

/// The capability a navigation request is gated on: Go to Definition's own
/// for `None`, else the one its kind names.
pub(super) fn location_feature(kind: Option<LocationKind>) -> ServerFeature {
    kind.map_or(ServerFeature::Definition, LocationKind::feature)
}

/// Issue the navigation request `kind` names on `client`.
///
/// References include the declaration, as an editor's "find all references"
/// conventionally does: the user asked where the symbol is *used*, and the
/// place it is introduced is the first answer to that.
pub(super) async fn locations(
    client: &LspClient,
    kind: Option<LocationKind>,
    path: &Path,
    position: LineCol,
) -> Result<Vec<Location>, LspError> {
    match kind {
        None => client.definition(path, position).await,
        Some(LocationKind::Declaration) => client.declaration(path, position).await,
        Some(LocationKind::TypeDefinition) => client.type_definition(path, position).await,
        Some(LocationKind::Implementation) => client.implementations(path, position).await,
        Some(LocationKind::References) => client.references(path, position, true).await,
    }
}

/// Answer a request command that arrived while the connection is down but a
/// reconnect is coming.
///
/// As [`answer_empty`], except an inlay-hint request is reported unanswered
/// rather than answered with none: the hints the editor shows are still the
/// best it has until the server is back, and an empty set would blank them for
/// the whole reconnect back-off. Only for a *transient* outage -- a server that
/// is gone for good really has no hints to offer, and being re-asked forever
/// would be the only effect of saying otherwise.
pub(super) fn answer_reconnecting(
    updates: &mpsc::UnboundedSender<LspUpdate>,
    cmd: ServerCmd,
    generation: u64,
) {
    match cmd {
        ServerCmd::InlayHints {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::InlayHintsFailed {
                generation,
                request,
                doc,
                version,
            });
        },
        // Nor has it said anything about a document's lenses.
        ServerCmd::CodeLens(cmd) => {
            let answer = cmd.unanswered(UNAVAILABLE);
            let _ = updates.send(LspUpdate::CodeLens { generation, answer });
        },
        // Nor its links and colours.
        ServerCmd::LinkColor(cmd) => {
            let answer = cmd.unanswered(UNAVAILABLE);
            let _ = updates.send(LspUpdate::LinkColor { generation, answer });
        },
        cmd => answer_empty(updates, cmd, generation),
    }
}

/// Answer a request command with an empty set (used whenever no live server can
/// answer, so the client is never left waiting).
pub(super) fn answer_empty(
    updates: &mpsc::UnboundedSender<LspUpdate>,
    cmd: ServerCmd,
    generation: u64,
) {
    match cmd {
        ServerCmd::Completion {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Completions {
                generation,
                request,
                doc,
                version,
                list: CompletionList::default(),
                options: CompletionOptions::default(),
            });
        },
        ServerCmd::ResolveCompletion { request, doc, .. } => {
            let _ = updates.send(LspUpdate::CompletionResolved {
                generation,
                request,
                doc,
                resolved: None,
            });
        },
        ServerCmd::InlayHints {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::InlayHints {
                generation,
                request,
                doc,
                version,
                hints: Vec::new(),
            });
        },
        ServerCmd::ResolveInlayHint { request, doc, hint } => {
            let _ = updates.send(LspUpdate::InlayHintResolved {
                generation,
                request,
                doc,
                hint,
            });
        },
        ServerCmd::DocumentSymbols {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Symbols {
                generation,
                request,
                doc,
                version,
                symbols: Vec::new(),
            });
        },
        ServerCmd::Hover {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::Hover {
                generation,
                request,
                doc,
                version,
                hover: None,
            });
        },
        ServerCmd::SignatureHelp { request, doc, .. } => {
            let _ = updates.send(LspUpdate::SignatureHelp {
                generation,
                request,
                doc,
                help: None,
                options: SignatureHelpOptions::default(),
            });
        },
        ServerCmd::SelectionRanges {
            request,
            doc,
            version,
            positions,
            ..
        } => {
            let answer = SelectionRangesAnswer {
                request,
                doc,
                version,
                chains: vec![Vec::new(); positions.len()],
            };
            let _ = updates.send(LspUpdate::SelectionRanges { generation, answer });
        },
        ServerCmd::DocumentHighlights {
            request,
            doc,
            version,
            ..
        } => {
            let answer = DocumentHighlightsAnswer {
                request,
                doc,
                version,
                highlights: None,
            };
            let _ = updates.send(LspUpdate::DocumentHighlights { generation, answer });
        },
        ServerCmd::Editing {
            request,
            doc,
            version,
            ask,
            ..
        } => {
            let answer = EditingAnswer {
                request,
                doc,
                version,
                reply: editing::unanswered(&ask),
            };
            let _ = updates.send(LspUpdate::Editing { generation, answer });
        },
        ServerCmd::Hierarchy {
            request, direction, ..
        } => {
            let _ = updates.send(LspUpdate::Hierarchy {
                generation,
                request,
                direction,
                items: Vec::new(),
            });
        },
        ServerCmd::Definition {
            request,
            doc,
            version,
            kind,
            ..
        } => {
            let _ = updates.send(LspUpdate::Definitions {
                generation,
                request,
                doc,
                version,
                kind,
                locations: Vec::new(),
            });
        },
        ServerCmd::WorkspaceSymbols { request, .. } => {
            let _ = updates.send(LspUpdate::WorkspaceSymbols {
                generation,
                request,
                symbols: Vec::new(),
            });
        },
        ServerCmd::Rename {
            request,
            doc,
            version,
            ..
        } => {
            let _ = updates.send(LspUpdate::WorkspaceEdit {
                generation,
                request,
                doc,
                version,
                edit: Ok(WorkspaceEdit::default()),
            });
        },
        ServerCmd::PrepareRename {
            request,
            doc,
            version,
            caret,
            ..
        } => {
            let _ = updates.send(LspUpdate::PrepareRename {
                generation,
                request,
                doc,
                version,
                caret,
                answer: RenameAnswer::NoServer,
            });
        },
        ServerCmd::Formatting {
            request,
            doc,
            version,
            ask,
            ..
        } => {
            let _ = updates.send(LspUpdate::Formatting {
                generation,
                request,
                doc,
                version,
                // There is no connection, so nothing formatted this file. Whether
                // the server would have been able to is a different question, and
                // not one the session needs answered: what it needs to know is
                // whether to reach for its own formatter, and it should.
                formatted: false,
                edits: Vec::new(),
                scope: ask.scope,
            });
        },
        ServerCmd::CodeAction(cmd) => {
            let _ = updates.send(LspUpdate::CodeAction {
                generation,
                answer: cmd.unanswered(UNAVAILABLE),
            });
        },
        // A server gone for good has no lenses: the editor may clear them.
        ServerCmd::CodeLens(cmd) => {
            let answer = cmd.unserved(UNAVAILABLE);
            let _ = updates.send(LspUpdate::CodeLens { generation, answer });
        },
        // Nor links or colours.
        ServerCmd::LinkColor(cmd) => {
            let answer = cmd.unserved(UNAVAILABLE);
            let _ = updates.send(LspUpdate::LinkColor { generation, answer });
        },
        // An inline-completion ask with no server proposes nothing, which is
        // what silence says; only a sign-in, which the user waits on, fails.
        ServerCmd::InlineCompletion(cmd) => cmd.unanswered(updates, generation),
        // A notice is not a request: nobody waits on it, and one that finds no
        // connection means nothing to the next (see `window::Notice`).
        ServerCmd::DidOpen { .. }
        | ServerCmd::DidChange { .. }
        | ServerCmd::DidClose { .. }
        | ServerCmd::DidSave { .. }
        | ServerCmd::Notify(_) => {},
    }
}

#[derive(Clone)]
pub(super) struct OpenDocument {
    pub(super) language: String,
    pub(super) version: i32,
    pub(super) text: String,
    /// Whether the rope numbers `text`'s lines as LSP does (see `delta`).
    pub(super) lines_agree: bool,
    /// Whether the server's copy may lag `text` because a `didChange` to it
    /// failed to send (see [`flush_pending`]): the next change must carry the
    /// whole text.
    pub(super) resync: bool,
}

/// Keep the replay set in step with `cmd`, and drop a change's delta that
/// cannot take the server's copy to the new text: one not starting from the
/// version last seen here (a change between was never queued), one following
/// a change that failed to send, or one whose old or new text has lines the
/// rope and the server number differently.
pub(super) fn remember_document(
    documents: &mut HashMap<PathBuf, OpenDocument>,
    cmd: &mut ServerCmd,
) {
    match cmd {
        ServerCmd::DidOpen {
            path,
            language,
            version,
            text,
        } => {
            documents.insert(
                path.clone(),
                OpenDocument {
                    language: language.clone(),
                    version: *version,
                    text: text.clone(),
                    lines_agree: lines_agree(text),
                    resync: false,
                },
            );
        },
        ServerCmd::DidChange {
            path,
            version,
            text,
            delta,
        } => {
            let Some(document) = documents.get_mut(path) else {
                *delta = None;
                return;
            };
            let agree = lines_agree(text);
            if !(agree && document.lines_agree)
                || document.resync
                || delta
                    .as_ref()
                    .is_some_and(|delta| delta.base_version != document.version)
            {
                *delta = None;
            }
            document.version = *version;
            document.text.clone_from(text);
            document.lines_agree = agree;
            document.resync = false;
        },
        ServerCmd::DidSave { path, text } => {
            if let Some(document) = documents.get_mut(path) {
                document.text.clone_from(text);
                document.lines_agree = lines_agree(text);
            }
        },
        ServerCmd::DidClose { path } => {
            documents.remove(path);
        },
        ServerCmd::Completion { .. }
        | ServerCmd::ResolveCompletion { .. }
        | ServerCmd::InlayHints { .. }
        | ServerCmd::ResolveInlayHint { .. }
        | ServerCmd::DocumentSymbols { .. }
        | ServerCmd::Hover { .. }
        | ServerCmd::SignatureHelp { .. }
        | ServerCmd::SelectionRanges { .. }
        | ServerCmd::DocumentHighlights { .. }
        | ServerCmd::Editing { .. }
        | ServerCmd::Hierarchy { .. }
        | ServerCmd::Definition { .. }
        | ServerCmd::WorkspaceSymbols { .. }
        | ServerCmd::Rename { .. }
        | ServerCmd::PrepareRename { .. }
        | ServerCmd::CodeAction(_)
        | ServerCmd::CodeLens(_)
        | ServerCmd::LinkColor(_)
        | ServerCmd::InlineCompletion(_)
        | ServerCmd::Formatting { .. }
        | ServerCmd::Notify(_) => {},
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;

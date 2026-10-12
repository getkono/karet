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
use karet_core::WorkspaceEdit;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::code_actions::UNAVAILABLE;
use super::health::FailureTally;
use super::message::LspUpdate;
use super::message::RenameAnswer;
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
pub(super) async fn flush_pending(
    client: &LspClient,
    pending: &mut Option<(PathBuf, i32, String)>,
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
    if let Some((path, version, text)) = pending.take() {
        let result = client.did_change(&path, version, &text).await;
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
}

pub(super) fn remember_document(documents: &mut HashMap<PathBuf, OpenDocument>, cmd: &ServerCmd) {
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
                },
            );
        },
        ServerCmd::DidChange {
            path,
            version,
            text,
        } => {
            if let Some(document) = documents.get_mut(path) {
                document.version = *version;
                document.text.clone_from(text);
            }
        },
        ServerCmd::DidSave { path, text } => {
            if let Some(document) = documents.get_mut(path) {
                document.text.clone_from(text);
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
        | ServerCmd::Definition { .. }
        | ServerCmd::WorkspaceSymbols { .. }
        | ServerCmd::Rename { .. }
        | ServerCmd::PrepareRename { .. }
        | ServerCmd::CodeAction(_)
        | ServerCmd::Formatting { .. }
        | ServerCmd::Notify(_) => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentId;
    use crate::RequestId;

    /// With no connection, nothing formatted the file — and the session has to
    /// hear exactly that, because it is what sends it to its own formatter.
    ///
    /// Reporting the server's *capability* here instead answered a question this
    /// path cannot know the answer to, and the session read it as "a server
    /// handled this". TOML then lost its built-in taplo pass for as long as the
    /// server was away: a whole 300s circuit-breaker cooldown, a retry backoff,
    /// or the rest of the session for a server that never launched.
    #[test]
    fn an_unanswerable_formatting_request_reports_that_nothing_formatted() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        answer_empty(
            &tx,
            ServerCmd::Formatting {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("Cargo.toml"),
                ask: crate::lsp::FormattingAsk::default(),
            },
            7,
        );

        assert!(
            matches!(
                rx.try_recv(),
                Ok(LspUpdate::Formatting {
                    generation: 7,
                    request: RequestId(1),
                    doc: DocumentId(2),
                    version: 3,
                    formatted: false,
                    ref edits,
                    scope: crate::lsp::FormattingScope::Save,
                }) if edits.is_empty()
            ),
            "an unreachable server must not be reported as having formatted the file"
        );

        // A format asked for by hand is answered as one, so its empty answer
        // is never mistaken for a save's.
        answer_empty(
            &tx,
            ServerCmd::Formatting {
                request: RequestId(4),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("Cargo.toml"),
                ask: crate::lsp::FormattingAsk {
                    scope: crate::lsp::FormattingScope::Document,
                    ..crate::lsp::FormattingAsk::default()
                },
            },
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::Formatting {
                request: RequestId(4),
                formatted: false,
                scope: crate::lsp::FormattingScope::Document,
                ..
            })
        ));
    }

    /// A rename asked of no connection still answers, both halves of it, so a
    /// client holding an open rename prompt is never left waiting.
    #[test]
    fn unanswerable_rename_requests_still_answer() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        answer_empty(
            &tx,
            ServerCmd::PrepareRename {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("main.rs"),
                position: karet_core::LineCol::new(0, 0),
                caret: karet_core::LineCol::new(0, 0),
            },
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::PrepareRename {
                generation: 7,
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                answer: RenameAnswer::NoServer,
                ..
            })
        ));
        answer_empty(
            &tx,
            ServerCmd::Rename {
                request: RequestId(4),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("main.rs"),
                position: karet_core::LineCol::new(0, 0),
                new_name: "renamed".into(),
            },
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::WorkspaceEdit {
                request: RequestId(4),
                edit: Ok(ref edit),
                ..
            }) if edit.is_empty()
        ));
    }

    /// A code-action step asked of no connection answers with the reason,
    /// through the same path as every other request.
    #[test]
    fn an_unanswerable_code_action_step_still_answers() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        answer_empty(
            &tx,
            ServerCmd::CodeAction(super::super::CodeActionCmd::List {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("main.rs"),
                range: karet_core::Range::default(),
                kinds: Vec::new(),
                trigger: crate::api::CodeActionTrigger::Automatic,
            }),
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::CodeAction {
                generation: 7,
                answer: super::super::CodeActionAnswer::Listed {
                    request: RequestId(1),
                    actions: Err(ref reason),
                    ..
                },
            }) if reason == UNAVAILABLE
        ));
    }

    fn hint_request() -> ServerCmd {
        ServerCmd::InlayHints {
            request: RequestId(4),
            doc: DocumentId(5),
            version: 6,
            path: PathBuf::from("main.rs"),
            range: karet_core::Range::default(),
        }
    }

    /// A server that is gone for good has no hints: the editor may clear them
    /// and stop asking.
    #[test]
    fn a_server_that_is_gone_answers_a_hint_request_with_none() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        answer_empty(&tx, hint_request(), 7);
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::InlayHints { ref hints, .. }) if hints.is_empty()
        ));
    }

    /// A server that is reconnecting has said nothing about the document, so
    /// the hints on screen must survive the back-off.
    #[test]
    fn a_reconnecting_server_reports_a_hint_request_unanswered() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        answer_reconnecting(&tx, hint_request(), 7);
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::InlayHintsFailed {
                generation: 7,
                request: RequestId(4),
                doc: DocumentId(5),
                version: 6,
            })
        ));
    }

    /// A resolve no server can serve is answered with the hint as it was, so
    /// the editor stops waiting on it and spends its payload.
    #[test]
    fn an_unserved_resolve_hands_the_hint_back_unchanged() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let hint = karet_core::InlayHint {
            payload: Some("{}".to_owned()),
            ..karet_core::InlayHint::new(
                LineCol::new(1, 2),
                ": i32",
                karet_core::InlayHintKind::Type,
            )
        };
        answer_reconnecting(
            &tx,
            ServerCmd::ResolveInlayHint {
                request: RequestId(3),
                doc: DocumentId(5),
                hint: hint.clone(),
            },
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::InlayHintResolved {
                generation: 7,
                request: RequestId(3),
                doc: DocumentId(5),
                hint: ref answered,
            }) if *answered == hint
        ));
    }

    /// Everything but a hint request is answered exactly as [`answer_empty`]
    /// answers it -- formatting included, which must still fall back.
    #[test]
    fn a_reconnecting_server_answers_other_requests_empty() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        answer_reconnecting(
            &tx,
            ServerCmd::Formatting {
                request: RequestId(1),
                doc: DocumentId(2),
                version: 3,
                path: PathBuf::from("Cargo.toml"),
                ask: crate::lsp::FormattingAsk::default(),
            },
            7,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(LspUpdate::Formatting {
                formatted: false,
                ..
            })
        ));
    }
}

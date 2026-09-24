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
use std::path::PathBuf;

use karet_core::ServerFeature;
use karet_core::WorkspaceEdit;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::message::LspUpdate;
use super::message::ServerCmd;
use super::slot::SlotKey;
use crate::RequestId;

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
                items: Vec::new(),
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
            ..
        } => {
            let _ = updates.send(LspUpdate::Definitions {
                generation,
                request,
                doc,
                version,
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
        ServerCmd::Rename { request, .. } => {
            let _ = updates.send(LspUpdate::WorkspaceEdit {
                generation,
                request,
                edit: WorkspaceEdit::default(),
            });
        },
        ServerCmd::Formatting {
            request,
            doc,
            version,
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
            });
        },
        ServerCmd::DidOpen { .. }
        | ServerCmd::DidChange { .. }
        | ServerCmd::DidClose { .. }
        | ServerCmd::DidSave { .. } => {},
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
        | ServerCmd::InlayHints { .. }
        | ServerCmd::DocumentSymbols { .. }
        | ServerCmd::Hover { .. }
        | ServerCmd::Definition { .. }
        | ServerCmd::WorkspaceSymbols { .. }
        | ServerCmd::Rename { .. }
        | ServerCmd::Formatting { .. } => {},
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
                indentation: karet_lsp::Indentation::default(),
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
                }) if edits.is_empty()
            ),
            "an unreachable server must not be reported as having formatted the file"
        );
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
                indentation: karet_lsp::Indentation::default(),
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

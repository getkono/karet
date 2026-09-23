//! What the task does with a command it cannot serve right now.
//!
//! Two halves of one contract. `answer_empty` keeps a caller from waiting on a
//! connection that is not there -- every request is answered, even when the answer
//! is nothing. `remember_document` maintains the task's authoritative copy of the
//! open-document set, which is what a reconnect replays; a document missing from
//! it is one the server never learns about again.

use std::collections::HashMap;
use std::path::PathBuf;

use karet_core::WorkspaceEdit;
use tokio::sync::mpsc;

use super::message::LspUpdate;
use super::message::ServerCmd;

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

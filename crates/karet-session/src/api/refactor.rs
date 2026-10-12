//! The wire model for refactorings: renaming a symbol, running a code
//! action, applying a workspace edit, and formatting by hand.
//!
//! Every workspace edit -- a rename's, a code action's, a client's
//! [`Command::ApplyWorkspaceEdit`](super::Command::ApplyWorkspaceEdit), a
//! language server's `workspace/applyEdit` -- goes through one apply path in
//! the backend, and every one reports through the same
//! [`Event::WorkspaceEdit`](super::Event::WorkspaceEdit), so a client learns
//! about edits it did not ask for exactly as it learns about its own.

use std::path::PathBuf;

use karet_core::ApplyWorkspaceEditResult;
use karet_core::Range;

use super::DocumentId;

/// Why a client asks for code actions
/// ([`Command::CodeActions`](super::Command::CodeActions)), which a language
/// server may use to decide how much work to do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CodeActionTrigger {
    /// The user asked (a quick-fix key, a palette command). A server that
    /// does not offer code actions is reported, as for any request made by
    /// hand.
    #[default]
    Invoked,
    /// The client is probing on its own, as the caret moves, to show that
    /// actions exist. Nothing is reported about a server that has none.
    Automatic,
}

/// A [`Command::Format`](super::Command::Format): format part or all of an
/// open document with its language's formatter, by hand.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FormatRequest {
    /// The document to format.
    pub doc: DocumentId,
    /// How much of it.
    pub scope: FormatScope,
}

/// How much of a document a [`FormatRequest`] formats.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FormatScope {
    /// The whole document (Format Document).
    Document,
    /// These ranges, in buffer (character) columns (Format Selection). An
    /// empty range (a bare caret) stands for its whole line, and ranges that
    /// touch or overlap are formatted once, as one.
    Ranges(Vec<Range>),
}

/// The span a rename replaces, and the name to prefill the prompt with.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RenameTarget {
    /// The span of the symbol, in buffer (character) columns.
    pub range: Range,
    /// The current name, or what the language server suggests instead.
    pub placeholder: String,
}

/// What became of one workspace edit.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppliedWorkspaceEdit {
    /// The producer's label for the edit (a server's `applyEdit` label), if it
    /// gave one.
    pub label: Option<String>,
    /// Whether it applied, and if not, why and at which step.
    ///
    /// An edit of text alone applies whole or not at all. One that also
    /// creates, renames, or deletes files stops at the first step that fails,
    /// keeping the steps before it.
    pub result: ApplyWorkspaceEditResult,
    /// Every file whose text the edit changed, in the order first changed.
    pub edited: Vec<PathBuf>,
    /// Documents the backend opened to edit them, because no client had them
    /// open. Each is held with one reference on the client's behalf, exactly
    /// as if the client had sent [`Command::OpenDocument`]: it carries unsaved,
    /// undoable changes, and the client owns closing it.
    ///
    /// [`Command::OpenDocument`]: super::Command::OpenDocument
    pub opened: Vec<(DocumentId, PathBuf)>,
    /// Files and directories the edit moved, `(from, to)`. Open documents
    /// under them have already been retargeted.
    pub renamed: Vec<(PathBuf, PathBuf)>,
}

#[cfg(test)]
mod tests {
    use karet_core::LineCol;
    use karet_core::WorkspaceEdit;

    use super::*;
    use crate::api::Command;
    use crate::api::Event;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A code action with every field filled, its payload included: the
    /// producer's opaque encoding has to come back byte for byte.
    fn action() -> karet_core::CodeAction {
        karet_core::CodeAction {
            title: "Import Foo".into(),
            kind: Some("quickfix".into()),
            preferred: true,
            disabled: Some("not here".into()),
            edit: Some(WorkspaceEdit::default()),
            command: Some(karet_core::CommandId("fix".into())),
            payload: Some(r#"{"title":"Import Foo","data":{"id":1}}"#.into()),
        }
    }

    /// The refactoring vocabulary crosses the seam intact.
    #[test]
    fn refactor_commands_and_events_round_trip() -> TestResult {
        let range = Range {
            start: LineCol::new(1, 2),
            end: LineCol::new(1, 5),
        };
        let commands = [
            Command::PrepareRename {
                doc: DocumentId(1),
                position: LineCol::new(1, 3),
            },
            Command::ApplyWorkspaceEdit {
                edit: WorkspaceEdit::default(),
                label: Some("fix".into()),
            },
            Command::CodeActions {
                doc: DocumentId(1),
                range,
                kinds: vec!["source.organizeImports".into()],
                trigger: CodeActionTrigger::Automatic,
            },
            Command::ApplyCodeAction {
                doc: DocumentId(1),
                version: 4,
                action: action(),
            },
            Command::Format(FormatRequest {
                doc: DocumentId(1),
                scope: FormatScope::Document,
            }),
            Command::Format(FormatRequest {
                doc: DocumentId(1),
                scope: FormatScope::Ranges(vec![range]),
            }),
        ];
        for command in commands {
            let json = serde_json::to_string(&command)?;
            let back: Command = serde_json::from_str(&json)?;
            assert_eq!(serde_json::to_string(&back)?, json);
        }
        let events = [
            Event::RenamePrepared {
                doc: DocumentId(1),
                version: 4,
                target: Some(RenameTarget {
                    range,
                    placeholder: "abc".into(),
                }),
                reason: None,
            },
            Event::CodeActions {
                doc: DocumentId(1),
                version: 4,
                actions: vec![action()],
                reason: Some("none here".into()),
            },
            Event::WorkspaceEdit {
                applied: AppliedWorkspaceEdit {
                    label: None,
                    result: ApplyWorkspaceEditResult::failed("stale", Some(1)),
                    edited: vec![PathBuf::from("/w/a.rs")],
                    opened: vec![(DocumentId(2), PathBuf::from("/w/b.rs"))],
                    renamed: vec![(PathBuf::from("/w/c.rs"), PathBuf::from("/w/d.rs"))],
                },
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event)?;
            let back: Event = serde_json::from_str(&json)?;
            assert_eq!(serde_json::to_string(&back)?, json);
        }
        Ok(())
    }
}

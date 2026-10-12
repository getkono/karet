//! The server-task half of code actions: listing them, filling in a deferred
//! edit, and running a command.
//!
//! One command and one update carry all three steps, so the task loop in
//! `runtime` spends one arm on them. The session sequences the steps -- it
//! applies an action's edit between resolving it and running its command,
//! because only the session holds the documents the edit lands in.

use karet_core::CodeAction;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;

use super::commands::report_unsupported;
use super::health::FailureTally;
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

impl CodeActionCmd {
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

impl StepContext<'_> {
    /// A request's error as the user's answer. A refusal or a server's error
    /// reply belongs to the request; anything else (a timeout, a closed
    /// connection) also counts against the connection.
    fn settle<T>(&mut self, result: Result<T, LspError>) -> Result<T, String> {
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

#[cfg(test)]
mod tests {
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
}

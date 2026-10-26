//! Code actions: listing them with the diagnostics they would fix, filling in
//! an edit the server deferred (`codeAction/resolve`), and running an
//! action's command (`workspace/executeCommand`).
//!
//! An action is applied by its consumer, not here: its edit goes through
//! whatever applies workspace edits (in karet, the session's one path), and
//! only then does its command run. Edits the server makes while running a
//! command arrive as `workspace/applyEdit` requests, through
//! [`LspClient::apply_edit_requests`].

use std::path::Path;

use karet_core::CodeAction;
use karet_core::Range;
use karet_core::ServerFeature;
use serde_json::Value;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::uri;

/// Why code actions are being asked for, which a server may use to decide
/// how much work to do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CodeActionTrigger {
    /// The user asked for them (a quick-fix key, a menu).
    #[default]
    Invoked,
    /// The editor asked on its own (a lightbulb probe as the caret moves).
    Automatic,
}

impl LspClient {
    /// Request every code action available for `range` in `doc`.
    ///
    /// Shorthand for [`Self::code_actions`] with no kind filter, as invoked
    /// by the user.
    ///
    /// # Errors
    /// As [`Self::code_actions`].
    pub async fn code_action(&self, doc: &Path, range: Range) -> Result<Vec<CodeAction>, LspError> {
        self.code_actions(doc, range, &[], CodeActionTrigger::Invoked)
            .await
    }

    /// Request the code actions available for `range` in `doc`.
    ///
    /// The diagnostics the server last published that touch `range` go along
    /// as the request's context, exactly as the server sent them. `only`
    /// limits the answer to those kinds and the kinds nested under them
    /// (`refactor` admits `refactor.extract`); empty admits every kind. The
    /// limit is applied to the answer too, since a server may ignore it.
    ///
    /// Disabled actions are kept, with their reason. An action may arrive
    /// without its edit; [`Self::resolve_code_action`] fills it in.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn code_actions(
        &self,
        doc: &Path,
        range: Range,
        only: &[String],
        trigger: CodeActionTrigger,
    ) -> Result<Vec<CodeAction>, LspError> {
        self.require_for(ServerFeature::CodeAction, "textDocument/codeAction", doc)?;
        let uri = uri::path_to_uri(doc)?;
        let range = convert::range_to_lsp(range);
        // Keyed the way `publishDiagnostics` keys it: by the path the URI
        // names, which is what the server's own spelling of this file maps to.
        let published = uri::uri_to_path(&uri).unwrap_or_else(|| doc.to_path_buf());
        let params = lsp_types::CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri),
            range,
            context: lsp_types::CodeActionContext {
                diagnostics: self.conn.published_diagnostics(&published, range),
                only: (!only.is_empty()).then(|| {
                    only.iter()
                        .map(|kind| lsp_types::CodeActionKind::from(kind.clone()))
                        .collect()
                }),
                trigger_kind: Some(match trigger {
                    CodeActionTrigger::Invoked => lsp_types::CodeActionTriggerKind::INVOKED,
                    CodeActionTrigger::Automatic => lsp_types::CodeActionTriggerKind::AUTOMATIC,
                }),
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::CodeActionResponse> =
            self.conn.request("textDocument/codeAction", params).await?;
        let mut actions = convert::code_actions_from_lsp(response);
        if !only.is_empty() {
            actions.retain(|action| only.iter().any(|kind| action.is_kind(kind)));
        }
        Ok(actions)
    }

    /// Fill in the edit of an action that arrived without one
    /// (`codeAction/resolve`).
    ///
    /// The action comes back as it went in when there is nothing to resolve:
    /// it already has an edit, it is a bare command, it did not come from a
    /// server, or the server does not resolve actions.
    ///
    /// # Errors
    /// Returns [`LspError::Server`], [`LspError::Timeout`],
    /// [`LspError::Protocol`] for a request that does not encode or an answer
    /// that is not an action, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn resolve_code_action(&self, action: &CodeAction) -> Result<CodeAction, LspError> {
        if action.edit.is_some() || !self.supports(ServerFeature::CodeActionResolve) {
            return Ok(action.clone());
        }
        let Some(literal) = convert::code_action_literal(action) else {
            return Ok(action.clone());
        };
        let resolved: lsp_types::CodeAction =
            self.conn.request("codeAction/resolve", literal).await?;
        Ok(convert::code_action_or_command_from_lsp(
            lsp_types::CodeActionOrCommand::CodeAction(resolved),
        ))
    }

    /// Run `command` on the server (`workspace/executeCommand`) and return
    /// its result.
    ///
    /// Any edit the command makes arrives separately, as a
    /// `workspace/applyEdit` request the server waits on before answering
    /// this one -- so whoever takes [`Self::apply_edit_requests`] must keep
    /// answering while this is awaited.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server does not execute commands, or listed the ones it does and this
    /// is not among them; else [`LspError::Server`], [`LspError::Timeout`],
    /// [`LspError::Protocol`] for arguments that do not encode, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn execute_command(
        &self,
        command: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, LspError> {
        if !self.capabilities().accepts_command(command) {
            return Err(LspError::Unsupported {
                method: "workspace/executeCommand",
            });
        }
        let params = lsp_types::ExecuteCommandParams {
            command: command.to_owned(),
            arguments,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let result: Option<Value> = self
            .conn
            .request("workspace/executeCommand", params)
            .await?;
        Ok(result.unwrap_or(Value::Null))
    }

    /// Run the command `action` carries, with the arguments the server gave
    /// it; nothing when it carries none.
    ///
    /// Apply the action's edit first: the protocol runs the command after it.
    ///
    /// # Errors
    /// As [`Self::execute_command`].
    pub async fn run_code_action_command(&self, action: &CodeAction) -> Result<(), LspError> {
        let Some(command) = convert::code_action_command(action) else {
            return Ok(());
        };
        self.execute_command(&command.command, command.arguments.unwrap_or_default())
            .await
            .map(drop)
    }
}

#[cfg(test)]
#[path = "code_action_tests.rs"]
mod tests;

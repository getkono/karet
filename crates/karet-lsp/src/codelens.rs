//! Code lenses: listing a document's (`textDocument/codeLens`), filling in the
//! title and command a server deferred for one (`codeLens/resolve`), and
//! running the command a lens carries (`workspace/executeCommand`).
//!
//! A lens keeps its wire form as its payload, so a resolve hands the server
//! back exactly what it sent -- `data` included, as LSP requires -- and a run
//! sends the command's arguments untouched.

use std::path::Path;

use karet_core::CodeLens;
use karet_core::CommandId;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::uri;

impl LspClient {
    /// Request every code lens in `doc`.
    ///
    /// A lens may arrive without its title and command;
    /// [`Self::resolve_code_lens`] fills them in.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`] or
    /// [`LspError::Timeout`].
    pub async fn code_lenses(&self, doc: &Path) -> Result<Vec<CodeLens>, LspError> {
        self.require_for(ServerFeature::CodeLens, "textDocument/codeLens", doc)?;
        let params = lsp_types::CodeLensParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<lsp_types::CodeLens>> =
            self.conn.request("textDocument/codeLens", params).await?;
        Ok(response
            .unwrap_or_default()
            .into_iter()
            .map(code_lens_from_lsp)
            .collect())
    }

    /// Fill in the title and command of a lens that arrived without them
    /// (`codeLens/resolve`).
    ///
    /// The lens comes back as it went in when there is nothing to resolve: it
    /// already has a title, it has no payload, or the server does not resolve
    /// lenses. The answer keeps `lens`'s own range -- a resolve fills a lens
    /// in, it does not move it -- so whatever columns the caller holds it in
    /// stay true.
    ///
    /// # Errors
    /// Returns [`LspError::Server`], [`LspError::Protocol`] (an answer that is
    /// not a lens), or [`LspError::Timeout`].
    pub async fn resolve_code_lens(&self, lens: &CodeLens) -> Result<CodeLens, LspError> {
        if lens.is_resolved() || !self.supports(ServerFeature::CodeLensResolve) {
            return Ok(lens.clone());
        }
        let Some(literal) = code_lens_literal(lens) else {
            return Ok(lens.clone());
        };
        let resolved: lsp_types::CodeLens = self.conn.request("codeLens/resolve", literal).await?;
        Ok(CodeLens {
            range: lens.range,
            ..code_lens_from_lsp(resolved)
        })
    }

    /// Run the command `lens` carries (`workspace/executeCommand`), with the
    /// arguments the server gave it; nothing when it carries none.
    ///
    /// Edits the command makes arrive as `workspace/applyEdit` requests, as
    /// [`Self::execute_command`] describes.
    ///
    /// # Errors
    /// As [`Self::execute_command`]: in particular [`LspError::Unsupported`],
    /// without issuing a request, for a command the server does not run --
    /// the client-side commands some servers put on lenses among them.
    pub async fn run_code_lens(&self, lens: &CodeLens) -> Result<(), LspError> {
        let Some(command) = code_lens_literal(lens).and_then(|literal| literal.command) else {
            return Ok(());
        };
        if command.command.is_empty() {
            return Ok(());
        }
        self.execute_command(&command.command, command.arguments.unwrap_or_default())
            .await
            .map(drop)
    }
}

/// Map one lens, keeping its wire form as the payload.
pub(crate) fn code_lens_from_lsp(lens: lsp_types::CodeLens) -> CodeLens {
    let payload = serde_json::to_string(&lens).ok();
    let (title, command) = match &lens.command {
        Some(command) => (
            Some(command.title.clone()),
            (!command.command.is_empty()).then(|| CommandId(command.command.clone())),
        ),
        None => (None, None),
    };
    CodeLens {
        range: convert::range_from_lsp(lens.range),
        title,
        command,
        payload,
    }
}

/// The lens literal a payload holds, for `codeLens/resolve` and for running
/// its command.
pub(crate) fn code_lens_literal(lens: &CodeLens) -> Option<lsp_types::CodeLens> {
    serde_json::from_str(lens.payload.as_deref()?).ok()
}

#[cfg(test)]
#[path = "codelens_tests.rs"]
mod tests;

//! Code navigation beyond Go to Definition: the declaration, the type's
//! definition, and every reference to a symbol.
//!
//! [`LspClient::definition`] and [`LspClient::implementations`] predate this
//! module and stay beside the other single-position requests; these share
//! their shape -- one position in, file locations out, non-file URIs dropped --
//! and their gate: a server that never advertised the capability is refused
//! without a request being issued.

use std::path::Path;

use karet_core::LineCol;
use karet_core::Location;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::text_document_position;

impl LspClient {
    /// Resolve the declaration location(s) of the symbol at `pos` -- for
    /// languages that separate the two, where it is declared rather than
    /// defined (a C prototype, a forward declaration).
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn declaration(&self, doc: &Path, pos: LineCol) -> Result<Vec<Location>, LspError> {
        self.require_for(ServerFeature::Declaration, "textDocument/declaration", doc)?;
        let params = lsp_types::request::GotoDeclarationParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::request::GotoDeclarationResponse> = self
            .conn
            .request("textDocument/declaration", params)
            .await?;
        Ok(convert::locations_from_lsp(response))
    }

    /// Resolve the definition location(s) of the *type* of the symbol at `pos`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn type_definition(
        &self,
        doc: &Path,
        pos: LineCol,
    ) -> Result<Vec<Location>, LspError> {
        self.require_for(
            ServerFeature::TypeDefinition,
            "textDocument/typeDefinition",
            doc,
        )?;
        let params = lsp_types::request::GotoTypeDefinitionParams {
            text_document_position_params: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<lsp_types::request::GotoTypeDefinitionResponse> = self
            .conn
            .request("textDocument/typeDefinition", params)
            .await?;
        Ok(convert::locations_from_lsp(response))
    }

    /// Find every reference to the symbol at `pos`, in server order.
    ///
    /// `include_declaration` asks the server to list the declaration among the
    /// references, as an editor's "find all references" conventionally does.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn references(
        &self,
        doc: &Path,
        pos: LineCol,
        include_declaration: bool,
    ) -> Result<Vec<Location>, LspError> {
        self.require_for(ServerFeature::References, "textDocument/references", doc)?;
        let params = lsp_types::ReferenceParams {
            text_document_position: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: lsp_types::ReferenceContext {
                include_declaration,
            },
        };
        let response: Option<Vec<lsp_types::Location>> =
            self.conn.request("textDocument/references", params).await?;
        Ok(convert::locations_from_lsp(
            response.map(lsp_types::GotoDefinitionResponse::Array),
        ))
    }
}

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod tests;

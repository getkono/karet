//! Requests that change text as it is edited or saved:
//! `textDocument/linkedEditingRange` (the ranges an edit at one of them must
//! mirror into, such as an HTML tag's name and its closing tag's),
//! `textDocument/onTypeFormatting` (the edits a server makes after a trigger
//! character is typed), and `textDocument/willSaveWaitUntil` (the edits a
//! server makes before a document is written).

use std::path::Path;

use karet_core::LineCol;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::TextEdit;

use crate::Indentation;
use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::formatting_options;
use crate::text_document_position;
use crate::uri;

/// The ranges a [`LspClient::linked_editing_ranges`] found: edit one and the
/// same edit belongs in every other.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkedEditingRanges {
    /// The linked ranges, in the server's order and the negotiated column
    /// encoding. All of them hold the same text.
    pub ranges: Vec<Range>,
    /// The regular expression, when the server gave one, that the text of
    /// every range must keep matching while the edit stays linked.
    pub word_pattern: Option<String>,
}

/// Why a document is about to be saved, as `textDocument/willSaveWaitUntil`
/// tells the server (LSP `TextDocumentSaveReason`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SaveReason {
    /// The user asked: a key, a command, a prompt.
    #[default]
    Manual,
    /// An autosave after a pause in typing.
    AfterDelay,
    /// An autosave because focus left the document.
    FocusOut,
}

impl LspClient {
    /// The ranges linked to the one at `position` in `doc`, or `None` when the
    /// server has none there. Columns pass through in the negotiated encoding.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn linked_editing_ranges(
        &self,
        doc: &Path,
        position: LineCol,
    ) -> Result<Option<LinkedEditingRanges>, LspError> {
        self.require_for(
            ServerFeature::LinkedEditingRange,
            "textDocument/linkedEditingRange",
            doc,
        )?;
        let params = lsp_types::LinkedEditingRangeParams {
            text_document_position_params: text_document_position(doc, position)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<lsp_types::LinkedEditingRanges> = self
            .conn
            .request("textDocument/linkedEditingRange", params)
            .await?;
        Ok(response.map(|linked| LinkedEditingRanges {
            ranges: linked
                .ranges
                .into_iter()
                .map(convert::range_from_lsp)
                .collect(),
            word_pattern: linked.word_pattern,
        }))
    }

    /// The edits the server makes after `ch` was typed at `position` in `doc`
    /// (the position just after it), indented as `indentation` says.
    ///
    /// A `ch` that is not one of the server's trigger characters
    /// ([`Self::on_type_formatting_triggers`]) is answered with no edits and no
    /// request: a caller may offer every typed character without first
    /// learning which ones the server formats on.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn on_type_formatting(
        &self,
        doc: &Path,
        position: LineCol,
        ch: char,
        indentation: Indentation,
    ) -> Result<Vec<TextEdit>, LspError> {
        self.require_for(
            ServerFeature::OnTypeFormatting,
            "textDocument/onTypeFormatting",
            doc,
        )?;
        if !self.on_type_formatting_triggers().contains(&ch) {
            return Ok(Vec::new());
        }
        let params = lsp_types::DocumentOnTypeFormattingParams {
            text_document_position: text_document_position(doc, position)?,
            ch: ch.to_string(),
            options: formatting_options(indentation),
        };
        let response: Option<Vec<lsp_types::TextEdit>> = self
            .conn
            .request("textDocument/onTypeFormatting", params)
            .await?;
        Ok(convert::text_edits_from_lsp(response))
    }

    /// Every character this server formats on as it is typed, its first
    /// trigger first; empty when it formats on none.
    ///
    /// Read from the handshake. A server that registers format-on-type later
    /// does not name its triggers through this, so it formats on none here.
    #[must_use]
    pub fn on_type_formatting_triggers(&self) -> Vec<char> {
        self.server_options()
            .on_type_formatting
            .map(|options| options.triggers())
            .unwrap_or_default()
    }

    /// The edits the server makes to `doc` before it is saved for `reason`,
    /// against the text it was last sent. The caller applies them before the
    /// write; nothing is written here.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not ask for it in its `textDocumentSync` options, else
    /// [`LspError::Server`], [`LspError::Timeout`], [`LspError::Protocol`] for
    /// an unconvertible path, a request that does not encode or a result that
    /// does not decode, or [`LspError::Closed`] if the connection is gone.
    pub async fn will_save_wait_until(
        &self,
        doc: &Path,
        reason: SaveReason,
    ) -> Result<Vec<TextEdit>, LspError> {
        self.require_for(
            ServerFeature::WillSaveWaitUntil,
            "textDocument/willSaveWaitUntil",
            doc,
        )?;
        let params = lsp_types::WillSaveTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            reason: match reason {
                SaveReason::Manual => lsp_types::TextDocumentSaveReason::MANUAL,
                SaveReason::AfterDelay => lsp_types::TextDocumentSaveReason::AFTER_DELAY,
                SaveReason::FocusOut => lsp_types::TextDocumentSaveReason::FOCUS_OUT,
            },
        };
        let response: Option<Vec<lsp_types::TextEdit>> = self
            .conn
            .request("textDocument/willSaveWaitUntil", params)
            .await?;
        Ok(convert::text_edits_from_lsp(response))
    }
}

#[cfg(test)]
#[path = "editing_tests.rs"]
mod tests;

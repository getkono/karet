//! `textDocument/documentHighlight`: the places in a document where the symbol
//! at a position occurs, each marked as read, written, or merely textual, which
//! an editor paints while the caret rests on the symbol.

use std::path::Path;

use karet_core::LineCol;
use karet_core::Range;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::text_document_position;

/// How a [`DocumentHighlight`] uses the symbol (LSP `DocumentHighlightKind`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    /// A textual occurrence the server does not class as a read or a write;
    /// also what a highlight that names no kind is, as the protocol defines.
    #[default]
    Text,
    /// A read of the symbol, such as a variable's use.
    Read,
    /// A write to the symbol, such as an assignment or the declaration.
    Write,
}

/// One occurrence of the symbol a [`LspClient::document_highlights`] asked
/// about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocumentHighlight {
    /// The occurrence, in the negotiated column encoding.
    pub range: Range,
    /// How the occurrence uses the symbol.
    pub kind: HighlightKind,
}

impl LspClient {
    /// The occurrences of the symbol at `position` in `doc`, in the order the
    /// server gave them. A server that answers with nothing yields an empty
    /// list. Columns pass through in the negotiated encoding.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`] or
    /// [`LspError::Timeout`].
    pub async fn document_highlights(
        &self,
        doc: &Path,
        position: LineCol,
    ) -> Result<Vec<DocumentHighlight>, LspError> {
        self.require_for(
            ServerFeature::DocumentHighlight,
            "textDocument/documentHighlight",
            doc,
        )?;
        let params = lsp_types::DocumentHighlightParams {
            text_document_position_params: text_document_position(doc, position)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<lsp_types::DocumentHighlight>> = self
            .conn
            .request("textDocument/documentHighlight", params)
            .await?;
        Ok(response
            .unwrap_or_default()
            .into_iter()
            .map(|highlight| DocumentHighlight {
                range: convert::range_from_lsp(highlight.range),
                kind: kind_from_lsp(highlight.kind),
            })
            .collect())
    }
}

/// The protocol's kind, absent meaning [`HighlightKind::Text`]; a kind the
/// protocol has not defined is read as text too, the least specific claim.
fn kind_from_lsp(kind: Option<lsp_types::DocumentHighlightKind>) -> HighlightKind {
    match kind {
        Some(lsp_types::DocumentHighlightKind::READ) => HighlightKind::Read,
        Some(lsp_types::DocumentHighlightKind::WRITE) => HighlightKind::Write,
        _ => HighlightKind::Text,
    }
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;

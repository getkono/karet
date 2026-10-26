//! `textDocument/selectionRange`: the server's own chain of ever-larger ranges
//! around a position, which an editor's Expand Selection grows through.

use std::path::Path;

use karet_core::LineCol;
use karet_core::Range;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::uri;

impl LspClient {
    /// The selection ranges around each of `positions`, one chain per position
    /// in the order given, each chain innermost first.
    ///
    /// The protocol links each range to its parent; this flattens the links
    /// into a list, dropping any parent that does not contain the range before
    /// it (the protocol requires it to, so such a link is out of spec). A
    /// server that answers with no ranges, or with fewer chains than
    /// positions, leaves the missing chains empty, so the result always has one
    /// chain per position. Columns pass through in the negotiated encoding.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn selection_ranges(
        &self,
        doc: &Path,
        positions: &[LineCol],
    ) -> Result<Vec<Vec<Range>>, LspError> {
        self.require_for(
            ServerFeature::SelectionRange,
            "textDocument/selectionRange",
            doc,
        )?;
        let params = lsp_types::SelectionRangeParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            positions: positions
                .iter()
                .copied()
                .map(convert::position_to_lsp)
                .collect(),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<lsp_types::SelectionRange>> = self
            .conn
            .request("textDocument/selectionRange", params)
            .await?;
        let mut chains: Vec<Vec<Range>> = response
            .unwrap_or_default()
            .into_iter()
            .take(positions.len())
            .map(selection_chain)
            .collect();
        chains.resize_with(positions.len(), Vec::new);
        Ok(chains)
    }
}

/// Flatten one linked `SelectionRange` into its ranges, innermost first,
/// stopping at the first parent that does not contain its child.
fn selection_chain(range: lsp_types::SelectionRange) -> Vec<Range> {
    let mut chain: Vec<Range> = Vec::new();
    let mut next = Some(range);
    while let Some(link) = next {
        let current = convert::range_from_lsp(link.range);
        if let Some(inner) = chain.last()
            && !(current.start <= inner.start && inner.end <= current.end)
        {
            break;
        }
        if chain.last() != Some(&current) {
            chain.push(current);
        }
        next = link.parent.map(|parent| *parent);
    }
    chain
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;

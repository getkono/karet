//! Folding ranges (`textDocument/foldingRange`): the regions a server says a
//! document folds into, as neutral [`FoldRegions`].
//!
//! The client declares `lineFoldingOnly`, since every karet fold hides whole
//! lines, so a range's start and end lines are all that is read. Ranges are
//! decoded one at a time: a `kind` outside LSP's three (a server is free to
//! send its own) would otherwise fail the whole answer, and one entry that is
//! not a range at all would take every other range with it.

use std::path::Path;

use karet_core::FoldKind;
use karet_core::FoldRegion;
use karet_core::FoldRegions;
use karet_core::ServerFeature;
use serde_json::Value;

use crate::LspClient;
use crate::LspError;
use crate::uri;

impl LspClient {
    /// Request the folding ranges of `doc`, in document order with at most one
    /// region per header line (see [`FoldRegions::from_unordered`]).
    ///
    /// A range's `comment`, `imports` and `region` kinds map to
    /// [`FoldKind::Comment`], [`FoldKind::Imports`] and [`FoldKind::Region`];
    /// any other kind, or none, is [`FoldKind::Code`]. An entry without both
    /// lines is skipped, as is a range that hides no line.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that is not a list, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn folding_ranges(&self, doc: &Path) -> Result<FoldRegions, LspError> {
        self.require_for(
            ServerFeature::FoldingRange,
            "textDocument/foldingRange",
            doc,
        )?;
        let params = lsp_types::FoldingRangeParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<Value>> = self
            .conn
            .request("textDocument/foldingRange", params)
            .await?;
        Ok(FoldRegions::from_unordered(
            response
                .unwrap_or_default()
                .iter()
                .filter_map(fold_region_from_lsp)
                .collect(),
        ))
    }
}

/// One LSP `FoldingRange` as a [`FoldRegion`], or `None` when it lacks a
/// start or end line.
pub(crate) fn fold_region_from_lsp(range: &Value) -> Option<FoldRegion> {
    let line = |key| {
        range
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|line| u32::try_from(line).ok())
    };
    let kind = match range.get("kind").and_then(Value::as_str) {
        Some("comment") => FoldKind::Comment,
        Some("imports") => FoldKind::Imports,
        Some("region") => FoldKind::Region,
        _ => FoldKind::Code,
    };
    Some(FoldRegion::new(line("startLine")?, line("endLine")?).with_kind(kind))
}

#[cfg(test)]
#[path = "folding_tests.rs"]
mod tests;

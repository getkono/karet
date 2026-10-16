//! Pull diagnostics: asking a server for a document's diagnostics
//! (`textDocument/diagnostic`) instead of waiting for it to publish them.
//!
//! Some servers only answer when asked. A pulled report is mapped exactly as a
//! publication is, and its wire form is remembered the same way, so the
//! code-action context sees pulled diagnostics as it sees pushed ones.

use std::path::Path;

use karet_core::Diagnostic;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::PublishedDiagnostics;
use crate::convert;
use crate::uri;

/// One `textDocument/diagnostic` answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PulledDiagnostics {
    /// The asked document's complete diagnostics, or `None` when the server
    /// reported them unchanged since the `previous_result_id` it was handed.
    pub diagnostics: Option<Vec<Diagnostic>>,
    /// The id to hand back as `previous_result_id` on the next pull, when the
    /// server issued one.
    pub result_id: Option<String>,
    /// Complete diagnostics the server reported alongside for other documents
    /// (`relatedDocuments`), each as a publication without a version. A
    /// related document reported unchanged is not listed.
    pub related: Vec<PublishedDiagnostics>,
}

impl LspClient {
    /// Ask the server for `doc`'s diagnostics (`textDocument/diagnostic`).
    ///
    /// `previous_result_id` is the [`PulledDiagnostics::result_id`] of the
    /// last pull for `doc`; with it the server may answer "unchanged" instead
    /// of resending the whole set. Ranges are in UTF-16 columns, like a
    /// publication's.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise or register pull diagnostics for `doc`, else
    /// [`LspError::Server`] (a server still computing may cancel the request)
    /// or [`LspError::Timeout`].
    pub async fn pull_diagnostics(
        &self,
        doc: &Path,
        previous_result_id: Option<&str>,
    ) -> Result<PulledDiagnostics, LspError> {
        self.require_for(
            ServerFeature::PullDiagnostics,
            "textDocument/diagnostic",
            doc,
        )?;
        let uri = uri::path_to_uri(doc)?;
        // Keyed the way a publication for this document is: by the path the
        // URI names.
        let path = uri::uri_to_path(&uri).unwrap_or_else(|| doc.to_path_buf());
        let params = lsp_types::DocumentDiagnosticParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri),
            identifier: None,
            previous_result_id: previous_result_id.map(str::to_owned),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let result: lsp_types::DocumentDiagnosticReportResult =
            self.conn.request("textDocument/diagnostic", params).await?;
        let (own, related) = match result {
            lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Full(report),
            ) => (
                Own::Full(report.full_document_diagnostic_report),
                report.related_documents,
            ),
            lsp_types::DocumentDiagnosticReportResult::Report(
                lsp_types::DocumentDiagnosticReport::Unchanged(report),
            ) => (
                Own::Unchanged(Some(report.unchanged_document_diagnostic_report.result_id)),
                report.related_documents,
            ),
            // Partial results stream only to a client that sent a
            // `partialResultToken`, which this one never does; a server
            // answering with one anyway has said nothing about `doc` itself.
            lsp_types::DocumentDiagnosticReportResult::Partial(partial) => {
                (Own::Unchanged(None), partial.related_documents)
            },
        };
        let mut pulled = PulledDiagnostics {
            related: related
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(uri, report)| match report {
                    lsp_types::DocumentDiagnosticReportKind::Full(full) => Some((uri, full)),
                    lsp_types::DocumentDiagnosticReportKind::Unchanged(_) => None,
                })
                .filter_map(|(uri, full)| {
                    let path = uri::uri_to_path(&uri)?;
                    Some(PublishedDiagnostics {
                        diagnostics: self.adopt(&path, full.items),
                        path,
                        version: None,
                    })
                })
                .collect(),
            ..PulledDiagnostics::default()
        };
        match own {
            Own::Full(report) => {
                pulled.diagnostics = Some(self.adopt(&path, report.items));
                pulled.result_id = report.result_id;
            },
            Own::Unchanged(result_id) => pulled.result_id = result_id,
        }
        Ok(pulled)
    }

    /// Remember `wire` as the server's complete set for `path`, and map it.
    fn adopt(&self, path: &Path, wire: Vec<lsp_types::Diagnostic>) -> Vec<Diagnostic> {
        self.conn.remember_diagnostics(path, &wire);
        wire.into_iter().map(convert::diagnostic_from_lsp).collect()
    }
}

/// What a report said about the asked document itself.
enum Own {
    /// Its complete diagnostics.
    Full(lsp_types::FullDocumentDiagnosticReport),
    /// Nothing new, with the result id to keep using.
    Unchanged(Option<String>),
}

#[cfg(test)]
#[path = "pull_tests.rs"]
mod tests;

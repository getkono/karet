//! Document links and document colours: listing a document's links
//! (`textDocument/documentLink`), filling in the target a server deferred for
//! one (`documentLink/resolve`), and listing the colours it spells
//! (`textDocument/documentColor`).
//!
//! A link keeps its wire form as its payload, so a resolve hands the server
//! back exactly what it sent -- `data` included, as LSP requires. Links are
//! decoded leniently, one at a time: a target is a URI on the wire, and one
//! that a strict URI parser rejects (a path with a space in it) would
//! otherwise fail the whole answer and take every other link with it.

use std::path::Path;

use karet_core::DocumentColor;
use karet_core::DocumentLink;
use karet_core::ServerFeature;
use serde_json::Value;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::uri;

impl LspClient {
    /// Request every link in `doc`.
    ///
    /// A link may arrive without its target; [`Self::resolve_document_link`]
    /// fills it in. An entry that is not a link at all is skipped.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that is not a list, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn document_links(&self, doc: &Path) -> Result<Vec<DocumentLink>, LspError> {
        self.require_for(
            ServerFeature::DocumentLink,
            "textDocument/documentLink",
            doc,
        )?;
        let params = lsp_types::DocumentLinkParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<Value>> = self
            .conn
            .request("textDocument/documentLink", params)
            .await?;
        Ok(response
            .unwrap_or_default()
            .into_iter()
            .filter_map(document_link_from_lsp)
            .collect())
    }

    /// Fill in the target of a link that arrived without one
    /// (`documentLink/resolve`).
    ///
    /// The link comes back as it went in when there is nothing to resolve: it
    /// already has a target, it has no payload, or the server does not
    /// resolve links. The answer keeps `link`'s own range -- a resolve fills a
    /// link in, it does not move it -- so whatever columns the caller holds it
    /// in stay true.
    ///
    /// # Errors
    /// Returns [`LspError::Server`], [`LspError::Timeout`],
    /// [`LspError::Protocol`] for a request that does not encode or an answer
    /// that is not a link, or [`LspError::Closed`] if the connection is gone.
    pub async fn resolve_document_link(
        &self,
        link: &DocumentLink,
    ) -> Result<DocumentLink, LspError> {
        if link.is_resolved() || !self.supports(ServerFeature::DocumentLinkResolve) {
            return Ok(link.clone());
        }
        let Some(literal) = link
            .payload
            .as_deref()
            .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        else {
            return Ok(link.clone());
        };
        let resolved: Value = self.conn.request("documentLink/resolve", literal).await?;
        let resolved = document_link_from_lsp(resolved).ok_or_else(|| {
            LspError::Protocol("documentLink/resolve answered something not a link".to_owned())
        })?;
        Ok(DocumentLink {
            range: link.range,
            ..resolved
        })
    }

    /// Request every colour `doc` spells.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`],
    /// [`LspError::Timeout`], [`LspError::Protocol`] for an unconvertible
    /// path, a request that does not encode or a result that does not decode,
    /// or [`LspError::Closed`] if the connection is gone.
    pub async fn document_colors(&self, doc: &Path) -> Result<Vec<DocumentColor>, LspError> {
        self.require_for(
            ServerFeature::DocumentColor,
            "textDocument/documentColor",
            doc,
        )?;
        let params = lsp_types::DocumentColorParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
        };
        let response: Option<Vec<lsp_types::ColorInformation>> = self
            .conn
            .request("textDocument/documentColor", params)
            .await?;
        Ok(response
            .unwrap_or_default()
            .into_iter()
            .map(|colour| DocumentColor {
                range: convert::range_from_lsp(colour.range),
                rgba: rgba_from_lsp(colour.color),
            })
            .collect())
    }
}

/// A link as it travels, with its target left a string so one a URI parser
/// would reject still arrives.
#[derive(serde::Deserialize)]
struct WireLink {
    range: lsp_types::Range,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    tooltip: Option<String>,
}

/// Map one link, keeping its wire form as the payload; `None` for an entry
/// that is not a link. An empty target is no target.
pub(crate) fn document_link_from_lsp(literal: Value) -> Option<DocumentLink> {
    let wire: WireLink = serde_json::from_value(literal.clone()).ok()?;
    Some(DocumentLink {
        range: convert::range_from_lsp(wire.range),
        target: wire.target.filter(|target| !target.is_empty()),
        tooltip: wire.tooltip.filter(|tooltip| !tooltip.is_empty()),
        payload: serde_json::to_string(&literal).ok(),
    })
}

/// A colour's channels, each `0.0..=1.0` on the wire, as 8-bit RGBA; a
/// channel out of range is clamped, and one that is not a number is `0`.
pub(crate) fn rgba_from_lsp(colour: lsp_types::Color) -> [u8; 4] {
    // In 0..=255 by the clamp; a NaN casts to 0.
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    [
        channel(colour.red),
        channel(colour.green),
        channel(colour.blue),
        channel(colour.alpha),
    ]
}

#[cfg(test)]
#[path = "link_color_tests.rs"]
mod tests;

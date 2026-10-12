//! Completion: asking for candidates at a position (`textDocument/completion`),
//! and filling in what a server deferred for one of them
//! (`completionItem/resolve`).
//!
//! A server that resolves items may leave an item's documentation, detail and
//! additional edits (an auto-import, typically) out of the list and send them
//! only for the item the user is looking at. Such an item keeps its wire form
//! as the payload a resolve hands back, untouched, as LSP requires.

use std::path::Path;

use karet_core::CompletionItem;
use karet_core::CompletionList;
use karet_core::CompletionTrigger;
use karet_core::LineCol;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::text_document_position;

impl LspClient {
    /// Request completions at `pos` in `doc` (`pos.col` in UTF-16 units, per
    /// the crate docs), saying why through `trigger`.
    ///
    /// The list says whether it is incomplete: a consumer narrowing the typed
    /// prefix over an incomplete list asks again with
    /// [`CompletionTrigger::Incomplete`] instead of only filtering it.
    /// Snippet-format insert text is degraded to plain text -- this client
    /// does not advertise `snippetSupport`. When the server resolves items,
    /// each carries the payload [`Self::resolve_completion`] needs.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`] or
    /// [`LspError::Timeout`].
    pub async fn completion(
        &self,
        doc: &Path,
        pos: LineCol,
        trigger: CompletionTrigger,
    ) -> Result<CompletionList, LspError> {
        self.require_for(ServerFeature::Completion, "textDocument/completion", doc)?;
        let params = lsp_types::CompletionParams {
            text_document_position: text_document_position(doc, pos)?,
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: Some(completion_context(trigger)),
        };
        let response: Option<lsp_types::CompletionResponse> =
            self.conn.request("textDocument/completion", params).await?;
        Ok(convert::completions_from_lsp(
            response,
            self.supports(ServerFeature::CompletionResolve),
        ))
    }

    /// Fill in what the server deferred for `item` -- its documentation, its
    /// detail, its additional edits (`completionItem/resolve`).
    ///
    /// `None` when there is nothing to resolve: `item` has no payload, or the
    /// server does not resolve items. Otherwise the server's answer, mapped
    /// as a list item is -- every range in it UTF-16, its payload spent -- for
    /// the caller to fold into the item it holds with
    /// [`CompletionItem::adopt_resolved`], which keeps what accepting the item
    /// inserts.
    ///
    /// # Errors
    /// Returns [`LspError::Server`], [`LspError::Protocol`] (an answer that is
    /// not an item), or [`LspError::Timeout`].
    pub async fn resolve_completion(
        &self,
        item: &CompletionItem,
    ) -> Result<Option<CompletionItem>, LspError> {
        if !self.supports(ServerFeature::CompletionResolve) {
            return Ok(None);
        }
        let Some(literal) = item
            .payload
            .as_deref()
            .and_then(|payload| serde_json::from_str::<lsp_types::CompletionItem>(payload).ok())
        else {
            return Ok(None);
        };
        let resolved: lsp_types::CompletionItem =
            self.conn.request("completionItem/resolve", literal).await?;
        Ok(Some(convert::completion_item_from_lsp(resolved)))
    }
}

/// The wire context for `trigger`.
fn completion_context(trigger: CompletionTrigger) -> lsp_types::CompletionContext {
    let (trigger_kind, trigger_character) = match trigger {
        CompletionTrigger::Invoked => (lsp_types::CompletionTriggerKind::INVOKED, None),
        CompletionTrigger::Character(ch) => (
            lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
            Some(ch.to_string()),
        ),
        CompletionTrigger::Incomplete => (
            lsp_types::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS,
            None,
        ),
    };
    lsp_types::CompletionContext {
        trigger_kind,
        trigger_character,
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;

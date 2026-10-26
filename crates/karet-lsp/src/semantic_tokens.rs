//! Semantic tokens: a whole document's (`textDocument/semanticTokens/full`),
//! or the edits since the last answer (`…/full/delta`), decoded against the
//! connection's legend into neutral [`SemanticToken`]s.
//!
//! The wire form is a flat array of five integers per token -- line delta,
//! start delta, length, type index, modifier bits -- each token positioned
//! relative to the one before it. A delta answer edits that *array*, not the
//! tokens, so the array is what a [`SemanticTokens`] keeps: decoding is a
//! separate step a caller takes against the legend of the same connection.
//!
//! Types and modifiers are read by name through [`SemanticTokenType`] and
//! [`SemanticTokenModifiers`]: a token whose type is outside that vocabulary
//! is dropped (the tree-sitter highlight beneath it stays), and an unknown
//! modifier is ignored.

use std::path::Path;

use karet_core::LineCol;
use karet_core::SemanticToken;
use karet_core::SemanticTokenModifiers;
use karet_core::SemanticTokenType;
use karet_core::SemanticTokensLegend;
use karet_core::ServerFeature;
use karet_core::TokenId;
use serde::Deserialize;

use crate::LspClient;
use crate::LspError;
use crate::uri;

/// A document's semantic tokens as a server last answered them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticTokens {
    /// What the server calls this answer, for a later delta request; `None`
    /// when it gave none, so the next request asks for the whole document.
    pub result_id: Option<String>,
    /// The packed wire array, five integers per token, kept so a delta can
    /// be applied to it.
    pub data: Vec<u32>,
}

impl SemanticTokens {
    /// The tokens `data` encodes, read against `legend` -- which must be the
    /// legend of the connection that answered. Positions are the server's
    /// UTF-16 columns.
    ///
    /// A token of a type outside the vocabulary, or of no length, is dropped;
    /// a trailing partial group of fewer than five integers is ignored.
    #[must_use]
    pub fn decode(&self, legend: &SemanticTokensLegend) -> Vec<SemanticToken> {
        let types: Vec<Option<SemanticTokenType>> = legend
            .token_types
            .iter()
            .map(|name| SemanticTokenType::from_name(name))
            .collect();
        let modifiers: Vec<SemanticTokenModifiers> = legend
            .token_modifiers
            .iter()
            .map(|name| SemanticTokenModifiers::from_name(name).unwrap_or_default())
            .collect();
        let mut line = 0u32;
        let mut start = 0u32;
        let mut out = Vec::with_capacity(self.data.len() / 5);
        for group in self.data.chunks_exact(5) {
            let &[delta_line, delta_start, length, ty, bits] = group else {
                continue;
            };
            if delta_line == 0 {
                start = start.saturating_add(delta_start);
            } else {
                line = line.saturating_add(delta_line);
                start = delta_start;
            }
            let Some(ty) = usize::try_from(ty)
                .ok()
                .and_then(|index| types.get(index).copied().flatten())
            else {
                continue;
            };
            if length == 0 {
                continue;
            }
            let set = modifiers
                .iter()
                .enumerate()
                .filter(|(bit, _)| {
                    u32::try_from(*bit).is_ok_and(|bit| bit < 32 && bits >> bit & 1 == 1)
                })
                .fold(SemanticTokenModifiers::NONE, |set, (_, flag)| set | *flag);
            out.push(SemanticToken {
                start: LineCol::new(line, start),
                length,
                token: TokenId::semantic(ty, set),
            });
        }
        out
    }

    /// Apply a delta answer's `edits` to `data`, each naming a span of the
    /// array *as it was* (so they are applied from the last to the first).
    ///
    /// # Errors
    /// [`LspError::Protocol`] when an edit reaches past the array or two
    /// edits overlap: the result would describe text nobody sent, so the
    /// caller asks for the whole document instead.
    fn apply(&mut self, mut edits: Vec<WireEdit>) -> Result<(), LspError> {
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        let mut limit = self.data.len();
        for edit in edits {
            let start = edit.start as usize;
            let end = start
                .checked_add(edit.delete_count as usize)
                .filter(|end| *end <= limit)
                .ok_or_else(|| {
                    LspError::Protocol("semantic-token edit outside the previous result".into())
                })?;
            self.data.splice(start..end, edit.data);
            limit = start;
        }
        Ok(())
    }
}

/// A `full` or `full/delta` answer, read loosely: a delta answer carries
/// `edits`, a full one `data`, and either may name its `resultId`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTokens {
    #[serde(default)]
    result_id: Option<String>,
    #[serde(default)]
    data: Option<Vec<u32>>,
    #[serde(default)]
    edits: Option<Vec<WireEdit>>,
}

/// One edit of a delta answer, in units of the packed array.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEdit {
    start: u32,
    delete_count: u32,
    #[serde(default)]
    data: Vec<u32>,
}

impl LspClient {
    /// Request `doc`'s semantic tokens: as a delta against `previous` when
    /// it names a result and the server offers deltas, else whole.
    ///
    /// A server answering a delta request with a whole result, or with no
    /// result at all, is taken at its word. An answer of `null` is a
    /// document with no tokens.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise semantic tokens for `doc`;
    /// [`LspError::Protocol`] for a delta that does not fit `previous` (ask
    /// again without one), an unconvertible path, a request that does not
    /// encode or a result that does not decode; else [`LspError::Server`],
    /// [`LspError::Timeout`], or [`LspError::Closed`] if the connection is
    /// gone.
    pub async fn semantic_tokens(
        &self,
        doc: &Path,
        previous: Option<&SemanticTokens>,
    ) -> Result<SemanticTokens, LspError> {
        self.require_for(
            ServerFeature::SemanticTokensFull,
            "textDocument/semanticTokens/full",
            doc,
        )?;
        let text_document = lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?);
        let delta_from = previous.filter(|previous| {
            previous.result_id.is_some()
                && self.supports_for(ServerFeature::SemanticTokensDelta, doc)
        });
        let answer: Option<WireTokens> = match delta_from.and_then(|p| p.result_id.clone()) {
            Some(previous_result_id) => {
                let params = lsp_types::SemanticTokensDeltaParams {
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                    partial_result_params: lsp_types::PartialResultParams::default(),
                    text_document,
                    previous_result_id,
                };
                self.conn
                    .request("textDocument/semanticTokens/full/delta", params)
                    .await?
            },
            None => {
                let params = lsp_types::SemanticTokensParams {
                    work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
                    partial_result_params: lsp_types::PartialResultParams::default(),
                    text_document,
                };
                self.conn
                    .request("textDocument/semanticTokens/full", params)
                    .await?
            },
        };
        let Some(answer) = answer else {
            return Ok(SemanticTokens::default());
        };
        match (answer.edits, delta_from) {
            (Some(edits), Some(previous)) if answer.data.is_none() => {
                let mut next = SemanticTokens {
                    result_id: answer.result_id,
                    data: previous.data.clone(),
                };
                next.apply(edits)?;
                Ok(next)
            },
            (Some(_), None) if answer.data.is_none() => Err(LspError::Protocol(
                "semantic-token edits answered a request for the whole document".into(),
            )),
            _ => Ok(SemanticTokens {
                result_id: answer.result_id,
                data: answer.data.unwrap_or_default(),
            }),
        }
    }

    /// The legend this connection's semantic tokens are decoded against, if
    /// the server emits semantic tokens.
    #[must_use]
    pub fn semantic_tokens_legend(&self) -> Option<SemanticTokensLegend> {
        self.capabilities().semantic_tokens_legend
    }
}

#[cfg(test)]
#[path = "semantic_tokens_tests.rs"]
mod tests;

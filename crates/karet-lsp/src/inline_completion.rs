//! Inline completions (LSP 3.18 `textDocument/inlineCompletion`): text a
//! server proposes at the caret, for an editor to show as ghost text, and
//! the command an accepted item asks to have run.
//!
//! The wire types are this module's own rather than `lsp_types`'. 0.97 has
//! them only behind its `proposed` feature, which also adds fields to
//! `ClientCapabilities` and `ServerCapabilities`; the capability is read from
//! raw JSON for the same reason (see [`crate::capability`]).

use std::path::Path;

use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::ServerFeature;
use serde::Deserialize;
use serde::Serialize;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::snippet;
use crate::uri;

/// The method this module speaks.
const METHOD: &str = "textDocument/inlineCompletion";

/// `InlineCompletionParams`, less the work-done token karet never sends.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Params {
    text_document: lsp_types::TextDocumentIdentifier,
    position: lsp_types::Position,
    context: Context,
}

/// `InlineCompletionContext`. `selectedCompletionInfo` is never sent: the
/// completion popup and ghost text are not shown together.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    trigger_kind: u8,
}

/// The answer: a bare array of items, or a list holding one.
#[derive(Deserialize)]
#[serde(untagged)]
enum Response {
    Items(Vec<WireItem>),
    List { items: Vec<WireItem> },
}

/// `InlineCompletionItem`, keeping what karet does not interpret as it came,
/// so the item can be handed back exactly.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct WireItem {
    /// `string | StringValue`: a plain string, or a snippet object.
    insert_text: InsertText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filter_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<lsp_types::Range>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<lsp_types::Command>,
    /// Not in the 3.18 draft that settled on `StringValue`, but sent by
    /// servers built against the earlier one (`lsp_types`' shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    insert_text_format: Option<lsp_types::InsertTextFormat>,
}

/// An item's `insertText`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
enum InsertText {
    Plain(String),
    /// `StringValue`: `{ kind: "snippet", value }`.
    Snippet {
        kind: String,
        value: String,
    },
}

impl LspClient {
    /// Ask for inline completions at `position` in `doc` (UTF-16 columns).
    ///
    /// Each item's text is what an accept inserts: line breaks are `\n`, and
    /// a snippet is flattened to the text it shows before anyone edits it.
    /// Its range is UTF-16, like every position crossing this crate.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not offer inline completions for `doc`, else
    /// [`LspError::Server`], [`LspError::Timeout`], [`LspError::Protocol`]
    /// for an unconvertible path or an answer that does not decode, or
    /// [`LspError::Closed`] if the connection is gone.
    pub async fn inline_completion(
        &self,
        doc: &Path,
        position: LineCol,
        trigger: InlineCompletionTrigger,
    ) -> Result<Vec<InlineCompletionItem>, LspError> {
        self.require_for(ServerFeature::InlineCompletion, METHOD, doc)?;
        let params = Params {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            position: convert::position_to_lsp(position),
            context: Context {
                trigger_kind: match trigger {
                    InlineCompletionTrigger::Invoked => 1,
                    _ => 2,
                },
            },
        };
        let response: Option<Response> = self.conn.request(METHOD, params).await?;
        Ok(items_from_lsp(response, position))
    }

    /// Tell the server `item` was accepted, by running the command it carries
    /// (`workspace/executeCommand`) -- which LSP says to do after inserting
    /// it, and which servers such as Copilot use to learn what was taken.
    /// Nothing when the item carries no command, or the server did not list
    /// it among those it runs.
    ///
    /// # Errors
    /// As [`Self::execute_command`], except that a command the server does not
    /// run is not an error.
    pub async fn accept_inline_completion(
        &self,
        item: &InlineCompletionItem,
    ) -> Result<(), LspError> {
        let Some(command) = wire_item(item).and_then(|wire| wire.command) else {
            return Ok(());
        };
        match self
            .execute_command(&command.command, command.arguments.unwrap_or_default())
            .await
        {
            Err(LspError::Unsupported { .. }) => Ok(()),
            outcome => outcome.map(drop),
        }
    }
}

/// Map an answer to an ask at `asked` into neutral items, dropping any whose
/// text is empty. An item without a range inserts at `asked`.
fn items_from_lsp(response: Option<Response>, asked: LineCol) -> Vec<InlineCompletionItem> {
    let items = match response {
        None => Vec::new(),
        Some(Response::Items(items) | Response::List { items }) => items,
    };
    items
        .into_iter()
        .filter_map(|wire| item_from_lsp(wire, asked))
        .collect()
}

fn item_from_lsp(wire: WireItem, asked: LineCol) -> Option<InlineCompletionItem> {
    let snippet_format = wire.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET);
    let text = match &wire.insert_text {
        InsertText::Snippet { kind, value } if kind == "snippet" => {
            snippet::parse_snippet(value).text
        },
        InsertText::Plain(value) if snippet_format => snippet::parse_snippet(value).text,
        InsertText::Plain(value) | InsertText::Snippet { value, .. } => value.clone(),
    };
    let insert_text = text.replace("\r\n", "\n").replace('\r', "\n");
    if insert_text.is_empty() {
        return None;
    }
    // Only an item with something to hand back keeps its wire form.
    let payload = wire
        .command
        .is_some()
        .then(|| serde_json::to_string(&wire).ok())
        .flatten();
    Some(InlineCompletionItem {
        insert_text,
        range: wire.range.map_or(
            Range {
                start: asked,
                end: asked,
            },
            convert::range_from_lsp,
        ),
        filter_text: wire.filter_text,
        payload,
    })
}

/// The wire item a payload holds.
fn wire_item(item: &InlineCompletionItem) -> Option<WireItem> {
    serde_json::from_str(item.payload.as_deref()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the asks in these tests were made.
    const ASKED: LineCol = LineCol { line: 7, col: 3 };

    fn items(json: serde_json::Value) -> Result<Vec<InlineCompletionItem>, serde_json::Error> {
        Ok(items_from_lsp(Some(serde_json::from_value(json)?), ASKED))
    }

    /// An item without a range inserts where the ask was made.
    #[test]
    fn a_list_and_a_bare_array_both_decode() -> Result<(), serde_json::Error> {
        let from_list = items(serde_json::json!({"items": [{"insertText": "a"}]}))?;
        let from_array = items(serde_json::json!([{"insertText": "a"}]))?;
        assert_eq!(from_list, vec![InlineCompletionItem::new("a", ASKED)]);
        assert_eq!(from_array, from_list);
        assert!(items_from_lsp(None, ASKED).is_empty(), "null is no items");
        Ok(())
    }

    /// The Copilot shape: a whole-line range and an acceptance command, which
    /// survives as the payload an accept hands back.
    #[test]
    fn range_and_command_survive_and_line_breaks_normalize() -> Result<(), serde_json::Error> {
        let mapped = items(serde_json::json!({"items": [{
            "insertText": "fn a() {\r\n}",
            "range": {"start": {"line": 1, "character": 0},
                      "end": {"line": 1, "character": 2}},
            "command": {"title": "", "command": "github.copilot.didAcceptCompletionItem",
                        "arguments": ["id-1"]}
        }]}))?;
        let item = mapped.first().cloned().unwrap_or_default();
        assert_eq!(item.insert_text, "fn a() {\n}");
        assert_eq!(
            item.range,
            Range {
                start: LineCol::new(1, 0),
                end: LineCol::new(1, 2),
            }
        );
        let command = wire_item(&item).and_then(|wire| wire.command);
        assert_eq!(
            command.map(|command| (command.command, command.arguments)),
            Some((
                "github.copilot.didAcceptCompletionItem".to_owned(),
                Some(vec![serde_json::json!("id-1")])
            ))
        );
        Ok(())
    }

    #[test]
    fn snippets_in_either_shape_flatten_to_their_text() -> Result<(), serde_json::Error> {
        let mapped = items(serde_json::json!([
            {"insertText": {"kind": "snippet", "value": "foo(${1:x})$0"}},
            {"insertText": "bar(${1:y})", "insertTextFormat": 2},
            {"insertText": "plain $1"},
        ]))?;
        let texts: Vec<&str> = mapped
            .iter()
            .map(|item| item.insert_text.as_str())
            .collect();
        assert_eq!(texts, ["foo(x)", "bar(y)", "plain $1"]);
        assert!(
            mapped.iter().all(|item| item.payload.is_none()),
            "nothing to hand back without a command"
        );
        Ok(())
    }

    #[test]
    fn an_empty_item_is_dropped() -> Result<(), serde_json::Error> {
        assert!(items(serde_json::json!([{"insertText": ""}]))?.is_empty());
        Ok(())
    }

    #[test]
    fn the_request_names_its_trigger_kind() -> Result<(), serde_json::Error> {
        let params = Params {
            text_document: lsp_types::TextDocumentIdentifier::new(
                "file:///a.rs".parse().map_err(serde::de::Error::custom)?,
            ),
            position: lsp_types::Position::new(3, 4),
            context: Context { trigger_kind: 2 },
        };
        assert_eq!(
            serde_json::to_value(params)?,
            serde_json::json!({
                "textDocument": {"uri": "file:///a.rs"},
                "position": {"line": 3, "character": 4},
                "context": {"triggerKind": 2}
            })
        );
        Ok(())
    }
}

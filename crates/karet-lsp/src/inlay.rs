//! Inlay hints: asking for a range's hints (`textDocument/inlayHint`), and
//! filling in what a server deferred for one of them (`inlayHint/resolve`).
//!
//! A label arrives either as one string or as parts, each of which may carry
//! its own tooltip and a location to follow. The parts are kept, so a
//! consumer can hover and follow each; the joined label is kept beside them
//! for a consumer that only paints. A hint the server can say more about
//! later carries `data`, and keeps its wire form as the payload a resolve
//! hands back, untouched, as LSP requires.

use std::path::Path;

use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::InlayHintLabelPart;
use karet_core::Location;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Range;
use karet_core::ServerFeature;

use crate::LspClient;
use crate::LspError;
use crate::convert;
use crate::uri;

impl LspClient {
    /// Request inlay hints within `range`.
    ///
    /// # Errors
    /// Returns [`LspError::Unsupported`] **without issuing a request** when the
    /// server did not advertise the capability, else [`LspError::Server`] or
    /// [`LspError::Timeout`].
    pub async fn inlay_hints(&self, doc: &Path, range: Range) -> Result<Vec<InlayHint>, LspError> {
        self.require_for(ServerFeature::InlayHint, "textDocument/inlayHint", doc)?;
        let params = lsp_types::InlayHintParams {
            text_document: lsp_types::TextDocumentIdentifier::new(uri::path_to_uri(doc)?),
            range: convert::range_to_lsp(range),
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
        };
        let response: Option<Vec<lsp_types::InlayHint>> =
            self.conn.request("textDocument/inlayHint", params).await?;
        Ok(inlay_hints_from_lsp(response))
    }

    /// Fill in what the server deferred for `hint` -- its tooltip, its
    /// parts' tooltips and locations (`inlayHint/resolve`).
    ///
    /// The hint comes back as it went in when there is nothing to resolve: it
    /// has no payload (the server gave it no `data`), or the server does not
    /// resolve hints. The answer keeps `hint`'s own position -- a resolve
    /// fills a hint in, it does not move it -- so whatever columns the caller
    /// holds it in stay true; its parts' locations are UTF-16, like every
    /// other location.
    ///
    /// # Errors
    /// Returns [`LspError::Server`], [`LspError::Protocol`] (an answer that is
    /// not a hint), or [`LspError::Timeout`].
    pub async fn resolve_inlay_hint(&self, hint: &InlayHint) -> Result<InlayHint, LspError> {
        if !self.supports(ServerFeature::InlayHintResolve) {
            return Ok(hint.clone());
        }
        let Some(literal) = inlay_hint_literal(hint) else {
            return Ok(hint.clone());
        };
        let resolved: lsp_types::InlayHint =
            self.conn.request("inlayHint/resolve", literal).await?;
        Ok(InlayHint {
            position: hint.position,
            ..inlay_hint_from_lsp(resolved)
        })
    }
}

/// Map inlay hints into renderable labels and positions.
pub(crate) fn inlay_hints_from_lsp(hints: Option<Vec<lsp_types::InlayHint>>) -> Vec<InlayHint> {
    hints
        .unwrap_or_default()
        .into_iter()
        .map(inlay_hint_from_lsp)
        .collect()
}

/// Map one hint, keeping its label parts, its tooltip, and -- when the server
/// gave it `data` to resolve with -- its wire form as the payload.
pub(crate) fn inlay_hint_from_lsp(hint: lsp_types::InlayHint) -> InlayHint {
    let payload = hint
        .data
        .is_some()
        .then(|| serde_json::to_string(&hint).ok())
        .flatten();
    let (label, parts) = match hint.label {
        lsp_types::InlayHintLabel::String(label) => (label, Vec::new()),
        lsp_types::InlayHintLabel::LabelParts(parts) => {
            let label = parts.iter().map(|part| part.value.as_str()).collect();
            (label, parts.into_iter().map(label_part_from_lsp).collect())
        },
    };
    InlayHint {
        position: convert::position_from_lsp(hint.position),
        label,
        kind: match hint.kind {
            Some(lsp_types::InlayHintKind::TYPE) => InlayHintKind::Type,
            Some(lsp_types::InlayHintKind::PARAMETER) => InlayHintKind::Parameter,
            _ => InlayHintKind::Other,
        },
        padding_left: hint.padding_left.unwrap_or(false),
        padding_right: hint.padding_right.unwrap_or(false),
        parts,
        tooltip: hint.tooltip.map(|tooltip| match tooltip {
            lsp_types::InlayHintTooltip::String(value) => plain(value),
            lsp_types::InlayHintTooltip::MarkupContent(content) => markup(content),
        }),
        payload,
    }
}

/// The hint literal a payload holds, for `inlayHint/resolve`.
pub(crate) fn inlay_hint_literal(hint: &InlayHint) -> Option<lsp_types::InlayHint> {
    serde_json::from_str(hint.payload.as_deref()?).ok()
}

fn label_part_from_lsp(part: lsp_types::InlayHintLabelPart) -> InlayHintLabelPart {
    InlayHintLabelPart {
        value: part.value,
        tooltip: part.tooltip.map(|tooltip| match tooltip {
            lsp_types::InlayHintLabelPartTooltip::String(value) => plain(value),
            lsp_types::InlayHintLabelPartTooltip::MarkupContent(content) => markup(content),
        }),
        // A location outside `file://` cannot be opened, so the part is just
        // text, as it would be had the server sent none.
        location: part.location.and_then(|location| {
            Some(Location {
                path: uri::uri_to_path(&location.uri)?,
                range: convert::range_from_lsp(location.range),
            })
        }),
    }
}

fn plain(value: String) -> Markup {
    Markup {
        kind: MarkupKind::PlainText,
        value,
    }
}

fn markup(content: lsp_types::MarkupContent) -> Markup {
    convert::markup_from_lsp(lsp_types::Documentation::MarkupContent(content))
}

#[cfg(test)]
#[path = "inlay_tests.rs"]
mod wire_tests;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_core::LineCol;

    use super::*;

    fn hints(json: serde_json::Value) -> Result<Vec<InlayHint>, serde_json::Error> {
        Ok(inlay_hints_from_lsp(Some(serde_json::from_value(json)?)))
    }

    #[test]
    fn kinds_map_and_a_missing_kind_is_other_not_type() -> Result<(), serde_json::Error> {
        let mapped = hints(serde_json::json!([
            {"position": {"line": 0, "character": 1}, "label": "a", "kind": 1},
            {"position": {"line": 0, "character": 1}, "label": "b", "kind": 2},
            {"position": {"line": 0, "character": 1}, "label": "c"},
        ]))?;
        let kinds: Vec<InlayHintKind> = mapped.iter().map(|hint| hint.kind).collect();
        assert_eq!(
            kinds,
            [
                InlayHintKind::Type,
                InlayHintKind::Parameter,
                InlayHintKind::Other
            ]
        );
        Ok(())
    }

    #[test]
    fn label_parts_keep_their_tooltips_and_locations() -> Result<(), serde_json::Error> {
        let mapped = hints(serde_json::json!([{
            "position": {"line": 2, "character": 7},
            "label": [
                {"value": ": "},
                {"value": "Vec", "tooltip": "struct Vec",
                 "location": {"uri": "file:///tmp/vec.rs",
                              "range": {"start": {"line": 40, "character": 11},
                                        "end": {"line": 40, "character": 14}}}},
                {"value": "<i32>", "tooltip": {"kind": "markdown", "value": "**i32**"}},
                {"value": "!", "location": {"uri": "untitled:1",
                              "range": {"start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 0}}}}
            ],
            "tooltip": "the whole hint"
        }]))?;
        let hint = mapped.first().cloned().unwrap_or_else(|| {
            InlayHint::new(LineCol::default(), "missing", InlayHintKind::Unknown)
        });
        assert_eq!(hint.label, ": Vec<i32>!");
        assert_eq!(hint.parts.len(), 4);
        assert_eq!(hint.tooltip, Some(plain("the whole hint".to_owned())));
        assert_eq!(hint.tooltip_for(1), Some(&plain("struct Vec".to_owned())));
        assert_eq!(
            hint.tooltip_for(2)
                .map(|markup| (markup.kind, markup.value.as_str())),
            Some((MarkupKind::Markdown, "**i32**"))
        );
        let location = hint.location_for(1);
        assert_eq!(
            location.map(|l| l.path.clone()),
            Some(PathBuf::from("/tmp/vec.rs"))
        );
        // Still UTF-16: the session converts it against the file it names.
        assert_eq!(location.map(|l| l.range.start), Some(LineCol::new(40, 11)));
        assert_eq!(hint.location_for(3), None, "a non-file location is dropped");
        assert_eq!(hint.payload, None, "nothing to resolve without data");
        Ok(())
    }

    /// The payload is the hint exactly as sent, `data` included, so a resolve
    /// hands the server back what it gave.
    #[test]
    fn a_hint_with_data_keeps_its_wire_form_for_resolve() -> Result<(), serde_json::Error> {
        let mapped = hints(serde_json::json!([{
            "position": {"line": 1, "character": 3},
            "label": [{"value": "x"}],
            "data": {"id": 42}
        }]))?;
        let literal = mapped.first().and_then(inlay_hint_literal);
        assert_eq!(
            literal.as_ref().and_then(|hint| hint.data.clone()),
            Some(serde_json::json!({"id": 42}))
        );
        assert_eq!(
            literal.map(|hint| hint.position),
            Some(lsp_types::Position {
                line: 1,
                character: 3
            })
        );
        Ok(())
    }
}

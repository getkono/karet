//! LSP wire types → neutral `karet-core` models.
//!
//! Positions cross this boundary **unconverted**: karet-lsp is faithful to the
//! negotiated LSP encoding (UTF-16), so an `lsp_types::Position`'s `character`
//! becomes a [`LineCol::col`] still counted in UTF-16 code units. The consumer
//! (karet-session) owns the text and performs the UTF-16 ↔ UTF-32 translation via
//! `karet_text::TextBuffer`.

use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::Range;
use karet_core::RelatedInfo;
use karet_core::Severity;
use karet_core::TextEdit;

use crate::snippet::strip_snippet;
use crate::uri::uri_to_path;

/// Map an LSP position (UTF-16 columns, passed through — see the module docs).
pub(crate) fn position_from_lsp(p: lsp_types::Position) -> LineCol {
    LineCol::new(p.line, p.character)
}

/// Map a karet position to LSP (UTF-16 columns, passed through unchanged).
pub(crate) fn position_to_lsp(p: LineCol) -> lsp_types::Position {
    lsp_types::Position {
        line: p.line,
        character: p.col,
    }
}

/// Map an LSP range, normalizing any (out-of-spec) reversed endpoints.
pub(crate) fn range_from_lsp(r: lsp_types::Range) -> Range {
    let a = position_from_lsp(r.start);
    let b = position_from_lsp(r.end);
    Range {
        start: a.min(b),
        end: a.max(b),
    }
}

/// Map an LSP severity; an absent severity is treated as an error, matching the
/// common client interpretation.
pub(crate) fn severity_from_lsp(s: Option<lsp_types::DiagnosticSeverity>) -> Severity {
    match s {
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => Severity::Information,
        Some(lsp_types::DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

/// Map one published diagnostic. Related locations whose URIs are not `file://`
/// are dropped (karet models locations as paths).
pub(crate) fn diagnostic_from_lsp(d: lsp_types::Diagnostic) -> Diagnostic {
    Diagnostic {
        range: range_from_lsp(d.range),
        severity: severity_from_lsp(d.severity),
        message: d.message,
        source: d.source,
        code: d.code.map(|c| match c {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s,
        }),
        tags: d
            .tags
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| match t {
                lsp_types::DiagnosticTag::UNNECESSARY => Some(DiagnosticTag::Unnecessary),
                lsp_types::DiagnosticTag::DEPRECATED => Some(DiagnosticTag::Deprecated),
                _ => None,
            })
            .collect(),
        related: d
            .related_information
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                Some(RelatedInfo {
                    location: Location {
                        path: uri_to_path(&r.location.uri)?,
                        range: range_from_lsp(r.location.range),
                    },
                    message: r.message,
                })
            })
            .collect(),
    }
}

/// Flatten a completion response to a plain item list.
///
/// `CompletionList.isIncomplete` is deliberately dropped: the pinned
/// `completion()` contract returns `Vec<CompletionItem>`, and the consumer's
/// strategy is to re-request on trigger characters rather than track
/// incompleteness (see the method docs on `LspClient::completion`).
pub(crate) fn completions_from_lsp(
    response: Option<lsp_types::CompletionResponse>,
) -> Vec<CompletionItem> {
    let items = match response {
        None => return Vec::new(),
        Some(lsp_types::CompletionResponse::Array(items)) => items,
        Some(lsp_types::CompletionResponse::List(list)) => list.items,
    };
    items.into_iter().map(completion_item_from_lsp).collect()
}

/// Map one completion item.
///
/// - **Insert text** resolves per LSP precedence: `textEdit.newText`, else
///   `insertText`, else the label. Snippet-format text is degraded to plain
///   text (see [`crate::snippet`]).
/// - **`textEdit`** maps to a neutral [`TextEdit`] (UTF-16 range passthrough);
///   an insert/replace edit uses its *insert* range — the conservative choice
///   that replaces only the typed prefix.
/// - **`filterText`** has no slot on the neutral model and is dropped; karet
///   filters on the label.
/// - **Deprecation** is the union of the legacy `deprecated` flag and the
///   `Deprecated` tag.
pub(crate) fn completion_item_from_lsp(item: lsp_types::CompletionItem) -> CompletionItem {
    let is_snippet = item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET);
    let degrade = |text: String| {
        if is_snippet {
            strip_snippet(&text)
        } else {
            text
        }
    };
    let edit = item.text_edit.map(|te| match te {
        lsp_types::CompletionTextEdit::Edit(e) => TextEdit {
            range: range_from_lsp(e.range),
            new_text: degrade(e.new_text),
        },
        lsp_types::CompletionTextEdit::InsertAndReplace(e) => TextEdit {
            range: range_from_lsp(e.insert),
            new_text: degrade(e.new_text),
        },
    });
    let insert_text = match (&edit, item.insert_text) {
        (Some(e), _) => e.new_text.clone(),
        (None, Some(text)) => degrade(text),
        (None, None) => item.label.clone(),
    };
    let deprecated = item.deprecated.unwrap_or(false)
        || item
            .tags
            .unwrap_or_default()
            .contains(&lsp_types::CompletionItemTag::DEPRECATED);
    CompletionItem {
        label: item.label,
        kind: completion_kind_from_lsp(item.kind),
        detail: item.detail,
        documentation: item.documentation.map(markup_from_lsp),
        insert_text,
        edit,
        sort_text: item.sort_text,
        deprecated,
    }
}

/// Map an LSP completion kind onto karet's smaller vocabulary; kinds with no
/// counterpart degrade to the nearest concept (constructor → function,
/// value/unit/enum-member → constant, event → field, operator → keyword,
/// type-parameter → class) and the purely-editor kinds (file, folder, color,
/// reference) to plain text.
pub(crate) fn completion_kind_from_lsp(
    kind: Option<lsp_types::CompletionItemKind>,
) -> CompletionKind {
    match kind {
        Some(lsp_types::CompletionItemKind::METHOD) => CompletionKind::Method,
        Some(
            lsp_types::CompletionItemKind::FUNCTION | lsp_types::CompletionItemKind::CONSTRUCTOR,
        ) => CompletionKind::Function,
        Some(lsp_types::CompletionItemKind::FIELD | lsp_types::CompletionItemKind::EVENT) => {
            CompletionKind::Field
        },
        Some(lsp_types::CompletionItemKind::VARIABLE) => CompletionKind::Variable,
        Some(
            lsp_types::CompletionItemKind::CLASS | lsp_types::CompletionItemKind::TYPE_PARAMETER,
        ) => CompletionKind::Class,
        Some(lsp_types::CompletionItemKind::INTERFACE) => CompletionKind::Interface,
        Some(lsp_types::CompletionItemKind::MODULE) => CompletionKind::Module,
        Some(lsp_types::CompletionItemKind::PROPERTY) => CompletionKind::Property,
        Some(lsp_types::CompletionItemKind::KEYWORD | lsp_types::CompletionItemKind::OPERATOR) => {
            CompletionKind::Keyword
        },
        Some(lsp_types::CompletionItemKind::SNIPPET) => CompletionKind::Snippet,
        Some(
            lsp_types::CompletionItemKind::CONSTANT
            | lsp_types::CompletionItemKind::VALUE
            | lsp_types::CompletionItemKind::UNIT
            | lsp_types::CompletionItemKind::ENUM_MEMBER,
        ) => CompletionKind::Constant,
        Some(lsp_types::CompletionItemKind::STRUCT) => CompletionKind::Struct,
        Some(lsp_types::CompletionItemKind::ENUM) => CompletionKind::Enum,
        _ => CompletionKind::Text,
    }
}

/// Map LSP documentation (a bare string is plain text).
pub(crate) fn markup_from_lsp(doc: lsp_types::Documentation) -> Markup {
    match doc {
        lsp_types::Documentation::String(value) => Markup {
            kind: MarkupKind::PlainText,
            value,
        },
        lsp_types::Documentation::MarkupContent(mc) => Markup {
            kind: match mc.kind {
                lsp_types::MarkupKind::PlainText => MarkupKind::PlainText,
                lsp_types::MarkupKind::Markdown => MarkupKind::Markdown,
            },
            value: mc.value,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::str::FromStr;

    use super::*;

    fn lsp_range(sl: u32, sc: u32, el: u32, ec: u32) -> lsp_types::Range {
        lsp_types::Range {
            start: lsp_types::Position {
                line: sl,
                character: sc,
            },
            end: lsp_types::Position {
                line: el,
                character: ec,
            },
        }
    }

    #[test]
    fn positions_pass_through_and_reversed_ranges_normalize() {
        assert_eq!(
            range_from_lsp(lsp_range(1, 2, 3, 4)),
            Range {
                start: LineCol::new(1, 2),
                end: LineCol::new(3, 4),
            }
        );
        // Reversed endpoints (seen from buggy servers) are normalized.
        assert_eq!(
            range_from_lsp(lsp_range(3, 4, 1, 2)),
            Range {
                start: LineCol::new(1, 2),
                end: LineCol::new(3, 4),
            }
        );
    }

    #[test]
    fn severity_defaults_to_error() {
        assert_eq!(severity_from_lsp(None), Severity::Error);
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::ERROR)),
            Severity::Error
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::WARNING)),
            Severity::Warning
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::INFORMATION)),
            Severity::Information
        );
        assert_eq!(
            severity_from_lsp(Some(lsp_types::DiagnosticSeverity::HINT)),
            Severity::Hint
        );
    }

    #[test]
    fn maps_a_full_diagnostic() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let lsp = lsp_types::Diagnostic {
            range: lsp_range(0, 1, 0, 5),
            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
            code: Some(lsp_types::NumberOrString::String("E0308".into())),
            source: Some("rustc".into()),
            message: "mismatched types".into(),
            tags: Some(vec![
                lsp_types::DiagnosticTag::UNNECESSARY,
                lsp_types::DiagnosticTag::DEPRECATED,
            ]),
            related_information: Some(vec![
                lsp_types::DiagnosticRelatedInformation {
                    location: lsp_types::Location {
                        uri: lsp_types::Uri::from_str("file:///src/lib.rs")?,
                        range: lsp_range(9, 0, 9, 3),
                    },
                    message: "expected due to this".into(),
                },
                // Non-file URIs are dropped.
                lsp_types::DiagnosticRelatedInformation {
                    location: lsp_types::Location {
                        uri: lsp_types::Uri::from_str("untitled:Untitled-1")?,
                        range: lsp_range(0, 0, 0, 0),
                    },
                    message: "ignored".into(),
                },
            ]),
            ..lsp_types::Diagnostic::default()
        };
        let core = diagnostic_from_lsp(lsp);
        assert_eq!(core.severity, Severity::Warning);
        assert_eq!(core.message, "mismatched types");
        assert_eq!(core.source.as_deref(), Some("rustc"));
        assert_eq!(core.code.as_deref(), Some("E0308"));
        assert_eq!(
            core.tags,
            vec![DiagnosticTag::Unnecessary, DiagnosticTag::Deprecated]
        );
        assert_eq!(core.related.len(), 1);
        assert_eq!(core.related[0].location.path, PathBuf::from("/src/lib.rs"));
        assert_eq!(core.related[0].message, "expected due to this");
        Ok(())
    }

    #[test]
    fn numeric_codes_become_strings() {
        let lsp = lsp_types::Diagnostic {
            range: lsp_range(0, 0, 0, 1),
            code: Some(lsp_types::NumberOrString::Number(404)),
            message: "x".into(),
            ..lsp_types::Diagnostic::default()
        };
        assert_eq!(diagnostic_from_lsp(lsp).code.as_deref(), Some("404"));
    }

    fn bare_item(label: &str) -> lsp_types::CompletionItem {
        lsp_types::CompletionItem {
            label: label.to_owned(),
            ..lsp_types::CompletionItem::default()
        }
    }

    #[test]
    fn empty_and_list_and_array_responses_flatten() {
        assert!(completions_from_lsp(None).is_empty());
        let array = lsp_types::CompletionResponse::Array(vec![bare_item("a")]);
        assert_eq!(completions_from_lsp(Some(array)).len(), 1);
        // `isIncomplete` is flattened away by design.
        let list = lsp_types::CompletionResponse::List(lsp_types::CompletionList {
            is_incomplete: true,
            items: vec![bare_item("b"), bare_item("c")],
        });
        let mapped = completions_from_lsp(Some(list));
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].label, "b");
    }

    #[test]
    fn insert_text_resolution_precedence() {
        // textEdit wins over insertText and label.
        let mut item = bare_item("label");
        item.insert_text = Some("insert".into());
        item.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: lsp_range(0, 2, 0, 5),
            new_text: "edited".into(),
        }));
        let mapped = completion_item_from_lsp(item);
        assert_eq!(mapped.insert_text, "edited");
        assert_eq!(
            mapped.edit,
            Some(TextEdit {
                range: Range {
                    start: LineCol::new(0, 2),
                    end: LineCol::new(0, 5),
                },
                new_text: "edited".into(),
            })
        );

        // insertText wins over label.
        let mut item = bare_item("label");
        item.insert_text = Some("insert".into());
        let mapped = completion_item_from_lsp(item);
        assert_eq!(mapped.insert_text, "insert");
        assert_eq!(mapped.edit, None);

        // The label is the last resort.
        assert_eq!(
            completion_item_from_lsp(bare_item("label")).insert_text,
            "label"
        );
    }

    #[test]
    fn snippet_format_degrades_everywhere() {
        let mut item = bare_item("push");
        item.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
        item.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
            range: lsp_range(1, 4, 1, 6),
            new_text: "push(${1:ch})$0".into(),
        }));
        let mapped = completion_item_from_lsp(item);
        assert_eq!(mapped.insert_text, "push(ch)");
        assert_eq!(mapped.edit.map(|e| e.new_text), Some("push(ch)".into()));

        let mut item = bare_item("if");
        item.insert_text_format = Some(lsp_types::InsertTextFormat::SNIPPET);
        item.insert_text = Some("if ${1:cond} {\n\t$0\n}".into());
        assert_eq!(
            completion_item_from_lsp(item).insert_text,
            "if cond {\n\t\n}"
        );

        // Plain-text format is left untouched even if it looks snippety.
        let mut item = bare_item("x");
        item.insert_text = Some("literal $1".into());
        assert_eq!(completion_item_from_lsp(item).insert_text, "literal $1");
    }

    #[test]
    fn insert_and_replace_uses_the_insert_range() {
        let mut item = bare_item("frobnicate");
        item.text_edit = Some(lsp_types::CompletionTextEdit::InsertAndReplace(
            lsp_types::InsertReplaceEdit {
                new_text: "frobnicate".into(),
                insert: lsp_range(0, 4, 0, 7),
                replace: lsp_range(0, 4, 0, 12),
            },
        ));
        let mapped = completion_item_from_lsp(item);
        assert_eq!(
            mapped.edit.map(|e| e.range),
            Some(Range {
                start: LineCol::new(0, 4),
                end: LineCol::new(0, 7),
            })
        );
    }

    #[test]
    fn kinds_map_onto_the_smaller_vocabulary() {
        use lsp_types::CompletionItemKind as K;
        let table = [
            (Some(K::TEXT), CompletionKind::Text),
            (Some(K::METHOD), CompletionKind::Method),
            (Some(K::FUNCTION), CompletionKind::Function),
            (Some(K::CONSTRUCTOR), CompletionKind::Function),
            (Some(K::FIELD), CompletionKind::Field),
            (Some(K::EVENT), CompletionKind::Field),
            (Some(K::VARIABLE), CompletionKind::Variable),
            (Some(K::CLASS), CompletionKind::Class),
            (Some(K::TYPE_PARAMETER), CompletionKind::Class),
            (Some(K::INTERFACE), CompletionKind::Interface),
            (Some(K::MODULE), CompletionKind::Module),
            (Some(K::PROPERTY), CompletionKind::Property),
            (Some(K::KEYWORD), CompletionKind::Keyword),
            (Some(K::OPERATOR), CompletionKind::Keyword),
            (Some(K::SNIPPET), CompletionKind::Snippet),
            (Some(K::CONSTANT), CompletionKind::Constant),
            (Some(K::VALUE), CompletionKind::Constant),
            (Some(K::UNIT), CompletionKind::Constant),
            (Some(K::ENUM_MEMBER), CompletionKind::Constant),
            (Some(K::STRUCT), CompletionKind::Struct),
            (Some(K::ENUM), CompletionKind::Enum),
            (Some(K::FILE), CompletionKind::Text),
            (Some(K::FOLDER), CompletionKind::Text),
            (Some(K::COLOR), CompletionKind::Text),
            (Some(K::REFERENCE), CompletionKind::Text),
            (None, CompletionKind::Text),
        ];
        for (lsp, expected) in table {
            assert_eq!(completion_kind_from_lsp(lsp), expected, "for {lsp:?}");
        }
    }

    #[test]
    fn documentation_maps_both_flavors() {
        let mut item = bare_item("a");
        item.documentation = Some(lsp_types::Documentation::String("plain docs".into()));
        assert_eq!(
            completion_item_from_lsp(item).documentation,
            Some(Markup {
                kind: MarkupKind::PlainText,
                value: "plain docs".into(),
            })
        );

        let mut item = bare_item("b");
        item.documentation = Some(lsp_types::Documentation::MarkupContent(
            lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: "# md".into(),
            },
        ));
        assert_eq!(
            completion_item_from_lsp(item).documentation,
            Some(Markup {
                kind: MarkupKind::Markdown,
                value: "# md".into(),
            })
        );
    }

    #[test]
    fn deprecation_unions_flag_and_tag() {
        assert!(!completion_item_from_lsp(bare_item("fresh")).deprecated);

        let mut item = bare_item("legacy-flag");
        item.deprecated = Some(true);
        assert!(completion_item_from_lsp(item).deprecated);

        let mut item = bare_item("tagged");
        item.tags = Some(vec![lsp_types::CompletionItemTag::DEPRECATED]);
        assert!(completion_item_from_lsp(item).deprecated);
    }

    #[test]
    fn sort_text_kept_detail_kept_filter_text_dropped() {
        let mut item = bare_item("x");
        item.sort_text = Some("0001".into());
        item.detail = Some("fn x()".into());
        item.filter_text = Some("filter-me".into());
        let mapped = completion_item_from_lsp(item);
        assert_eq!(mapped.sort_text.as_deref(), Some("0001"));
        assert_eq!(mapped.detail.as_deref(), Some("fn x()"));
        // filter_text has no neutral slot: label is the filter key.
    }

    #[test]
    fn positions_convert_to_lsp_unchanged() {
        let p = position_to_lsp(LineCol::new(7, 42));
        assert_eq!((p.line, p.character), (7, 42));
    }
}

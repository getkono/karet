//! LSP wire types → neutral `karet-core` models.
//!
//! Positions cross this boundary **unconverted**: karet-lsp is faithful to the
//! negotiated LSP encoding (UTF-16), so an `lsp_types::Position`'s `character`
//! becomes a [`LineCol::col`] still counted in UTF-16 code units. The consumer
//! (karet-session) owns the text and performs the UTF-16 ↔ UTF-32 translation via
//! `karet_text::TextBuffer`.

use karet_core::CodeAction;
use karet_core::CommandId;
use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::Hover;
use karet_core::InlayHint;
use karet_core::InlayHintKind;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::ParamInfo;
use karet_core::Range;
use karet_core::RelatedInfo;
use karet_core::Severity;
use karet_core::Signature;
use karet_core::SignatureHelp;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;

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

/// Map a karet range to LSP positions without changing the negotiated encoding.
pub(crate) fn range_to_lsp(range: Range) -> lsp_types::Range {
    lsp_types::Range {
        start: position_to_lsp(range.start),
        end: position_to_lsp(range.end),
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

/// Map either LSP document-symbol response shape into the neutral nested model.
pub(crate) fn document_symbols_from_lsp(
    response: Option<lsp_types::DocumentSymbolResponse>,
) -> Vec<Symbol> {
    match response {
        None => Vec::new(),
        Some(lsp_types::DocumentSymbolResponse::Nested(symbols)) => {
            symbols.into_iter().map(document_symbol_from_lsp).collect()
        },
        #[allow(deprecated)] // LSP still permits the flat SymbolInformation response.
        Some(lsp_types::DocumentSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|symbol| Symbol {
                name: symbol.name,
                kind: symbol_kind_from_lsp(symbol.kind),
                detail: None,
                range: range_from_lsp(symbol.location.range),
                selection_range: range_from_lsp(symbol.location.range),
                container_name: symbol.container_name,
                children: Vec::new(),
            })
            .collect(),
    }
}

/// Map a hover response, preserving markdown when the server supplies it.
pub(crate) fn hover_from_lsp(hover: Option<lsp_types::Hover>) -> Option<Hover> {
    let hover = hover?;
    let contents = match hover.contents {
        lsp_types::HoverContents::Markup(markup) => Markup {
            kind: markup_kind_from_lsp(markup.kind),
            value: markup.value,
        },
        lsp_types::HoverContents::Scalar(marked) => marked_string_from_lsp(marked),
        lsp_types::HoverContents::Array(marked) => Markup {
            kind: MarkupKind::Markdown,
            value: marked
                .into_iter()
                .map(marked_string_value)
                .collect::<Vec<_>>()
                .join("\n\n"),
        },
    };
    Some(Hover {
        contents,
        range: hover.range.map(range_from_lsp),
    })
}

fn marked_string_from_lsp(marked: lsp_types::MarkedString) -> Markup {
    Markup {
        kind: MarkupKind::Markdown,
        value: marked_string_value(marked),
    }
}

fn marked_string_value(marked: lsp_types::MarkedString) -> String {
    match marked {
        lsp_types::MarkedString::String(value) => value,
        lsp_types::MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        },
    }
}

fn markup_kind_from_lsp(kind: lsp_types::MarkupKind) -> MarkupKind {
    match kind {
        lsp_types::MarkupKind::Markdown => MarkupKind::Markdown,
        lsp_types::MarkupKind::PlainText => MarkupKind::PlainText,
    }
}

/// Map workspace symbols into the same neutral symbol shape used by outlines.
pub(crate) fn workspace_symbols_from_lsp(
    response: Option<lsp_types::WorkspaceSymbolResponse>,
) -> Vec<Symbol> {
    match response {
        None => Vec::new(),
        #[allow(deprecated)] // LSP retains the flat response for compatibility.
        Some(lsp_types::WorkspaceSymbolResponse::Flat(symbols)) => symbols
            .into_iter()
            .map(|symbol| Symbol {
                name: symbol.name,
                kind: symbol_kind_from_lsp(symbol.kind),
                detail: None,
                range: range_from_lsp(symbol.location.range),
                selection_range: range_from_lsp(symbol.location.range),
                container_name: symbol.container_name,
                children: Vec::new(),
            })
            .collect(),
        Some(lsp_types::WorkspaceSymbolResponse::Nested(symbols)) => symbols
            .into_iter()
            .filter_map(|symbol| {
                let lsp_types::OneOf::Left(location) = symbol.location else {
                    // The unresolved URI-only form has no range that the neutral
                    // model can navigate to.
                    return None;
                };
                Some(Symbol {
                    name: symbol.name,
                    kind: symbol_kind_from_lsp(symbol.kind),
                    detail: None,
                    range: range_from_lsp(location.range),
                    selection_range: range_from_lsp(location.range),
                    container_name: symbol.container_name,
                    children: Vec::new(),
                })
            })
            .collect(),
    }
}

/// Map definition response variants, dropping non-file URIs.
pub(crate) fn locations_from_lsp(
    response: Option<lsp_types::GotoDefinitionResponse>,
) -> Vec<Location> {
    let locations = match response {
        None => return Vec::new(),
        Some(lsp_types::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(lsp_types::GotoDefinitionResponse::Array(locations)) => locations,
        Some(lsp_types::GotoDefinitionResponse::Link(links)) => {
            return links
                .into_iter()
                .filter_map(|link| {
                    Some(Location {
                        path: uri_to_path(&link.target_uri)?,
                        range: range_from_lsp(link.target_selection_range),
                    })
                })
                .collect();
        },
    };
    locations
        .into_iter()
        .filter_map(|location| {
            Some(Location {
                path: uri_to_path(&location.uri)?,
                range: range_from_lsp(location.range),
            })
        })
        .collect()
}

/// Map inlay hints into renderable labels and positions.
pub(crate) fn inlay_hints_from_lsp(hints: Option<Vec<lsp_types::InlayHint>>) -> Vec<InlayHint> {
    hints
        .unwrap_or_default()
        .into_iter()
        .map(|hint| InlayHint {
            position: position_from_lsp(hint.position),
            label: match hint.label {
                lsp_types::InlayHintLabel::String(label) => label,
                lsp_types::InlayHintLabel::LabelParts(parts) => {
                    parts.into_iter().map(|part| part.value).collect()
                },
            },
            kind: match hint.kind {
                Some(lsp_types::InlayHintKind::PARAMETER) => InlayHintKind::Parameter,
                _ => InlayHintKind::Type,
            },
            padding_left: hint.padding_left.unwrap_or(false),
            padding_right: hint.padding_right.unwrap_or(false),
        })
        .collect()
}

/// Map a workspace edit while deliberately ignoring resource operations that
/// the neutral text-edit model cannot represent.
pub(crate) fn workspace_edit_from_lsp(edit: lsp_types::WorkspaceEdit) -> WorkspaceEdit {
    let mut changes = Vec::new();
    if let Some(uri_changes) = edit.changes {
        changes.extend(uri_changes.into_iter().filter_map(|(uri, edits)| {
            Some((
                uri_to_path(&uri)?,
                edits.into_iter().map(text_edit_from_lsp).collect(),
            ))
        }));
    }
    if let Some(document_changes) = edit.document_changes {
        let edits = match document_changes {
            lsp_types::DocumentChanges::Edits(edits) => edits,
            lsp_types::DocumentChanges::Operations(operations) => operations
                .into_iter()
                .filter_map(|operation| match operation {
                    lsp_types::DocumentChangeOperation::Edit(edit) => Some(edit),
                    lsp_types::DocumentChangeOperation::Op(_) => None,
                })
                .collect(),
        };
        changes.extend(edits.into_iter().filter_map(|edit| {
            let path = uri_to_path(&edit.text_document.uri)?;
            let edits = edit
                .edits
                .into_iter()
                .map(|edit| match edit {
                    lsp_types::OneOf::Left(edit) => edit,
                    lsp_types::OneOf::Right(edit) => edit.text_edit,
                })
                .map(text_edit_from_lsp)
                .collect();
            Some((path, edits))
        }));
    }
    changes.sort_by(|(left, _), (right, _)| left.cmp(right));
    WorkspaceEdit { changes }
}

/// Map signature help, clamping server-provided active indices.
pub(crate) fn signature_help_from_lsp(
    help: Option<lsp_types::SignatureHelp>,
) -> Option<SignatureHelp> {
    let help = help?;
    let signatures = help
        .signatures
        .into_iter()
        .map(|signature| {
            let label = signature.label;
            let parameters = signature
                .parameters
                .unwrap_or_default()
                .into_iter()
                .map(|parameter| ParamInfo {
                    label: match parameter.label {
                        lsp_types::ParameterLabel::Simple(label) => label,
                        lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
                            utf16_slice(&label, start, end)
                        },
                    },
                    documentation: parameter.documentation.map(markup_from_lsp),
                })
                .collect();
            Signature {
                label,
                documentation: signature.documentation.map(markup_from_lsp),
                parameters,
            }
        })
        .collect::<Vec<_>>();
    let active_signature = usize::try_from(help.active_signature.unwrap_or(0))
        .unwrap_or(0)
        .min(signatures.len().saturating_sub(1));
    let active_parameter = usize::try_from(help.active_parameter.unwrap_or(0)).unwrap_or(0);
    Some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter,
    })
}

fn utf16_slice(value: &str, start: u32, end: u32) -> String {
    let mut offset = 0_u32;
    value
        .chars()
        .filter(|character| {
            let width = character.len_utf16() as u32;
            let include = offset >= start && offset < end;
            offset = offset.saturating_add(width);
            include
        })
        .collect()
}

/// Map code actions and commands, dropping disabled actions.
pub(crate) fn code_actions_from_lsp(
    response: Option<lsp_types::CodeActionResponse>,
) -> Vec<CodeAction> {
    response
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| match item {
            lsp_types::CodeActionOrCommand::Command(command) => Some(CodeAction {
                title: command.title,
                edit: None,
                command: Some(CommandId(command.command)),
            }),
            lsp_types::CodeActionOrCommand::CodeAction(action) => {
                action.disabled.is_none().then(|| CodeAction {
                    title: action.title,
                    edit: action.edit.map(workspace_edit_from_lsp),
                    command: action.command.map(|command| CommandId(command.command)),
                })
            },
        })
        .collect()
}

/// Map formatting edits.
pub(crate) fn text_edits_from_lsp(edits: Option<Vec<lsp_types::TextEdit>>) -> Vec<TextEdit> {
    edits
        .unwrap_or_default()
        .into_iter()
        .map(text_edit_from_lsp)
        .collect()
}

fn text_edit_from_lsp(edit: lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        range: range_from_lsp(edit.range),
        new_text: edit.new_text,
    }
}

fn document_symbol_from_lsp(symbol: lsp_types::DocumentSymbol) -> Symbol {
    Symbol {
        name: symbol.name,
        kind: symbol_kind_from_lsp(symbol.kind),
        detail: symbol.detail,
        range: range_from_lsp(symbol.range),
        selection_range: range_from_lsp(symbol.selection_range),
        container_name: None,
        children: symbol
            .children
            .unwrap_or_default()
            .into_iter()
            .map(document_symbol_from_lsp)
            .collect(),
    }
}

fn symbol_kind_from_lsp(kind: lsp_types::SymbolKind) -> SymbolKind {
    match kind {
        lsp_types::SymbolKind::FILE => SymbolKind::File,
        lsp_types::SymbolKind::MODULE => SymbolKind::Module,
        lsp_types::SymbolKind::NAMESPACE => SymbolKind::Namespace,
        lsp_types::SymbolKind::PACKAGE => SymbolKind::Package,
        lsp_types::SymbolKind::CLASS => SymbolKind::Class,
        lsp_types::SymbolKind::METHOD => SymbolKind::Method,
        lsp_types::SymbolKind::PROPERTY => SymbolKind::Property,
        lsp_types::SymbolKind::FIELD => SymbolKind::Field,
        lsp_types::SymbolKind::CONSTRUCTOR => SymbolKind::Constructor,
        lsp_types::SymbolKind::ENUM => SymbolKind::Enum,
        lsp_types::SymbolKind::INTERFACE => SymbolKind::Interface,
        lsp_types::SymbolKind::FUNCTION => SymbolKind::Function,
        lsp_types::SymbolKind::VARIABLE => SymbolKind::Variable,
        lsp_types::SymbolKind::CONSTANT => SymbolKind::Constant,
        lsp_types::SymbolKind::STRING => SymbolKind::String,
        lsp_types::SymbolKind::NUMBER => SymbolKind::Number,
        lsp_types::SymbolKind::BOOLEAN => SymbolKind::Boolean,
        lsp_types::SymbolKind::ARRAY => SymbolKind::Array,
        lsp_types::SymbolKind::OBJECT => SymbolKind::Object,
        lsp_types::SymbolKind::KEY => SymbolKind::Key,
        lsp_types::SymbolKind::NULL => SymbolKind::Null,
        lsp_types::SymbolKind::ENUM_MEMBER => SymbolKind::EnumMember,
        lsp_types::SymbolKind::STRUCT => SymbolKind::Struct,
        lsp_types::SymbolKind::EVENT => SymbolKind::Event,
        lsp_types::SymbolKind::OPERATOR => SymbolKind::Operator,
        lsp_types::SymbolKind::TYPE_PARAMETER => SymbolKind::TypeParameter,
        _ => SymbolKind::Variable,
    }
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

    #[test]
    fn nested_document_symbols_preserve_hierarchy_and_details() {
        let child = lsp_types::DocumentSymbol {
            name: "run".into(),
            detail: Some("(&self)".into()),
            kind: lsp_types::SymbolKind::METHOD,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: lsp_range(2, 0, 4, 1),
            selection_range: lsp_range(2, 3, 2, 6),
            children: None,
        };
        let parent = lsp_types::DocumentSymbol {
            name: "Runner".into(),
            detail: None,
            kind: lsp_types::SymbolKind::STRUCT,
            tags: None,
            #[allow(deprecated)]
            deprecated: None,
            range: lsp_range(0, 0, 5, 1),
            selection_range: lsp_range(0, 7, 0, 13),
            children: Some(vec![child]),
        };
        let mapped =
            document_symbols_from_lsp(Some(lsp_types::DocumentSymbolResponse::Nested(vec![
                parent,
            ])));
        assert_eq!(mapped[0].kind, SymbolKind::Struct);
        assert_eq!(mapped[0].children[0].name, "run");
        assert_eq!(mapped[0].children[0].detail.as_deref(), Some("(&self)"));
        assert_eq!(
            mapped[0].children[0].selection_range.start,
            LineCol::new(2, 3)
        );
    }

    #[test]
    fn maps_hover_definitions_and_inlay_hints() -> Result<(), Box<dyn std::error::Error>> {
        let hover: lsp_types::Hover = serde_json::from_value(serde_json::json!({
            "contents": {"kind": "markdown", "value": "**type**"},
            "range": {"start": {"line": 1, "character": 2},
                      "end": {"line": 1, "character": 5}}
        }))?;
        let mapped = hover_from_lsp(Some(hover)).unwrap_or(Hover {
            contents: Markup {
                kind: MarkupKind::PlainText,
                value: String::new(),
            },
            range: None,
        });
        assert_eq!(mapped.contents.kind, MarkupKind::Markdown);
        assert_eq!(mapped.contents.value, "**type**");
        assert_eq!(
            mapped.range.map(|range| range.start),
            Some(LineCol::new(1, 2))
        );

        let definitions: lsp_types::GotoDefinitionResponse =
            serde_json::from_value(serde_json::json!([{
                "targetUri": "file:///tmp/lib.rs",
                "targetRange": {"start": {"line": 3, "character": 0},
                                "end": {"line": 4, "character": 0}},
                "targetSelectionRange": {"start": {"line": 3, "character": 4},
                                         "end": {"line": 3, "character": 8}}
            }]))?;
        let mapped = locations_from_lsp(Some(definitions));
        assert_eq!(mapped[0].path, PathBuf::from("/tmp/lib.rs"));
        assert_eq!(mapped[0].range.start, LineCol::new(3, 4));

        let hints: Vec<lsp_types::InlayHint> = serde_json::from_value(serde_json::json!([{
            "position": {"line": 2, "character": 7},
            "label": [{"value": "value"}, {"value": ": i32"}],
            "kind": 2,
            "paddingLeft": true
        }]))?;
        let mapped = inlay_hints_from_lsp(Some(hints));
        assert_eq!(mapped[0].label, "value: i32");
        assert_eq!(mapped[0].kind, InlayHintKind::Parameter);
        assert!(mapped[0].padding_left);
        Ok(())
    }

    #[test]
    fn maps_workspace_edits_signatures_and_actions() -> Result<(), Box<dyn std::error::Error>> {
        let edit: lsp_types::WorkspaceEdit = serde_json::from_value(serde_json::json!({
            "changes": {
                "file:///tmp/a.rs": [{
                    "range": {"start": {"line": 0, "character": 1},
                              "end": {"line": 0, "character": 2}},
                    "newText": "renamed"
                }]
            },
            "documentChanges": [{
                "textDocument": {"uri": "file:///tmp/b.rs", "version": 4},
                "edits": [{
                    "range": {"start": {"line": 1, "character": 0},
                              "end": {"line": 1, "character": 3}},
                    "newText": "new"
                }]
            }]
        }))?;
        let mapped = workspace_edit_from_lsp(edit);
        assert_eq!(mapped.changes.len(), 2);
        assert_eq!(mapped.changes[0].0, PathBuf::from("/tmp/a.rs"));

        let help: lsp_types::SignatureHelp = serde_json::from_value(serde_json::json!({
            "signatures": [{
                "label": "call(😀arg)",
                "documentation": "docs",
                "parameters": [{"label": [5, 10], "documentation": "parameter"}]
            }],
            "activeSignature": 99,
            "activeParameter": 1
        }))?;
        let mapped = signature_help_from_lsp(Some(help)).unwrap_or(SignatureHelp {
            signatures: Vec::new(),
            active_signature: 0,
            active_parameter: 0,
        });
        assert_eq!(mapped.active_signature, 0);
        assert_eq!(mapped.signatures[0].parameters[0].label, "😀arg");

        let actions: lsp_types::CodeActionResponse = serde_json::from_value(serde_json::json!([
            {"title": "Run fix", "command": "fix.run"},
            {"title": "Unavailable", "disabled": {"reason": "nope"}}
        ]))?;
        let mapped = code_actions_from_lsp(Some(actions));
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].command, Some(CommandId("fix.run".into())));
        Ok(())
    }

    #[test]
    fn maps_workspace_symbols_and_formatting_edits() -> Result<(), Box<dyn std::error::Error>> {
        let symbols: lsp_types::WorkspaceSymbolResponse =
            serde_json::from_value(serde_json::json!([{
                "name": "Runner",
                "kind": 23,
                "location": {
                    "uri": "file:///tmp/a.rs",
                    "range": {"start": {"line": 4, "character": 2},
                              "end": {"line": 4, "character": 8}}
                }
            }]))?;
        let mapped = workspace_symbols_from_lsp(Some(symbols));
        assert_eq!(mapped[0].name, "Runner");
        assert_eq!(mapped[0].kind, SymbolKind::Struct);

        let edits: Vec<lsp_types::TextEdit> = serde_json::from_value(serde_json::json!([{
            "range": {"start": {"line": 0, "character": 0},
                      "end": {"line": 0, "character": 2}},
            "newText": "  "
        }]))?;
        let mapped = text_edits_from_lsp(Some(edits));
        assert_eq!(mapped[0].new_text, "  ");
        assert_eq!(mapped[0].range.end, LineCol::new(0, 2));
        Ok(())
    }
}

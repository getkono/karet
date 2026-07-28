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
#[path = "convert_tests.rs"]
mod tests;

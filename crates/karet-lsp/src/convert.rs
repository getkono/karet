//! LSP wire types → neutral `karet-core` models.
//!
//! Positions cross this boundary **unconverted**: karet-lsp is faithful to the
//! negotiated LSP encoding (UTF-16), so an `lsp_types::Position`'s `character`
//! becomes a [`LineCol::col`] still counted in UTF-16 code units. The consumer
//! (karet-session) owns the text and performs the UTF-16 ↔ UTF-32 translation via
//! `karet_text::TextBuffer`.

use karet_core::ApplyWorkspaceEditResult;
use karet_core::CodeAction;
use karet_core::CommandId;
use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::CompletionList;
use karet_core::DeleteOptions;
use karet_core::Diagnostic;
use karet_core::DiagnosticTag;
use karet_core::DocumentChange;
use karet_core::DocumentEdit;
use karet_core::FileOptions;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Markup;
use karet_core::MarkupKind;
use karet_core::ParamInfo;
use karet_core::PrepareRename;
use karet_core::Range;
use karet_core::RelatedInfo;
use karet_core::Severity;
use karet_core::Signature;
use karet_core::SignatureHelp;
use karet_core::Symbol;
use karet_core::SymbolKind;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;

use crate::LspError;
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

/// Map a completion response to a neutral list, keeping `isIncomplete` (a bare
/// array is complete).
///
/// With `resolvable` -- the server resolves items -- every item keeps its
/// wire form as the payload a `completionItem/resolve` hands back; otherwise
/// none does, so a list nobody can resolve is not serialized twice.
pub(crate) fn completions_from_lsp(
    response: Option<lsp_types::CompletionResponse>,
    resolvable: bool,
) -> CompletionList {
    let (items, incomplete) = match response {
        None => return CompletionList::default(),
        Some(lsp_types::CompletionResponse::Array(items)) => (items, false),
        Some(lsp_types::CompletionResponse::List(list)) => (list.items, list.is_incomplete),
    };
    CompletionList {
        items: items
            .into_iter()
            .map(|item| {
                let payload = resolvable
                    .then(|| serde_json::to_string(&item).ok())
                    .flatten();
                CompletionItem {
                    payload,
                    ..completion_item_from_lsp(item)
                }
            })
            .collect(),
        incomplete,
    }
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
/// Turn type-hierarchy items into locations, keeping their selection ranges.
///
/// The selection range is the one worth navigating to — the name, not the whole body —
/// which matches how every other location in this crate is anchored.
pub(crate) fn type_hierarchy_locations(
    response: Option<Vec<lsp_types::TypeHierarchyItem>>,
) -> Vec<Location> {
    response
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            Some(Location {
                path: uri_to_path(&item.uri)?,
                range: range_from_lsp(item.selection_range),
            })
        })
        .collect()
}

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

/// Map a workspace edit, resource operations included.
///
/// LSP says a client that understands `documentChanges` uses them and ignores
/// `changes`, so when both are present only `documentChanges` is kept. Their
/// order is preserved: a server may create a file and then edit it.
///
/// # Errors
/// [`LspError::Protocol`] when a step names a URI that is not `file://`.
/// Dropping that step would apply the rest of an edit the server meant as a
/// whole -- the steps after it can depend on it -- and report success.
pub(crate) fn workspace_edit_from_lsp(
    edit: lsp_types::WorkspaceEdit,
) -> Result<WorkspaceEdit, LspError> {
    if let Some(document_changes) = edit.document_changes {
        let operations = match document_changes {
            lsp_types::DocumentChanges::Edits(edits) => edits
                .into_iter()
                .map(lsp_types::DocumentChangeOperation::Edit)
                .collect(),
            lsp_types::DocumentChanges::Operations(operations) => operations,
        };
        let document_changes = operations
            .into_iter()
            .map(document_change_from_lsp)
            .collect::<Result<_, _>>()?;
        return Ok(WorkspaceEdit {
            changes: Vec::new(),
            document_changes,
        });
    }
    let mut changes = edit
        .changes
        .unwrap_or_default()
        .into_iter()
        .map(|(uri, edits)| {
            Ok((
                file_path(&uri)?,
                edits.into_iter().map(text_edit_from_lsp).collect(),
            ))
        })
        .collect::<Result<Vec<_>, LspError>>()?;
    changes.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(WorkspaceEdit {
        changes,
        document_changes: Vec::new(),
    })
}

/// Map one ordered `documentChanges` step.
fn document_change_from_lsp(
    operation: lsp_types::DocumentChangeOperation,
) -> Result<DocumentChange, LspError> {
    use lsp_types::DocumentChangeOperation as Step;
    use lsp_types::ResourceOp;
    Ok(match operation {
        Step::Edit(edit) => DocumentChange::Edit(DocumentEdit {
            path: file_path(&edit.text_document.uri)?,
            version: edit.text_document.version,
            edits: edit
                .edits
                .into_iter()
                .map(|edit| match edit {
                    lsp_types::OneOf::Left(edit) => edit,
                    lsp_types::OneOf::Right(edit) => edit.text_edit,
                })
                .map(text_edit_from_lsp)
                .collect(),
        }),
        Step::Op(ResourceOp::Create(create)) => {
            let (overwrite, ignore_if_exists) = create.options.map_or((None, None), |options| {
                (options.overwrite, options.ignore_if_exists)
            });
            DocumentChange::Create {
                path: file_path(&create.uri)?,
                options: FileOptions {
                    overwrite: overwrite.unwrap_or(false),
                    ignore_if_exists: ignore_if_exists.unwrap_or(false),
                },
            }
        },
        Step::Op(ResourceOp::Rename(rename)) => {
            let (overwrite, ignore_if_exists) = rename.options.map_or((None, None), |options| {
                (options.overwrite, options.ignore_if_exists)
            });
            DocumentChange::Rename {
                from: file_path(&rename.old_uri)?,
                to: file_path(&rename.new_uri)?,
                options: FileOptions {
                    overwrite: overwrite.unwrap_or(false),
                    ignore_if_exists: ignore_if_exists.unwrap_or(false),
                },
            }
        },
        Step::Op(ResourceOp::Delete(delete)) => {
            let (recursive, ignore_if_not_exists) =
                delete.options.map_or((None, None), |options| {
                    (options.recursive, options.ignore_if_not_exists)
                });
            DocumentChange::Delete {
                path: file_path(&delete.uri)?,
                options: DeleteOptions {
                    recursive: recursive.unwrap_or(false),
                    ignore_if_not_exists: ignore_if_not_exists.unwrap_or(false),
                },
            }
        },
    })
}

/// The path a workspace-edit URI names, or why it names none.
fn file_path(uri: &lsp_types::Uri) -> Result<std::path::PathBuf, LspError> {
    uri_to_path(uri).ok_or_else(|| {
        LspError::Protocol(format!(
            "workspace edit names {}, which is not a file",
            uri.as_str()
        ))
    })
}

/// Map a `textDocument/prepareRename` answer; `None` means "not renameable".
pub(crate) fn prepare_rename_from_lsp(
    response: Option<lsp_types::PrepareRenameResponse>,
) -> Option<PrepareRename> {
    use lsp_types::PrepareRenameResponse as Answer;
    match response? {
        Answer::Range(range) => Some(PrepareRename::Range {
            range: range_from_lsp(range),
            placeholder: None,
        }),
        Answer::RangeWithPlaceholder { range, placeholder } => Some(PrepareRename::Range {
            range: range_from_lsp(range),
            placeholder: Some(placeholder),
        }),
        // `false` is not in the spec; a server sending it is saying no.
        Answer::DefaultBehavior { default_behavior } => {
            default_behavior.then_some(PrepareRename::DefaultBehavior)
        },
    }
}

/// Map a karet apply outcome to the `workspace/applyEdit` reply.
pub(crate) fn apply_result_to_lsp(
    result: ApplyWorkspaceEditResult,
) -> lsp_types::ApplyWorkspaceEditResponse {
    lsp_types::ApplyWorkspaceEditResponse {
        applied: result.applied,
        failure_reason: result.failure_reason,
        failed_change: result
            .failed_change
            .map(|index| u32::try_from(index).unwrap_or(u32::MAX)),
    }
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

/// Map code actions and commands, keeping each one's wire form as its
/// payload so it can be resolved or run later. A disabled action is kept,
/// with its reason, so the user can see it exists.
pub(crate) fn code_actions_from_lsp(
    response: Option<lsp_types::CodeActionResponse>,
) -> Vec<CodeAction> {
    response
        .unwrap_or_default()
        .into_iter()
        .map(code_action_or_command_from_lsp)
        .collect()
}

/// Map one item of a code-action answer.
pub(crate) fn code_action_or_command_from_lsp(item: lsp_types::CodeActionOrCommand) -> CodeAction {
    let payload = serde_json::to_string(&item).ok();
    let mut action = match item {
        lsp_types::CodeActionOrCommand::Command(command) => CodeAction {
            title: command.title,
            command: Some(CommandId(command.command)),
            ..CodeAction::default()
        },
        lsp_types::CodeActionOrCommand::CodeAction(action) => code_action_from_lsp(action),
    };
    action.payload = payload;
    action
}

/// Map a code-action literal (a `codeAction/resolve` answer is one too).
pub(crate) fn code_action_from_lsp(action: lsp_types::CodeAction) -> CodeAction {
    let mut disabled = action.disabled.map(|disabled| disabled.reason);
    // An edit naming a non-file URI cannot be applied as the server meant it,
    // so the action offers no edit, and says why it cannot run.
    let edit = match action.edit.map(workspace_edit_from_lsp) {
        Some(Ok(edit)) => Some(edit),
        Some(Err(error)) => {
            disabled.get_or_insert_with(|| error.to_string());
            None
        },
        None => None,
    };
    CodeAction {
        title: action.title,
        kind: action.kind.map(|kind| kind.as_str().to_owned()),
        preferred: action.is_preferred.unwrap_or(false),
        disabled,
        edit,
        command: action.command.map(|command| CommandId(command.command)),
        payload: None,
    }
}

/// The command a code action runs, in wire form, read back from its payload:
/// the item itself when the server answered with a bare command, else the
/// action's own `command`.
pub(crate) fn code_action_command(action: &CodeAction) -> Option<lsp_types::Command> {
    match serde_json::from_str(action.payload.as_deref()?).ok()? {
        lsp_types::CodeActionOrCommand::Command(command) => Some(command),
        lsp_types::CodeActionOrCommand::CodeAction(action) => action.command,
    }
}

/// The code-action literal a payload holds, for `codeAction/resolve`; `None`
/// for a bare command, which has nothing to resolve.
pub(crate) fn code_action_literal(action: &CodeAction) -> Option<lsp_types::CodeAction> {
    match serde_json::from_str(action.payload.as_deref()?).ok()? {
        lsp_types::CodeActionOrCommand::CodeAction(literal) => Some(literal),
        lsp_types::CodeActionOrCommand::Command(_) => None,
    }
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
///   an insert/replace edit puts its *insert* range there and keeps its
///   *replace* range beside it, so the consumer chooses.
/// - **`additionalTextEdits`**, **`filterText`** and **`commitCharacters`**
///   carry over; a commit "character" that is not exactly one character is
///   dropped.
/// - **Deprecation** is the union of the legacy `deprecated` flag and the
///   `Deprecated` tag.
///
/// The payload is left empty: [`completions_from_lsp`] fills it when the
/// server resolves items.
pub(crate) fn completion_item_from_lsp(item: lsp_types::CompletionItem) -> CompletionItem {
    let is_snippet = item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET);
    let degrade = |text: String| {
        if is_snippet {
            strip_snippet(&text)
        } else {
            text
        }
    };
    let (edit, replace_range) = match item.text_edit {
        None => (None, None),
        Some(lsp_types::CompletionTextEdit::Edit(e)) => (
            Some(TextEdit {
                range: range_from_lsp(e.range),
                new_text: degrade(e.new_text),
            }),
            None,
        ),
        Some(lsp_types::CompletionTextEdit::InsertAndReplace(e)) => (
            Some(TextEdit {
                range: range_from_lsp(e.insert),
                new_text: degrade(e.new_text),
            }),
            Some(range_from_lsp(e.replace)),
        ),
    };
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
        filter_text: item.filter_text,
        replace_range,
        additional_edits: item
            .additional_text_edits
            .unwrap_or_default()
            .into_iter()
            .map(text_edit_from_lsp)
            .collect(),
        commit_characters: item.commit_characters.map(|all| {
            all.iter()
                .filter_map(|text| {
                    let mut it = text.chars();
                    it.next().filter(|_| it.next().is_none())
                })
                .collect()
        }),
        payload: None,
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

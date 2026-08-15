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
        // `DocumentSymbol` still carries its deprecated `deprecated` field, and a
        // struct literal must fill it.
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
        // `DocumentSymbol` still carries its deprecated `deprecated` field, and a
        // struct literal must fill it.
        #[allow(deprecated)]
        deprecated: None,
        range: lsp_range(0, 0, 5, 1),
        selection_range: lsp_range(0, 7, 0, 13),
        children: Some(vec![child]),
    };
    let mapped = document_symbols_from_lsp(Some(lsp_types::DocumentSymbolResponse::Nested(vec![
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
    let symbols: lsp_types::WorkspaceSymbolResponse = serde_json::from_value(serde_json::json!([{
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

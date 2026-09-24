//! Tests for reading a server's advertised capabilities.
//!
//! The shapes here are the ones real servers send, because the whole failure
//! mode being fixed is a shape karet did not expect being read as "supports
//! nothing" or "supports everything".

use serde_json::json;

use super::*;

#[test]
fn a_reply_with_no_capabilities_supports_nothing() {
    // Not a hypothetical: a server that fails its own initialization can
    // answer with an empty result, and issuing every request at it afterwards
    // is exactly what this change exists to stop.
    let caps = parse(&json!({}));
    assert!(caps.is_empty());
    assert!(!caps.supports(ServerFeature::Hover));
}

#[test]
fn both_spellings_of_a_provider_are_understood() {
    // `true` and an options object both mean yes; `false` and absent mean no.
    // Servers use all four, often in the same reply.
    let caps = parse(&json!({"capabilities": {
        "hoverProvider": true,
        "referencesProvider": false,
        "definitionProvider": {},
        "codeActionProvider": {"codeActionKinds": ["quickfix", "refactor"]},
    }}));
    assert!(caps.supports(ServerFeature::Hover));
    assert!(!caps.supports(ServerFeature::References));
    assert!(caps.supports(ServerFeature::Definition));
    assert!(caps.supports(ServerFeature::CodeAction));
    assert!(!caps.supports(ServerFeature::Rename));
    assert_eq!(caps.code_action_kinds, vec!["quickfix", "refactor"]);
}

#[test]
fn a_resolve_provider_is_read_separately_from_its_feature() {
    // The distinction that matters: offering completion says nothing about
    // whether `completionItem/resolve` will be answered.
    let caps = parse(&json!({"capabilities": {
        "completionProvider": {"resolveProvider": true, "triggerCharacters": [".", ":"]},
        "codeActionProvider": true,
        "inlayHintProvider": {"resolveProvider": false},
    }}));
    assert!(caps.supports(ServerFeature::Completion));
    assert!(caps.supports(ServerFeature::CompletionResolve));

    assert!(caps.supports(ServerFeature::CodeAction));
    assert!(!caps.supports(ServerFeature::CodeActionResolve));

    assert!(caps.supports(ServerFeature::InlayHint));
    assert!(!caps.supports(ServerFeature::InlayHintResolve));
}

#[test]
fn rename_and_prepare_rename_are_independent() {
    let plain = parse(&json!({"capabilities": {"renameProvider": true}}));
    assert!(plain.supports(ServerFeature::Rename));
    assert!(!plain.supports(ServerFeature::PrepareRename));

    let prepared = parse(&json!({"capabilities": {
        "renameProvider": {"prepareProvider": true},
    }}));
    assert!(prepared.supports(ServerFeature::Rename));
    assert!(prepared.supports(ServerFeature::PrepareRename));
}

#[test]
fn trigger_characters_survive_and_multi_character_entries_do_not() {
    let caps = parse(&json!({"capabilities": {
        "completionProvider": {
            "triggerCharacters": [".", "::", ":"],
            "allCommitCharacters": ["("],
        },
        "signatureHelpProvider": {
            "triggerCharacters": ["(", ","],
            "retriggerCharacters": [","],
        },
    }}));
    // `::` is dropped rather than truncated to `:`, which would have made
    // every `:` a trigger.
    assert_eq!(caps.completion.trigger_characters, vec!['.', ':']);
    assert_eq!(caps.completion.commit_characters, vec!['(']);
    assert!(caps.is_completion_trigger('.'));
    assert!(!caps.is_completion_trigger('x'));
    assert_eq!(caps.signature_help.retrigger_characters, vec![',']);
}

#[test]
fn the_semantic_token_legend_keeps_its_order() {
    // The order *is* the meaning: a packed token indexes into these lists, so
    // a reordered legend recolours the whole file.
    let caps = parse(&json!({"capabilities": {
        "semanticTokensProvider": {
            "legend": {
                "tokenTypes": ["namespace", "type", "function"],
                "tokenModifiers": ["declaration", "readonly"],
            },
            "full": {"delta": true},
            "range": true,
        },
    }}));
    assert!(
        caps.semantic_tokens_legend.is_some(),
        "a legend was advertised"
    );
    let legend = caps.semantic_tokens_legend.clone().unwrap_or_default();
    assert_eq!(legend.token_types, vec!["namespace", "type", "function"]);
    assert_eq!(legend.token_modifiers, vec!["declaration", "readonly"]);
    assert!(caps.supports(ServerFeature::SemanticTokensFull));
    assert!(caps.supports(ServerFeature::SemanticTokensDelta));
    assert!(caps.supports(ServerFeature::SemanticTokensRange));
}

#[test]
fn semantic_tokens_without_delta_do_not_claim_it() {
    let caps = parse(&json!({"capabilities": {
        "semanticTokensProvider": {
            "legend": {"tokenTypes": ["type"], "tokenModifiers": []},
            "full": true,
        },
    }}));
    assert!(caps.supports(ServerFeature::SemanticTokensFull));
    assert!(!caps.supports(ServerFeature::SemanticTokensDelta));
    assert!(!caps.supports(ServerFeature::SemanticTokensRange));
}

#[test]
fn text_sync_reads_both_the_number_and_the_object_form() {
    // gopls sends the object; several servers send the bare number.
    let bare = parse(&json!({"capabilities": {"textDocumentSync": 2}}));
    assert_eq!(bare.text_sync, TextSyncKind::Incremental);

    let object = parse(&json!({"capabilities": {
        "textDocumentSync": {
            "openClose": true,
            "change": 1,
            "willSaveWaitUntil": true,
            "save": {"includeText": true},
        },
    }}));
    assert_eq!(object.text_sync, TextSyncKind::Full);
    assert!(object.save_includes_text);
    assert!(object.supports(ServerFeature::WillSaveWaitUntil));

    let none = parse(&json!({"capabilities": {"textDocumentSync": 0}}));
    assert_eq!(none.text_sync, TextSyncKind::None);

    // Unrecognised degrades to Full: resending everything is wasteful, never
    // wrong, where guessing Incremental would corrupt the server's copy.
    let odd = parse(&json!({"capabilities": {"textDocumentSync": 99}}));
    assert_eq!(odd.text_sync, TextSyncKind::Full);
}

#[test]
fn a_save_advertised_as_a_bare_bool_is_understood() {
    let caps = parse(&json!({"capabilities": {
        "textDocumentSync": {"change": 2, "save": true},
    }}));
    // `save: true` means notify-on-save, not include-the-text.
    assert!(!caps.save_includes_text);
    assert_eq!(caps.text_sync, TextSyncKind::Incremental);
}

#[test]
fn position_encoding_is_taken_from_the_server_and_defaults_to_utf16() {
    let default = parse(&json!({"capabilities": {}}));
    assert_eq!(default.position_encoding, PositionEncoding::Utf16);

    // clangd's preference, and the reason this is a live correctness bug:
    // UTF-8 and UTF-16 agree on every ASCII line, so the mismatch only shows
    // up once a line contains one non-ASCII character.
    let utf8 = parse(&json!({"capabilities": {"positionEncoding": "utf-8"}}));
    assert_eq!(utf8.position_encoding, PositionEncoding::Utf8);

    let utf32 = parse(&json!({"capabilities": {"positionEncoding": "utf-32"}}));
    assert_eq!(utf32.position_encoding, PositionEncoding::Utf32);
}

#[test]
fn type_hierarchy_is_read_even_though_lsp_types_has_no_field_for_it() {
    // The reason this module reads raw JSON. karet already issues
    // `textDocument/prepareTypeHierarchy`, and `lsp_types` 0.97 cannot
    // represent the capability that says whether it will be answered.
    let caps = parse(&json!({"capabilities": {"typeHierarchyProvider": true}}));
    assert!(caps.supports(ServerFeature::TypeHierarchy));
}

#[test]
fn execute_command_carries_its_command_list() {
    let caps = parse(&json!({"capabilities": {
        "executeCommandProvider": {"commands": ["rust-analyzer.runSingle"]},
    }}));
    assert!(caps.supports(ServerFeature::ExecuteCommand));
    assert!(caps.accepts_command("rust-analyzer.runSingle"));
    assert!(!caps.accepts_command("gopls.tidy"));
}

#[test]
fn pull_diagnostics_separate_document_scope_from_workspace_scope() {
    let caps = parse(&json!({"capabilities": {
        "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false},
    }}));
    assert!(caps.supports(ServerFeature::PullDiagnostics));
    assert!(!caps.supports(ServerFeature::WorkspacePullDiagnostics));
}

#[test]
fn on_type_formatting_needs_its_first_trigger_to_count() {
    let caps = parse(&json!({"capabilities": {
        "documentOnTypeFormattingProvider": {
            "firstTriggerCharacter": "}",
            "moreTriggerCharacter": [";", "\n"],
        },
    }}));
    assert!(
        caps.on_type_formatting.is_some(),
        "on-type formatting was advertised"
    );
    let options = caps.on_type_formatting.unwrap_or(OnTypeFormattingOptions {
        first_trigger_character: '\0',
        more_trigger_characters: Vec::new(),
    });
    assert_eq!(options.first_trigger_character, '}');
    assert_eq!(options.more_trigger_characters, vec![';', '\n']);

    // Advertised but malformed: no usable trigger, so no options.
    let broken = parse(&json!({"capabilities": {
        "documentOnTypeFormattingProvider": {"moreTriggerCharacter": [";"]},
    }}));
    assert!(broken.on_type_formatting.is_none());
}

#[test]
fn a_realistic_rust_analyzer_reply_reads_as_expected() {
    // Trimmed from a real handshake. The point is the combination: plenty
    // advertised, and some conspicuously not.
    let caps = parse(&json!({"capabilities": {
        "positionEncoding": "utf-16",
        "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
        "hoverProvider": true,
        "completionProvider": {
            "resolveProvider": true,
            "triggerCharacters": [":", ".", "'", "("],
        },
        "signatureHelpProvider": {"triggerCharacters": ["(", ",", "<"]},
        "definitionProvider": true,
        "typeDefinitionProvider": true,
        "implementationProvider": true,
        "referencesProvider": true,
        "documentHighlightProvider": true,
        "documentSymbolProvider": true,
        "workspaceSymbolProvider": true,
        "codeActionProvider": {"resolveProvider": true},
        "codeLensProvider": {"resolveProvider": true},
        "documentFormattingProvider": true,
        "renameProvider": {"prepareProvider": true},
        "foldingRangeProvider": true,
        "callHierarchyProvider": true,
        "semanticTokensProvider": {
            "legend": {"tokenTypes": ["comment", "keyword"], "tokenModifiers": ["mutable"]},
            "full": {"delta": true},
            "range": true,
        },
        "inlayHintProvider": {"resolveProvider": true},
        "experimental": {"serverStatusNotification": true},
    }}));

    assert_eq!(caps.text_sync, TextSyncKind::Incremental);
    for supported in [
        ServerFeature::Hover,
        ServerFeature::Completion,
        ServerFeature::CompletionResolve,
        ServerFeature::InlayHint,
        ServerFeature::PrepareRename,
        ServerFeature::CallHierarchy,
        ServerFeature::SemanticTokensDelta,
    ] {
        assert!(
            caps.supports(supported),
            "{supported:?} should be supported"
        );
    }
    // rust-analyzer does not offer these, and asking anyway is the blind
    // request that reads to a user as an unreliable integration.
    for absent in [
        ServerFeature::DocumentLink,
        ServerFeature::DocumentColor,
        ServerFeature::RangeFormatting,
        ServerFeature::LinkedEditingRange,
        ServerFeature::PullDiagnostics,
    ] {
        assert!(!caps.supports(absent), "{absent:?} should be absent");
    }
}

#[test]
fn a_minimal_server_advertises_almost_nothing() {
    // What a small server really sends. Most of karet's feature set has to be
    // refused for this one, and refusing is the correct outcome.
    let caps = parse(&json!({"capabilities": {
        "textDocumentSync": 1,
        "documentFormattingProvider": true,
    }}));
    assert!(caps.supports(ServerFeature::Formatting));
    assert!(!caps.supports(ServerFeature::Hover));
    assert!(!caps.supports(ServerFeature::Completion));
    assert!(!caps.supports(ServerFeature::Definition));
    assert_eq!(caps.len(), 1);
}

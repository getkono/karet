//! Tests for reading a server's advertised capabilities.
//!
//! The shapes here are the ones real servers send, because the whole failure
//! mode being fixed is a shape karet did not expect being read as "supports
//! nothing" or "supports everything".

use serde_json::json;

use super::*;
use crate::gate::Gate;

/// The feature half of a parsed reply.
fn features(result: &Value) -> Capabilities {
    parse(result).0
}

/// The negotiated half of a parsed reply.
fn options(result: &Value) -> ServerOptions {
    parse(result).1
}

/// A gate seeded with `result`, as the handshake seeds a connection's.
fn gate(result: &Value) -> Gate {
    let (capabilities, options) = parse(result);
    let mut gate = Gate::default();
    gate.advertise(capabilities, options);
    gate
}

#[test]
fn a_reply_with_no_capabilities_supports_nothing() {
    // Not a hypothetical: a server that fails its own initialization can
    // answer with an empty result, and issuing every request at it afterwards
    // is exactly what this change exists to stop.
    let (caps, negotiated) = parse(&json!({}));
    assert!(caps.is_empty());
    assert!(!caps.supports(ServerFeature::Hover));
    // Nor did it negotiate anything: whole-document sync, no legend, no
    // commands, no format-on-type triggers.
    assert_eq!(negotiated, ServerOptions::default());
    assert_eq!(negotiated.text_sync, TextSyncKind::Full);
}

#[test]
fn both_spellings_of_a_provider_are_understood() {
    // `true` and an options object both mean yes; `false` and absent mean no.
    // Servers use all four, often in the same reply.
    let caps = features(&json!({"capabilities": {
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
}

#[test]
fn a_resolve_provider_is_read_separately_from_its_feature() {
    // The distinction that matters: offering completion says nothing about
    // whether `completionItem/resolve` will be answered.
    let caps = features(&json!({"capabilities": {
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
    let plain = features(&json!({"capabilities": {"renameProvider": true}}));
    assert!(plain.supports(ServerFeature::Rename));
    assert!(!plain.supports(ServerFeature::PrepareRename));

    let prepared = features(&json!({"capabilities": {
        "renameProvider": {"prepareProvider": true},
    }}));
    assert!(prepared.supports(ServerFeature::Rename));
    assert!(prepared.supports(ServerFeature::PrepareRename));
}

#[test]
fn trigger_characters_survive_and_multi_character_entries_do_not() {
    let caps = features(&json!({"capabilities": {
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
    let (caps, negotiated) = parse(&json!({"capabilities": {
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
        negotiated.semantic_tokens_legend.is_some(),
        "a legend was advertised"
    );
    let legend = negotiated.semantic_tokens_legend.unwrap_or_default();
    assert_eq!(legend.token_types, vec!["namespace", "type", "function"]);
    assert_eq!(legend.token_modifiers, vec!["declaration", "readonly"]);
    assert!(caps.supports(ServerFeature::SemanticTokensFull));
    assert!(caps.supports(ServerFeature::SemanticTokensDelta));
    assert!(caps.supports(ServerFeature::SemanticTokensRange));
}

#[test]
fn semantic_tokens_without_delta_do_not_claim_it() {
    let caps = features(&json!({"capabilities": {
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
    let bare = options(&json!({"capabilities": {"textDocumentSync": 2}}));
    assert_eq!(bare.text_sync, TextSyncKind::Incremental);

    let (object_caps, object) = parse(&json!({"capabilities": {
        "textDocumentSync": {
            "openClose": true,
            "change": 1,
            "willSaveWaitUntil": true,
            "save": {"includeText": true},
        },
    }}));
    assert_eq!(object.text_sync, TextSyncKind::Full);
    assert!(object_caps.supports(ServerFeature::WillSaveWaitUntil));

    let none = options(&json!({"capabilities": {"textDocumentSync": 0}}));
    assert_eq!(none.text_sync, TextSyncKind::None);

    // Unrecognised degrades to Full: resending everything is wasteful, never
    // wrong, where guessing Incremental would corrupt the server's copy.
    let odd = options(&json!({"capabilities": {"textDocumentSync": 99}}));
    assert_eq!(odd.text_sync, TextSyncKind::Full);
}

#[test]
fn a_save_advertised_as_a_bare_bool_leaves_the_sync_kind_alone() {
    let (caps, negotiated) = parse(&json!({"capabilities": {
        "textDocumentSync": {"change": 2, "save": true},
    }}));
    assert_eq!(negotiated.text_sync, TextSyncKind::Incremental);
    assert!(!caps.supports(ServerFeature::WillSaveWaitUntil));
}

#[test]
fn type_hierarchy_is_read_even_though_lsp_types_has_no_field_for_it() {
    // The reason this module reads raw JSON. karet already issues
    // `textDocument/prepareTypeHierarchy`, and `lsp_types` 0.97 cannot
    // represent the capability that says whether it will be answered.
    let caps = features(&json!({"capabilities": {"typeHierarchyProvider": true}}));
    assert!(caps.supports(ServerFeature::TypeHierarchy));
}

#[test]
fn inline_completion_is_read_though_lsp_types_hides_it_behind_a_feature() {
    let caps = features(&json!({"capabilities": {"inlineCompletionProvider": {}}}));
    assert!(caps.supports(ServerFeature::InlineCompletion));
    let off = features(&json!({"capabilities": {"inlineCompletionProvider": false}}));
    assert!(!off.supports(ServerFeature::InlineCompletion));
    assert_eq!(
        super::feature_for_method("textDocument/inlineCompletion"),
        Some(ServerFeature::InlineCompletion),
        "a server registering it later must be honoured too"
    );
}

#[test]
fn execute_command_carries_its_command_list() {
    let reply = json!({"capabilities": {
        "executeCommandProvider": {"commands": ["rust-analyzer.runSingle"]},
    }});
    assert!(features(&reply).supports(ServerFeature::ExecuteCommand));
    let listed = gate(&reply);
    assert!(listed.accepts_command("rust-analyzer.runSingle"));
    assert!(!listed.accepts_command("gopls.tidy"));
}

#[test]
fn execute_command_is_refused_unless_offered_and_unlisted_means_any() {
    // Not offered at all: every command is refused, listed or not.
    assert!(!gate(&json!({"capabilities": {}})).accepts_command("rust-analyzer.runSingle"));

    // Offered with no list: the list is optional in the protocol, so take the
    // server at its word rather than refusing everything.
    let unlisted = gate(&json!({"capabilities": {"executeCommandProvider": {}}}));
    assert!(unlisted.accepts_command("rust-analyzer.runSingle"));
    assert!(unlisted.accepts_command("gopls.tidy"));

    // A registration offers it as surely as the handshake does.
    let mut registered = gate(&json!({"capabilities": {}}));
    registered.register("1".to_owned(), ServerFeature::ExecuteCommand, None);
    assert!(registered.accepts_command("gopls.tidy"));
}

#[test]
fn pull_diagnostics_separate_document_scope_from_workspace_scope() {
    let caps = features(&json!({"capabilities": {
        "diagnosticProvider": {"interFileDependencies": true, "workspaceDiagnostics": false},
    }}));
    assert!(caps.supports(ServerFeature::PullDiagnostics));
    assert!(!caps.supports(ServerFeature::WorkspacePullDiagnostics));
    assert!(caps.supports(ServerFeature::InterFileDiagnostics));
}

#[test]
fn pull_diagnostics_without_inter_file_dependencies_pull_only_the_edited_file() {
    for provider in [json!(true), json!({"interFileDependencies": false})] {
        let caps = features(&json!({"capabilities": {"diagnosticProvider": provider}}));
        assert!(caps.supports(ServerFeature::PullDiagnostics));
        assert!(!caps.supports(ServerFeature::InterFileDiagnostics));
    }
}

#[test]
fn on_type_formatting_needs_its_first_trigger_to_count() {
    let negotiated = options(&json!({"capabilities": {
        "documentOnTypeFormattingProvider": {
            "firstTriggerCharacter": "}",
            "moreTriggerCharacter": [";", "\n"],
        },
    }}));
    assert!(
        negotiated.on_type_formatting.is_some(),
        "on-type formatting was advertised"
    );
    let triggers = negotiated
        .on_type_formatting
        .unwrap_or(OnTypeFormattingOptions {
            first_trigger_character: '\0',
            more_trigger_characters: Vec::new(),
        });
    assert_eq!(triggers.first_trigger_character, '}');
    assert_eq!(triggers.more_trigger_characters, vec![';', '\n']);
    // Every trigger, the first one first.
    assert_eq!(triggers.triggers(), vec!['}', ';', '\n']);

    // Advertised but malformed: no usable trigger, so no options.
    let broken = options(&json!({"capabilities": {
        "documentOnTypeFormattingProvider": {"moreTriggerCharacter": [";"]},
    }}));
    assert!(broken.on_type_formatting.is_none());
}

#[test]
fn a_realistic_rust_analyzer_reply_reads_as_expected() {
    // Trimmed from a real handshake. The point is the combination: plenty
    // advertised, and some conspicuously not.
    let (caps, negotiated) = parse(&json!({"capabilities": {
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

    assert_eq!(negotiated.text_sync, TextSyncKind::Incremental);
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
    let caps = features(&json!({"capabilities": {
        "textDocumentSync": 1,
        "documentFormattingProvider": true,
    }}));
    assert!(caps.supports(ServerFeature::Formatting));
    assert!(!caps.supports(ServerFeature::Hover));
    assert!(!caps.supports(ServerFeature::Completion));
    assert!(!caps.supports(ServerFeature::Definition));
    assert_eq!(caps.len(), 1);
}

//! What the client says about itself in `initialize`.
//!
//! Every capability declared here is a promise the rest of the crate keeps:
//! `refreshSupport` is a promise to answer `workspace/inlayHint/refresh`,
//! `workDoneProgress` a promise to accept `window/workDoneProgress/create` and
//! surface `$/progress`, `configuration` a promise to answer
//! `workspace/configuration` from the settings the consumer supplied, and a
//! `dynamicRegistration` a promise that a later `client/registerCapability` is
//! honoured. Declaring one the client does not keep changes what a server does
//! without anything here noticing, so each sits beside the code that keeps it
//! named in a comment.

use std::path::Path;

use crate::LspError;
use crate::uri;

/// What `LspClient::semantic_tokens` asks for and can read: whole-document
/// requests with deltas, in the relative format, naming every type and
/// modifier `karet_core`'s vocabulary carries -- the names a token is decoded
/// by, so a server that tailors its legend to the client's list loses
/// nothing. Overlapping and multi-line tokens are declined: the overlay is a
/// flat, line-local layer. `augmentsSyntaxTokens` says the tokens are laid
/// over a syntax highlighter's, so a server may leave out what syntax shows.
/// Without dynamic registration, whose options carry the legend the gate
/// drops.
fn semantic_token_capabilities() -> lsp_types::SemanticTokensClientCapabilities {
    lsp_types::SemanticTokensClientCapabilities {
        dynamic_registration: None,
        requests: lsp_types::SemanticTokensClientCapabilitiesRequests {
            range: Some(false),
            full: Some(lsp_types::SemanticTokensFullOptions::Delta { delta: Some(true) }),
        },
        token_types: karet_core::SemanticTokenType::ALL
            .into_iter()
            .map(|ty| lsp_types::SemanticTokenType::new(ty.name()))
            .collect(),
        token_modifiers: karet_core::SemanticTokenModifiers::ALL
            .into_iter()
            .map(|(name, _)| lsp_types::SemanticTokenModifier::new(name))
            .collect(),
        formats: vec![lsp_types::TokenFormat::RELATIVE],
        overlapping_token_support: Some(false),
        multiline_token_support: Some(false),
        server_cancel_support: Some(true),
        augments_syntax_tokens: Some(true),
    }
}

/// What this client does with code actions: literals of every kind it groups
/// by, preferred and disabled flags, and edits deferred to
/// `codeAction/resolve` (whose `data` it hands back untouched).
fn code_action_capabilities() -> lsp_types::CodeActionClientCapabilities {
    let kinds = [
        "",
        "quickfix",
        "refactor",
        "refactor.extract",
        "refactor.inline",
        "refactor.rewrite",
        "source",
        "source.organizeImports",
        "source.fixAll",
    ];
    lsp_types::CodeActionClientCapabilities {
        code_action_literal_support: Some(lsp_types::CodeActionLiteralSupport {
            code_action_kind: lsp_types::CodeActionKindLiteralSupport {
                value_set: kinds.into_iter().map(str::to_owned).collect(),
            },
        }),
        is_preferred_support: Some(true),
        disabled_support: Some(true),
        data_support: Some(true),
        resolve_support: Some(lsp_types::CodeActionCapabilityResolveSupport {
            properties: vec!["edit".to_owned()],
        }),
        ..lsp_types::CodeActionClientCapabilities::default()
    }
}

/// What the client does with the `window/*` traffic.
fn window_capabilities() -> lsp_types::WindowClientCapabilities {
    lsp_types::WindowClientCapabilities {
        // `window/workDoneProgress/create` is acknowledged and `$/progress`
        // relayed through `LspClient::work_progress`.
        work_done_progress: Some(true),
        // `window/showMessageRequest` is relayed through
        // `LspClient::message_requests`; the chosen action goes back exactly
        // as the server sent it, extra properties included.
        show_message: Some(lsp_types::ShowMessageRequestClientCapabilities {
            message_action_item: Some(lsp_types::MessageActionItemCapabilities {
                additional_properties_support: Some(true),
            }),
        }),
        show_document: None,
    }
}

/// Build the `initialize` params advertising what this client actually does.
pub(crate) fn initialize_params(root: &Path) -> Result<lsp_types::InitializeParams, LspError> {
    let root_uri = uri::path_to_uri(root)?;
    let folder_name = root.file_name().map_or_else(
        || "workspace".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    // `dynamicRegistration` is declared exactly where the registration handler
    // honours it: a gated request whose registration carries nothing karet
    // reads beyond "on" and its `documentSelector`. Completion, signature help and code actions are left
    // out on purpose -- their registrations carry trigger characters and
    // action kinds the handler would drop, so a server switching to dynamic
    // registration for them would lose what its handshake would have said.
    let dynamic = lsp_types::DynamicRegistrationClientCapabilities {
        dynamic_registration: Some(true),
    };
    let goto = lsp_types::GotoCapability {
        dynamic_registration: Some(true),
        link_support: None,
    };
    let capabilities = lsp_types::ClientCapabilities {
        general: Some(lsp_types::GeneralClientCapabilities {
            position_encodings: Some(vec![lsp_types::PositionEncodingKind::UTF16]),
            ..lsp_types::GeneralClientCapabilities::default()
        }),
        window: Some(window_capabilities()),
        text_document: Some(lsp_types::TextDocumentClientCapabilities {
            completion: Some(lsp_types::CompletionClientCapabilities {
                completion_item: Some(lsp_types::CompletionItemCapability {
                    // Snippets expand to their text plus their stops (see
                    // the crate docs).
                    snippet_support: Some(true),
                    deprecated_support: Some(true),
                    commit_characters_support: Some(true),
                    documentation_format: Some(vec![
                        lsp_types::MarkupKind::Markdown,
                        lsp_types::MarkupKind::PlainText,
                    ]),
                    tag_support: Some(lsp_types::TagSupport {
                        value_set: vec![lsp_types::CompletionItemTag::DEPRECATED],
                    }),
                    insert_replace_support: Some(true),
                    // What `LspClient::resolve_completion` may fill in later;
                    // what an accept inserts is never among them.
                    resolve_support: Some(lsp_types::CompletionItemCapabilityResolveSupport {
                        properties: vec![
                            "documentation".to_owned(),
                            "detail".to_owned(),
                            "additionalTextEdits".to_owned(),
                        ],
                    }),
                    ..lsp_types::CompletionItemCapability::default()
                }),
                context_support: Some(true),
                ..lsp_types::CompletionClientCapabilities::default()
            }),
            publish_diagnostics: Some(lsp_types::PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                // Mapped to `Diagnostic::code_href`.
                code_description_support: Some(true),
                ..lsp_types::PublishDiagnosticsClientCapabilities::default()
            }),
            // `LspClient::pull_diagnostics`. A registration's options carry
            // only an identifier and dependency flags, which a pull does not
            // need, so registering dynamically loses nothing. Related
            // documents' reports are returned beside the asked one's.
            diagnostic: Some(lsp_types::DiagnosticClientCapabilities {
                dynamic_registration: Some(true),
                related_document_support: Some(true),
            }),
            // Declared at all so a server knows the client renders hints;
            // some only compute them for a client that says so. What a hover
            // or a Ctrl+click needs may be deferred: `LspClient::resolve_inlay_hint`
            // fills it in on demand, handing `data` back untouched.
            inlay_hint: Some(lsp_types::InlayHintClientCapabilities {
                dynamic_registration: Some(true),
                resolve_support: Some(lsp_types::InlayHintResolveClientCapabilities {
                    properties: vec![
                        "tooltip".to_owned(),
                        "label.tooltip".to_owned(),
                        "label.location".to_owned(),
                    ],
                }),
            }),
            hover: Some(lsp_types::HoverClientCapabilities {
                dynamic_registration: Some(true),
                content_format: None,
            }),
            definition: Some(goto),
            implementation: Some(goto),
            call_hierarchy: Some(dynamic),
            type_hierarchy: Some(dynamic),
            document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                dynamic_registration: Some(true),
                ..lsp_types::DocumentSymbolClientCapabilities::default()
            }),
            rename: Some(lsp_types::RenameClientCapabilities {
                dynamic_registration: Some(true),
                // `LspClient::prepare_rename`; a "default behavior" answer is
                // resolved by the consumer to the identifier at the position.
                prepare_support: Some(true),
                prepare_support_default_behavior: Some(
                    lsp_types::PrepareSupportDefaultBehavior::IDENTIFIER,
                ),
                ..lsp_types::RenameClientCapabilities::default()
            }),
            formatting: Some(dynamic),
            range_formatting: Some(dynamic),
            // `LspClient::on_type_formatting`. Without dynamic registration:
            // its options carry the trigger characters, which the gate drops.
            on_type_formatting: Some(
                lsp_types::DocumentOnTypeFormattingClientCapabilities::default(),
            ),
            // `LspClient::linked_editing_ranges`, for Start Linked Editing.
            linked_editing_range: Some(lsp_types::LinkedEditingRangeClientCapabilities {
                dynamic_registration: Some(true),
            }),
            // `LspClient::will_save_wait_until`, asked before every save that
            // may format; `didSave` is sent after every save.
            synchronization: Some(lsp_types::TextDocumentSyncClientCapabilities {
                will_save_wait_until: Some(true),
                did_save: Some(true),
                ..lsp_types::TextDocumentSyncClientCapabilities::default()
            }),
            // `LspClient::selection_ranges`, for Expand Selection.
            selection_range: Some(lsp_types::SelectionRangeClientCapabilities {
                dynamic_registration: Some(true),
            }),
            // `LspClient::document_highlights`, for occurrence highlighting.
            document_highlight: Some(dynamic),
            code_action: Some(code_action_capabilities()),
            // `LspClient::code_lenses`; a deferred title and command are filled
            // in by `LspClient::resolve_code_lens`, handing `data` back
            // untouched. Without dynamic registration, whose options carry the
            // `resolveProvider` flag the gate would drop. And without
            // `workspace.codeLens.refreshSupport`: lenses are re-asked as the
            // text changes instead.
            code_lens: Some(lsp_types::CodeLensClientCapabilities::default()),
            semantic_tokens: Some(semantic_token_capabilities()),
            ..lsp_types::TextDocumentClientCapabilities::default()
        }),
        workspace: Some(lsp_types::WorkspaceClientCapabilities {
            symbol: Some(lsp_types::WorkspaceSymbolClientCapabilities {
                dynamic_registration: Some(true),
                ..lsp_types::WorkspaceSymbolClientCapabilities::default()
            }),
            // Answered, and relayed through `LspClient::refreshes`.
            inlay_hint: Some(lsp_types::InlayHintWorkspaceClientCapabilities {
                refresh_support: Some(true),
            }),
            // Answered, and relayed through `LspClient::refreshes` as
            // `ServerRefresh::SemanticTokens`.
            semantic_tokens: Some(lsp_types::SemanticTokensWorkspaceClientCapabilities {
                refresh_support: Some(true),
            }),
            // Relayed through `LspClient::apply_edit_requests`.
            apply_edit: Some(true),
            // `LspClient::execute_command`; declared without dynamic
            // registration, which would carry a command list the gate drops.
            execute_command: Some(lsp_types::DynamicRegistrationClientCapabilities::default()),
            // Answered from the consumer's settings (`ConnectOptions::settings`),
            // and pushed again by `LspClient::did_change_configuration`.
            configuration: Some(true),
            did_change_configuration: Some(
                lsp_types::DynamicRegistrationClientCapabilities::default(),
            ),
            // The watchers a registration names are kept per registration and
            // matched by `LspClient::did_change_watched_files`; a relative
            // pattern is matched under its base like a document selector's.
            did_change_watched_files: Some(lsp_types::DidChangeWatchedFilesClientCapabilities {
                dynamic_registration: Some(true),
                relative_pattern_support: Some(true),
            }),
            // What `karet_core::WorkspaceEdit` carries. Failure handling is
            // what karet-session's apply path does: an edit of text alone is
            // checked whole before anything changes, while one with file
            // operations stops at the first step that fails.
            workspace_edit: Some(lsp_types::WorkspaceEditClientCapabilities {
                document_changes: Some(true),
                resource_operations: Some(vec![
                    lsp_types::ResourceOperationKind::Create,
                    lsp_types::ResourceOperationKind::Rename,
                    lsp_types::ResourceOperationKind::Delete,
                ]),
                failure_handling: Some(lsp_types::FailureHandlingKind::TextOnlyTransactional),
                ..lsp_types::WorkspaceEditClientCapabilities::default()
            }),
            ..lsp_types::WorkspaceClientCapabilities::default()
        }),
        ..lsp_types::ClientCapabilities::default()
    };
    // `root_uri` is deprecated in favour of `workspace_folders`, but older
    // servers read it exclusively, so we deliberately send both.
    #[allow(deprecated)]
    Ok(lsp_types::InitializeParams {
        process_id: Some(std::process::id()),
        root_uri: Some(root_uri.clone()),
        workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
            uri: root_uri,
            name: folder_name,
        }]),
        capabilities,
        ..lsp_types::InitializeParams::default()
    })
}

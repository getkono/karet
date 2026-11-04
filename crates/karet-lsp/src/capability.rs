//! Reading a server's `initialize` reply into [`Capabilities`] — the neutral
//! record a consumer gates on — and [`ServerOptions`], what this crate alone
//! needs to talk to the server.
//!
//! Parsed from the raw JSON rather than `lsp_types::ServerCapabilities`, on
//! purpose. The typed struct is a snapshot of one crate version's view of the
//! spec: 0.97 has no `typeHierarchyProvider` field at all, and puts
//! `inlineCompletionProvider` behind a `proposed` feature — so gating on it
//! would cap what karet can ever support at whatever `lsp-types` has caught up
//! with, and adding a capability would mean waiting for a dependency release.
//!
//! The protocol's own convention makes the raw read simple and uniform: a
//! provider field is `true`, `false`, an options object, or absent, and
//! anything but `false`/absent means the server provides it. A new capability
//! is then one line here.

use karet_core::Capabilities;
use karet_core::CompletionOptions;
use karet_core::ServerFeature;
use karet_core::SignatureHelpOptions;
use serde_json::Value;

use crate::SemanticTokensLegend;

/// How much text a server wants on each change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TextSyncKind {
    /// The server wants no change notifications at all.
    None,
    /// The server wants the whole document on every change.
    #[default]
    Full,
    /// The server wants only the ranges that changed.
    Incremental,
}

/// The characters that trigger formatting as you type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OnTypeFormattingOptions {
    /// The one character that always triggers it.
    pub(crate) first_trigger_character: char,
    /// Further characters that also trigger it.
    pub(crate) more_trigger_characters: Vec<char>,
}

impl OnTypeFormattingOptions {
    /// Every trigger, the first one first.
    pub(crate) fn triggers(&self) -> Vec<char> {
        let mut all = vec![self.first_trigger_character];
        all.extend(&self.more_trigger_characters);
        all
    }
}

/// What the handshake negotiated beyond the feature set: how this crate must
/// talk to the server, which nothing above it reads.
///
/// `Default` is a server that negotiated nothing: whole-document sync, no
/// legend, no listed commands, no format-on-type triggers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServerOptions {
    /// How much text it wants on a change.
    pub(crate) text_sync: TextSyncKind,
    /// The semantic-token legend, when it emits semantic tokens.
    pub(crate) semantic_tokens_legend: Option<SemanticTokensLegend>,
    /// The commands `workspace/executeCommand` will accept; empty when it
    /// listed none.
    pub(crate) execute_commands: Vec<String>,
    /// The format-on-type triggers, when it formats on type.
    pub(crate) on_type_formatting: Option<OnTypeFormattingOptions>,
}

/// Whether `caps[key]` advertises a provider.
///
/// `false` and absent mean no; `true` and an options object both mean yes. A
/// server that sends something else entirely is taken as a yes, because the
/// only shapes the spec allows there are those, and treating an unrecognised
/// one as a refusal would silently disable a feature the server has.
fn provides(caps: &Value, key: &str) -> bool {
    match caps.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(on)) => *on,
        Some(_) => true,
    }
}

/// A nested boolean flag, defaulting to `false` when any level is missing.
fn flag(value: Option<&Value>, key: &str) -> bool {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The single-character entries of a string array, ignoring any entry that is
/// not exactly one character.
///
/// Trigger characters are single characters in practice and in the spec's
/// intent; a server sending a longer string would otherwise be silently
/// truncated into a wrong trigger.
fn chars(value: Option<&Value>, key: &str) -> Vec<char> {
    let Some(array) = value
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|text| {
            let mut it = text.chars();
            match (it.next(), it.next()) {
                (Some(ch), None) => Some(ch),
                _ => None,
            }
        })
        .collect()
}

/// The string entries of a string array.
fn strings(value: Option<&Value>, key: &str) -> Vec<String> {
    value
        .and_then(|value| value.get(key))
        .and_then(Value::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Read the `capabilities` member of an `initialize` result.
///
/// A reply that carries no capabilities object yields the defaults — a server
/// that supports nothing and negotiated nothing — which is the safe reading:
/// every gated request is then refused rather than issued blind.
pub(crate) fn parse(result: &Value) -> (Capabilities, ServerOptions) {
    let Some(caps) = result.get("capabilities") else {
        return (Capabilities::default(), ServerOptions::default());
    };
    let mut out = Capabilities::default();
    let mut options = ServerOptions::default();

    for (key, feature) in [
        ("hoverProvider", ServerFeature::Hover),
        ("completionProvider", ServerFeature::Completion),
        ("signatureHelpProvider", ServerFeature::SignatureHelp),
        ("declarationProvider", ServerFeature::Declaration),
        ("definitionProvider", ServerFeature::Definition),
        ("typeDefinitionProvider", ServerFeature::TypeDefinition),
        ("implementationProvider", ServerFeature::Implementation),
        ("referencesProvider", ServerFeature::References),
        (
            "documentHighlightProvider",
            ServerFeature::DocumentHighlight,
        ),
        ("documentSymbolProvider", ServerFeature::DocumentSymbol),
        ("workspaceSymbolProvider", ServerFeature::WorkspaceSymbol),
        ("codeActionProvider", ServerFeature::CodeAction),
        ("codeLensProvider", ServerFeature::CodeLens),
        ("documentLinkProvider", ServerFeature::DocumentLink),
        ("colorProvider", ServerFeature::DocumentColor),
        ("documentFormattingProvider", ServerFeature::Formatting),
        (
            "documentRangeFormattingProvider",
            ServerFeature::RangeFormatting,
        ),
        (
            "documentOnTypeFormattingProvider",
            ServerFeature::OnTypeFormatting,
        ),
        ("renameProvider", ServerFeature::Rename),
        ("foldingRangeProvider", ServerFeature::FoldingRange),
        ("selectionRangeProvider", ServerFeature::SelectionRange),
        ("callHierarchyProvider", ServerFeature::CallHierarchy),
        // Absent from `lsp_types` 0.97's struct entirely, which is the reason
        // this reads raw JSON.
        ("typeHierarchyProvider", ServerFeature::TypeHierarchy),
        (
            "linkedEditingRangeProvider",
            ServerFeature::LinkedEditingRange,
        ),
        ("inlayHintProvider", ServerFeature::InlayHint),
        ("inlineValueProvider", ServerFeature::InlineValue),
        // Behind `lsp_types` 0.97's `proposed` feature, so likewise only
        // readable raw.
        ("inlineCompletionProvider", ServerFeature::InlineCompletion),
        ("diagnosticProvider", ServerFeature::PullDiagnostics),
        ("executeCommandProvider", ServerFeature::ExecuteCommand),
    ] {
        out.set(feature, provides(caps, key));
    }

    // Resolve providers are nested flags, not separate capabilities: a server
    // offering completion may or may not fill an item in afterwards, and
    // issuing `completionItem/resolve` at one that does not is exactly the
    // blind request this whole change exists to stop.
    out.set(
        ServerFeature::CompletionResolve,
        flag(caps.get("completionProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::CodeActionResolve,
        flag(caps.get("codeActionProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::CodeLensResolve,
        flag(caps.get("codeLensProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::DocumentLinkResolve,
        flag(caps.get("documentLinkProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::InlayHintResolve,
        flag(caps.get("inlayHintProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::WorkspaceSymbolResolve,
        flag(caps.get("workspaceSymbolProvider"), "resolveProvider"),
    );
    out.set(
        ServerFeature::PrepareRename,
        flag(caps.get("renameProvider"), "prepareProvider"),
    );
    out.set(
        ServerFeature::WorkspacePullDiagnostics,
        flag(caps.get("diagnosticProvider"), "workspaceDiagnostics"),
    );
    out.set(
        ServerFeature::InterFileDiagnostics,
        flag(caps.get("diagnosticProvider"), "interFileDependencies"),
    );

    options.semantic_tokens_legend = parse_semantic_tokens(caps, &mut out);
    options.text_sync = parse_text_sync(caps, &mut out);
    parse_workspace(caps, &mut out);

    out.completion = CompletionOptions {
        trigger_characters: chars(caps.get("completionProvider"), "triggerCharacters"),
        commit_characters: chars(caps.get("completionProvider"), "allCommitCharacters"),
    };
    out.signature_help = SignatureHelpOptions {
        trigger_characters: chars(caps.get("signatureHelpProvider"), "triggerCharacters"),
        retrigger_characters: chars(caps.get("signatureHelpProvider"), "retriggerCharacters"),
    };
    options.execute_commands = strings(caps.get("executeCommandProvider"), "commands");
    options.on_type_formatting = parse_on_type_formatting(caps);
    (out, options)
}

/// Semantic tokens carry a legend whose *order* is the meaning of every packed
/// index, so a result is only interpretable against the legend from the same
/// connection. Returns that legend, when the server emits semantic tokens.
fn parse_semantic_tokens(caps: &Value, out: &mut Capabilities) -> Option<SemanticTokensLegend> {
    let provider = caps.get("semanticTokensProvider")?;
    let legend = provider.get("legend");
    let legend = SemanticTokensLegend {
        token_types: strings(legend, "tokenTypes"),
        token_modifiers: strings(legend, "tokenModifiers"),
    };
    let full = provider.get("full");
    let full_on = matches!(full, Some(Value::Bool(true)) | Some(Value::Object(_)));
    out.set(ServerFeature::SemanticTokensFull, full_on);
    out.set(ServerFeature::SemanticTokensDelta, flag(full, "delta"));
    out.set(
        ServerFeature::SemanticTokensRange,
        provides(provider, "range"),
    );
    Some(legend)
}

/// `textDocumentSync` is either a bare number or an options object; both
/// shapes are current, and servers use both. Returns the sync kind it asks for.
fn parse_text_sync(caps: &Value, out: &mut Capabilities) -> TextSyncKind {
    let Some(sync) = caps.get("textDocumentSync") else {
        return TextSyncKind::default();
    };
    let kind = sync
        .as_i64()
        .or_else(|| sync.get("change").and_then(Value::as_i64));
    out.set(
        ServerFeature::WillSaveWaitUntil,
        flag(Some(sync), "willSaveWaitUntil"),
    );
    match kind {
        Some(0) => TextSyncKind::None,
        Some(2) => TextSyncKind::Incremental,
        // 1 is Full, and so is anything unrecognised: resending the whole
        // document is always correct, just wasteful.
        _ => TextSyncKind::Full,
    }
}

/// Workspace-scoped capabilities live one level down.
fn parse_workspace(caps: &Value, out: &mut Capabilities) {
    let workspace = caps.get("workspace");
    let file_operations = workspace
        .and_then(|workspace| workspace.get("fileOperations"))
        .is_some();
    out.set(ServerFeature::FileOperations, file_operations);
    // Watched-file notifications are almost always registered dynamically
    // rather than advertised here, so this is the floor, not the whole answer.
    out.set(
        ServerFeature::DidChangeWatchedFiles,
        flag(
            workspace.and_then(|workspace| workspace.get("didChangeWatchedFiles")),
            "dynamicRegistration",
        ),
    );
}

fn parse_on_type_formatting(caps: &Value) -> Option<OnTypeFormattingOptions> {
    let provider = caps.get("documentOnTypeFormattingProvider")?;
    let first = provider
        .get("firstTriggerCharacter")
        .and_then(Value::as_str)?
        .chars()
        .next()?;
    Some(OnTypeFormattingOptions {
        first_trigger_character: first,
        more_trigger_characters: chars(Some(provider), "moreTriggerCharacter"),
    })
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod tests;

/// The feature a dynamic registration's method name enables.
///
/// `client/registerCapability` names a *method*, not a provider field, so this
/// is a second mapping onto [`ServerFeature`] rather than a reuse of the
/// handshake's. Servers that advertise little or nothing statically and
/// register afterwards are common — haskell-language-server, eslint and some
/// jdtls configurations all do it — so a gate that ignored registrations would
/// refuse them features they really have.
pub(crate) fn feature_for_method(method: &str) -> Option<ServerFeature> {
    Some(match method {
        "textDocument/completion" => ServerFeature::Completion,
        "textDocument/hover" => ServerFeature::Hover,
        "textDocument/signatureHelp" => ServerFeature::SignatureHelp,
        "textDocument/declaration" => ServerFeature::Declaration,
        "textDocument/definition" => ServerFeature::Definition,
        "textDocument/typeDefinition" => ServerFeature::TypeDefinition,
        "textDocument/implementation" => ServerFeature::Implementation,
        "textDocument/references" => ServerFeature::References,
        "textDocument/documentHighlight" => ServerFeature::DocumentHighlight,
        "textDocument/documentSymbol" => ServerFeature::DocumentSymbol,
        "workspace/symbol" => ServerFeature::WorkspaceSymbol,
        "textDocument/codeAction" => ServerFeature::CodeAction,
        "textDocument/codeLens" => ServerFeature::CodeLens,
        "textDocument/documentLink" => ServerFeature::DocumentLink,
        "textDocument/documentColor" => ServerFeature::DocumentColor,
        "textDocument/formatting" => ServerFeature::Formatting,
        "textDocument/rangeFormatting" => ServerFeature::RangeFormatting,
        "textDocument/onTypeFormatting" => ServerFeature::OnTypeFormatting,
        "textDocument/rename" => ServerFeature::Rename,
        "textDocument/foldingRange" => ServerFeature::FoldingRange,
        "textDocument/selectionRange" => ServerFeature::SelectionRange,
        "textDocument/prepareCallHierarchy" => ServerFeature::CallHierarchy,
        "textDocument/prepareTypeHierarchy" => ServerFeature::TypeHierarchy,
        "textDocument/linkedEditingRange" => ServerFeature::LinkedEditingRange,
        "textDocument/inlayHint" => ServerFeature::InlayHint,
        "textDocument/inlineValue" => ServerFeature::InlineValue,
        "textDocument/inlineCompletion" => ServerFeature::InlineCompletion,
        "textDocument/diagnostic" => ServerFeature::PullDiagnostics,
        "textDocument/semanticTokens" => ServerFeature::SemanticTokensFull,
        "workspace/executeCommand" => ServerFeature::ExecuteCommand,
        "workspace/didChangeWatchedFiles" => ServerFeature::DidChangeWatchedFiles,
        "workspace/willRenameFiles" | "workspace/didRenameFiles" => ServerFeature::FileOperations,
        _ => return None,
    })
}

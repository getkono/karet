//! What a language server said it can do.
//!
//! A server advertises its capabilities once, at the handshake, and they vary
//! enormously between servers: no two of rust-analyzer, gopls, jdtls and
//! `vscode-json-language-server` implement the same set. A client that ignores
//! the advertisement issues every request blind, and a request a server does
//! not implement comes back as a protocol error indistinguishable from a
//! failure — so a missing feature reads as a broken server rather than an
//! absent one.
//!
//! [`Capabilities`] is the neutral record of that advertisement, and
//! [`ServerFeature`] names the things karet gates on. The mapping from the
//! protocol's own shape lives in the producer (`karet-lsp`); everything above
//! it asks only `supports`.
//!
//! This is a **mutable** record on purpose. Servers may add and remove
//! capabilities after the handshake through dynamic registration, which is how
//! typescript-language-server asks for file-watching and how several servers
//! enable formatting, so a frozen snapshot would be wrong for them.

use std::collections::BTreeSet;

/// One capability karet gates a request on.
///
/// Named for the editor feature rather than the wire method, because that is
/// what a caller is deciding about; the producer maps protocol names onto
/// these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ServerFeature {
    /// Completion candidates at a position.
    Completion,
    /// Filling in a completion item's detail on demand.
    CompletionResolve,
    /// Documentation for the symbol at a position.
    Hover,
    /// Parameter help at a call site.
    SignatureHelp,
    /// Jump to where a symbol is declared.
    Declaration,
    /// Jump to where a symbol is defined.
    Definition,
    /// Jump to the definition of a symbol's type.
    TypeDefinition,
    /// Jump to the implementations of an interface or trait.
    Implementation,
    /// Find every reference to a symbol.
    References,
    /// Highlight the other occurrences of the symbol under the caret.
    DocumentHighlight,
    /// The symbol outline of one document.
    DocumentSymbol,
    /// Symbol search across the workspace.
    WorkspaceSymbol,
    /// Filling in a workspace symbol's location on demand.
    WorkspaceSymbolResolve,
    /// Quick fixes and refactors for a range.
    CodeAction,
    /// Filling in a code action's edit on demand.
    CodeActionResolve,
    /// Actionable annotations above a line.
    CodeLens,
    /// Filling in a code lens's command on demand.
    CodeLensResolve,
    /// Navigable links within a document.
    DocumentLink,
    /// Filling in a document link's target on demand.
    DocumentLinkResolve,
    /// Colour literals and their presentations.
    DocumentColor,
    /// Formatting a whole document.
    Formatting,
    /// Formatting a range.
    RangeFormatting,
    /// Formatting as you type.
    OnTypeFormatting,
    /// Renaming a symbol across the workspace.
    Rename,
    /// Validating a rename, and finding the range to seed the prompt with.
    PrepareRename,
    /// Server-computed fold regions.
    FoldingRange,
    /// Server-computed expand/shrink selection ranges.
    SelectionRange,
    /// Incoming and outgoing call hierarchy.
    CallHierarchy,
    /// Supertype and subtype hierarchy.
    TypeHierarchy,
    /// Semantic tokens for a whole document.
    SemanticTokensFull,
    /// Semantic tokens as a delta against a previous result.
    SemanticTokensDelta,
    /// Semantic tokens for a range.
    SemanticTokensRange,
    /// Ranges that must be renamed together, such as an HTML tag pair.
    LinkedEditingRange,
    /// Inferred types and parameter names rendered inline.
    InlayHint,
    /// Filling in an inlay hint's tooltip or edits on demand.
    InlayHintResolve,
    /// Values shown inline while debugging.
    InlineValue,
    /// Text proposed at the caret, shown as ghost text to accept or type over
    /// (LSP 3.18 `textDocument/inlineCompletion`).
    InlineCompletion,
    /// Diagnostics the client pulls, rather than the server pushing them.
    PullDiagnostics,
    /// Pulling diagnostics for the whole workspace at once.
    WorkspacePullDiagnostics,
    /// Pulled diagnostics that depend on other files
    /// (`diagnosticProvider.interFileDependencies`): an edit to one document
    /// can change another's, so every open document is pulled again.
    InterFileDiagnostics,
    /// Running a server-provided command.
    ExecuteCommand,
    /// Edits the server supplies on save, before the write.
    WillSaveWaitUntil,
    /// Being told which files changed on disk.
    DidChangeWatchedFiles,
    /// Being told about file creates, renames and deletes.
    FileOperations,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// The characters that open, and close, a completion session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionOptions {
    /// Typing one of these asks the server for candidates.
    pub trigger_characters: Vec<char>,
    /// Typing one of these accepts the selected candidate.
    pub commit_characters: Vec<char>,
}

/// The characters that open, and re-open, signature help.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SignatureHelpOptions {
    /// Typing one of these opens the signature popup.
    pub trigger_characters: Vec<char>,
    /// Typing one of these moves to the next parameter.
    pub retrigger_characters: Vec<char>,
}

/// What a server said it can do, plus the trigger characters a consumer reacts
/// to as the user types.
///
/// Only what a layer above the producer reads lives here. How the producer
/// talks to the server — column encoding, document sync, the semantic-token
/// legend, the commands it accepts — stays in the producer.
///
/// `Default` is "a server that advertised nothing", which is the honest
/// starting point before a handshake completes: every gated request is refused
/// rather than issued blind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Capabilities {
    /// The features currently available.
    features: BTreeSet<ServerFeature>,
    /// Completion trigger and commit characters.
    pub completion: CompletionOptions,
    /// Signature-help trigger and retrigger characters.
    pub signature_help: SignatureHelpOptions,
}

impl Capabilities {
    /// Whether `feature` is currently available.
    #[must_use]
    pub fn supports(&self, feature: ServerFeature) -> bool {
        self.features.contains(&feature)
    }

    /// Turn `feature` on.
    ///
    /// Used both when reading the handshake and when a server registers a
    /// capability afterwards.
    pub fn enable(&mut self, feature: ServerFeature) {
        self.features.insert(feature);
    }

    /// Turn `feature` off, reporting whether it had been on.
    pub fn disable(&mut self, feature: ServerFeature) -> bool {
        self.features.remove(&feature)
    }

    /// Turn `feature` on or off.
    pub fn set(&mut self, feature: ServerFeature, enabled: bool) {
        if enabled {
            self.enable(feature);
        } else {
            self.disable(feature);
        }
    }

    /// Every feature currently available, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = ServerFeature> + '_ {
        self.features.iter().copied()
    }

    /// How many features are available.
    #[must_use]
    pub fn len(&self) -> usize {
        self.features.len()
    }

    /// Whether no [`ServerFeature`] is available.
    ///
    /// Only the feature set is consulted; the trigger characters may still
    /// carry advertised values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Whether a completion session should open on `ch`.
    #[must_use]
    pub fn is_completion_trigger(&self, ch: char) -> bool {
        self.completion.trigger_characters.contains(&ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_said_nothing_supports_nothing() {
        let capabilities = Capabilities::default();
        assert!(capabilities.is_empty());
        assert_eq!(capabilities.len(), 0);
        // The point of the default: before a handshake lands, every gated
        // request is refused rather than issued blind.
        assert!(!capabilities.supports(ServerFeature::Hover));
        assert!(!capabilities.supports(ServerFeature::Rename));
        // And no character opens anything.
        assert_eq!(capabilities.completion, CompletionOptions::default());
        assert_eq!(capabilities.signature_help, SignatureHelpOptions::default());
        assert!(!capabilities.is_completion_trigger('.'));
    }

    #[test]
    fn features_toggle_and_enumerate_in_a_stable_order() {
        let mut capabilities = Capabilities::default();
        capabilities.enable(ServerFeature::Hover);
        capabilities.enable(ServerFeature::Rename);
        assert!(capabilities.supports(ServerFeature::Hover));
        assert_eq!(capabilities.len(), 2);
        assert!(!capabilities.is_empty());

        // Enabling twice is idempotent, so a re-registration cannot double-count.
        capabilities.enable(ServerFeature::Hover);
        assert_eq!(capabilities.len(), 2);

        assert!(capabilities.disable(ServerFeature::Hover));
        // Reports whether it *had* been on, so an unregister for something that
        // was never registered is distinguishable.
        assert!(!capabilities.disable(ServerFeature::Hover));
        assert!(!capabilities.supports(ServerFeature::Hover));

        capabilities.set(ServerFeature::Definition, true);
        capabilities.set(ServerFeature::Rename, false);
        let listed: Vec<_> = capabilities.iter().collect();
        assert_eq!(listed, vec![ServerFeature::Definition]);
    }

    #[test]
    fn trigger_characters_answer_per_character() {
        let capabilities = Capabilities {
            completion: CompletionOptions {
                trigger_characters: vec!['.', ':'],
                commit_characters: vec!['('],
            },
            signature_help: SignatureHelpOptions {
                trigger_characters: vec!['('],
                retrigger_characters: vec![','],
            },
            ..Capabilities::default()
        };
        assert!(capabilities.is_completion_trigger('.'));
        assert!(capabilities.is_completion_trigger(':'));
        assert!(!capabilities.is_completion_trigger('x'));
        // A commit character is not a trigger, and neither is a signature-help
        // trigger: each list answers only for itself.
        assert!(!capabilities.is_completion_trigger('('));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn capabilities_round_trip_and_degrade_to_what_was_said() -> Result<(), serde_json::Error> {
        let mut capabilities = Capabilities {
            completion: CompletionOptions {
                trigger_characters: vec!['.'],
                commit_characters: vec!['('],
            },
            signature_help: SignatureHelpOptions {
                trigger_characters: vec!['('],
                retrigger_characters: vec![','],
            },
            ..Capabilities::default()
        };
        capabilities.enable(ServerFeature::Hover);
        capabilities.enable(ServerFeature::ExecuteCommand);
        let json = serde_json::to_string(&capabilities)?;
        assert_eq!(serde_json::from_str::<Capabilities>(&json)?, capabilities);

        // A newer peer's feature decodes to `Unknown` beside the known ones, and
        // every field it left out takes the "advertised nothing" default.
        let newer = serde_json::from_str::<Capabilities>(r#"{"features":["Hover","Telepathy"]}"#)?;
        assert_eq!(
            newer.iter().collect::<Vec<_>>(),
            vec![ServerFeature::Hover, ServerFeature::Unknown]
        );
        assert_eq!(newer.completion, CompletionOptions::default());
        assert!(!newer.supports(ServerFeature::Rename));

        // An older peer still sends the negotiation fields this record no
        // longer carries; they are ignored rather than failing the payload.
        let older = serde_json::from_str::<Capabilities>(
            r#"{"features":["Hover"],"position_encoding":"Utf8","text_sync":"Incremental","execute_commands":["run"]}"#,
        )?;
        assert_eq!(older.iter().collect::<Vec<_>>(), vec![ServerFeature::Hover]);
        Ok(())
    }
}

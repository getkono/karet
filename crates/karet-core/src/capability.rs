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
    /// Diagnostics the client pulls, rather than the server pushing them.
    PullDiagnostics,
    /// Pulling diagnostics for the whole workspace at once.
    WorkspacePullDiagnostics,
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

/// How a server counts the columns in a position.
///
/// LSP's historical default is UTF-16, which is why it is the fallback here,
/// but a server may negotiate something else and clangd prefers UTF-8. Getting
/// this wrong misplaces every position in a line containing any non-ASCII
/// character — and silently, because ASCII-only lines agree under all three.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum PositionEncoding {
    /// Columns count UTF-8 bytes.
    Utf8,
    /// Columns count UTF-16 code units. The protocol's default.
    #[default]
    Utf16,
    /// Columns count Unicode scalar values.
    Utf32,
}

/// How much text a server wants on each change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum TextSyncKind {
    /// The server wants no change notifications at all.
    None,
    /// The server wants the whole document on every change.
    #[default]
    Full,
    /// The server wants only the ranges that changed.
    Incremental,
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

/// The token types and modifiers a server will emit, in the order its packed
/// results index into.
///
/// The order is the whole meaning of a semantic-token result: an index is only
/// interpretable against the legend the same connection advertised, so this
/// must be re-read whenever a server is replaced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SemanticTokensLegend {
    /// Token types, indexed by the value in a token's `type` slot.
    pub token_types: Vec<String>,
    /// Token modifiers, indexed by bit position in a token's modifier mask.
    pub token_modifiers: Vec<String>,
}

/// The characters that trigger formatting as you type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OnTypeFormattingOptions {
    /// The one character that always triggers it.
    pub first_trigger_character: char,
    /// Further characters that also trigger it.
    pub more_trigger_characters: Vec<char>,
}

/// What a server said it can do, plus the options that shape how karet asks.
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
    /// How this server counts columns.
    pub position_encoding: PositionEncoding,
    /// How much text it wants on a change.
    pub text_sync: TextSyncKind,
    /// Whether it wants the document's text with a save notification.
    pub save_includes_text: bool,
    /// Completion trigger and commit characters.
    pub completion: CompletionOptions,
    /// Signature-help trigger and retrigger characters.
    pub signature_help: SignatureHelpOptions,
    /// The code-action kinds it offers, empty when it did not say.
    pub code_action_kinds: Vec<String>,
    /// The semantic-token legend, when it emits semantic tokens.
    pub semantic_tokens_legend: Option<SemanticTokensLegend>,
    /// The commands `workspace/executeCommand` will accept.
    pub execute_commands: Vec<String>,
    /// The format-on-type triggers, when it formats on type.
    pub on_type_formatting: Option<OnTypeFormattingOptions>,
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

    /// Whether the server advertised nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// Whether a completion session should open on `ch`.
    #[must_use]
    pub fn is_completion_trigger(&self, ch: char) -> bool {
        self.completion.trigger_characters.contains(&ch)
    }

    /// Whether signature help should open on `ch`.
    #[must_use]
    pub fn is_signature_help_trigger(&self, ch: char) -> bool {
        self.signature_help.trigger_characters.contains(&ch)
    }

    /// Whether `command` is one the server said it accepts.
    ///
    /// A server that advertised [`ServerFeature::ExecuteCommand`] without
    /// listing any command is taken at its word for every command: the list is
    /// optional in the protocol, and refusing everything would be worse than
    /// letting the server decline one it does not know.
    #[must_use]
    pub fn accepts_command(&self, command: &str) -> bool {
        if !self.supports(ServerFeature::ExecuteCommand) {
            return false;
        }
        self.execute_commands.is_empty()
            || self.execute_commands.iter().any(|known| known == command)
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
        // LSP's historical default, and the one a server that omits the field
        // is promising.
        assert_eq!(capabilities.position_encoding, PositionEncoding::Utf16);
        assert_eq!(capabilities.text_sync, TextSyncKind::Full);
    }

    #[test]
    fn features_toggle_and_enumerate_in_a_stable_order() {
        let mut capabilities = Capabilities::default();
        capabilities.enable(ServerFeature::Hover);
        capabilities.enable(ServerFeature::Rename);
        assert!(capabilities.supports(ServerFeature::Hover));
        assert_eq!(capabilities.len(), 2);

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
        assert!(!capabilities.is_completion_trigger('x'));
        assert!(capabilities.is_signature_help_trigger('('));
        assert!(!capabilities.is_signature_help_trigger('.'));
    }

    #[test]
    fn execute_command_is_refused_unless_advertised() {
        let mut capabilities = Capabilities::default();
        assert!(!capabilities.accepts_command("rust-analyzer.runSingle"));

        // Advertised with no list: the list is optional in the protocol, so
        // take the server at its word rather than refusing everything.
        capabilities.enable(ServerFeature::ExecuteCommand);
        assert!(capabilities.accepts_command("rust-analyzer.runSingle"));

        capabilities
            .execute_commands
            .push("rust-analyzer.runSingle".to_owned());
        assert!(capabilities.accepts_command("rust-analyzer.runSingle"));
        assert!(!capabilities.accepts_command("gopls.tidy"));
    }
}

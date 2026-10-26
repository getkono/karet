//! Neutral producer→renderer models — the shared "currency" of the toolkit.
//!
//! Producers (`karet-lsp`, `karet-vcs`, `karet-dap`, spell-check, …) emit these;
//! renderers (`karet-editor`, `karet-widgets`) consume them. They reference only
//! [`coord`](crate::coord) and [`token`](crate::token) types so they stay cheap to
//! serialize across the client-server seam. The structs are intentionally *not*
//! `#[non_exhaustive]` so producers can construct them with literal syntax.

use std::path::PathBuf;

use crate::coord::Range;
use crate::edit::WorkspaceEdit;

mod completion;
mod decoration;
mod hierarchy;
mod inlay;
mod lens;
mod virtual_text;

pub use completion::CompletionItem;
pub use completion::CompletionKind;
pub use completion::CompletionList;
pub use completion::CompletionTrigger;
pub use completion::Tabstop;
pub use decoration::Decoration;
pub use decoration::DecorationColor;
pub use decoration::DecorationKind;
pub use decoration::TextModifiers;
pub use hierarchy::HierarchyDirection;
pub use hierarchy::HierarchyItem;
pub use inlay::InlayHint;
pub use inlay::InlayHintKind;
pub use inlay::InlayHintLabelPart;
pub use lens::CodeLens;
pub use virtual_text::EolAlign;
pub use virtual_text::VirtualLine;
pub use virtual_text::VirtualLineSide;
pub use virtual_text::VirtualText;
pub use virtual_text::VirtualTextPlacement;

/// Severity of a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Severity {
    /// A hard error.
    Error,
    /// A warning.
    Warning,
    /// Informational.
    Information,
    /// A hint (often rendered subtly).
    Hint,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// A rendering hint attached to a [`Diagnostic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DiagnosticTag {
    /// Unused or unnecessary code (often dimmed).
    Unnecessary,
    /// Deprecated code (often struck through).
    Deprecated,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// A secondary location related to a [`Diagnostic`] (e.g. "first defined here").
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RelatedInfo {
    /// Where the related information points.
    pub location: Location,
    /// A human-readable description.
    pub message: String,
}

/// A diagnostic (error/warning/info/hint) anchored to a source range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Diagnostic {
    /// The affected range.
    pub range: Range,
    /// Severity.
    pub severity: Severity,
    /// The message text.
    pub message: String,
    /// The producing tool (e.g. `"rustc"`, `"eslint"`).
    pub source: Option<String>,
    /// A machine-readable code, if any.
    pub code: Option<String>,
    /// A link to documentation explaining [`code`](Self::code) (LSP
    /// `codeDescription.href`), kept verbatim.
    #[cfg_attr(feature = "serde", serde(default))]
    pub code_href: Option<String>,
    /// Rendering tags.
    pub tags: Vec<DiagnosticTag>,
    /// Related locations.
    pub related: Vec<RelatedInfo>,
}

/// The kind of a [`Symbol`] (mirrors the LSP `SymbolKind` set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum SymbolKind {
    /// A file.
    File,
    /// A module.
    Module,
    /// A namespace.
    Namespace,
    /// A package.
    Package,
    /// A class.
    Class,
    /// A method.
    Method,
    /// A property.
    Property,
    /// A field.
    Field,
    /// A constructor.
    Constructor,
    /// An enum.
    Enum,
    /// An interface.
    Interface,
    /// A function.
    Function,
    /// A variable.
    Variable,
    /// A constant.
    Constant,
    /// A string value.
    String,
    /// A numeric value.
    Number,
    /// A boolean value.
    Boolean,
    /// An array value.
    Array,
    /// An object value.
    Object,
    /// An object key.
    Key,
    /// A null value.
    Null,
    /// A struct.
    Struct,
    /// An enum member.
    EnumMember,
    /// An event.
    Event,
    /// An operator.
    Operator,
    /// A type parameter.
    TypeParameter,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// A document or workspace symbol, possibly with nested children.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Symbol {
    /// The symbol's name.
    pub name: String,
    /// Its kind.
    pub kind: SymbolKind,
    /// Optional detail (e.g. a signature).
    pub detail: Option<String>,
    /// The full range of the symbol's definition.
    pub range: Range,
    /// The range to select/reveal (usually just the name).
    pub selection_range: Range,
    /// The containing symbol's name, if known.
    pub container_name: Option<String>,
    /// Nested child symbols.
    pub children: Vec<Symbol>,
}

/// One hit of a workspace-wide symbol search: a symbol and the file it lives
/// in, so a picker can open it without the document being open.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkspaceSymbol {
    /// The symbol's name.
    pub name: String,
    /// Its kind.
    pub kind: SymbolKind,
    /// The containing symbol's name, if known.
    pub container_name: Option<String>,
    /// The file the symbol is in.
    pub path: PathBuf,
    /// Where in `path` the symbol is, or `None` when the producer named only
    /// the file (LSP's URI-only form, which a later resolve would complete).
    pub range: Option<Range>,
}

/// An opaque, application-resolved command identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandId(pub String);

/// A path-based location (works for files that are not currently open).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Location {
    /// The file path.
    pub path: PathBuf,
    /// The range within the file.
    pub range: Range,
}

/// The flavor of a [`Markup`] payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MarkupKind {
    /// Plain text.
    PlainText,
    /// CommonMark markdown (rendered by `karet-markdown` at the edge).
    Markdown,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// A documentation payload carried verbatim; rendered by the presentation layer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Markup {
    /// The payload flavor.
    pub kind: MarkupKind,
    /// The raw content.
    pub value: String,
}

/// Hover information for a position.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Hover {
    /// The hover contents.
    pub contents: Markup,
    /// The range the hover applies to, if known.
    pub range: Option<Range>,
}

/// One parameter within a [`Signature`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParamInfo {
    /// The parameter label.
    pub label: String,
    /// Optional parameter documentation.
    pub documentation: Option<Markup>,
}

/// One overload within [`SignatureHelp`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Signature {
    /// The full signature label.
    pub label: String,
    /// Optional signature documentation.
    pub documentation: Option<Markup>,
    /// The parameters, in order.
    pub parameters: Vec<ParamInfo>,
}

/// Signature help for a call site.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SignatureHelp {
    /// The candidate signatures.
    pub signatures: Vec<Signature>,
    /// Index of the active signature.
    pub active_signature: usize,
    /// Index of the active parameter within the active signature.
    pub active_parameter: usize,
}

/// A server's answer to "can the symbol here be renamed?" (LSP
/// `textDocument/prepareRename`), when the answer is yes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum PrepareRename {
    /// Rename the text in `range`.
    Range {
        /// The span the new name replaces.
        range: Range,
        /// The text to prefill the new name with; `None` means the text in
        /// `range`.
        placeholder: Option<String>,
    },
    /// The position is renameable, and the client picks the identifier around
    /// it by its own rules.
    DefaultBehavior,
}

/// A code action (quick fix / refactor) offered for a range.
///
/// Running one means applying its [`edit`](Self::edit), then its
/// [`command`](Self::command). An action may arrive with neither filled in
/// yet: its producer fills them in on request, from
/// [`payload`](Self::payload).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodeAction {
    /// The action title.
    pub title: String,
    /// The action's kind, a dot-separated hierarchy such as `quickfix`,
    /// `refactor.extract`, or `source.organizeImports`; `None` when the
    /// producer gave none.
    pub kind: Option<String>,
    /// Whether the producer marks this the preferred way to act (the fix a
    /// "fix all" or auto-fix would pick).
    pub preferred: bool,
    /// Why the action cannot run right now, when the producer offers it
    /// anyway so the user can see it exists.
    pub disabled: Option<String>,
    /// An edit to apply.
    pub edit: Option<WorkspaceEdit>,
    /// A command to run.
    pub command: Option<CommandId>,
    /// The producer's own encoding of the action, opaque to everyone else.
    ///
    /// Handed back to the producer to fill in a deferred edit or to run the
    /// command; `None` when the producer needs nothing back.
    pub payload: Option<String>,
}

impl CodeAction {
    /// Whether this action's kind is `kind` or nested under it (`refactor`
    /// matches `refactor.extract`, but not `refactoring`). An action with no
    /// kind matches no kind.
    #[must_use]
    pub fn is_kind(&self, kind: &str) -> bool {
        self.kind.as_deref().is_some_and(|own| {
            own.strip_prefix(kind)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "serde")]
    use crate::token::ThemeRole;
    #[cfg(feature = "serde")]
    use crate::token::TokenId;

    #[test]
    fn severity_orders_most_severe_first() {
        assert!(Severity::Error < Severity::Hint);
    }

    /// A kind matches itself and whatever nests under it, never a mere
    /// string prefix, and an action without a kind matches nothing.
    #[test]
    fn code_action_kinds_match_by_hierarchy() {
        let action = |kind: Option<&str>| CodeAction {
            kind: kind.map(str::to_owned),
            ..CodeAction::default()
        };
        let extract = action(Some("refactor.extract"));
        assert!(extract.is_kind("refactor"));
        assert!(extract.is_kind("refactor.extract"));
        assert!(!extract.is_kind("refactor.ex"));
        assert!(!extract.is_kind("source"));
        assert!(!action(Some("refactoring")).is_kind("refactor"));
        assert!(!action(None).is_kind("quickfix"));
    }

    /// The wire payloads the backend seam actually carries must round-trip; this
    /// is the compile- and run-time guarantee behind the "serde-ready" claim.
    #[cfg(feature = "serde")]
    #[test]
    fn wire_payloads_round_trip_through_serde() -> Result<(), serde_json::Error> {
        fn rt<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
            value: &T,
        ) -> Result<(), serde_json::Error> {
            let json = serde_json::to_string(value)?;
            assert_eq!(&serde_json::from_str::<T>(&json)?, value);
            Ok(())
        }

        rt(&Diagnostic {
            range: Range::default(),
            severity: Severity::Warning,
            message: "m".into(),
            source: None,
            code: Some("E0001".into()),
            code_href: Some("https://example.com/E0001".into()),
            tags: vec![DiagnosticTag::Unnecessary],
            related: vec![RelatedInfo {
                location: Location {
                    path: PathBuf::from("a.rs"),
                    range: Range::default(),
                },
                message: "here".into(),
            }],
        })?;
        rt(&Decoration::new(
            Range::default(),
            DecorationKind::ColorSwatch { rgba: [1, 2, 3, 4] },
            Some(ThemeRole::Muted),
        ))?;
        rt(
            &Decoration::new(Range::default(), DecorationKind::TextStyle, None)
                .with_fg(TokenId::KEYWORD)
                .with_modifiers(TextModifiers::BOLD | TextModifiers::DIM)
                .with_priority(5),
        )?;
        // A payload from before the style fields existed still decodes, as
        // an unstyled decoration at the default priority.
        let old = r#"{"range":{"start":{"line":0,"col":0},"end":{"line":0,"col":1}},"kind":"TextBackground","role":null}"#;
        assert_eq!(
            serde_json::from_str::<Decoration>(old)?,
            Decoration::new(
                Range {
                    start: crate::coord::LineCol::new(0, 0),
                    end: crate::coord::LineCol::new(0, 1),
                },
                DecorationKind::TextBackground,
                None,
            )
        );
        rt(&VirtualText {
            line: 3,
            placement: VirtualTextPlacement::EndOfLine {
                align: EolAlign::Right,
                priority: 7,
            },
            text: "blame".into(),
            role: ThemeRole::Muted,
        })?;
        rt(&VirtualText::inline(0, 4, ": i32", ThemeRole::InlayHint))?;
        rt(&VirtualLine::above(1, "Run test", ThemeRole::Muted))?;
        rt(&VirtualLine::below(1, "- old", ThemeRole::Muted))?;
        rt(&CodeLens {
            title: Some("Run".into()),
            command: Some(CommandId("run".into())),
            payload: Some("{}".into()),
            ..CodeLens::default()
        })?;
        rt(&Symbol {
            name: "f".into(),
            kind: SymbolKind::Function,
            detail: None,
            range: Range::default(),
            selection_range: Range::default(),
            container_name: None,
            children: Vec::new(),
        })?;
        rt(&CompletionItem::new("push", CompletionKind::Method))?;
        rt(&Hover {
            contents: Markup {
                kind: MarkupKind::Markdown,
                value: "# t".into(),
            },
            range: None,
        })?;
        rt(&SignatureHelp {
            signatures: vec![Signature {
                label: "f(x)".into(),
                documentation: None,
                parameters: vec![ParamInfo {
                    label: "x".into(),
                    documentation: None,
                }],
            }],
            active_signature: 0,
            active_parameter: 0,
        })?;
        Ok(())
    }

    /// A newer peer's unknown enum value deserializes to the `Unknown` fallback
    /// instead of failing the whole payload.
    #[cfg(feature = "serde")]
    #[test]
    fn unknown_enum_values_degrade_instead_of_failing() -> Result<(), serde_json::Error> {
        assert_eq!(
            serde_json::from_str::<Severity>("\"Catastrophic\"")?,
            Severity::Unknown
        );
        assert_eq!(
            serde_json::from_str::<SymbolKind>("\"Quasar\"")?,
            SymbolKind::Unknown
        );
        assert_eq!(
            serde_json::from_str::<CompletionKind>("\"Novel\"")?,
            CompletionKind::Unknown
        );
        assert_eq!(
            serde_json::from_str::<DiagnosticTag>("\"Experimental\"")?,
            DiagnosticTag::Unknown
        );
        assert_eq!(
            serde_json::from_str::<MarkupKind>("\"AsciiDoc\"")?,
            MarkupKind::Unknown
        );
        // The fallback absorbs one field, not the payload around it.
        let diagnostic = r#"{"range":{"start":{"line":0,"col":0},"end":{"line":0,"col":0}},
            "severity":"Fatal","message":"m","source":null,"code":null,"code_href":null,
            "tags":["Unnecessary","Experimental"],"related":[]}"#;
        let diagnostic = serde_json::from_str::<Diagnostic>(diagnostic)?;
        assert_eq!(diagnostic.severity, Severity::Unknown);
        assert_eq!(
            diagnostic.tags,
            vec![DiagnosticTag::Unnecessary, DiagnosticTag::Unknown]
        );
        assert_eq!(diagnostic.message, "m");
        Ok(())
    }
}

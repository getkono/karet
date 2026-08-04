//! Neutral producer→renderer models — the shared "currency" of the toolkit.
//!
//! Producers (`karet-lsp`, `karet-vcs`, `karet-dap`, spell-check, …) emit these;
//! renderers (`karet-editor`, `karet-widgets`) consume them. They reference only
//! [`coord`](crate::coord) and [`token`](crate::token) types so they stay cheap to
//! serialize across the client-server seam. The structs are intentionally *not*
//! `#[non_exhaustive]` so producers can construct them with literal syntax.

use std::path::PathBuf;

use crate::coord::LineCol;
use crate::coord::Range;
use crate::edit::TextEdit;
use crate::edit::WorkspaceEdit;
use crate::token::ThemeRole;

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
    /// Rendering tags.
    pub tags: Vec<DiagnosticTag>,
    /// Related locations.
    pub related: Vec<RelatedInfo>,
}

/// The visual style of an [`DecorationKind::Underline`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum UnderlineStyle {
    /// A straight underline.
    Straight,
    /// A curly/squiggly underline.
    Curly,
    /// A dotted underline.
    Dotted,
    /// A dashed underline.
    Dashed,
    /// A double underline.
    Double,
}

/// The kind of a [`Decoration`] — the visual treatment to apply to a range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DecorationKind {
    /// A glyph in the gutter (e.g. a VCS change marker).
    GutterMarker {
        /// The glyph to draw.
        glyph: char,
    },
    /// A named gutter icon, resolved by the renderer.
    GutterIcon {
        /// Icon identifier.
        id: u16,
    },
    /// Highlight the whole line's background.
    LineBackground,
    /// Highlight the text background within the range.
    TextBackground,
    /// Underline the range with the given style.
    Underline(UnderlineStyle),
    /// Strike through the range.
    Strikethrough,
    /// A debugger breakpoint marker.
    Breakpoint {
        /// Whether the breakpoint is enabled.
        enabled: bool,
        /// Whether it is conditional.
        condition: bool,
    },
    /// Inline "ghost" text (e.g. VCS blame, parameter names).
    InlineText {
        /// The text to render.
        text: String,
        /// Whether to render before (`true`) or after (`false`) the range.
        before: bool,
    },
}

/// A presentation-neutral decoration: a range plus a visual treatment.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Decoration {
    /// The decorated range.
    pub range: Range,
    /// How to decorate it.
    pub kind: DecorationKind,
    /// An optional theme role supplying the color.
    pub role: Option<ThemeRole>,
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

/// The kind of an [`InlayHint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InlayHintKind {
    /// An inferred type annotation.
    Type,
    /// A parameter name.
    Parameter,
}

/// An inline hint rendered between characters (inferred types, parameter names).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InlayHint {
    /// Where the hint is anchored.
    pub position: LineCol,
    /// The hint label.
    pub label: String,
    /// The hint kind.
    pub kind: InlayHintKind,
    /// Render padding before the hint.
    pub padding_left: bool,
    /// Render padding after the hint.
    pub padding_right: bool,
}

/// An opaque, application-resolved command identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommandId(pub String);

/// A code lens: an actionable annotation shown above a range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodeLens {
    /// The range the lens annotates.
    pub range: Range,
    /// The lens title (e.g. `"3 references"`).
    pub title: String,
    /// The command to run when activated.
    pub command: Option<CommandId>,
}

/// A path-based location (works for files that are not currently open).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Location {
    /// The file path.
    pub path: PathBuf,
    /// The range within the file.
    pub range: Range,
}

/// The kind of a [`CompletionItem`] (drives its icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum CompletionKind {
    /// Plain text.
    Text,
    /// A method.
    Method,
    /// A function.
    Function,
    /// A field.
    Field,
    /// A variable.
    Variable,
    /// A class.
    Class,
    /// An interface.
    Interface,
    /// A module.
    Module,
    /// A property.
    Property,
    /// A keyword.
    Keyword,
    /// A snippet.
    Snippet,
    /// A constant.
    Constant,
    /// A struct.
    Struct,
    /// An enum.
    Enum,
}

/// A completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionItem {
    /// The label shown in the list.
    pub label: String,
    /// The completion kind.
    pub kind: CompletionKind,
    /// Optional detail (e.g. a type or signature).
    pub detail: Option<String>,
    /// Optional documentation.
    pub documentation: Option<Markup>,
    /// The text inserted when no `edit` is supplied.
    pub insert_text: String,
    /// A precise edit to apply instead of `insert_text`.
    pub edit: Option<TextEdit>,
    /// A sort key overriding `label`.
    pub sort_text: Option<String>,
    /// Whether the item is deprecated.
    pub deprecated: bool,
}

/// The flavor of a [`Markup`] payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MarkupKind {
    /// Plain text.
    PlainText,
    /// CommonMark markdown (rendered by `karet-markdown` at the edge).
    Markdown,
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

/// A code action (quick fix / refactor) offered for a range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodeAction {
    /// The action title.
    pub title: String,
    /// An edit to apply.
    pub edit: Option<WorkspaceEdit>,
    /// A command to run.
    pub command: Option<CommandId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_constructs_and_clones() {
        let d = Diagnostic {
            range: Range::default(),
            severity: Severity::Error,
            message: "boom".to_owned(),
            source: Some("rustc".to_owned()),
            code: None,
            tags: vec![DiagnosticTag::Deprecated],
            related: Vec::new(),
        };
        assert_eq!(d.clone(), d);
        assert!(Severity::Error < Severity::Hint);
    }

    #[test]
    fn decoration_kinds() {
        let dec = Decoration {
            range: Range::default(),
            kind: DecorationKind::Underline(UnderlineStyle::Curly),
            role: Some(ThemeRole::DiagnosticError),
        };
        assert_eq!(dec.kind, DecorationKind::Underline(UnderlineStyle::Curly));
    }

    #[test]
    fn symbol_kinds_cover_structural_and_value_symbols() {
        let kinds = [
            SymbolKind::String,
            SymbolKind::Number,
            SymbolKind::Boolean,
            SymbolKind::Array,
            SymbolKind::Object,
            SymbolKind::Key,
            SymbolKind::Null,
            SymbolKind::Event,
            SymbolKind::Operator,
        ];
        assert_eq!(kinds.len(), 9);
        assert!(kinds.contains(&SymbolKind::Object));
    }
}

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
    /// Rendering tags.
    pub tags: Vec<DiagnosticTag>,
    /// Related locations.
    pub related: Vec<RelatedInfo>,
}

/// The kind of a [`Decoration`] — the visual treatment to apply to a range.
///
/// `#[non_exhaustive]`: richer treatments (underlines, gutter icons, breakpoint
/// markers, …) are added as variants when a producer and a renderer exist for them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DecorationKind {
    /// A glyph in the gutter (e.g. a VCS change marker).
    GutterMarker {
        /// The glyph to draw.
        glyph: char,
    },
    /// Highlight the whole line's background.
    LineBackground,
    /// Highlight the text background within the range.
    TextBackground,
    /// Underline the text within the range, leaving its colors alone (e.g. a
    /// Ctrl-hovered symbol a click would navigate to).
    ///
    /// The decoration's `role` colors the underline; `None` underlines in the
    /// text's own foreground, which no theme can render illegibly.
    Underline,
    /// A color chip over the range, carrying an arbitrary color instead of a
    /// theme role (e.g. a CSS color literal). The renderer paints the range's
    /// background in this color and picks a contrasting foreground itself.
    ColorSwatch {
        /// The color, as straight (un-premultiplied) 8-bit RGBA. Renderers
        /// that cannot blend may ignore the alpha channel.
        rgba: [u8; 4],
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

/// The kind of an [`InlayHint`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InlayHintKind {
    /// An inferred type annotation.
    Type,
    /// A parameter name.
    Parameter,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
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

/// Which edge of the end-of-line slot a [`VirtualTextPlacement::EndOfLine`]
/// text keeps to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EolAlign {
    /// Immediately after the line's content (and any left-aligned text before
    /// it).
    #[default]
    Left,
    /// Flush against the viewport's right edge, when the row leaves room for
    /// it; otherwise immediately after the left-aligned text.
    Right,
}

/// Where a [`VirtualText`] sits on its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum VirtualTextPlacement {
    /// Between characters: immediately before the character at `col`, after
    /// that column's caret slot, so a caret at `col` sits against the text it
    /// edits rather than past the annotation. A `col` at or past the line's
    /// end trails its last character.
    ///
    /// It is part of the line's layout: it takes cells when the line wraps,
    /// shifts every caret and click after it, and a horizontal reveal scrolls
    /// as far as it is drawn. A click on it resolves to `col`.
    Inline {
        /// The buffer column (in `char`s) it renders before.
        col: u32,
    },
    /// The end-of-line slot: the cells a line's last row leaves free after
    /// its content and trailing inline text. It never wraps, never shifts the
    /// text, and no caret enters it (a click on it resolves to the end of the
    /// line). When the slot is too narrow for everything on the line, higher
    /// `priority` keeps its cells first, and a text cut short ends in `…`.
    ///
    /// The slot begins at the end-of-line caret cell: a text that should
    /// stand apart from the code carries its own leading gap.
    EndOfLine {
        /// Which edge of the slot it keeps to.
        align: EolAlign,
        /// Truncation priority: higher is cut last. Equal priorities are cut
        /// last-supplied first.
        priority: u8,
    },
}

/// Presentation-neutral virtual text: cells a renderer paints on a line that
/// are not in the buffer (inlay hints, inline blame, dependency versions,
/// inline diagnostics, ghost text).
///
/// One model for every producer, so a renderer has one path that wrapping,
/// caret placement, hit-testing and horizontal scrolling all account for.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VirtualText {
    /// The line it is on.
    pub line: u32,
    /// Where on the line.
    pub placement: VirtualTextPlacement,
    /// The text to paint, on one row (a renderer does not break it).
    pub text: String,
    /// The theme role supplying its color.
    pub role: ThemeRole,
}

impl VirtualText {
    /// Text anchored before the character at `col` on `line`
    /// ([`VirtualTextPlacement::Inline`]).
    #[must_use]
    pub fn inline(line: u32, col: u32, text: impl Into<String>, role: ThemeRole) -> Self {
        Self {
            line,
            placement: VirtualTextPlacement::Inline { col },
            text: text.into(),
            role,
        }
    }

    /// Left-aligned text in `line`'s end-of-line slot at `priority`
    /// ([`VirtualTextPlacement::EndOfLine`]).
    #[must_use]
    pub fn end_of_line(line: u32, text: impl Into<String>, role: ThemeRole, priority: u8) -> Self {
        Self {
            line,
            placement: VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority,
            },
            text: text.into(),
            role,
        }
    }
}

impl From<&InlayHint> for VirtualText {
    /// The hint's label at its position, with its padding as one space either
    /// side, in the [`ThemeRole::InlayHint`] color.
    fn from(hint: &InlayHint) -> Self {
        let mut text = String::with_capacity(hint.label.len().saturating_add(2));
        if hint.padding_left {
            text.push(' ');
        }
        text.push_str(&hint.label);
        if hint.padding_right {
            text.push(' ');
        }
        Self::inline(
            hint.position.line,
            hint.position.col,
            text,
            ThemeRole::InlayHint,
        )
    }
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
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
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
    fn an_inlay_hint_becomes_inline_virtual_text_with_its_padding_as_spaces() {
        let hint = InlayHint {
            position: LineCol::new(2, 9),
            label: "i32".to_owned(),
            kind: InlayHintKind::Type,
            padding_left: true,
            padding_right: false,
        };
        assert_eq!(
            VirtualText::from(&hint),
            VirtualText {
                line: 2,
                placement: VirtualTextPlacement::Inline { col: 9 },
                text: " i32".to_owned(),
                role: ThemeRole::InlayHint,
            }
        );
        let both = InlayHint {
            padding_right: true,
            ..hint
        };
        assert_eq!(VirtualText::from(&both).text, " i32 ");
    }

    #[test]
    fn end_of_line_text_defaults_to_the_left_edge_of_the_slot() {
        let text = VirtualText::end_of_line(4, "  Ada", ThemeRole::Muted, 3);
        assert_eq!(text.line, 4);
        assert_eq!(
            text.placement,
            VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority: 3,
            }
        );
        assert_eq!(EolAlign::default(), EolAlign::Left);
    }

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
            kind: DecorationKind::GutterMarker { glyph: '▎' },
            role: Some(ThemeRole::DiagnosticError),
        };
        assert_eq!(dec.kind, DecorationKind::GutterMarker { glyph: '▎' });
    }

    #[test]
    fn a_color_swatch_carries_its_color_instead_of_a_role() {
        // The one decoration whose color is data, not a theme role: a color
        // literal's own value travels with the decoration.
        let dec = Decoration {
            range: Range::default(),
            kind: DecorationKind::ColorSwatch {
                rgba: [0x12, 0x34, 0x56, 0xff],
            },
            role: None,
        };
        assert_eq!(
            dec.kind,
            DecorationKind::ColorSwatch {
                rgba: [0x12, 0x34, 0x56, 0xff],
            }
        );
    }

    #[test]
    fn an_underline_decoration_carries_no_color_of_its_own() {
        // `role: None` is the case a navigable-symbol underline uses: the
        // underline takes the text's own foreground, so no theme can render it
        // illegibly against its own background.
        let dec = Decoration {
            range: Range::default(),
            kind: DecorationKind::Underline,
            role: None,
        };
        assert_eq!(dec.clone(), dec);
        assert_ne!(dec.kind, DecorationKind::TextBackground);
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
            tags: vec![DiagnosticTag::Unnecessary],
            related: vec![RelatedInfo {
                location: Location {
                    path: PathBuf::from("a.rs"),
                    range: Range::default(),
                },
                message: "here".into(),
            }],
        })?;
        rt(&Decoration {
            range: Range::default(),
            kind: DecorationKind::ColorSwatch { rgba: [1, 2, 3, 4] },
            role: Some(ThemeRole::Muted),
        })?;
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
        rt(&Symbol {
            name: "f".into(),
            kind: SymbolKind::Function,
            detail: None,
            range: Range::default(),
            selection_range: Range::default(),
            container_name: None,
            children: Vec::new(),
        })?;
        rt(&CompletionItem {
            label: "push".into(),
            kind: CompletionKind::Method,
            detail: None,
            documentation: None,
            insert_text: "push".into(),
            edit: None,
            sort_text: None,
            deprecated: false,
        })?;
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
        Ok(())
    }
}

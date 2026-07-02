//! Semantic highlight and UI-role vocabulary shared by `karet-syntax` (which
//! stamps spans with a [`TokenId`]) and `karet-theme` (which resolves a `TokenId`
//! or [`ThemeRole`] to a color).

/// A cheap, `Copy` identifier for a semantic highlight class.
///
/// Highlight spans hold one per token, so this is deliberately a small newtype.
/// The well-known classes have associated constants; the canonical set is
/// enumerated by [`StandardToken`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TokenId(pub u16);

impl TokenId {
    /// Keywords (`if`, `fn`, `return`, …).
    pub const KEYWORD: TokenId = StandardToken::Keyword.id();
    /// Function names.
    pub const FUNCTION: TokenId = StandardToken::Function.id();
    /// Type names.
    pub const TYPE: TokenId = StandardToken::Type.id();
    /// String literals.
    pub const STRING: TokenId = StandardToken::String.id();
    /// Comments.
    pub const COMMENT: TokenId = StandardToken::Comment.id();
    /// Variables.
    pub const VARIABLE: TokenId = StandardToken::Variable.id();
    /// Constants.
    pub const CONSTANT: TokenId = StandardToken::Constant.id();
    /// Numeric literals.
    pub const NUMBER: TokenId = StandardToken::Number.id();
    /// Operators.
    pub const OPERATOR: TokenId = StandardToken::Operator.id();

    /// Create a token id from a raw value.
    #[must_use]
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }
}

/// The canonical set of tree-sitter highlight capture classes karet recognizes.
///
/// Each maps to a stable [`TokenId`] and a tree-sitter capture name; unknown
/// captures fall back to the closest standard class at the highlighter edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum StandardToken {
    /// A keyword.
    Keyword,
    /// A control-flow keyword.
    KeywordControl,
    /// A function name.
    Function,
    /// A built-in function.
    FunctionBuiltin,
    /// A method name.
    Method,
    /// A type name.
    Type,
    /// A built-in type.
    TypeBuiltin,
    /// A variable.
    Variable,
    /// A function/closure parameter.
    Parameter,
    /// A field or property access.
    Property,
    /// A constant.
    Constant,
    /// A string literal.
    String,
    /// An escape sequence inside a string.
    StringEscape,
    /// A comment.
    Comment,
    /// A numeric literal.
    Number,
    /// A boolean literal.
    Boolean,
    /// An operator.
    Operator,
    /// Punctuation (delimiters, separators).
    Punctuation,
    /// A markup/HTML tag.
    Tag,
    /// A markup/HTML attribute.
    Attribute,
    /// A namespace or module path.
    Namespace,
    /// A label.
    Label,
}

impl StandardToken {
    /// The stable [`TokenId`] for this class (its discriminant).
    #[must_use]
    pub const fn id(self) -> TokenId {
        TokenId(self as u16)
    }

    /// The tree-sitter capture name (without a leading `@`).
    #[must_use]
    pub const fn capture_name(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::KeywordControl => "keyword.control",
            Self::Function => "function",
            Self::FunctionBuiltin => "function.builtin",
            Self::Method => "function.method",
            Self::Type => "type",
            Self::TypeBuiltin => "type.builtin",
            Self::Variable => "variable",
            Self::Parameter => "variable.parameter",
            Self::Property => "property",
            Self::Constant => "constant",
            Self::String => "string",
            Self::StringEscape => "string.escape",
            Self::Comment => "comment",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Operator => "operator",
            Self::Punctuation => "punctuation",
            Self::Tag => "tag",
            Self::Attribute => "attribute",
            Self::Namespace => "namespace",
            Self::Label => "label",
        }
    }

    /// Parse a tree-sitter capture name into a standard class, if recognized.
    #[must_use]
    pub fn from_capture_name(name: &str) -> Option<Self> {
        let token = match name {
            "keyword" => Self::Keyword,
            "keyword.control" => Self::KeywordControl,
            "function" => Self::Function,
            "function.builtin" => Self::FunctionBuiltin,
            "function.method" => Self::Method,
            "type" => Self::Type,
            "type.builtin" => Self::TypeBuiltin,
            "variable" => Self::Variable,
            "variable.parameter" => Self::Parameter,
            "property" => Self::Property,
            "constant" => Self::Constant,
            "string" => Self::String,
            "string.escape" => Self::StringEscape,
            "comment" => Self::Comment,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "operator" => Self::Operator,
            "punctuation" => Self::Punctuation,
            "tag" => Self::Tag,
            "attribute" => Self::Attribute,
            "namespace" => Self::Namespace,
            "label" => Self::Label,
            _ => return None,
        };
        Some(token)
    }
}

/// The finite vocabulary of UI-chrome colors that widgets reference, resolved by
/// `karet-theme`. Distinct from [`TokenId`], which colors *code* tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum ThemeRole {
    /// Editor background.
    Background,
    /// Default foreground text.
    Foreground,
    /// Background of the line the cursor is on.
    CursorLine,
    /// The cursor itself.
    Cursor,
    /// Selected-text background.
    Selection,
    /// Gutter line numbers.
    LineNumber,
    /// The active line's number.
    LineNumberActive,
    /// Indent-guide rules.
    IndentGuide,
    /// A matching bracket highlight.
    MatchingBracket,
    /// Status bar background.
    StatusBarBackground,
    /// Status bar foreground.
    StatusBarForeground,
    /// Error diagnostics.
    DiagnosticError,
    /// Warning diagnostics.
    DiagnosticWarning,
    /// Informational diagnostics.
    DiagnosticInfo,
    /// Hint diagnostics.
    DiagnosticHint,
    /// Added lines in a diff.
    DiffAdded,
    /// Removed lines in a diff.
    DiffRemoved,
    /// Modified lines in a diff.
    DiffModified,
    /// A search-match highlight.
    SearchMatch,
    /// A debugger breakpoint marker.
    Breakpoint,
    /// A mouse-hover highlight for list rows (a secondary accent, distinct from the
    /// primary [`Selection`](Self::Selection) highlight).
    HoverHighlight,
    /// Background of the explorer row whose file is shown in the focused editor pane
    /// (the "you are here" marker). Brighter than [`Selection`](Self::Selection) so
    /// the focused editor's file reads as the strongest of the active-file tiers.
    ActiveEditorRow,
    /// De-emphasized UI text (gitignored / disabled explorer rows, etc.). Readable,
    /// unlike the near-background [`IndentGuide`](Self::IndentGuide) rule color.
    Muted,
    /// Explorer icon tint for text-like files (code, markup, data, config, shell).
    FileIconText,
    /// Explorer icon tint for media and documents (images, PDFs, office docs).
    FileIconMedia,
    /// Explorer icon tint for opaque binaries and archives.
    FileIconBinary,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_ids_are_stable_and_distinct() {
        assert_eq!(TokenId::KEYWORD, StandardToken::Keyword.id());
        assert_ne!(TokenId::KEYWORD, TokenId::FUNCTION);
        assert_eq!(TokenId::new(7).0, 7);
    }

    #[test]
    fn capture_name_round_trips() {
        for tok in [
            StandardToken::Keyword,
            StandardToken::FunctionBuiltin,
            StandardToken::Comment,
            StandardToken::Label,
        ] {
            assert_eq!(
                StandardToken::from_capture_name(tok.capture_name()),
                Some(tok)
            );
        }
        assert_eq!(StandardToken::from_capture_name("not.a.capture"), None);
    }
}

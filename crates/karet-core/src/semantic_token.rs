//! Semantic-token vocabulary: the token types and modifiers a language server
//! classifies identifiers with, and how one pair travels as a [`TokenId`].
//!
//! A semantic token rides the same [`TokenId`] a tree-sitter highlight span
//! carries, so an overlay is just a highlight span with a different id and
//! every renderer that paints highlights paints it unchanged. The id space is
//! split by its top five bits: zero is the [`StandardToken`] space a
//! tree-sitter grammar stamps (`0..2048`), and `1..=31` is a semantic token
//! type plus one, with the low eleven bits holding its modifier set. The theme
//! then resolves a semantic id against its `semanticTokenColors` rules, falling
//! back to the closest standard class.
//!
//! The vocabulary is fixed rather than interned per server, because the id
//! must mean the same thing to the producer and to the theme without either
//! holding the other's state. It covers every type and modifier the LSP 3.17
//! specification defines, plus the custom names the most widely used servers
//! emit that a theme commonly styles (rust-analyzer's `mutable` and
//! `unresolvedReference`, among others). A name outside it has no id: a
//! producer drops a token of an unknown type and ignores an unknown modifier.
//!
//! [`StandardToken`]: crate::StandardToken

use crate::coord::LineCol;
use crate::token::TokenId;

/// How many low bits of a semantic [`TokenId`] hold its modifier set.
const MODIFIER_BITS: u16 = 11;

/// The mask of a semantic [`TokenId`]'s modifier bits.
const MODIFIER_MASK: u16 = (1 << MODIFIER_BITS) - 1;

/// The token type of a semantic token: what a language server says an
/// identifier *is*, beyond what its syntax shows.
///
/// The first twenty-three variants are the LSP 3.17 standard types, in the
/// specification's order; the rest are custom types widely used servers emit.
/// At most 31 types fit the [`TokenId`] encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum SemanticTokenType {
    /// A namespace, module or package (`namespace`).
    Namespace,
    /// A type that is not one of the more specific kinds below (`type`).
    Type,
    /// A class (`class`).
    Class,
    /// An enum (`enum`).
    Enum,
    /// An interface or trait (`interface`).
    Interface,
    /// A struct (`struct`).
    Struct,
    /// A type parameter (`typeParameter`).
    TypeParameter,
    /// A function or method parameter (`parameter`).
    Parameter,
    /// A local or global variable (`variable`).
    Variable,
    /// A member property or field (`property`).
    Property,
    /// An enum member (`enumMember`).
    EnumMember,
    /// An event property (`event`).
    Event,
    /// A function (`function`).
    Function,
    /// A member function (`method`).
    Method,
    /// A macro (`macro`).
    Macro,
    /// A language keyword (`keyword`).
    Keyword,
    /// A modifier keyword (`modifier`).
    Modifier,
    /// A comment (`comment`).
    Comment,
    /// A string literal (`string`).
    String,
    /// A number literal (`number`).
    Number,
    /// A regular expression literal (`regexp`).
    Regexp,
    /// An operator (`operator`).
    Operator,
    /// A decorator or annotation (`decorator`).
    Decorator,
    /// A name the server could not resolve (rust-analyzer's
    /// `unresolvedReference`).
    UnresolvedReference,
    /// A lifetime (rust-analyzer's `lifetime`).
    Lifetime,
    /// The `self` keyword (rust-analyzer's `selfKeyword`).
    SelfKeyword,
    /// A built-in type such as `u32` or `str` (rust-analyzer's `builtinType`).
    BuiltinType,
    /// A type alias (`typeAlias`).
    TypeAlias,
    /// An attribute (`attribute`).
    Attribute,
    /// A boolean literal (`boolean`).
    Boolean,
    /// A label (`label`).
    Label,
}

impl SemanticTokenType {
    /// Every type, in [`TokenId`] encoding order.
    pub const ALL: [Self; 31] = [
        Self::Namespace,
        Self::Type,
        Self::Class,
        Self::Enum,
        Self::Interface,
        Self::Struct,
        Self::TypeParameter,
        Self::Parameter,
        Self::Variable,
        Self::Property,
        Self::EnumMember,
        Self::Event,
        Self::Function,
        Self::Method,
        Self::Macro,
        Self::Keyword,
        Self::Modifier,
        Self::Comment,
        Self::String,
        Self::Number,
        Self::Regexp,
        Self::Operator,
        Self::Decorator,
        Self::UnresolvedReference,
        Self::Lifetime,
        Self::SelfKeyword,
        Self::BuiltinType,
        Self::TypeAlias,
        Self::Attribute,
        Self::Boolean,
        Self::Label,
    ];

    /// The name a server's legend and a theme's selector spell this type with.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Namespace => "namespace",
            Self::Type => "type",
            Self::Class => "class",
            Self::Enum => "enum",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::TypeParameter => "typeParameter",
            Self::Parameter => "parameter",
            Self::Variable => "variable",
            Self::Property => "property",
            Self::EnumMember => "enumMember",
            Self::Event => "event",
            Self::Function => "function",
            Self::Method => "method",
            Self::Macro => "macro",
            Self::Keyword => "keyword",
            Self::Modifier => "modifier",
            Self::Comment => "comment",
            Self::String => "string",
            Self::Number => "number",
            Self::Regexp => "regexp",
            Self::Operator => "operator",
            Self::Decorator => "decorator",
            Self::UnresolvedReference => "unresolvedReference",
            Self::Lifetime => "lifetime",
            Self::SelfKeyword => "selfKeyword",
            Self::BuiltinType => "builtinType",
            Self::TypeAlias => "typeAlias",
            Self::Attribute => "attribute",
            Self::Boolean => "boolean",
            Self::Label => "label",
        }
    }

    /// The type a legend or selector names, if it is in the vocabulary.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ty| ty.name() == name)
    }

    /// This type's position in [`ALL`](Self::ALL).
    const fn index(self) -> u16 {
        self as u16
    }
}

/// A set of semantic-token modifiers: what a server adds about a token
/// (`readonly`, `deprecated`, `mutable`, …), combined with `|`.
///
/// The first ten flags are the LSP 3.17 standard modifiers, in the
/// specification's order; [`MUTABLE`](Self::MUTABLE) is rust-analyzer's. At
/// most eleven flags fit the [`TokenId`] encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(transparent)
)]
pub struct SemanticTokenModifiers(u16);

impl SemanticTokenModifiers {
    /// No modifier.
    pub const NONE: Self = Self(0);
    /// A declaration of the symbol (`declaration`).
    pub const DECLARATION: Self = Self(1);
    /// A definition of the symbol (`definition`).
    pub const DEFINITION: Self = Self(1 << 1);
    /// A read-only variable or member (`readonly`).
    pub const READONLY: Self = Self(1 << 2);
    /// A static member (`static`).
    pub const STATIC: Self = Self(1 << 3);
    /// A symbol that should no longer be used (`deprecated`).
    pub const DEPRECATED: Self = Self(1 << 4);
    /// An abstract type or member (`abstract`).
    pub const ABSTRACT: Self = Self(1 << 5);
    /// An async function (`async`).
    pub const ASYNC: Self = Self(1 << 6);
    /// A variable reference that is written to (`modification`).
    pub const MODIFICATION: Self = Self(1 << 7);
    /// An occurrence in documentation (`documentation`).
    pub const DOCUMENTATION: Self = Self(1 << 8);
    /// A symbol of the standard library (`defaultLibrary`).
    pub const DEFAULT_LIBRARY: Self = Self(1 << 9);
    /// A mutable binding or a reference through one (rust-analyzer's
    /// `mutable`).
    pub const MUTABLE: Self = Self(1 << 10);

    /// Every flag with the name a legend and a selector spell it with, in bit
    /// order.
    pub const ALL: [(&'static str, Self); 11] = [
        ("declaration", Self::DECLARATION),
        ("definition", Self::DEFINITION),
        ("readonly", Self::READONLY),
        ("static", Self::STATIC),
        ("deprecated", Self::DEPRECATED),
        ("abstract", Self::ABSTRACT),
        ("async", Self::ASYNC),
        ("modification", Self::MODIFICATION),
        ("documentation", Self::DOCUMENTATION),
        ("defaultLibrary", Self::DEFAULT_LIBRARY),
        ("mutable", Self::MUTABLE),
    ];

    /// The single flag a legend or selector names, if it is in the
    /// vocabulary.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find_map(|(spelled, flag)| (spelled == name).then_some(flag))
    }

    /// Whether every flag of `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags set in either `self` or `other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// How many flags are set.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }
}

impl std::ops::BitOr for SemanticTokenModifiers {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl std::ops::BitOrAssign for SemanticTokenModifiers {
    fn bitor_assign(&mut self, other: Self) {
        *self = self.union(other);
    }
}

impl TokenId {
    /// The first raw value of the semantic id space. Every
    /// [`StandardToken`](crate::StandardToken) id lies below it.
    pub const SEMANTIC_BASE: u16 = 1 << MODIFIER_BITS;

    /// The id of a semantic token of type `ty` with `modifiers`.
    #[must_use]
    pub const fn semantic(ty: SemanticTokenType, modifiers: SemanticTokenModifiers) -> Self {
        Self(((ty.index() + 1) << MODIFIER_BITS) | (modifiers.0 & MODIFIER_MASK))
    }

    /// Whether this id is a semantic token's rather than a standard class's.
    #[must_use]
    pub const fn is_semantic(self) -> bool {
        self.0 >= Self::SEMANTIC_BASE
    }

    /// The semantic type and modifiers this id encodes, or `None` for a
    /// standard class's id.
    #[must_use]
    pub fn semantic_parts(self) -> Option<(SemanticTokenType, SemanticTokenModifiers)> {
        let slot = usize::from(self.0 >> MODIFIER_BITS).checked_sub(1)?;
        let ty = SemanticTokenType::ALL.get(slot).copied()?;
        Some((ty, SemanticTokenModifiers(self.0 & MODIFIER_MASK)))
    }
}

/// One semantic token a producer classified: a single-line run of text and
/// the semantic [`TokenId`] it carries.
///
/// The columns are in the producer's own encoding -- UTF-16 code units for a
/// language server -- so the consumer converts them against the text they
/// describe, as it does every other position a server sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SemanticToken {
    /// Where the token starts.
    pub start: LineCol,
    /// How many columns it covers, on `start`'s line.
    pub length: u32,
    /// Its type and modifiers (see [`TokenId::semantic`]).
    pub token: TokenId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::StandardToken;

    #[test]
    fn every_type_and_modifier_set_round_trips_through_a_token_id() {
        let all = SemanticTokenModifiers::ALL
            .iter()
            .fold(SemanticTokenModifiers::NONE, |set, (_, flag)| set | *flag);
        for ty in SemanticTokenType::ALL {
            for modifiers in [SemanticTokenModifiers::NONE, all] {
                let id = TokenId::semantic(ty, modifiers);
                assert!(id.is_semantic());
                assert_eq!(id.semantic_parts(), Some((ty, modifiers)));
            }
        }
    }

    /// The two id spaces never meet: the last standard class is not semantic,
    /// and the widest semantic id still fits a `u16`.
    #[test]
    fn standard_ids_are_never_semantic() {
        let last = StandardToken::CommentMark.id();
        assert!(!last.is_semantic());
        assert_eq!(last.semantic_parts(), None);
        assert_eq!(TokenId(TokenId::SEMANTIC_BASE - 1).semantic_parts(), None);
        let widest = TokenId::semantic(
            SemanticTokenType::Label,
            SemanticTokenModifiers(MODIFIER_MASK),
        );
        assert_eq!(widest, TokenId(u16::MAX));
    }

    #[test]
    fn names_round_trip_and_unknown_names_have_no_id() {
        for ty in SemanticTokenType::ALL {
            assert_eq!(SemanticTokenType::from_name(ty.name()), Some(ty));
        }
        for (name, flag) in SemanticTokenModifiers::ALL {
            assert_eq!(SemanticTokenModifiers::from_name(name), Some(flag));
            assert_eq!(flag.len(), 1);
        }
        assert_eq!(SemanticTokenType::from_name("Variable"), None);
        assert_eq!(SemanticTokenModifiers::from_name("consuming"), None);
    }

    #[test]
    fn modifier_sets_combine_and_test_membership() {
        let set = SemanticTokenModifiers::READONLY | SemanticTokenModifiers::STATIC;
        assert!(set.contains(SemanticTokenModifiers::READONLY));
        assert!(!set.contains(SemanticTokenModifiers::READONLY | SemanticTokenModifiers::MUTABLE));
        assert!(set.contains(SemanticTokenModifiers::NONE));
        assert_eq!(set.len(), 2);
        assert!(SemanticTokenModifiers::default().is_empty());
        let mut grown = SemanticTokenModifiers::NONE;
        grown |= SemanticTokenModifiers::MUTABLE;
        assert_eq!(grown, SemanticTokenModifiers::MUTABLE);
    }
}

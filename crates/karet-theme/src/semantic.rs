//! Resolving a semantic [`TokenId`](karet_core::TokenId) to a style: the
//! theme's `semanticTokenColors` rules, over the standard class a semantic
//! type falls back to.
//!
//! A rule's selector is VS Code's: a type name or `*`, followed by any number
//! of `.modifier`s (`variable.readonly`, `*.deprecated`). A rule matches a
//! token whose type it names (any type, for `*`) and whose modifiers include
//! all of its own. Each style property -- the foreground and each emphasis
//! flag -- is taken separately from the most specific matching rule that sets
//! it, scored as VS Code's `tokenClassificationRegistry` scores a selector:
//! 100 for naming the type and 100 for each modifier, so `*.readonly` ties
//! `variable` and `*.readonly.static` beats it, and the later rule wins a
//! tie. A property
//! no rule sets comes from the fallback class, so a theme without
//! `semanticTokenColors` still colours a semantic token in its own palette.

use karet_core::SemanticTokenModifiers;
use karet_core::SemanticTokenType;
use karet_core::StandardToken;

use crate::Rgba;

/// What one rule sets. `None` leaves a property to a less specific rule, or
/// to the fallback class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SemanticStyle {
    /// The foreground colour.
    pub(crate) fg: Option<Rgba>,
    /// Bold text.
    pub(crate) bold: Option<bool>,
    /// Italic text.
    pub(crate) italic: Option<bool>,
    /// Underlined text.
    pub(crate) underline: Option<bool>,
    /// Struck-through text.
    pub(crate) strikethrough: Option<bool>,
}

impl SemanticStyle {
    /// A style that sets only the foreground.
    pub(crate) const fn fg(color: Rgba) -> Self {
        Self {
            fg: Some(color),
            bold: None,
            italic: None,
            underline: None,
            strikethrough: None,
        }
    }

    /// Whether the style sets nothing.
    #[cfg(feature = "vscode")]
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// One `semanticTokenColors` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SemanticRule {
    /// The type the selector names; `None` for `*`.
    pub(crate) ty: Option<SemanticTokenType>,
    /// The modifiers the selector requires.
    pub(crate) modifiers: SemanticTokenModifiers,
    /// What the rule sets.
    pub(crate) style: SemanticStyle,
}

impl SemanticRule {
    /// Parse `selector` (`type.mod…` or `*.mod…`) into a rule applying
    /// `style`.
    ///
    /// `None` for a selector this vocabulary cannot honour: one naming a type
    /// or modifier outside it -- which no token can carry -- or one scoped to
    /// a language (`type:rust`), which a token does not record. Dropping the
    /// rule is VS Code's own reading of a selector that matches nothing.
    pub(crate) fn parse(selector: &str, style: SemanticStyle) -> Option<Self> {
        if selector.contains(':') {
            return None;
        }
        let mut parts = selector.trim().split('.');
        let ty = match parts.next()? {
            "*" => None,
            name => Some(SemanticTokenType::from_name(name)?),
        };
        let mut modifiers = SemanticTokenModifiers::NONE;
        for name in parts {
            modifiers |= SemanticTokenModifiers::from_name(name)?;
        }
        Some(Self {
            ty,
            modifiers,
            style,
        })
    }

    /// How specifically this rule matches a token of `ty` with `modifiers`,
    /// or `None` when it does not match: 100 for a named type plus 100 per
    /// modifier, VS Code's own weights.
    fn score(&self, ty: SemanticTokenType, modifiers: SemanticTokenModifiers) -> Option<u32> {
        if self.ty.is_some_and(|own| own != ty) || !modifiers.contains(self.modifiers) {
            return None;
        }
        Some((u32::from(self.ty.is_some()) + self.modifiers.len()) * 100)
    }
}

/// The style `rules` give a token of `ty` with `modifiers`: each property
/// from the most specific rule that sets it, the later rule on a tie.
pub(crate) fn resolve(
    rules: &[SemanticRule],
    ty: SemanticTokenType,
    modifiers: SemanticTokenModifiers,
) -> SemanticStyle {
    fn take<T: Copy>(best: &mut Option<(u32, T)>, score: u32, value: Option<T>) {
        if let Some(value) = value
            && best.is_none_or(|(held, _)| score >= held)
        {
            *best = Some((score, value));
        }
    }
    let (mut fg, mut bold, mut italic, mut underline, mut strike) = (None, None, None, None, None);
    for rule in rules {
        let Some(score) = rule.score(ty, modifiers) else {
            continue;
        };
        take(&mut fg, score, rule.style.fg);
        take(&mut bold, score, rule.style.bold);
        take(&mut italic, score, rule.style.italic);
        take(&mut underline, score, rule.style.underline);
        take(&mut strike, score, rule.style.strikethrough);
    }
    let value = |best: Option<(u32, bool)>| best.map(|(_, v)| v);
    SemanticStyle {
        fg: fg.map(|(_, color)| color),
        bold: value(bold),
        italic: value(italic),
        underline: value(underline),
        strikethrough: value(strike),
    }
}

/// The standard class a semantic token borrows whatever its rules leave
/// unset: the class a tree-sitter grammar would most likely have given the
/// same text, so a theme without `semanticTokenColors` keeps its palette.
pub(crate) fn fallback(ty: SemanticTokenType, modifiers: SemanticTokenModifiers) -> StandardToken {
    use SemanticTokenType as T;
    let library = modifiers.contains(SemanticTokenModifiers::DEFAULT_LIBRARY);
    match ty {
        T::Namespace => StandardToken::Namespace,
        T::Type
        | T::Class
        | T::Enum
        | T::Interface
        | T::Struct
        | T::TypeParameter
        | T::TypeAlias => {
            if library {
                StandardToken::TypeBuiltin
            } else {
                StandardToken::Type
            }
        },
        T::BuiltinType => StandardToken::TypeBuiltin,
        T::Parameter => StandardToken::Parameter,
        T::Property | T::Event => StandardToken::Property,
        T::EnumMember => StandardToken::Constant,
        T::Function | T::Macro => {
            if library {
                StandardToken::FunctionBuiltin
            } else {
                StandardToken::Function
            }
        },
        T::Method => StandardToken::Method,
        T::Keyword | T::Modifier | T::SelfKeyword => StandardToken::Keyword,
        T::Comment => {
            if modifiers.contains(SemanticTokenModifiers::DOCUMENTATION) {
                StandardToken::CommentDoc
            } else {
                StandardToken::Comment
            }
        },
        T::String | T::Regexp => StandardToken::String,
        T::Number => StandardToken::Number,
        T::Boolean => StandardToken::Boolean,
        T::Operator => StandardToken::Operator,
        T::Decorator | T::Attribute => StandardToken::Attribute,
        T::Lifetime | T::Label => StandardToken::Label,
        // A readonly variable is the constant a grammar paints `@constant`
        // (VS Code falls `variable.readonly` back to `variable.other.constant`).
        T::Variable if modifiers.contains(SemanticTokenModifiers::READONLY) => {
            StandardToken::Constant
        },
        // An unknown variant (the enum is non-exhaustive) reads as plain
        // code, as does a reference nobody could resolve.
        _ => StandardToken::Variable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba = Rgba::rgb(255, 0, 0);
    const GREEN: Rgba = Rgba::rgb(0, 255, 0);
    const BLUE: Rgba = Rgba::rgb(0, 0, 255);

    fn rule(selector: &str, style: SemanticStyle) -> Result<SemanticRule, String> {
        SemanticRule::parse(selector, style).ok_or_else(|| format!("`{selector}` did not parse"))
    }

    #[test]
    fn selectors_parse_types_wildcards_and_modifiers() -> Result<(), String> {
        let parsed = rule("variable.readonly.static", SemanticStyle::fg(RED))?;
        assert_eq!(parsed.ty, Some(SemanticTokenType::Variable));
        assert_eq!(
            parsed.modifiers,
            SemanticTokenModifiers::READONLY | SemanticTokenModifiers::STATIC
        );
        let wildcard = rule("*.mutable", SemanticStyle::fg(RED))?;
        assert_eq!(wildcard.ty, None);
        assert_eq!(wildcard.modifiers, SemanticTokenModifiers::MUTABLE);
        // Names outside the vocabulary, and language scopes, match nothing.
        assert_eq!(SemanticRule::parse("widget", SemanticStyle::fg(RED)), None);
        assert_eq!(
            SemanticRule::parse("variable.consuming", SemanticStyle::fg(RED)),
            None
        );
        assert_eq!(
            SemanticRule::parse("variable:rust", SemanticStyle::fg(RED)),
            None
        );
        Ok(())
    }

    /// VS Code's weights: a named type and each modifier count 100 apiece, so
    /// `*.readonly.static` (200) beats `variable` (100) and ties
    /// `variable.readonly`, which wins as the later rule; a rule whose
    /// modifiers the token lacks never applies.
    #[test]
    fn the_most_specific_matching_rule_sets_each_property() -> Result<(), String> {
        let rules = [
            rule("variable", SemanticStyle::fg(RED))?,
            rule("variable.readonly", SemanticStyle::fg(BLUE))?,
            rule("*.readonly.static", SemanticStyle::fg(GREEN))?,
            rule(
                "*.mutable",
                SemanticStyle {
                    underline: Some(true),
                    ..SemanticStyle::default()
                },
            )?,
        ];
        let readonly_static = SemanticTokenModifiers::READONLY | SemanticTokenModifiers::STATIC;
        let style = resolve(&rules, SemanticTokenType::Variable, readonly_static);
        assert_eq!(style.fg, Some(GREEN));
        let readonly = resolve(
            &rules,
            SemanticTokenType::Variable,
            SemanticTokenModifiers::READONLY,
        );
        assert_eq!(readonly.fg, Some(BLUE));
        assert_eq!(style.underline, None);
        let plain = resolve(
            &rules,
            SemanticTokenType::Variable,
            SemanticTokenModifiers::MUTABLE,
        );
        assert_eq!(plain.fg, Some(RED));
        assert_eq!(plain.underline, Some(true));
        let other = resolve(&rules, SemanticTokenType::Parameter, readonly_static);
        assert_eq!(other.fg, Some(GREEN));
        assert_eq!(
            resolve(
                &rules,
                SemanticTokenType::Macro,
                SemanticTokenModifiers::NONE
            ),
            SemanticStyle::default()
        );
        Ok(())
    }

    /// A wildcard with one modifier ties a bare type name, so the later of
    /// the two wins whichever it is. Falsified by weighting the type above
    /// modifiers (the type would win in both orders).
    #[test]
    fn a_wildcard_modifier_ties_a_bare_type() -> Result<(), String> {
        let readonly = SemanticTokenModifiers::READONLY;
        let type_first = [
            rule("variable", SemanticStyle::fg(RED))?,
            rule("*.readonly", SemanticStyle::fg(GREEN))?,
        ];
        assert_eq!(
            resolve(&type_first, SemanticTokenType::Variable, readonly).fg,
            Some(GREEN)
        );
        let wildcard_first = [
            rule("*.readonly", SemanticStyle::fg(GREEN))?,
            rule("variable", SemanticStyle::fg(RED))?,
        ];
        assert_eq!(
            resolve(&wildcard_first, SemanticTokenType::Variable, readonly).fg,
            Some(RED)
        );
        Ok(())
    }

    #[test]
    fn a_later_rule_wins_a_tie() -> Result<(), String> {
        let rules = [
            rule("*.readonly", SemanticStyle::fg(RED))?,
            rule("*.static", SemanticStyle::fg(GREEN))?,
        ];
        let both = SemanticTokenModifiers::READONLY | SemanticTokenModifiers::STATIC;
        assert_eq!(
            resolve(&rules, SemanticTokenType::Variable, both).fg,
            Some(GREEN)
        );
        Ok(())
    }

    #[test]
    fn fallbacks_follow_the_library_and_documentation_modifiers() {
        let none = SemanticTokenModifiers::NONE;
        let library = SemanticTokenModifiers::DEFAULT_LIBRARY;
        assert_eq!(
            fallback(SemanticTokenType::Struct, none),
            StandardToken::Type
        );
        assert_eq!(
            fallback(SemanticTokenType::Struct, library),
            StandardToken::TypeBuiltin
        );
        assert_eq!(
            fallback(SemanticTokenType::Function, library),
            StandardToken::FunctionBuiltin
        );
        assert_eq!(
            fallback(SemanticTokenType::Macro, none),
            StandardToken::Function
        );
        assert_eq!(
            fallback(
                SemanticTokenType::Comment,
                SemanticTokenModifiers::DOCUMENTATION
            ),
            StandardToken::CommentDoc
        );
        assert_eq!(
            fallback(SemanticTokenType::UnresolvedReference, none),
            StandardToken::Variable
        );
        assert_eq!(
            fallback(SemanticTokenType::Lifetime, none),
            StandardToken::Label
        );
        // A readonly variable keeps the `@constant` colour a grammar gives
        // an all-caps constant; a plain one stays a variable.
        assert_eq!(
            fallback(
                SemanticTokenType::Variable,
                SemanticTokenModifiers::READONLY
            ),
            StandardToken::Constant
        );
        assert_eq!(
            fallback(SemanticTokenType::Variable, none),
            StandardToken::Variable
        );
    }
}

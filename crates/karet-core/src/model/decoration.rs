//! Decorations: a range plus the visual treatment a renderer gives it — a
//! background, an underline, a gutter marker, a recolouring, text modifiers.

use crate::coord::Range;
use crate::token::Emphasis;
use crate::token::ThemeRole;
use crate::token::TokenId;

/// The kind of a [`Decoration`] — the visual treatment to apply to a range.
///
/// `#[non_exhaustive]`: richer treatments (underlines, gutter icons, breakpoint
/// markers, …) are added as variants when a producer and a renderer exist for them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DecorationKind {
    /// A glyph in the gutter's leading marker column (e.g. a breakpoint, a
    /// diagnostic's severity sign).
    GutterMarker {
        /// The glyph to draw.
        glyph: char,
    },
    /// A glyph marking lines changed against a baseline (e.g. a VCS
    /// uncommitted-change bar), drawn in a gutter column of its own so it
    /// never competes with a [`GutterMarker`](Self::GutterMarker) on the same
    /// line.
    ///
    /// A renderer with no such column may draw it in the marker column
    /// instead, behind every `GutterMarker` on the line.
    ChangeMarker {
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
    /// Restyle the text within the range with the decoration's
    /// [`fg`](Decoration::fg) and [`modifiers`](Decoration::modifiers) only,
    /// painting no background or underline (e.g. a semantic token's colour, a
    /// struck-through deprecated name). The `role` is unused.
    TextStyle,
}

/// A theme-resolved color a [`Decoration`] recolours its text with: a UI
/// [`ThemeRole`], or a code [`TokenId`] class (e.g. a semantic token), whose
/// color the renderer's theme supplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DecorationColor {
    /// The color of a UI role.
    Role(ThemeRole),
    /// The color of a code token class.
    Token(TokenId),
}

impl From<ThemeRole> for DecorationColor {
    fn from(role: ThemeRole) -> Self {
        Self::Role(role)
    }
}

impl From<TokenId> for DecorationColor {
    fn from(token: TokenId) -> Self {
        Self::Token(token)
    }
}

/// A presentation-neutral decoration: a range plus a visual treatment.
///
/// Build one with [`Decoration::new`] and refine it with the `with_*`
/// methods, or as a literal.
///
/// Where decorations overlap, each single-valued attribute (a background, a
/// foreground, an underline color, a gutter glyph) comes from the covering
/// decoration with the highest [`priority`](Self::priority); among equal
/// priorities the one supplied first wins. [`modifiers`](Self::modifiers)
/// blend instead (see [`Emphasis`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Decoration {
    /// The decorated range.
    pub range: Range,
    /// How to decorate it.
    pub kind: DecorationKind,
    /// An optional theme role supplying the color.
    pub role: Option<ThemeRole>,
    /// An optional color recolouring the text the decoration covers.
    ///
    /// Applies to the text-level kinds (backgrounds, underlines, swatches,
    /// [`TextStyle`](DecorationKind::TextStyle)); gutter markers ignore it. A
    /// [`ColorSwatch`](DecorationKind::ColorSwatch)'s own contrasting
    /// foreground and a selection's background still take precedence, so the
    /// text stays legible.
    #[cfg_attr(feature = "serde", serde(default))]
    pub fg: Option<DecorationColor>,
    /// Text attributes added to the covered text (text-level kinds only, as
    /// with [`fg`](Self::fg)). Overlapping decorations *blend* these: a cell
    /// shows the union of every covering decoration's flags, whatever their
    /// priorities.
    #[cfg_attr(feature = "serde", serde(default))]
    pub modifiers: Emphasis,
    /// The overlap priority: higher wins (default `0`).
    #[cfg_attr(feature = "serde", serde(default))]
    pub priority: i32,
}

impl Decoration {
    /// A decoration of `kind` over `range` colored by `role`, with no
    /// foreground, no modifiers and the default priority.
    #[must_use]
    pub const fn new(range: Range, kind: DecorationKind, role: Option<ThemeRole>) -> Self {
        Self {
            range,
            kind,
            role,
            fg: None,
            modifiers: Emphasis::NONE,
            priority: 0,
        }
    }

    /// Recolour the covered text with `fg` (a [`ThemeRole`] or a [`TokenId`]).
    #[must_use]
    pub fn with_fg(mut self, fg: impl Into<DecorationColor>) -> Self {
        self.fg = Some(fg.into());
        self
    }

    /// Add `modifiers` to the covered text.
    #[must_use]
    pub const fn with_modifiers(mut self, modifiers: Emphasis) -> Self {
        self.modifiers = self.modifiers.union(modifiers);
        self
    }

    /// Set the overlap priority (higher wins).
    #[must_use]
    pub const fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoration_kinds() {
        let dec = Decoration::new(
            Range::default(),
            DecorationKind::GutterMarker { glyph: '▎' },
            Some(ThemeRole::DiagnosticError),
        );
        assert_eq!(dec.kind, DecorationKind::GutterMarker { glyph: '▎' });
        // A change marker is its own kind, never equal to a marker with the
        // same glyph, so a renderer can give it its own lane.
        assert_ne!(
            DecorationKind::ChangeMarker { glyph: '▎' },
            DecorationKind::GutterMarker { glyph: '▎' }
        );
    }

    #[test]
    fn a_color_swatch_carries_its_color_instead_of_a_role() {
        // The one decoration whose color is data, not a theme role: a color
        // literal's own value travels with the decoration.
        let dec = Decoration::new(
            Range::default(),
            DecorationKind::ColorSwatch {
                rgba: [0x12, 0x34, 0x56, 0xff],
            },
            None,
        );
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
        let dec = Decoration::new(Range::default(), DecorationKind::Underline, None);
        assert_eq!(dec.clone(), dec);
        assert_ne!(dec.kind, DecorationKind::TextBackground);
    }

    /// A new decoration carries no style of its own and the default
    /// priority, so a producer that sets only a range, kind and role keeps
    /// the old first-supplied-wins behaviour.
    #[test]
    fn a_new_decoration_is_unstyled_at_the_default_priority() {
        let dec = Decoration::new(Range::default(), DecorationKind::TextBackground, None);
        assert_eq!(dec.fg, None);
        assert!(dec.modifiers.is_empty());
        assert_eq!(dec.priority, 0);
        let styled = dec
            .clone()
            .with_fg(TokenId::KEYWORD)
            .with_modifiers(Emphasis::BOLD)
            .with_modifiers(Emphasis::ITALIC)
            .with_priority(-3);
        assert_eq!(styled.fg, Some(DecorationColor::Token(TokenId::KEYWORD)));
        // `with_modifiers` adds; it never drops a flag set earlier.
        assert_eq!(styled.modifiers, Emphasis::BOLD | Emphasis::ITALIC);
        assert_eq!(styled.priority, -3);
        let role = dec.with_fg(ThemeRole::Muted);
        assert_eq!(role.fg, Some(DecorationColor::Role(ThemeRole::Muted)));
    }
}

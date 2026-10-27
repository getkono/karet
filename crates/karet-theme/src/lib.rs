//! `karet-theme` — color tokens, theme loading and contrast checking for karet.
//!
//! Maps the semantic [`TokenId`]/[`ThemeRole`] vocabulary (from `karet-core`) to
//! concrete [`Rgba`] colors, independent of any renderer. Ships a built-in dark
//! theme ([`Theme::dark`], also [`Theme::default`]); the `vscode` feature loads VS
//! Code JSON themes. Enable `view` to convert colors into ratatui values.
//!
//! A semantic [`TokenId`] (a language server's token type and modifiers, see
//! [`TokenId::semantic`]) resolves through the theme's `semanticTokenColors`
//! rules, falling back to the standard class closest to its type.

use karet_core::ThemeRole;
use karet_core::TokenId;

mod default;
#[cfg(feature = "vscode")]
mod load_vscode;
mod semantic;

/// Number of [`StandardToken`](karet_core::StandardToken) classes (token id space).
pub(crate) const TOKEN_COUNT: usize = 32;
/// Number of [`ThemeRole`] variants.
pub(crate) const ROLE_COUNT: usize = 41;

/// Errors produced while loading a theme.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ThemeError {
    /// The theme file could not be parsed.
    #[error("failed to parse theme")]
    Parse,
    /// The theme format is not supported.
    #[error("unsupported theme format")]
    Unsupported,
}

/// An 8-bit-per-channel RGBA color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel (255 = opaque).
    pub a: u8,
}

impl Rgba {
    /// Create an opaque color from `r`/`g`/`b` (alpha 255).
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Parse a `#rgb`, `#rrggbb`, or `#rrggbbaa` hex string (the `#` is optional).
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        // `from_str_radix` accepts a leading `+`, so `#+f+f+f` would otherwise parse.
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        let nibble = |i: usize| {
            u8::from_str_radix(hex.get(i..i + 1)?, 16)
                .ok()
                .map(|v| v * 17)
        };
        match hex.len() {
            3 => Some(Self {
                r: nibble(0)?,
                g: nibble(1)?,
                b: nibble(2)?,
                a: 255,
            }),
            6 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: 255,
            }),
            8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => None,
        }
    }

    /// Convert to a ratatui truecolor value (alpha dropped — terminals are opaque).
    #[cfg(feature = "view")]
    #[must_use]
    pub fn to_ratatui(self) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(self.r, self.g, self.b)
    }
}

pub use karet_core::Emphasis;

/// Ratatui conversion for the neutral [`Emphasis`] flags (which live in
/// `karet-core`; this view-only extension keeps ratatui out of the vocabulary).
#[cfg(feature = "view")]
pub trait EmphasisExt {
    /// Convert to a ratatui modifier set (empty when no flag is set).
    fn to_ratatui(self) -> ratatui::style::Modifier;
}

#[cfg(feature = "view")]
impl EmphasisExt for Emphasis {
    fn to_ratatui(self) -> ratatui::style::Modifier {
        let mut m = ratatui::style::Modifier::empty();
        if self.bold {
            m |= ratatui::style::Modifier::BOLD;
        }
        if self.italic {
            m |= ratatui::style::Modifier::ITALIC;
        }
        if self.underline {
            m |= ratatui::style::Modifier::UNDERLINED;
        }
        if self.strikethrough {
            m |= ratatui::style::Modifier::CROSSED_OUT;
        }
        m
    }
}

/// A resolved color theme: maps token classes and UI roles to colors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    tokens: [Rgba; TOKEN_COUNT],
    emphasis: [Emphasis; TOKEN_COUNT],
    fallback_fg: Rgba,
    roles: [Rgba; ROLE_COUNT],
    dark: bool,
    /// `semanticTokenColors`, in the order a tie between equally specific
    /// rules is settled by (the later wins).
    semantic: Vec<semantic::SemanticRule>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The built-in dark theme.
    #[must_use]
    pub fn dark() -> Self {
        default::dark()
    }

    /// Load a VS Code JSON theme (requires the `vscode` feature).
    ///
    /// Unknown keys fall back to the built-in dark theme's values.
    ///
    /// # Errors
    /// Returns [`ThemeError::Parse`] if the JSON is malformed, or
    /// [`ThemeError::Unsupported`] if the `vscode` feature is disabled.
    pub fn load_vscode(json: &str) -> Result<Self, ThemeError> {
        #[cfg(feature = "vscode")]
        {
            load_vscode::load(json)
        }
        #[cfg(not(feature = "vscode"))]
        {
            let _ = json;
            Err(ThemeError::Unsupported)
        }
    }

    /// The color for a token class (the fallback foreground if unmapped).
    ///
    /// A semantic token's id takes the foreground its most specific
    /// `semanticTokenColors` rule sets, else its fallback class's color.
    #[must_use]
    pub fn color(&self, token: TokenId) -> Rgba {
        if let Some((ty, modifiers)) = token.semantic_parts() {
            return semantic::resolve(&self.semantic, ty, modifiers)
                .fg
                .unwrap_or_else(|| self.color(semantic::fallback(ty, modifiers).id()));
        }
        self.tokens
            .get(token.0 as usize)
            .copied()
            .unwrap_or(self.fallback_fg)
    }

    /// The text emphasis for a token class (none if unmapped).
    ///
    /// A semantic token's id takes each flag its most specific
    /// `semanticTokenColors` rule sets, the rest from its fallback class.
    #[must_use]
    pub fn emphasis(&self, token: TokenId) -> Emphasis {
        if let Some((ty, modifiers)) = token.semantic_parts() {
            let style = semantic::resolve(&self.semantic, ty, modifiers);
            let base = self.emphasis(semantic::fallback(ty, modifiers).id());
            return Emphasis {
                bold: style.bold.unwrap_or(base.bold),
                italic: style.italic.unwrap_or(base.italic),
                underline: style.underline.unwrap_or(base.underline),
                strikethrough: style.strikethrough.unwrap_or(base.strikethrough),
            };
        }
        self.emphasis
            .get(token.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    /// The color for a UI role (the fallback foreground if unmapped).
    #[must_use]
    pub fn role(&self, role: ThemeRole) -> Rgba {
        self.roles
            .get(role as usize)
            .copied()
            .unwrap_or(self.fallback_fg)
    }

    /// The colours bracket-pair colourisation cycles through, outermost pair
    /// first: the theme's constant, keyword and function colours.
    ///
    /// Drawn from the token palette rather than a palette of their own, so
    /// every theme — a loaded VS Code theme included — colours its brackets
    /// in hues it already uses, in the gold / purple / blue order VS Code's
    /// own bracket colours follow.
    #[must_use]
    pub fn bracket_pair_colors(&self) -> [Rgba; 3] {
        use karet_core::StandardToken;
        [
            StandardToken::Constant,
            StandardToken::Keyword,
            StandardToken::Function,
        ]
        .map(|token| self.color(token.id()))
    }

    /// Whether this is a dark theme (background luminance below the midpoint).
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// The ratatui foreground style for a UI role — the one-step spelling of
    /// `Style::default().fg(theme.role(role).to_ratatui())`.
    #[cfg(feature = "view")]
    #[must_use]
    pub fn style(&self, role: ThemeRole) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(self.role(role).to_ratatui())
    }

    /// The full ratatui style for a semantic token class: its foreground color
    /// plus its emphasis modifiers (bold/italic/…), so themed emphasis is never
    /// silently dropped.
    #[cfg(feature = "view")]
    #[must_use]
    pub fn token_style(&self, token: TokenId) -> ratatui::style::Style {
        ratatui::style::Style::default()
            .fg(self.color(token).to_ratatui())
            .add_modifier(EmphasisExt::to_ratatui(self.emphasis(token)))
    }
}

/// The WCAG 2.1 contrast ratio between two colors (1.0 – 21.0).
///
/// Symmetric in its arguments; alpha is ignored. Behind the `view` feature:
/// contrast checking is a presentation-time concern.
#[cfg(feature = "view")]
#[must_use]
pub fn contrast_ratio(fg: Rgba, bg: Rgba) -> f32 {
    let (a, b) = (relative_luminance(fg), relative_luminance(bg));
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

/// The WCAG 2.1 relative luminance of `c` (0.0 for black – 1.0 for white).
///
/// The weights are the luminance (Y) row of the sRGB → CIE XYZ (D65) matrix;
/// WCAG prints the same row rounded to four places.
#[cfg(feature = "view")]
fn relative_luminance(c: Rgba) -> f32 {
    0.212_672_9 * srgb_to_linear(c.r)
        + 0.715_152_2 * srgb_to_linear(c.g)
        + 0.072_175 * srgb_to_linear(c.b)
}

/// Decodes one 8-bit sRGB channel to linear light (0.0 – 1.0).
///
/// No 8-bit value falls between WCAG 2.1's `0.03928` knee and the sRGB
/// standard's `0.04045` (10/255 ≈ 0.0392, 11/255 ≈ 0.0431), so either reads
/// the same here.
#[cfg(feature = "view")]
fn srgb_to_linear(channel: u8) -> f32 {
    let v = f32::from(channel) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Whether `c` is a dark color (Rec. 601 luma below the midpoint).
pub(crate) fn is_dark_color(c: Rgba) -> bool {
    let luma = 0.299 * f32::from(c.r) + 0.587 * f32::from(c.g) + 0.114 * f32::from(c.b);
    luma < 128.0
}

#[cfg(test)]
mod tests {
    use karet_core::StandardToken;

    use super::*;

    #[test]
    fn error_displays() {
        assert_eq!(
            ThemeError::Unsupported.to_string(),
            "unsupported theme format"
        );
    }

    #[cfg(not(feature = "vscode"))]
    #[test]
    fn load_vscode_without_the_feature_is_unsupported() {
        assert!(matches!(
            Theme::load_vscode("{}"),
            Err(ThemeError::Unsupported)
        ));
    }

    #[test]
    fn bracket_pair_colors_are_three_distinct_token_hues() {
        let t = Theme::dark();
        let colors = t.bracket_pair_colors();
        assert_eq!(colors[0], t.color(StandardToken::Constant.id()));
        assert_eq!(colors[1], t.color(StandardToken::Keyword.id()));
        assert_eq!(colors[2], t.color(StandardToken::Function.id()));
        assert!(
            colors[0] != colors[1] && colors[1] != colors[2] && colors[0] != colors[2],
            "adjacent depths must read apart: {colors:?}"
        );
    }

    #[test]
    fn rgba_from_hex() {
        assert_eq!(Rgba::from_hex("#1a1b26"), Some(Rgba::rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(Rgba::from_hex("fff"), Some(Rgba::rgb(255, 255, 255)));
        assert_eq!(
            Rgba::from_hex("#11223344"),
            Some(Rgba {
                r: 0x11,
                g: 0x22,
                b: 0x33,
                a: 0x44
            })
        );
        assert_eq!(Rgba::from_hex("#xyz"), None);
        assert_eq!(Rgba::from_hex("#12345"), None);
    }

    #[test]
    fn rgba_from_hex_rejects_sign_characters() {
        // `from_str_radix` alone accepts a leading `+` per component.
        assert_eq!(Rgba::from_hex("#+f+f+f"), None);
        assert_eq!(Rgba::from_hex("+ff"), None);
        assert_eq!(Rgba::from_hex("#+1+2+3+4"), None);
        assert_eq!(Rgba::from_hex("#-1-2-3"), None);
        assert_eq!(Rgba::from_hex("# ff"), None);
    }

    #[test]
    fn dark_theme_resolves_colors() {
        let t = Theme::dark();
        assert!(t.is_dark());
        // Background is dark, foreground is light.
        assert!(is_dark_color(t.role(ThemeRole::Background)));
        assert!(!is_dark_color(t.role(ThemeRole::Foreground)));
        // Keyword has a non-default color, distinct from a string.
        assert_ne!(
            t.color(StandardToken::Keyword.id()),
            t.color(StandardToken::String.id())
        );
        // Unmapped token id falls back to foreground.
        // (Below the semantic id space, which resolves differently.)
        let unmapped = TokenId(TokenId::SEMANTIC_BASE - 1);
        assert_eq!(t.color(unmapped), t.role(ThemeRole::Foreground));
    }

    #[test]
    fn token_palette_covers_every_standard_token() {
        // `TOKEN_COUNT` must track the `StandardToken` enum: a token whose id lands past
        // the palette silently renders as plain foreground. `CommentMark` is the last
        // variant, so bounding it bounds them all.
        assert!((StandardToken::CommentMark.id().0 as usize) < TOKEN_COUNT);
        // And the palette is not oversized relative to the enum.
        assert_eq!(StandardToken::CommentMark.id().0 as usize, TOKEN_COUNT - 1);
    }

    /// A semantic token no built-in rule styles takes its fallback class's
    /// colour and emphasis exactly, so turning semantic tokens on does not
    /// repaint plain code; the built-in rules restyle only what tree-sitter
    /// cannot tell apart.
    #[test]
    fn semantic_tokens_resolve_through_rules_over_their_fallback() {
        use karet_core::SemanticTokenModifiers as M;
        use karet_core::SemanticTokenType as T;

        let t = Theme::dark();
        let variable = TokenId::semantic(T::Variable, M::DECLARATION);
        assert_eq!(t.color(variable), t.color(TokenId::VARIABLE));
        assert_eq!(t.emphasis(variable), Emphasis::NONE);
        let builtin = TokenId::semantic(T::Struct, M::DEFAULT_LIBRARY);
        assert_eq!(t.color(builtin), t.color(StandardToken::TypeBuiltin.id()));
        let doc = TokenId::semantic(T::Comment, M::DOCUMENTATION);
        assert_eq!(t.emphasis(doc), t.emphasis(StandardToken::CommentDoc.id()));

        let mac = TokenId::semantic(T::Macro, M::NONE);
        assert_ne!(t.color(mac), t.color(TokenId::FUNCTION));
        let unresolved = TokenId::semantic(T::UnresolvedReference, M::NONE);
        assert_ne!(t.color(unresolved), t.color(TokenId::VARIABLE));
        let mutable = TokenId::semantic(T::Variable, M::MUTABLE);
        assert_eq!(t.color(mutable), t.color(TokenId::VARIABLE));
        assert!(t.emphasis(mutable).underline);
        let deprecated = TokenId::semantic(T::Method, M::DEPRECATED | M::MUTABLE);
        assert!(t.emphasis(deprecated).strikethrough);
        assert!(t.emphasis(deprecated).underline);
    }

    #[test]
    fn markup_emphasis_defaults() {
        let t = Theme::dark();
        assert!(t.emphasis(StandardToken::MarkupBold.id()).bold);
        assert!(t.emphasis(StandardToken::MarkupItalic.id()).italic);
        assert!(t.emphasis(StandardToken::MarkupHeading.id()).bold);
        assert!(t.emphasis(StandardToken::CommentDoc.id()).italic);
        assert!(
            t.emphasis(StandardToken::MarkupStrikethrough.id())
                .strikethrough
        );
        // Code tokens stay unemphasized.
        assert_eq!(t.emphasis(StandardToken::Keyword.id()), Emphasis::default());
        // An unmapped id yields no emphasis rather than panicking.
        assert_eq!(
            t.emphasis(TokenId(TokenId::SEMANTIC_BASE - 1)),
            Emphasis::default()
        );
    }

    #[cfg(feature = "view")]
    #[test]
    fn doc_comment_is_brighter_than_comment() {
        let t = Theme::dark();
        let bg = t.role(ThemeRole::Background);
        assert!(
            contrast_ratio(t.color(StandardToken::CommentDoc.id()), bg)
                > contrast_ratio(t.color(StandardToken::Comment.id()), bg)
        );
    }

    #[cfg(feature = "view")]
    #[test]
    fn semantic_comment_marker_stands_out_from_comments() {
        let t = Theme::dark();
        let bg = t.role(ThemeRole::Background);
        // The codetag marker must be distinct from — and louder than — both the plain
        // comment and the doc comment, or it fails its one job of drawing the eye.
        let mark = t.color(StandardToken::CommentMark.id());
        assert_ne!(mark, t.color(StandardToken::Comment.id()));
        assert_ne!(mark, t.color(StandardToken::CommentDoc.id()));
        assert!(
            contrast_ratio(mark, bg) > contrast_ratio(t.color(StandardToken::Comment.id()), bg)
        );
        assert!(
            contrast_ratio(mark, bg) > contrast_ratio(t.color(StandardToken::CommentDoc.id()), bg)
        );
        // And it carries weight, not just hue.
        assert!(t.emphasis(StandardToken::CommentMark.id()).bold);
    }

    #[cfg(feature = "view")]
    #[test]
    fn foreground_on_background_is_readable() {
        let t = Theme::dark();
        let ratio = contrast_ratio(t.role(ThemeRole::Foreground), t.role(ThemeRole::Background));
        // WCAG AA for normal text is 4.5; a code theme's default fg should clear it.
        assert!(ratio > 4.5, "contrast ratio was {ratio}");
    }

    #[cfg(feature = "view")]
    #[test]
    fn contrast_ratio_spans_one_to_twenty_one_symmetrically() {
        let black = Rgba::rgb(0, 0, 0);
        let white = Rgba::rgb(255, 255, 255);
        assert!((contrast_ratio(white, black) - 21.0).abs() < 1e-4);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-4);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-6);
        let teal = Rgba::rgb(0x12, 0x80, 0x9a);
        assert!((contrast_ratio(teal, white) - contrast_ratio(white, teal)).abs() < 1e-6);
        // Alpha plays no part.
        let translucent = Rgba { a: 0x10, ..teal };
        assert!((contrast_ratio(translucent, white) - contrast_ratio(teal, white)).abs() < 1e-6);
    }

    /// The two greys either side of WCAG AA on white: #767676 passes (4.54:1),
    /// #777777 fails (4.48:1). They pin the transfer curve and the weights,
    /// which a 4.5 threshold check reads at the second decimal.
    #[cfg(feature = "view")]
    #[test]
    fn contrast_ratio_matches_published_aa_greys() {
        let white = Rgba::rgb(255, 255, 255);
        let pass = contrast_ratio(Rgba::rgb(0x76, 0x76, 0x76), white);
        let fail = contrast_ratio(Rgba::rgb(0x77, 0x77, 0x77), white);
        assert!((pass - 4.54).abs() < 0.005, "{pass}");
        assert!((fail - 4.48).abs() < 0.005, "{fail}");
        // A dark channel below the linear-segment knee (10/255) reads on the
        // linear segment: black-on-it is (L + 0.05) / 0.05 with L = 10/255/12.92.
        let near_black = contrast_ratio(Rgba::rgb(10, 10, 10), Rgba::rgb(0, 0, 0));
        let expected = (10.0 / 255.0 / 12.92 + 0.05) / 0.05;
        assert!((near_black - expected).abs() < 1e-4, "{near_black}");
    }

    #[test]
    fn the_scrollbar_roles_resolve_to_their_own_colors() {
        // `role()` indexes a fixed-size array by `role as usize` and silently falls
        // back to the foreground when the index is out of range, so a role added
        // without bumping ROLE_COUNT would degrade invisibly. The last-declared
        // roles are the ones that catch that, and the track must not resolve to the
        // fallback — a track painted in the foreground color is a solid bar.
        let theme = Theme::dark();
        assert!((ThemeRole::DebugStoppedLine as usize) < ROLE_COUNT);
        assert_eq!(
            theme.role(ThemeRole::ScrollbarTrack),
            theme.role(ThemeRole::IndentGuide)
        );
        assert_ne!(theme.role(ThemeRole::ScrollbarTrack), theme.fallback_fg);
    }

    #[test]
    fn each_inlay_hint_kind_has_its_own_dim_readable_color() {
        // A type and a parameter name must read apart, and neither may fall
        // back to the foreground (which would make a hint look like code).
        let theme = Theme::dark();
        assert!((ThemeRole::InlayHintParameter as usize) < ROLE_COUNT);
        #[cfg(feature = "view")]
        let bg = theme.role(ThemeRole::Background);
        let other = theme.role(ThemeRole::InlayHint);
        let ty = theme.role(ThemeRole::InlayHintType);
        let param = theme.role(ThemeRole::InlayHintParameter);
        assert_ne!(ty, param);
        assert_ne!(ty, other);
        assert_ne!(param, other);
        for hint in [other, ty, param] {
            assert_ne!(hint, theme.role(ThemeRole::Foreground));
            // Contrast checking (and its color-science dep) is `view`-only.
            #[cfg(feature = "view")]
            assert!(contrast_ratio(hint, bg) > 2.0);
        }
    }

    #[test]
    fn code_lenses_have_their_own_dim_readable_color() {
        // The last-declared role: an index past ROLE_COUNT would fall back
        // to the foreground and make a lens read as code.
        let theme = Theme::dark();
        assert!((ThemeRole::CodeLens as usize) < ROLE_COUNT);
        let lens = theme.role(ThemeRole::CodeLens);
        assert_ne!(lens, theme.role(ThemeRole::Foreground));
        assert_ne!(lens, theme.role(ThemeRole::InlayHint));
        #[cfg(feature = "view")]
        assert!(contrast_ratio(lens, theme.role(ThemeRole::Background)) > 2.0);
    }

    #[test]
    fn gutter_change_roles_are_distinct_readable_accents() {
        // The three markers share one gutter cell, so they are told apart by hue
        // alone: each must be its own color, and each must read against the
        // editor background (the diff background tints do not).
        let theme = Theme::dark();
        assert!((ThemeRole::GutterDeleted as usize) < ROLE_COUNT);
        #[cfg(feature = "view")]
        let bg = theme.role(ThemeRole::Background);
        let added = theme.role(ThemeRole::GutterAdded);
        let modified = theme.role(ThemeRole::GutterModified);
        let deleted = theme.role(ThemeRole::GutterDeleted);
        assert_ne!(added, modified);
        assert_ne!(added, deleted);
        assert_ne!(modified, deleted);
        for marker in [added, modified, deleted] {
            assert_ne!(marker, theme.fallback_fg);
            #[cfg(feature = "view")]
            assert!(contrast_ratio(marker, bg) > 3.0);
        }
    }

    #[test]
    fn the_current_search_match_reads_apart_from_the_other_matches() {
        // Both are text backgrounds under code, so the current one must be its
        // own color (not the fallback foreground, which would hide the text).
        let theme = Theme::dark();
        assert!((ThemeRole::SearchMatchCurrent as usize) < ROLE_COUNT);
        let current = theme.role(ThemeRole::SearchMatchCurrent);
        assert_ne!(current, theme.role(ThemeRole::SearchMatch));
        assert_ne!(current, theme.role(ThemeRole::Selection));
        assert_ne!(current, theme.fallback_fg);
    }

    #[test]
    fn occurrence_roles_are_their_own_readable_text_backgrounds() {
        // The last-declared role: an index past ROLE_COUNT would fall back to
        // the foreground and paint every occurrence as a solid bar over its
        // own text.
        let theme = Theme::dark();
        assert!((ThemeRole::WordHighlightStrong as usize) < ROLE_COUNT);
        let read = theme.role(ThemeRole::WordHighlight);
        let write = theme.role(ThemeRole::WordHighlightStrong);
        // A write stands out from a read, and an occurrence reads apart from
        // every other background it can sit beside or under.
        assert_ne!(read, write);
        for other in [
            ThemeRole::Background,
            ThemeRole::CursorLine,
            ThemeRole::Selection,
            ThemeRole::SearchMatch,
            ThemeRole::SearchMatchCurrent,
            ThemeRole::MatchingBracket,
            ThemeRole::HoverHighlight,
        ] {
            assert_ne!(read, theme.role(other), "{other:?}");
            assert_ne!(write, theme.role(other), "{other:?}");
        }
        // The code on an occurrence stays readable (WCAG AA for normal text).
        #[cfg(feature = "view")]
        {
            let fg = theme.role(ThemeRole::Foreground);
            for bg in [read, write] {
                assert!(contrast_ratio(fg, bg) > 4.5, "{bg:?}");
            }
        }
    }

    #[cfg(feature = "view")]
    #[test]
    fn style_and_token_style_resolve_color_and_emphasis() {
        use karet_core::StandardToken;
        let theme = Theme::dark();
        let role_style = theme.style(karet_core::ThemeRole::Foreground);
        assert_eq!(
            role_style.fg,
            Some(theme.role(karet_core::ThemeRole::Foreground).to_ratatui())
        );
        // Keywords carry the theme's emphasis (bold in the built-in dark theme's
        // mapping, or none) — either way the color must match and the modifier
        // set must equal the theme's emphasis conversion.
        let token = StandardToken::Keyword.id();
        let token_style = theme.token_style(token);
        assert_eq!(token_style.fg, Some(theme.color(token).to_ratatui()));
        assert_eq!(
            token_style.add_modifier,
            EmphasisExt::to_ratatui(theme.emphasis(token))
        );
    }
}

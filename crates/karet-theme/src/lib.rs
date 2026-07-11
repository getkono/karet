//! `karet-theme` — color tokens, theme loading and contrast checking for karet.
//!
//! Maps the semantic [`TokenId`]/[`ThemeRole`] vocabulary (from `karet-core`) to
//! concrete [`Rgba`] colors, independent of any renderer. Ships a built-in dark
//! theme ([`Theme::dark`], also [`Theme::default`]); the `vscode` feature loads VS
//! Code JSON themes. Enable `view` to convert colors into ratatui values.

use karet_core::ThemeRole;
use karet_core::TokenId;

mod default;
#[cfg(feature = "vscode")]
mod load_vscode;

/// Number of [`StandardToken`](karet_core::StandardToken) classes (token id space).
pub(crate) const TOKEN_COUNT: usize = 32;
/// Number of [`ThemeRole`] variants.
pub(crate) const ROLE_COUNT: usize = 28;

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

/// The text emphasis a theme requests for a token class, independent of any
/// renderer. Markup tokens carry weight/slant as much as color — a markdown
/// heading reads as a heading because it is **bold**, not merely blue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Emphasis {
    /// Render bold.
    pub bold: bool,
    /// Render italic.
    pub italic: bool,
    /// Render struck through.
    pub strikethrough: bool,
}

impl Emphasis {
    /// Bold, and nothing else.
    pub(crate) const BOLD: Self = Self {
        bold: true,
        italic: false,
        strikethrough: false,
    };
    /// Italic, and nothing else.
    pub(crate) const ITALIC: Self = Self {
        bold: false,
        italic: true,
        strikethrough: false,
    };
    /// Struck through, and nothing else.
    pub(crate) const STRIKETHROUGH: Self = Self {
        bold: false,
        italic: false,
        strikethrough: true,
    };

    /// Convert to a ratatui modifier set (empty when neither flag is set).
    #[cfg(feature = "view")]
    #[must_use]
    pub fn to_ratatui(self) -> ratatui::style::Modifier {
        let mut m = ratatui::style::Modifier::empty();
        if self.bold {
            m |= ratatui::style::Modifier::BOLD;
        }
        if self.italic {
            m |= ratatui::style::Modifier::ITALIC;
        }
        if self.strikethrough {
            m |= ratatui::style::Modifier::CROSSED_OUT;
        }
        m
    }
}

/// A resolved color theme: maps token classes and UI roles to colors.
#[derive(Clone, Debug)]
pub struct Theme {
    tokens: [Rgba; TOKEN_COUNT],
    emphasis: [Emphasis; TOKEN_COUNT],
    fallback_fg: Rgba,
    roles: [Rgba; ROLE_COUNT],
    dark: bool,
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

    /// Load a TextMate `.tmTheme` (plist XML) theme.
    ///
    /// # Errors
    /// Currently always returns [`ThemeError::Unsupported`] — tmTheme loading is
    /// reserved; the built-in [`Theme::dark`] is used meanwhile.
    pub fn load_tmtheme(bytes: &[u8]) -> Result<Self, ThemeError> {
        let _ = bytes;
        Err(ThemeError::Unsupported)
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

    /// The color for a semantic token class (the fallback foreground if unmapped).
    #[must_use]
    pub fn color(&self, token: TokenId) -> Rgba {
        self.tokens
            .get(token.0 as usize)
            .copied()
            .unwrap_or(self.fallback_fg)
    }

    /// The text emphasis for a semantic token class (none if unmapped).
    #[must_use]
    pub fn emphasis(&self, token: TokenId) -> Emphasis {
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

    /// Whether this is a dark theme (background luminance below the midpoint).
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.dark
    }
}

/// The WCAG 2.1 contrast ratio between two colors (1.0 – 21.0).
#[must_use]
pub fn contrast_ratio(fg: Rgba, bg: Rgba) -> f32 {
    use palette::Srgb;
    use palette::color_difference::Wcag21RelativeContrast;

    let f = Srgb::new(fg.r, fg.g, fg.b).into_format::<f32>();
    let b = Srgb::new(bg.r, bg.g, bg.b).into_format::<f32>();
    f.relative_contrast(b)
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
        assert_eq!(t.color(TokenId(60000)), t.role(ThemeRole::Foreground));
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
        assert_eq!(t.emphasis(TokenId(60000)), Emphasis::default());
    }

    #[test]
    fn doc_comment_is_brighter_than_comment() {
        let t = Theme::dark();
        let bg = t.role(ThemeRole::Background);
        assert!(
            contrast_ratio(t.color(StandardToken::CommentDoc.id()), bg)
                > contrast_ratio(t.color(StandardToken::Comment.id()), bg)
        );
    }

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

    #[test]
    fn foreground_on_background_is_readable() {
        let t = Theme::dark();
        let ratio = contrast_ratio(t.role(ThemeRole::Foreground), t.role(ThemeRole::Background));
        // WCAG AA for normal text is 4.5; a code theme's default fg should clear it.
        assert!(ratio > 4.5, "contrast ratio was {ratio}");
    }
}

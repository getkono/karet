//! `karet-theme` — color tokens, theme loading and contrast checking for karet.
//!
//! Maps the semantic [`TokenId`]/[`ThemeRole`] vocabulary (from `karet-core`) to
//! concrete [`Rgba`] colors, independent of any renderer. Enable the `view`
//! feature to convert resolved colors into ratatui `Style`s.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! loaders, detection and contrast logic are filled in separately.

use karet_core::{ThemeRole, TokenId};

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

/// A resolved color theme: maps token classes and UI roles to colors.
#[derive(Clone, Debug, Default)]
pub struct Theme {}

impl Theme {
    /// Load a TextMate `.tmTheme` (plist XML) theme.
    ///
    /// # Errors
    /// Returns [`ThemeError::Parse`] if the data is malformed.
    pub fn load_tmtheme(bytes: &[u8]) -> Result<Self, ThemeError> {
        let _ = bytes;
        todo!()
    }

    /// Load a VS Code JSON theme.
    ///
    /// # Errors
    /// Returns [`ThemeError::Parse`] if the JSON is malformed.
    pub fn load_vscode(json: &str) -> Result<Self, ThemeError> {
        let _ = json;
        todo!()
    }

    /// The color for a semantic token class.
    #[must_use]
    pub fn color(&self, token: TokenId) -> Rgba {
        let _ = token;
        todo!()
    }

    /// The color for a UI role.
    #[must_use]
    pub fn role(&self, role: ThemeRole) -> Rgba {
        let _ = role;
        todo!()
    }

    /// Whether this is a dark theme (background luminance below the midpoint).
    #[must_use]
    pub fn is_dark(&self) -> bool {
        todo!()
    }
}

/// The WCAG contrast ratio between two colors (1.0 – 21.0).
#[must_use]
pub fn contrast_ratio(fg: Rgba, bg: Rgba) -> f32 {
    let _ = (fg, bg);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_defaults_transparent_black() {
        assert_eq!(
            Rgba::default(),
            Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0
            }
        );
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            ThemeError::Unsupported.to_string(),
            "unsupported theme format"
        );
    }
}

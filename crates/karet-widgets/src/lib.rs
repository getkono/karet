//! `karet-widgets` — a reusable ratatui widget toolkit for building editors.
//!
//! A lightweight (ratatui-only) crate of the UI widgets an editor needs. Widgets
//! render data fed in by the application — they consume `karet-core` models and a
//! [`SymbolProvider`], and so do **not** depend on the producers
//! (`karet-lsp`/`karet-vcs`/`karet-dap`). This crate also hosts the merged
//! [`image`] module and the LSP [`completion`]/[`hover`] popups, which render
//! `karet-core` models supplied over the backend's event stream.
//!
//! This is the implementation *skeleton*: each widget's data joint (the borrowed
//! inputs it renders) is defined as a builder struct; the ratatui `Widget` render
//! impls are filled in separately.

use karet_core::{Diagnostic, LineCol, SymbolProvider};
use karet_fuzzy::Matcher;

pub mod file_tree;
pub mod hex;
pub mod viewer;

pub use file_tree::{FileTree, FileTreeRow, FileTreeState, IconSet};
pub use hex::HexView;

/// A symbol outline tree over a [`SymbolProvider`].
pub struct Outline<'a> {
    /// The symbols to display.
    pub provider: &'a dyn SymbolProvider,
}

/// Breadcrumbs showing the symbol path containing a position.
pub struct Breadcrumbs<'a> {
    /// The symbols to walk.
    pub provider: &'a dyn SymbolProvider,
    /// The cursor position whose containing symbols are shown.
    pub position: LineCol,
}

/// A diagnostics ("problems") list.
pub struct Problems<'a> {
    /// The diagnostics to list.
    pub diagnostics: &'a [Diagnostic],
}

/// A fuzzy quick-open / command-palette picker over arbitrary items.
pub struct Picker<'a, T> {
    /// The items to choose from.
    pub items: &'a [T],
    /// The matcher used for incremental filtering.
    pub matcher: &'a mut Matcher,
}

/// A status bar with a left and right section.
#[derive(Clone, Debug, Default)]
pub struct StatusBar {
    /// Left-aligned text.
    pub left: String,
    /// Right-aligned text.
    pub right: String,
}

/// A pane split tree with a focus ring (the editor's window layout).
#[derive(Clone, Debug, Default)]
pub struct PaneLayout {}

/// The LSP completion popup (relocated here from `karet-lsp`).
pub mod completion {
    use karet_core::CompletionItem;
    use karet_fuzzy::Matcher;

    /// A completion popup that fuzzy-filters [`CompletionItem`]s as you type.
    pub struct CompletionPopup<'a> {
        /// The candidate items, supplied by the backend.
        pub items: &'a [CompletionItem],
        /// The matcher used for incremental filtering.
        pub matcher: &'a mut Matcher,
    }
}

/// The LSP hover / documentation popup (relocated here from `karet-lsp`).
pub mod hover {
    use karet_core::Markup;

    /// A hover popup rendering markup (via `karet-markdown` for the Markdown kind).
    pub struct HoverPopup<'a> {
        /// The markup payload to render.
        pub markup: &'a Markup,
    }
}

/// Terminal image rendering (merged from the former `karet-image` crate).
pub mod image {
    /// Errors decoding or rendering an image.
    #[derive(Debug, thiserror::Error)]
    #[non_exhaustive]
    pub enum ImageError {
        /// The image bytes could not be decoded.
        #[error("failed to decode image")]
        Decode,
    }

    /// A decoded, scalable image.
    pub struct Image {}

    /// Decode image bytes into an [`Image`].
    ///
    /// # Errors
    /// Returns [`ImageError::Decode`] if the bytes are not a supported format.
    pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
        let _ = bytes;
        todo!()
    }

    /// A ratatui widget that renders an [`Image`] using terminal graphics
    /// (halfblocks / Kitty / Sixel / iTerm2).
    pub struct ImageWidget<'a> {
        /// The image to render.
        pub image: &'a Image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use karet_core::Symbol;

    #[test]
    fn outline_consumes_a_provider() {
        let syms: Vec<Symbol> = Vec::new();
        let outline = Outline { provider: &syms };
        assert!(outline.provider.symbols().is_empty());
    }

    #[test]
    fn image_error_displays() {
        assert_eq!(
            image::ImageError::Decode.to_string(),
            "failed to decode image"
        );
    }
}

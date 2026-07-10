//! `karet-widgets` — a reusable ratatui widget toolkit for building editors.
//!
//! A lightweight (ratatui-only) crate of the UI widgets an editor needs. Widgets
//! render data fed in by the application — they consume `karet-core` models and a
//! [`SymbolProvider`], and so do **not** depend on the producers
//! (`karet-lsp`/`karet-vcs`/`karet-dap`). This crate also hosts the LSP
//! [`completion`]/[`hover`] popups, which render `karet-core` models supplied over
//! the backend's event stream. The read-only file-view primitives (hex dump,
//! terminal image, placeholder) live in `karet-fileview`.
//!
//! This is the implementation *skeleton*: each widget's data joint (the borrowed
//! inputs it renders) is defined as a builder struct; the ratatui `Widget` render
//! impls are filled in separately.

use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::SymbolProvider;
use karet_fuzzy::Matcher;

pub mod file_tree;
pub mod glyph;
pub mod notify;
pub mod pane;
pub mod select;

pub use file_tree::FileTree;
pub use file_tree::FileTreeRow;
pub use file_tree::FileTreeState;
pub use file_tree::PendingEdit;
pub use glyph::UiIcon;
pub use karet_filetype::IconStyle;
pub use notify::Corner;
pub use notify::ToastSlot;
pub use notify::Toasts;
pub use pane::DropZone;
pub use pane::PaneId;
pub use pane::PaneLayout;
pub use pane::SplitAxis;
pub use pane::SplitDir;
pub use pane::drop_preview_rect;
pub use pane::drop_zone;
pub use select::ListSelection;

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
    use karet_core::MarkupKind;
    use karet_theme::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::text::Line;
    use ratatui::widgets::Widget;

    /// A hover popup rendering markup (via `karet-markdown` for the Markdown kind).
    ///
    /// Markdown is parsed, soft-wrapped to the popup's width and painted through the
    /// theme, so a `///` doc comment's headings render bold and its fenced code blocks
    /// are syntax-highlighted as the language they name.
    pub struct HoverPopup<'a> {
        /// The markup payload to render.
        pub markup: &'a Markup,
        /// The theme resolving token colors and emphasis.
        pub theme: &'a Theme,
    }

    impl<'a> HoverPopup<'a> {
        /// Build a popup for `markup`.
        #[must_use]
        pub fn new(markup: &'a Markup, theme: &'a Theme) -> Self {
            Self { markup, theme }
        }

        /// The styled lines this popup paints, soft-wrapped to `width` columns.
        ///
        /// Plain-text markup is emitted verbatim, one line per source line — an LSP
        /// server that sends plain text means it literally.
        #[must_use]
        pub fn lines(&self, width: u16) -> Vec<Line<'static>> {
            match self.markup.kind {
                MarkupKind::Markdown => {
                    let doc = karet_markdown::parse(&self.markup.value).wrap(width);
                    karet_markdown::view::to_ratatui(&doc, self.theme)
                },
                MarkupKind::PlainText => self
                    .markup
                    .value
                    .lines()
                    .map(|l| Line::from(l.to_owned()))
                    .collect(),
            }
        }
    }

    impl Widget for HoverPopup<'_> {
        fn render(self, area: Rect, buf: &mut Buffer) {
            if area.width == 0 || area.height == 0 {
                return;
            }
            // Overflow is clipped, not wrapped again: the caller sized the popup.
            for (row, line) in self
                .lines(area.width)
                .iter()
                .take(area.height.into())
                .enumerate()
            {
                let y = area
                    .y
                    .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
                buf.set_line(area.x, y, line, area.width);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use karet_core::Symbol;

    use super::*;

    #[test]
    fn outline_consumes_a_provider() {
        let syms: Vec<Symbol> = Vec::new();
        let outline = Outline { provider: &syms };
        assert!(outline.provider.symbols().is_empty());
    }

    mod hover_render {
        use karet_core::Markup;
        use karet_core::MarkupKind;
        use karet_core::StandardToken;
        use karet_theme::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::Modifier;
        use ratatui::widgets::Widget;

        use crate::hover::HoverPopup;

        fn render(markup: &Markup, theme: &Theme, width: u16, height: u16) -> Buffer {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            HoverPopup::new(markup, theme).render(area, &mut buf);
            buf
        }

        fn markdown(value: &str) -> Markup {
            Markup {
                kind: MarkupKind::Markdown,
                value: value.to_owned(),
            }
        }

        #[test]
        fn markdown_heading_renders_bold_in_the_heading_color() {
            let theme = Theme::dark();
            let buf = render(&markdown("# Title"), &theme, 20, 2);
            // "# Title" — the '#' marker is the first cell.
            let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
            assert_eq!(cell.symbol(), "#");
            assert!(cell.modifier.contains(Modifier::BOLD));
            assert_eq!(
                cell.fg,
                theme.color(StandardToken::MarkupHeading.id()).to_ratatui()
            );
        }

        #[test]
        fn markdown_code_fence_renders_as_code() {
            let theme = Theme::dark();
            let buf = render(&markdown("```rust\nfn f() {}\n```"), &theme, 20, 3);
            // The fence delimiters are stripped; its body is what paints.
            let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
            assert_eq!(cell.symbol(), "f");
            // This crate compiles in no grammars, so the fence paints as raw markup. The
            // app enables `all-languages`, and karet-markdown's own tests cover the
            // highlighted path against a real grammar.
            let keyword = theme.color(karet_core::TokenId::KEYWORD).to_ratatui();
            let raw = theme.color(StandardToken::MarkupRaw.id()).to_ratatui();
            assert!(
                cell.fg == keyword || cell.fg == raw,
                "expected keyword or raw markup, got {:?}",
                cell.fg
            );
            assert_ne!(
                cell.fg,
                theme.role(karet_core::ThemeRole::Foreground).to_ratatui()
            );
        }

        #[test]
        fn plain_text_markup_renders_verbatim_and_unstyled() {
            let theme = Theme::dark();
            let markup = Markup {
                kind: MarkupKind::PlainText,
                value: "# not a heading".to_owned(),
            };
            let buf = render(&markup, &theme, 20, 1);
            let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
            assert_eq!(cell.symbol(), "#");
            assert!(cell.modifier.is_empty(), "plain text carries no emphasis");
        }

        #[test]
        fn a_zero_sized_area_paints_nothing() {
            let theme = Theme::dark();
            let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
            HoverPopup::new(&markdown("# T"), &theme).render(Rect::new(0, 0, 0, 0), &mut buf);
            assert_eq!(
                buf.cell((0, 0)).map(|c| c.symbol().to_owned()),
                Some(" ".to_owned())
            );
        }
    }
}

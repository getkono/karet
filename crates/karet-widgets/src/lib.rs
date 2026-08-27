//! `karet-widgets` — a reusable ratatui widget toolkit for building editors.
//!
//! A lightweight (ratatui-only) crate of the UI widgets an editor needs. Widgets
//! render data fed in by the application — they consume `karet-core` models, and
//! so do **not** depend on the producers (`karet-lsp`/`karet-vcs`). This crate
//! also hosts the LSP [`completion`] popup, and (behind the `hover` feature) the
//! LSP hover/doc popup, which render `karet-core` models supplied over the
//! backend's event stream. The [`menu`] and [`dialog`] widgets share one row
//! model ([`choice`]), so a context menu and a modal confirmation navigate
//! identically. [`rowselect`] is the shared pointer-selection model for the
//! read-only surfaces that paint their own rows (diffs, hex dumps, previews) and
//! so have no document to select over. The [`transcript`] widget is the live-conversation surface: an
//! append-only, wrapping, stick-to-bottom view with a post-wrap scroll extent.
//! The read-only file-view primitives (hex dump,
//! terminal image, placeholder) live in `karet-fileview`.

pub mod ansi;
pub mod breadcrumbs;
pub mod choice;
pub mod columns;
pub mod completion;
pub mod dialog;
pub mod file_tree;
pub mod glyph;
pub mod menu;
pub mod notify;
pub mod pane;
pub mod picker;
pub mod rowselect;
pub mod scroll;
pub mod select;
pub mod spinner;
pub mod status;
pub mod text;
pub mod textarea;
pub mod textfield;
pub mod transcript;

pub use choice::Choice;
pub use choice::ChoiceList;
pub use columns::Column;
pub use columns::ColumnRow;
pub use columns::ColumnStyle;
pub use columns::Columns;
pub use columns::RowEmphasis;
pub use completion::CompletionPopup;
pub use completion::CompletionState;
pub use dialog::Dialog;
pub use dialog::DialogBody;
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
pub use pane::PaneDivider;
pub use pane::PaneId;
pub use pane::PaneLayout;
pub use pane::SplitAxis;
pub use pane::SplitDir;
pub use pane::drop_preview_rect;
pub use pane::drop_zone;
pub use rowselect::RowGeometry;
pub use rowselect::RowPos;
pub use rowselect::RowSelection;
pub use select::ListSelection;
pub use spinner::Spinner;
pub use transcript::Transcript;
pub use transcript::TranscriptBody;
pub use transcript::TranscriptMessage;

/// The LSP hover / documentation popup (relocated here from `karet-lsp`).
#[cfg(feature = "hover")]
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
                // An unrecognized kind (a newer peer) degrades to plain text.
                _ => self
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

#[cfg(all(test, feature = "hover"))]
mod tests {
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

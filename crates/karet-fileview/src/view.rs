//! [`FileView`] — the cheap per-frame renderer that dispatches a [`FileDoc`] to the
//! right primitive, plus its [`FileViewState`] (scroll + reserved-image tracking)
//! and the [`flush_kitty_image`] post-draw hook.

use std::io;
use std::io::Write;

use karet_core::Decoration;
use karet_core::ThemeRole;
use karet_editor::Editor;
use karet_editor::EditorState;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::StatefulWidget;
use ratatui::widgets::Widget;

use crate::doc::Content;
use crate::doc::FileDoc;
use crate::hex::HexView;
use crate::image;
use crate::image::GraphicsProtocol;
use crate::image::ImageWidget;
use crate::viewer::Placeholder;

/// The bytes shown per hex row (matches [`HexView`]).
const HEX_ROW_WIDTH: usize = 16;

/// The persistent per-view state for a [`FileView`]: scroll position and, for the
/// Kitty image path, the rect reserved this frame (see [`flush_kitty_image`]).
///
/// The scroll helpers drive both the text and hex branches at once; only the
/// active branch's scroll is read when rendering, so a consumer can call them
/// without knowing the document's kind.
#[derive(Clone, Debug, Default)]
pub struct FileViewState {
    /// Scroll state for the read-only text branch.
    editor: EditorState,
    /// First visible 16-byte row for the hex branch.
    hex_scroll: usize,
    /// Viewport height captured at the last render, for page scrolling.
    page: u16,
    /// The rect reserved for a Kitty image this frame, consumed by
    /// [`flush_kitty_image`]. `None` for every other branch/protocol.
    pending_image: Option<Rect>,
}

impl FileViewState {
    /// A fresh state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scroll down by `lines` (text lines, or hex rows). Clamped to the document
    /// when rendered.
    pub fn scroll_down(&mut self, lines: u32) {
        self.editor.scroll_line = self.editor.scroll_line.saturating_add(lines);
        self.hex_scroll = self.hex_scroll.saturating_add(lines as usize);
    }

    /// Scroll up by `lines` (text lines, or hex rows).
    pub fn scroll_up(&mut self, lines: u32) {
        self.editor.scroll_line = self.editor.scroll_line.saturating_sub(lines);
        self.hex_scroll = self.hex_scroll.saturating_sub(lines as usize);
    }

    /// Scroll down one viewport page.
    pub fn page_down(&mut self) {
        self.scroll_down(u32::from(self.page.max(1)));
    }

    /// Scroll up one viewport page.
    pub fn page_up(&mut self) {
        self.scroll_up(u32::from(self.page.max(1)));
    }

    /// Jump to the top of the document.
    pub fn scroll_to_top(&mut self) {
        self.editor.scroll_line = 0;
        self.hex_scroll = 0;
    }

    /// Center the viewport on text `line` (e.g. a search match). Applies to the
    /// text branch; ignored by the others. Clamped to the document when rendered.
    pub fn center_on(&mut self, line: u32) {
        let half = u32::from(self.page.max(1)) / 2;
        let scroll = line.saturating_sub(half);
        self.editor.scroll_line = scroll;
        self.hex_scroll = scroll as usize;
    }
}

/// A read-only widget that renders any [`FileDoc`] — highlighted text, an image, a
/// hex dump, or a placeholder — dispatching on the document's kind.
///
/// Render it as a ratatui [`StatefulWidget`] with a [`FileViewState`]. Search
/// matches (or any overlay) are supplied as [`Decoration`]s and painted on the
/// text branch. For the Kitty image path, call [`flush_kitty_image`] once after
/// `terminal.draw(...)`; the truecolor halfblock path (the default) is fully
/// self-contained and needs no flush.
pub struct FileView<'a> {
    doc: &'a FileDoc,
    theme: Option<&'a Theme>,
    protocol: GraphicsProtocol,
    decorations: &'a [Decoration],
}

impl<'a> FileView<'a> {
    /// Start building a view over `doc`.
    #[must_use]
    pub fn new(doc: &'a FileDoc) -> Self {
        Self {
            doc,
            theme: None,
            protocol: GraphicsProtocol::Halfblocks,
            decorations: &[],
        }
    }

    /// Supply the active theme (used by the text and hex branches, and the Kitty
    /// reserve background).
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Select the terminal graphics protocol for images. Defaults to
    /// [`GraphicsProtocol::Halfblocks`], which renders inline with no flush; use
    /// [`GraphicsProtocol::Kitty`] (detected via
    /// [`detect_protocol`](crate::image::detect_protocol)) plus
    /// [`flush_kitty_image`] for pixel-perfect images.
    #[must_use]
    pub fn graphics(mut self, protocol: GraphicsProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Supply decorations painted on the text branch — e.g. search matches as
    /// [`DecorationKind::TextBackground`](karet_core::DecorationKind::TextBackground)
    /// / `LineBackground`.
    #[must_use]
    pub fn decorations(mut self, decorations: &'a [Decoration]) -> Self {
        self.decorations = decorations;
        self
    }

    /// Resolve the theme, falling back to the default dark theme.
    fn resolved_theme(&self) -> Theme {
        self.theme.cloned().unwrap_or_else(Theme::dark)
    }
}

impl StatefulWidget for FileView<'_> {
    type State = FileViewState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut FileViewState) {
        state.page = area.height;
        state.pending_image = None;
        let theme = self.resolved_theme();

        match &self.doc.content {
            Content::Text {
                buffer, highlights, ..
            } => {
                Editor::new(buffer)
                    .theme(&theme)
                    .highlights(highlights)
                    .decorations(self.decorations)
                    .read_only(true)
                    .render(area, buf, &mut state.editor);
            },
            Content::Image(img) => match self.protocol {
                GraphicsProtocol::Kitty => {
                    // Reserve the area: paint the background and record the rect so
                    // `flush_kitty_image` can transmit pixels after the frame.
                    buf.set_style(
                        area,
                        Style::default().bg(theme.role(ThemeRole::Background).to_ratatui()),
                    );
                    state.pending_image = Some(area);
                },
                GraphicsProtocol::Halfblocks => ImageWidget::new(img).render(area, buf),
            },
            Content::Binary(bytes) => {
                let rows = bytes.len().div_ceil(HEX_ROW_WIDTH);
                state.hex_scroll = state.hex_scroll.min(rows.saturating_sub(1));
                HexView::new(bytes)
                    .scroll(state.hex_scroll)
                    .theme(&theme)
                    .render(area, buf);
            },
            Content::Placeholder => {
                Placeholder::new(&self.doc.path, self.doc.kind, self.doc.dims, self.doc.len)
                    .render(area, buf);
            },
        }
    }
}

/// Transmit the Kitty image reserved by the last [`FileView`] render to `out`.
///
/// Call once per frame, **after** `terminal.draw(...)`, when rendering with
/// [`GraphicsProtocol::Kitty`]. It is a no-op unless the last frame reserved an
/// image (halfblock rendering and the non-image branches never do). The image is
/// re-transmitted on every call, so a consumer that redraws continuously may guard
/// it to only fire when the frame actually changed.
///
/// # Errors
/// Propagates any write/flush error from `out`.
pub fn flush_kitty_image(
    doc: &FileDoc,
    state: &FileViewState,
    out: &mut impl Write,
) -> io::Result<()> {
    let (Some(rect), Content::Image(img)) = (state.pending_image, &doc.content) else {
        return Ok(());
    };
    write!(out, "{}", image::kitty_delete_all())?;
    // Position the cursor at the reserved rect's top-left (VT coordinates are 1-based).
    write!(out, "\x1b[{};{}H", rect.y + 1, rect.x + 1)?;
    write!(out, "{}", img.kitty_escape(rect.width, rect.height))?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_theme::Theme;

    use super::*;
    use crate::doc::FileDoc;
    use crate::doc::Limits;

    fn render(doc: &FileDoc, area: Rect, state: &mut FileViewState) -> Buffer {
        let theme = Theme::dark();
        let mut buf = Buffer::empty(area);
        FileView::new(doc)
            .theme(&theme)
            .render(area, &mut buf, state);
        buf
    }

    #[test]
    fn text_branch_paints_gutter_and_no_caret() {
        let doc = FileDoc::prepare(Path::new("a.rs"), b"fn main() {}\n", 13, &Limits::default());
        let area = Rect::new(0, 0, 30, 4);
        let mut state = FileViewState::new();
        let buf = render(&doc, area, &mut state);
        let row0: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row0.contains('1'), "gutter line number missing: {row0:?}");
        // Read-only: no reversed caret cell anywhere.
        let any_caret = (0..area.width).any(|x| {
            (0..area.height).any(|y| {
                buf[(x, y)]
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
        });
        assert!(!any_caret, "read-only text branch must not draw a caret");
    }

    #[test]
    fn hex_branch_renders_offsets() {
        let doc = FileDoc::prepare(
            Path::new("x.bin"),
            &[0u8, 1, 2, 3, 4],
            5,
            &Limits::default(),
        );
        let area = Rect::new(0, 0, 80, 2);
        let mut state = FileViewState::new();
        let buf = render(&doc, area, &mut state);
        let text: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(text.contains("00000000"), "hex offset missing");
    }

    #[test]
    fn placeholder_branch_shows_title() {
        let doc = FileDoc::prepare(Path::new("doc.pdf"), b"%PDF-1.7", 8, &Limits::default());
        let area = Rect::new(0, 0, 40, 8);
        let mut state = FileViewState::new();
        let buf = render(&doc, area, &mut state);
        let text: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(text.contains("doc.pdf"), "placeholder title missing");
    }

    #[test]
    fn kitty_reserves_and_flushes() {
        let mut png = Vec::new();
        let img = ::image::RgbaImage::from_pixel(2, 2, ::image::Rgba([1, 2, 3, 255]));
        let _ = ::image::DynamicImage::ImageRgba8(img).write_to(
            &mut std::io::Cursor::new(&mut png),
            ::image::ImageFormat::Png,
        );
        let doc = FileDoc::prepare(
            Path::new("x.png"),
            &png,
            png.len() as u64,
            &Limits::default(),
        );
        let area = Rect::new(0, 0, 10, 6);
        let mut state = FileViewState::new();
        let mut buf = Buffer::empty(area);
        FileView::new(&doc)
            .graphics(GraphicsProtocol::Kitty)
            .render(area, &mut buf, &mut state);
        // Kitty mode reserves the rect rather than painting pixels into the buffer.
        let mut out = Vec::new();
        let _ = flush_kitty_image(&doc, &state, &mut out); // writing to a Vec is infallible
        let escape = String::from_utf8_lossy(&out);
        assert!(
            escape.contains("\x1b_G"),
            "expected a Kitty graphics escape"
        );

        // Halfblocks mode is self-contained (nothing to flush).
        let mut hb_state = FileViewState::new();
        let hb = render(&doc, area, &mut hb_state);
        assert!(
            hb.content().iter().any(|c| c.symbol() == "▀"),
            "halfblock cells expected"
        );
        let mut none = Vec::new();
        let _ = flush_kitty_image(&doc, &hb_state, &mut none); // infallible; expect no bytes
        assert!(none.is_empty(), "halfblock path must not flush escapes");
    }

    #[test]
    fn scroll_helpers_move_the_viewport() {
        let mut state = FileViewState::new();
        state.page = 10;
        state.page_down();
        state.scroll_down(3);
        assert_eq!(state.editor.scroll_line, 13);
        assert_eq!(state.hex_scroll, 13);
        state.center_on(50);
        assert_eq!(state.editor.scroll_line, 45);
        state.scroll_to_top();
        assert_eq!(state.editor.scroll_line, 0);
        assert_eq!(state.hex_scroll, 0);
    }
}

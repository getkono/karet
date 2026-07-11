//! A graceful placeholder for files the code window cannot render inline.
//!
//! File-type classification itself lives in [`karet_filetype`]; this module
//! re-exports [`FileKind`]/[`classify`]/[`SIZE_GUARD`] for callers and supplies the
//! ratatui [`Placeholder`] widget. The application opens a file by
//! [`classify`]ing it and then choosing a renderer: [`FileKind::Text`] /
//! [`FileKind::Markdown`] → the editor widget, [`FileKind::Image`] → the image
//! widget, [`FileKind::Binary`] → the hex view, and [`FileKind::Pdf`] /
//! [`FileKind::TooLarge`] (or an image that fails to decode) → a [`Placeholder`].

use std::path::Path;

pub use karet_filetype::FileKind;
pub use karet_filetype::SIZE_GUARD;
pub use karet_filetype::classify;
pub use karet_filetype::classify_ignoring_size;
use ratatui::buffer::Buffer;
use ratatui::layout::Alignment;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

/// A centered placeholder describing a file that is not rendered inline.
#[derive(Clone, Debug)]
pub struct Placeholder {
    title: String,
    lines: Vec<String>,
    hint: Option<String>,
}

impl Placeholder {
    /// Build a placeholder for `path` of `kind`, optionally annotated with image
    /// `dimensions` and the file `len`.
    #[must_use]
    pub fn new(path: &Path, kind: FileKind, dimensions: Option<(u32, u32)>, len: u64) -> Self {
        let title = file_name(path);
        let mut lines = vec![describe(kind).to_string()];
        if let Some((w, h)) = dimensions {
            lines.push(format!("{w} × {h}"));
        }
        lines.push(human_size(len));
        Self {
            title,
            lines,
            hint: None,
        }
    }

    /// A placeholder telling the user this file renders as an image and therefore
    /// needs a terminal that speaks the Kitty graphics protocol. Shown for document
    /// formats (e.g. PDF) when no graphics protocol was detected — so the message
    /// attributes the limitation to the terminal, not to a missing feature.
    #[must_use]
    pub fn requires_kitty(path: &Path) -> Self {
        Self {
            title: file_name(path),
            lines: vec![
                "This document renders as an image,".to_string(),
                "which needs a terminal with the Kitty graphics protocol".to_string(),
                "(kitty, ghostty, WezTerm, …).".to_string(),
            ],
            hint: None,
        }
    }

    /// Add an action-hint line shown below the description (e.g. an "open anyway"
    /// override). The caller supplies the full text — including any key chord — so
    /// the widget stays agnostic of the consumer's keybindings.
    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// The file name of `path` for a placeholder title, or the full path if it has no
/// final component.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or_else(|| path.display().to_string(), str::to_string)
}

impl Widget for Placeholder {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(self.title));
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let mut lines = self.lines;
        if let Some(hint) = self.hint {
            // A blank spacer sets the hint apart from the description above it.
            lines.push(String::new());
            lines.push(hint);
        }
        let height = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .min(inner.height);
        let text = Text::from(lines.into_iter().map(Line::from).collect::<Vec<_>>());
        let y = inner.y + inner.height.saturating_sub(height) / 2;
        let area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height,
        };
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}

/// A short human-readable description of `kind` for a [`Placeholder`].
fn describe(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Image => "Image preview unavailable",
        // Shown only when PDF rendering is not compiled in (the `pdf` feature); when
        // it is, a PDF either renders or shows the requires-Kitty placeholder.
        FileKind::Pdf => "PDF document",
        // Shown when DOCX conversion is not compiled in (the `docx` feature), or —
        // with it on — when the bytes fail to parse as a Word document.
        #[cfg(feature = "docx")]
        FileKind::Docx => "Word document",
        #[cfg(not(feature = "docx"))]
        FileKind::Docx => "DOCX rendering is not available yet",
        FileKind::Binary => "Binary file",
        FileKind::TooLarge { .. } => "File too large to open",
        FileKind::Text | FileKind::Markdown => "Text file",
        _ => "File",
    }
}

/// Format `bytes` as a human-readable size (e.g. `12.3 KiB`).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_renders_title_and_size() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        Placeholder::new(Path::new("doc.pdf"), FileKind::Pdf, None, 2048).render(area, &mut buf);
        let rendered: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("doc.pdf"));
        assert!(rendered.contains("2.0 KiB"));
    }

    #[test]
    fn placeholder_renders_an_action_hint() {
        let area = Rect::new(0, 0, 40, 8);
        let mut buf = Buffer::empty(area);
        let len = SIZE_GUARD + 1;
        Placeholder::new(Path::new("big.cbor"), FileKind::TooLarge { len }, None, len)
            .hint("Press Enter to open anyway")
            .render(area, &mut buf);
        let rendered: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("File too large to open"));
        assert!(rendered.contains("open anyway"));
    }

    #[test]
    fn requires_kitty_names_the_protocol_not_a_missing_feature() {
        let area = Rect::new(0, 0, 60, 8);
        let mut buf = Buffer::empty(area);
        Placeholder::requires_kitty(Path::new("report.pdf")).render(area, &mut buf);
        let rendered: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("report.pdf"));
        assert!(rendered.contains("Kitty graphics protocol"));
    }

    #[cfg(feature = "docx")]
    #[test]
    fn describe_docx_names_the_format() {
        // With conversion compiled in, the placeholder (shown only for an
        // unparseable file) describes the format rather than a missing feature.
        assert_eq!(describe(FileKind::Docx), "Word document");
    }

    #[cfg(not(feature = "docx"))]
    #[test]
    fn describe_docx_is_pending_not_unsupported() {
        assert_eq!(
            describe(FileKind::Docx),
            "DOCX rendering is not available yet"
        );
    }

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    }
}

//! File-type classification and a graceful placeholder for the code window.
//!
//! The application opens a file by classifying it ([`classify`]) and then choosing
//! a renderer: [`FileKind::Text`]/[`FileKind::Markdown`] → the editor widget,
//! [`FileKind::Image`] → the image widget, [`FileKind::Binary`] → the hex view,
//! and [`FileKind::Pdf`]/[`FileKind::TooLarge`] (or an image that fails to decode)
//! → a [`Placeholder`] describing why the file is not rendered inline.

use std::path::Path;

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

/// Files larger than this are classified [`FileKind::TooLarge`] (10 MiB).
pub const SIZE_GUARD: u64 = 10 * 1024 * 1024;

/// The renderer a file should be opened with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// UTF-8 text — code or plain prose (the application resolves the grammar).
    Text,
    /// Markdown source.
    Markdown,
    /// A raster image (png/jpg/gif/webp/bmp/…).
    Image,
    /// A PDF document.
    Pdf,
    /// Binary content (shown as a hex dump).
    Binary,
    /// A file too large to load inline.
    TooLarge {
        /// The file length in bytes.
        len: u64,
    },
}

/// Classify a file from its `path`, a `head` sample of its leading bytes (e.g. the
/// first 8 KiB), and its total `len` in bytes.
///
/// This deliberately does **not** distinguish code from plain text — both are
/// [`FileKind::Text`]; the application resolves a grammar from the path. Image and
/// PDF kinds are detected by extension and confirmed (or recovered) by magic
/// bytes, so a mislabeled file still routes sensibly.
#[must_use]
pub fn classify(path: &Path, head: &[u8], len: u64) -> FileKind {
    if len > SIZE_GUARD {
        return FileKind::TooLarge { len };
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if let Some(ext) = ext.as_deref() {
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" => {
                return FileKind::Image;
            }
            "pdf" => return FileKind::Pdf,
            "md" | "markdown" | "mdown" | "mkd" => return FileKind::Markdown,
            _ => {}
        }
    }
    if is_pdf(head) {
        return FileKind::Pdf;
    }
    if is_image(head) {
        return FileKind::Image;
    }
    if looks_binary(head) {
        return FileKind::Binary;
    }
    FileKind::Text
}

/// Whether `head` begins with a PDF signature.
fn is_pdf(head: &[u8]) -> bool {
    head.starts_with(b"%PDF-")
}

/// Whether `head` begins with a known raster-image signature.
fn is_image(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")            // PNG
        || head.starts_with(&[0xFF, 0xD8, 0xFF])     // JPEG
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || head.starts_with(b"BM")                    // BMP
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
}

/// Whether `head` looks like binary content: a NUL byte, or an invalid UTF-8 byte
/// that is not merely a multi-byte sequence truncated at the sample boundary.
fn looks_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    match std::str::from_utf8(head) {
        Ok(_) => false,
        // `error_len() == None` means the sample ended mid-character (truncation),
        // which is fine; `Some(_)` is a genuinely invalid byte → binary.
        Err(e) => e.error_len().is_some(),
    }
}

/// A centered placeholder describing a file that is not rendered inline.
#[derive(Clone, Debug)]
pub struct Placeholder {
    title: String,
    lines: Vec<String>,
}

impl Placeholder {
    /// Build a placeholder for `path` of `kind`, optionally annotated with image
    /// `dimensions` and the file `len`.
    #[must_use]
    pub fn new(path: &Path, kind: FileKind, dimensions: Option<(u32, u32)>, len: u64) -> Self {
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map_or_else(|| path.display().to_string(), str::to_string);
        let mut lines = vec![describe(kind).to_string()];
        if let Some((w, h)) = dimensions {
            lines.push(format!("{w} × {h}"));
        }
        lines.push(human_size(len));
        Self { title, lines }
    }
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
        let height = u16::try_from(self.lines.len())
            .unwrap_or(u16::MAX)
            .min(inner.height);
        let text = Text::from(self.lines.into_iter().map(Line::from).collect::<Vec<_>>());
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
        FileKind::Pdf => "PDF preview is not supported yet",
        FileKind::Binary => "Binary file",
        FileKind::TooLarge { .. } => "File too large to open",
        FileKind::Text | FileKind::Markdown => "Text file",
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
    fn classifies_by_extension() {
        assert_eq!(classify(Path::new("a.png"), b"", 10), FileKind::Image);
        assert_eq!(classify(Path::new("a.pdf"), b"", 10), FileKind::Pdf);
        assert_eq!(classify(Path::new("a.md"), b"# hi", 4), FileKind::Markdown);
        assert_eq!(
            classify(Path::new("a.rs"), b"fn main(){}", 11),
            FileKind::Text
        );
    }

    #[test]
    fn classifies_by_magic_bytes() {
        // A PDF mislabeled as .bin is still recognized.
        assert_eq!(classify(Path::new("x.bin"), b"%PDF-1.7", 8), FileKind::Pdf);
        let png = b"\x89PNG\r\n\x1a\n....";
        assert_eq!(classify(Path::new("x.bin"), png, 12), FileKind::Image);
    }

    #[test]
    fn nul_byte_is_binary_utf8_is_text() {
        assert_eq!(classify(Path::new("x"), b"a\x00b", 3), FileKind::Binary);
        assert_eq!(
            classify(Path::new("x"), "héllo".as_bytes(), 6),
            FileKind::Text
        );
    }

    #[test]
    fn truncated_utf8_head_is_not_binary() {
        // "é" is two bytes; a head cut after the first byte must not read as binary.
        let full = "café".as_bytes();
        let head = &full[..full.len() - 1];
        assert_eq!(
            classify(Path::new("x.txt"), head, full.len() as u64),
            FileKind::Text
        );
    }

    #[test]
    fn oversize_is_too_large() {
        let len = SIZE_GUARD + 1;
        assert_eq!(
            classify(Path::new("big.rs"), b"fn", len),
            FileKind::TooLarge { len }
        );
    }

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
    fn human_size_scales_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    }
}

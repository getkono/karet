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

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub use karet_filetype::{FileKind, SIZE_GUARD, classify};

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
    fn human_size_scales_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1024 * 1024), "1.0 MiB");
    }
}

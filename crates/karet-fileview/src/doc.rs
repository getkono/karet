//! [`FileDoc`] — a file prepared once for read-only rendering.
//!
//! [`FileDoc::prepare`] runs the expensive step: classify the bytes, then decode /
//! parse / highlight into an owned payload a [`FileView`](crate::FileView) renders
//! cheaply each frame. Consumers cache the [`FileDoc`] and re-render it as the
//! viewport scrolls.

use std::path::Path;
use std::path::PathBuf;

use karet_filetype::FileKind;
use karet_filetype::classify_with_guard;
use karet_syntax::Highlighter;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;

use crate::image;

/// How many leading bytes to sample for file-type classification.
const HEAD_BYTES: usize = 8192;

/// Size and highlighting budgets for [`FileDoc::prepare`].
///
/// The two budgets are independent knobs a consumer tunes per context. An inline
/// preview might use a small `max_bytes` (e.g. 256 KiB) and a low
/// `highlight_line_budget` (e.g. 500) for instant open; a full-file reader a
/// larger `max_bytes` (e.g. 4 MiB) with a higher line budget (e.g. 20 000), above
/// which text is rendered un-highlighted so even huge files open instantly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Files whose length exceeds this are [`FileKind::TooLarge`] (a placeholder).
    pub max_bytes: u64,
    /// Text with more lines than this is rendered without syntax highlighting so
    /// it opens instantly; the buffer itself is unaffected.
    pub highlight_line_budget: usize,
}

impl Default for Limits {
    /// A general-purpose default: [`karet_filetype::SIZE_GUARD`] (10 MiB) and a
    /// 20 000-line highlight budget.
    fn default() -> Self {
        Self {
            max_bytes: karet_filetype::SIZE_GUARD,
            highlight_line_budget: 20_000,
        }
    }
}

impl Limits {
    /// Construct limits from an explicit byte ceiling and highlight line budget.
    #[must_use]
    pub fn new(max_bytes: u64, highlight_line_budget: usize) -> Self {
        Self {
            max_bytes,
            highlight_line_budget,
        }
    }
}

/// The prepared, owned payload for each renderable branch.
pub(crate) enum Content {
    /// Text/Markdown: a read-only buffer and (possibly empty) highlight spans.
    Text {
        buffer: TextBuffer,
        highlights: Highlights,
        language: &'static str,
    },
    /// A decoded raster image.
    Image(image::Image),
    /// Raw bytes shown as a hex dump.
    Binary(Vec<u8>),
    /// Nothing to render inline (PDF, too-large, or an undecodable image).
    Placeholder,
}

/// A file classified and prepared for read-only rendering.
///
/// Build one with [`prepare`](Self::prepare) (the expensive step, run once) and
/// render it with [`FileView`](crate::FileView). The [`FileDoc`] owns everything a
/// frame needs — the decoded image, the parsed buffer and its highlights, or the
/// raw bytes — so per-frame rendering allocates almost nothing.
pub struct FileDoc {
    pub(crate) kind: FileKind,
    pub(crate) content: Content,
    pub(crate) path: PathBuf,
    pub(crate) dims: Option<(u32, u32)>,
    pub(crate) len: u64,
}

impl FileDoc {
    /// Prepare `path`'s content for rendering: classify the bytes against
    /// `limits`, then decode an image / parse+highlight text / keep raw bytes for a
    /// hex dump / fall back to a placeholder.
    ///
    /// This is the **expensive** step; run it once when the file is opened and keep
    /// the result. `bytes` should be the file's full content, with `len` its true
    /// size — except when `len > limits.max_bytes`, where the file is classified
    /// [`FileKind::TooLarge`] without inspecting `bytes` beyond a leading sample, so
    /// a consumer may pass only a head slice for very large files.
    ///
    /// Highlighting requires the relevant tree-sitter grammar to be compiled in
    /// (enable the `all-languages` feature, or a per-language feature); without it
    /// text still renders, just unhighlighted.
    #[must_use]
    pub fn prepare(path: &Path, bytes: &[u8], len: u64, limits: &Limits) -> Self {
        let head = &bytes[..bytes.len().min(HEAD_BYTES)];
        let kind = classify_with_guard(path, head, len, limits.max_bytes);
        let content = match kind {
            FileKind::Text | FileKind::Markdown => prepare_text(path, bytes, limits),
            FileKind::Image => match image::decode(bytes) {
                Ok(img) => Content::Image(img),
                Err(_) => Content::Placeholder,
            },
            FileKind::Binary => Content::Binary(bytes.to_vec()),
            // Pdf, TooLarge, and any future `#[non_exhaustive]` kind → placeholder.
            _ => Content::Placeholder,
        };
        // Annotate an undecodable-image placeholder with the pixel dimensions.
        let dims = if matches!(kind, FileKind::Image) && matches!(content, Content::Placeholder) {
            image::dimensions(bytes)
        } else {
            None
        };
        Self {
            kind,
            content,
            path: path.to_path_buf(),
            dims,
            len,
        }
    }

    /// The renderer branch this file classified into.
    #[must_use]
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// The display language name for a text file (e.g. `"Rust"`), or `None` for a
    /// non-text branch.
    #[must_use]
    pub fn language(&self) -> Option<&'static str> {
        match &self.content {
            Content::Text { language, .. } => Some(language),
            _ => None,
        }
    }

    /// The number of scrollable units — text lines or hex rows — or `0` for the
    /// image / placeholder branches. Useful for sizing a scrollbar.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match &self.content {
            Content::Text { buffer, .. } => buffer.line_count(),
            Content::Binary(bytes) => bytes.len().div_ceil(16),
            Content::Image(_) | Content::Placeholder => 0,
        }
    }
}

/// Build the text branch: a read-only buffer plus highlights, skipping the
/// (expensive) highlight pass when the file exceeds the line budget.
fn prepare_text(path: &Path, bytes: &[u8], limits: &Limits) -> Content {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let language = language_name_from_path(path).unwrap_or("plaintext");
    let buffer = TextBuffer::from_text(&text);
    let highlights = if buffer.line_count() <= limits.highlight_line_budget {
        highlight(path, &text)
    } else {
        Highlights::default()
    };
    Content::Text {
        buffer,
        highlights,
        language,
    }
}

/// Parse and highlight `text` for `path`'s language, or empty highlights when no
/// grammar is available or parsing fails.
fn highlight(path: &Path, text: &str) -> Highlights {
    let Some(lang) = language_id_from_path(path) else {
        return Highlights::default();
    };
    let mut pool = ParserPool::new();
    let Ok(tree) = SyntaxTree::parse(&mut pool, lang, text) else {
        return Highlights::default();
    };
    let Ok(highlighter) = Highlighter::new(lang) else {
        return Highlights::default();
    };
    highlighter.highlight(&tree, text).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 PNG built in-memory (no fixtures on disk).
    fn tiny_png() -> Vec<u8> {
        let mut img = ::image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, ::image::Rgba([255, 0, 0, 255]));
        let mut bytes = Vec::new();
        let _ = ::image::DynamicImage::ImageRgba8(img).write_to(
            &mut std::io::Cursor::new(&mut bytes),
            ::image::ImageFormat::Png,
        );
        bytes
    }

    #[test]
    fn prepares_text_with_highlights() {
        let doc = FileDoc::prepare(Path::new("a.rs"), b"fn main() {}\n", 13, &Limits::default());
        assert_eq!(doc.kind(), FileKind::Text);
        assert_eq!(doc.row_count(), 2);
        assert!(
            matches!(&doc.content, Content::Text { .. }),
            "expected Text content"
        );
        if let Content::Text { highlights, .. } = &doc.content {
            // The rust grammar (all-languages dev-dep) yields highlight spans.
            assert!(
                !highlights.all().is_empty(),
                "expected highlight spans for a rust file"
            );
        }
    }

    #[test]
    fn line_budget_disables_highlighting() {
        let src = "fn a() {}\n".repeat(10);
        let limits = Limits::new(karet_filetype::SIZE_GUARD, 3);
        let doc = FileDoc::prepare(Path::new("a.rs"), src.as_bytes(), src.len() as u64, &limits);
        assert!(
            matches!(&doc.content, Content::Text { .. }),
            "expected Text content"
        );
        if let Content::Text { highlights, .. } = &doc.content {
            assert!(
                highlights.all().is_empty(),
                "over-budget text must render unhighlighted"
            );
        }
    }

    #[test]
    fn binary_becomes_hex() {
        let doc = FileDoc::prepare(Path::new("x.bin"), &[0u8, 1, 2, 3], 4, &Limits::default());
        assert_eq!(doc.kind(), FileKind::Binary);
        assert!(matches!(doc.content, Content::Binary(_)));
        assert_eq!(doc.row_count(), 1);
    }

    #[test]
    fn image_decodes() {
        let png = tiny_png();
        let doc = FileDoc::prepare(
            Path::new("x.png"),
            &png,
            png.len() as u64,
            &Limits::default(),
        );
        assert_eq!(doc.kind(), FileKind::Image);
        assert!(matches!(doc.content, Content::Image(_)));
    }

    #[test]
    fn undecodable_image_falls_back_with_dims() {
        // A .png extension over non-image bytes classifies Image, fails to decode.
        let doc = FileDoc::prepare(Path::new("x.png"), b"not a png", 9, &Limits::default());
        assert_eq!(doc.kind(), FileKind::Image);
        assert!(matches!(doc.content, Content::Placeholder));
    }

    #[test]
    fn oversized_is_placeholder_without_reading_body() {
        let limits = Limits::new(1024, 20_000);
        // Pass only a head sample with a large reported len — must not touch a body.
        let doc = FileDoc::prepare(Path::new("big.rs"), b"fn", 4096, &limits);
        assert_eq!(doc.kind(), FileKind::TooLarge { len: 4096 });
        assert!(matches!(doc.content, Content::Placeholder));
    }
}

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

#[cfg(feature = "images")]
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
    #[cfg(feature = "images")]
    Image(image::Image),
    /// A parsed multi-page document (e.g. PDF) rasterized to images on demand.
    #[cfg(feature = "pdf")]
    Document {
        /// The parsed document; pages are rasterized lazily during rendering.
        doc: karet_pdf::Document,
        /// The total page count, cached so navigation needn't re-query.
        page_count: usize,
    },
    /// Raw bytes shown as a hex dump.
    Binary(Vec<u8>),
    /// Nothing to render inline (too-large, undecodable image, or an unrendered
    /// document format).
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
            #[cfg(feature = "images")]
            FileKind::Image => match image::decode(bytes) {
                Ok(img) => Content::Image(img),
                Err(_) => Content::Placeholder,
            },
            FileKind::Binary => Content::Binary(bytes.to_vec()),
            #[cfg(feature = "pdf")]
            FileKind::Pdf => match karet_pdf::Document::load(bytes.to_vec()) {
                Ok(doc) => {
                    let page_count = doc.page_count();
                    Content::Document { doc, page_count }
                },
                Err(_) => Content::Placeholder,
            },
            // A Word document converts to markdown (via karet-docx) and renders
            // through the text branch, highlighted as markdown whatever the
            // original extension. An unparseable file falls back gracefully.
            #[cfg(feature = "docx")]
            FileKind::Docx => match karet_docx::parse(bytes) {
                Ok(docx) => prepare_text(
                    Path::new("converted.md"),
                    karet_docx::to_markdown(&docx).as_bytes(),
                    limits,
                ),
                Err(_) => Content::Placeholder,
            },
            // TooLarge, DOCX/PDF (without their features), and any future
            // `#[non_exhaustive]` kind → placeholder.
            _ => Content::Placeholder,
        };
        // Annotate an undecodable-image placeholder with the pixel dimensions.
        #[cfg(feature = "images")]
        let dims = if matches!(kind, FileKind::Image) && matches!(content, Content::Placeholder) {
            image::dimensions(bytes)
        } else {
            None
        };
        #[cfg(not(feature = "images"))]
        let dims = None;
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
    /// image / document / placeholder branches. Useful for sizing a scrollbar.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match &self.content {
            Content::Text { buffer, .. } => buffer.line_count(),
            Content::Binary(bytes) => bytes.len().div_ceil(16),
            #[cfg(feature = "pdf")]
            Content::Document { .. } => 0,
            #[cfg(feature = "images")]
            Content::Image(_) => 0,
            Content::Placeholder => 0,
        }
    }

    /// The number of pages for a document branch (e.g. PDF), or `None` for every
    /// other kind. Drives a page indicator and page navigation.
    #[must_use]
    pub fn page_count(&self) -> Option<usize> {
        match &self.content {
            #[cfg(feature = "pdf")]
            Content::Document { page_count, .. } => Some(*page_count),
            _ => None,
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
    #[cfg(feature = "images")]
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

    #[cfg(feature = "images")]
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

    #[cfg(feature = "images")]
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

    /// A minimal single-page PDF (empty US-Letter page), inline so there is no fixture.
    #[cfg(feature = "pdf")]
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
trailer<</Size 4/Root 1 0 R>>\n%%EOF";

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_prepares_to_a_document_with_page_count() {
        let doc = FileDoc::prepare(
            Path::new("a.pdf"),
            MINIMAL_PDF,
            MINIMAL_PDF.len() as u64,
            &Limits::default(),
        );
        assert_eq!(doc.kind(), FileKind::Pdf);
        assert!(matches!(doc.content, Content::Document { .. }));
        assert_eq!(doc.page_count(), Some(1));
    }

    /// A minimal DOCX (one heading + one bold run) zipped in-memory (no fixture).
    #[cfg(feature = "docx")]
    fn tiny_docx() -> Vec<u8> {
        use std::io::Write as _;
        const DOCUMENT_XML: &str = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Report</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut buf = Vec::new();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        // In-memory ZIP writes do not realistically fail; ignore the results
        // rather than `unwrap`/`expect` (denied by the workspace lints).
        let _ = writer.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        );
        let _ = writer.write_all(DOCUMENT_XML.as_bytes());
        let _ = writer.finish();
        buf
    }

    #[cfg(feature = "docx")]
    #[test]
    fn docx_prepares_to_converted_markdown_text() {
        let bytes = tiny_docx();
        let doc = FileDoc::prepare(
            Path::new("report.docx"),
            &bytes,
            bytes.len() as u64,
            &Limits::default(),
        );
        assert_eq!(doc.kind(), FileKind::Docx);
        // The converted markdown renders through the text branch, as Markdown.
        assert_eq!(doc.language(), Some("Markdown"));
        assert!(
            matches!(&doc.content, Content::Text { .. }),
            "expected Text content"
        );
        if let Content::Text { buffer, .. } = &doc.content {
            assert_eq!(buffer.text(), "# Report\n\n**bold**");
        }
    }

    #[cfg(feature = "docx")]
    #[test]
    fn unparseable_docx_falls_back_to_placeholder() {
        // The `.docx` extension classifies Docx, but the bytes are not a ZIP.
        let doc = FileDoc::prepare(Path::new("bad.docx"), b"not a zip", 9, &Limits::default());
        assert_eq!(doc.kind(), FileKind::Docx);
        assert!(matches!(doc.content, Content::Placeholder));
    }

    #[cfg(not(feature = "docx"))]
    #[test]
    fn docx_without_the_feature_is_a_placeholder() {
        let doc = FileDoc::prepare(
            Path::new("report.docx"),
            b"PK\x03\x04",
            4,
            &Limits::default(),
        );
        assert_eq!(doc.kind(), FileKind::Docx);
        assert!(matches!(doc.content, Content::Placeholder));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn undecodable_pdf_falls_back_to_placeholder() {
        // The `.pdf` extension classifies Pdf, but the bytes are not a parseable PDF.
        let doc = FileDoc::prepare(
            Path::new("bad.pdf"),
            b"totally not a pdf",
            17,
            &Limits::default(),
        );
        assert_eq!(doc.kind(), FileKind::Pdf);
        assert!(matches!(doc.content, Content::Placeholder));
    }
}

//! `karet-pdf` — headless, pure-Rust PDF rasterization for karet.
//!
//! It wraps the [`hayro`] PDF interpreter/renderer (pure Rust, no C-sys
//! dependencies) to turn PDF bytes into [`RenderedPage`]s of straight
//! (un-premultiplied) 8-bit RGBA pixels. A renderer such as `karet-fileview` can
//! then hand those pixels to the Kitty graphics protocol (or a halfblock
//! fallback). The crate is headless — no ratatui, no terminal — so a PDF can be
//! turned into pixels anywhere.
//!
//! Parsing happens once in [`Document::load`]; pages are rasterized on demand via
//! [`Document::render_page`], so a large document is not fully rendered up front.
//!
//! ```no_run
//! # fn demo(bytes: Vec<u8>) -> Result<(), karet_pdf::PdfError> {
//! let doc = karet_pdf::Document::load(bytes)?;
//! for i in 0..doc.page_count() {
//!     let page = doc.render_page(i, 2.0)?; // 2× the native 72-DPI size
//!     assert_eq!(
//!         page.rgba().len(),
//!         page.width() as usize * page.height() as usize * 4
//!     );
//! }
//! # Ok(())
//! # }
//! ```

mod error;

pub use error::PdfError;
use hayro::RenderSettings;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;

/// A single rasterized PDF page: straight 8-bit RGBA pixels plus dimensions.
#[derive(Clone, Debug)]
pub struct RenderedPage {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl RenderedPage {
    /// The straight (un-premultiplied) RGBA8 pixels: row-major, 4 bytes per pixel,
    /// exactly `width * height * 4` bytes long.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Consume the page, returning ownership of its RGBA8 pixel buffer.
    #[must_use]
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }

    /// The rendered page width, in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The rendered page height, in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// A parsed PDF document. Load once, then rasterize pages on demand.
pub struct Document {
    pdf: Pdf,
}

impl Document {
    /// Parse a PDF document from its raw bytes.
    ///
    /// # Errors
    /// Returns [`PdfError::Parse`] if the bytes are not a readable PDF, or
    /// [`PdfError::Encrypted`] if the document is password-protected.
    pub fn load(bytes: Vec<u8>) -> Result<Self, PdfError> {
        let pdf = Pdf::new(bytes).map_err(PdfError::from_load)?;
        Ok(Self { pdf })
    }

    /// The number of pages in the document.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Rasterize page `index` (0-based) at `scale` (1.0 renders at the native
    /// 72-DPI size; 2.0 is twice as large) over an opaque white background,
    /// producing straight RGBA8 pixels.
    ///
    /// # Errors
    /// Returns [`PdfError::PageOutOfRange`] if `index >= self.page_count()`.
    pub fn render_page(&self, index: usize, scale: f32) -> Result<RenderedPage, PdfError> {
        let pages = self.pdf.pages();
        let count = pages.len();
        let page = pages
            .get(index)
            .ok_or(PdfError::PageOutOfRange { index, count })?;

        let cache = hayro::RenderCache::new();
        let interpreter = InterpreterSettings::default();
        let render_settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..Default::default()
        };

        let pixmap = hayro::render(page, &cache, &interpreter, &render_settings);
        let width = u32::from(pixmap.width());
        let height = u32::from(pixmap.height());
        let rgba = pixmap
            .take_unpremultiplied()
            .into_iter()
            .flat_map(|px| [px.r, px.g, px.b, px.a])
            .collect();

        Ok(RenderedPage {
            rgba,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, valid single-page PDF (an empty US-Letter page). Kept inline so
    /// the test needs no on-disk fixture.
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
xref\n\
0 4\n\
0000000000 65535 f \n\
0000000009 00000 n \n\
0000000052 00000 n \n\
0000000101 00000 n \n\
trailer<</Size 4/Root 1 0 R>>\n\
startxref\n\
164\n\
%%EOF";

    // The workspace clippy policy denies unwrap/expect/panic even in tests, so these
    // extract values through `Result`/`Option` combinators and assert on those.

    #[test]
    fn loads_and_counts_pages() {
        let count = Document::load(MINIMAL_PDF.to_vec())
            .map(|doc| doc.page_count())
            .ok();
        assert_eq!(count, Some(1));
    }

    #[test]
    fn renders_page_to_rgba_of_expected_size() {
        let page = Document::load(MINIMAL_PDF.to_vec())
            .and_then(|doc| doc.render_page(0, 1.0))
            .ok();
        // 612×792 pt at scale 1.0 → 612×792 px.
        assert_eq!(
            page.as_ref().map(|p| (p.width(), p.height())),
            Some((612, 792))
        );
        // The RGBA buffer is exactly width*height*4 bytes.
        assert!(
            page.as_ref()
                .is_some_and(|p| p.rgba().len() == p.width() as usize * p.height() as usize * 4)
        );
        // The empty page renders as opaque white.
        assert!(page.as_ref().is_some_and(|p| {
            p.rgba()
                .chunks_exact(4)
                .all(|px| px == [255, 255, 255, 255])
        }));
    }

    #[test]
    fn scale_changes_pixel_dimensions() {
        let dims = Document::load(MINIMAL_PDF.to_vec())
            .and_then(|doc| doc.render_page(0, 0.5))
            .ok()
            .map(|p| (p.width(), p.height()));
        assert_eq!(dims, Some((306, 396)));
    }

    #[test]
    fn out_of_range_page_errors() {
        let result = Document::load(MINIMAL_PDF.to_vec()).and_then(|doc| doc.render_page(5, 1.0));
        assert!(matches!(
            result,
            Err(PdfError::PageOutOfRange { index: 5, count: 1 })
        ));
    }

    #[test]
    fn garbage_bytes_fail_to_parse() {
        assert!(matches!(
            Document::load(b"not a pdf".to_vec()),
            Err(PdfError::Parse)
        ));
    }

    /// A 100×100 PDF whose content stream fills a black 80×80 rectangle — so
    /// rendering it actually exercises hayro's content interpreter, not just the
    /// background fill. The `Length` (25) matches the content bytes exactly.
    const RECT_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 100 100]/Contents 4 0 R>>endobj\n\
4 0 obj<</Length 25>>stream\n0 0 0 rg 10 10 80 80 re f\nendstream endobj\n\
trailer<</Size 5/Root 1 0 R>>\n%%EOF";

    #[test]
    fn renders_actual_page_content_not_just_background() {
        let page = Document::load(RECT_PDF.to_vec())
            .and_then(|doc| doc.render_page(0, 1.0))
            .ok();
        assert_eq!(
            page.as_ref().map(|p| (p.width(), p.height())),
            Some((100, 100))
        );
        // The interpreter must have drawn the rectangle: some pixels are black…
        let has_black = page
            .as_ref()
            .is_some_and(|p| p.rgba().chunks_exact(4).any(|px| px[0] < 16 && px[1] < 16 && px[2] < 16));
        // …and the margin is still white.
        let has_white = page
            .as_ref()
            .is_some_and(|p| p.rgba().chunks_exact(4).any(|px| px == [255, 255, 255, 255]));
        assert!(has_black, "expected the filled rectangle to render as black pixels");
        assert!(has_white, "expected the page margin to stay white");
    }
}

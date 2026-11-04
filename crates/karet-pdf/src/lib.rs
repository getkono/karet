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

use std::collections::HashMap;
use std::collections::HashSet;

pub use error::PdfError;
use hayro::RenderSettings;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::Array;
use hayro::hayro_syntax::object::Dict;
use hayro::hayro_syntax::object::Name;
use hayro::hayro_syntax::object::ObjectIdentifier;
use hayro::hayro_syntax::object::String as PdfString;
use hayro::hayro_syntax::object::dict::keys;
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

/// The largest width or height, in pixels, [`Document::render_page`] produces.
const MAX_RENDER_SIDE: u32 = 16_384;

/// The largest pixel count (`width * height`) [`Document::render_page`]
/// produces: 32 Mi pixels, a 128 MiB RGBA buffer.
const MAX_RENDER_PIXELS: u64 = 1 << 25;

/// Reduce `scale` so a page of `(width, height)` points renders within
/// [`MAX_RENDER_SIDE`] and [`MAX_RENDER_PIXELS`]. A scale that is not positive
/// (zero, negative, NaN) or a page size that is not positive and finite maps to
/// `0.0`, which renders an empty page.
fn bounded_scale((width, height): (f32, f32), scale: f32) -> f32 {
    let (w, h) = (f64::from(width), f64::from(height));
    let page_ok = w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0;
    if !page_ok || scale.is_nan() || scale <= 0.0 {
        return 0.0;
    }
    let cap =
        (f64::from(MAX_RENDER_SIDE) / w.max(h)).min((MAX_RENDER_PIXELS as f64 / (w * h)).sqrt());
    if f64::from(scale) <= cap {
        return scale;
    }
    // `cap` is finite and at most `MAX_RENDER_SIDE`, so the cast is exact enough.
    let narrowed = cap as f32;
    // Round toward zero, so the f32 scale never exceeds the f64 cap.
    if f64::from(narrowed) > cap {
        narrowed.next_down()
    } else {
        narrowed
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
    /// The output is bounded: where the page box at `scale` would exceed 16384
    /// pixels on either side or 2^25 (about 33.5 million) pixels in total,
    /// including at `scale = f32::INFINITY`, the page is rendered at the largest
    /// scale that fits both, keeping its aspect ratio. A zero, negative, or NaN
    /// `scale`, or a page box with no finite size, yields an empty 0×0 page.
    ///
    /// # Errors
    /// Returns [`PdfError::PageOutOfRange`] if `index >= self.page_count()`.
    pub fn render_page(&self, index: usize, scale: f32) -> Result<RenderedPage, PdfError> {
        let pages = self.pdf.pages();
        let count = pages.len();
        let page = pages
            .get(index)
            .ok_or(PdfError::PageOutOfRange { index, count })?;
        let scale = bounded_scale(page.render_dimensions(), scale);

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

    /// Extract the document's navigation outline (bookmarks) as a tree.
    ///
    /// Each entry maps to a 0-based page index where its destination can be
    /// resolved. A document with no outline — or a malformed one — yields an empty
    /// `Vec` rather than an error. Named destinations (`/Names` → `/Dests` name
    /// trees) and remote/URI actions are not resolved: those entries keep their
    /// title but report [`OutlineDest::Unresolved`]; an entry with neither a destination
    /// nor an action reports [`OutlineDest::Absent`].
    #[must_use]
    pub fn outline(&self) -> Vec<OutlineItem> {
        let xref = self.pdf.xref();
        let Some(catalog) = xref.get::<Dict>(xref.root_id()) else {
            return Vec::new();
        };
        let Some(outlines) = catalog.get::<Dict>(keys::OUTLINES) else {
            return Vec::new();
        };
        let Some(first) = outlines.get::<Dict>(keys::FIRST) else {
            return Vec::new();
        };
        let page_index = self.page_id_index_map();
        let mut visited = HashSet::new();
        walk_outline_siblings(first, &page_index, &mut visited, 0)
    }

    /// Map each page's indirect-object id to its 0-based index, so an outline
    /// destination's target page reference can be resolved to a page number.
    fn page_id_index_map(&self) -> HashMap<ObjectIdentifier, usize> {
        self.pdf
            .pages()
            .iter()
            .enumerate()
            .filter_map(|(index, page)| page.raw().obj_id().map(|id| (id, index)))
            .collect()
    }
}

/// One entry in a PDF's navigation outline (a bookmark / table-of-contents node).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutlineItem {
    /// The bookmark's display title, decoded from the PDF text string: UTF-16BE
    /// when it starts with a `FE FF` byte-order mark, otherwise byte-by-byte as
    /// Latin-1 (an approximation of PDFDocEncoding; a UTF-8 BOM is not handled).
    pub title: String,
    /// Where the bookmark points.
    pub dest: OutlineDest,
    /// Nested child bookmarks, in document order.
    pub children: Vec<OutlineItem>,
}

impl OutlineItem {
    /// An outline entry from its parts, for a consumer that builds an outline
    /// itself (the struct is `#[non_exhaustive]`, so it has no literal syntax
    /// outside this crate).
    #[must_use]
    pub fn new(title: impl Into<String>, dest: OutlineDest, children: Vec<OutlineItem>) -> Self {
        Self {
            title: title.into(),
            dest,
            children,
        }
    }
}

/// Where an [`OutlineItem`] points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutlineDest {
    /// A 0-based page index in this document, reached through an explicit
    /// destination or a `GoTo` action.
    Page(usize),
    /// The entry has a destination or action this crate does not resolve: a
    /// named destination, a remote/URI action, or an explicit destination whose
    /// target is not a page of this document.
    Unresolved,
    /// The entry has no destination and no action at all (a pure grouping node).
    Absent,
}

impl OutlineDest {
    /// The 0-based page index, when the destination resolved to one.
    #[must_use]
    pub fn page(self) -> Option<usize> {
        match self {
            Self::Page(page) => Some(page),
            Self::Unresolved | Self::Absent => None,
        }
    }
}

/// Cap on outline nesting depth, guarding against a pathological or cyclic `/First`
/// chain blowing the stack.
const MAX_OUTLINE_DEPTH: usize = 64;

/// Walk one chain of `/Next` siblings, recursing into each entry's `/First` child.
fn walk_outline_siblings(
    first: Dict<'_>,
    page_index: &HashMap<ObjectIdentifier, usize>,
    visited: &mut HashSet<ObjectIdentifier>,
    depth: usize,
) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut current = Some(first);
    while let Some(item) = current {
        // Cycle guard: stop if a `/Next`/`/First` link points back at a seen node.
        if let Some(id) = item.obj_id()
            && !visited.insert(id)
        {
            break;
        }
        let title = outline_title(&item).unwrap_or_default();
        let dest = outline_dest(&item, page_index);
        let children = if depth < MAX_OUTLINE_DEPTH {
            item.get::<Dict>(keys::FIRST)
                .map(|child| walk_outline_siblings(child, page_index, visited, depth + 1))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        items.push(OutlineItem {
            title,
            dest,
            children,
        });
        current = item.get::<Dict>(keys::NEXT);
    }
    items
}

/// Read and decode an outline entry's `/Title`.
fn outline_title(item: &Dict<'_>) -> Option<String> {
    item.get::<PdfString>(keys::TITLE)
        .map(|s| decode_pdf_text_string(s.as_bytes()))
}

/// Classify an outline entry's destination: [`OutlineDest::Absent`] when it has
/// neither `/Dest` nor `/A`, otherwise the page [`outline_page`] resolves, or
/// [`OutlineDest::Unresolved`] when it resolves none.
fn outline_dest(item: &Dict<'_>, page_index: &HashMap<ObjectIdentifier, usize>) -> OutlineDest {
    if !item.contains_key(keys::DEST) && !item.contains_key(keys::A) {
        return OutlineDest::Absent;
    }
    outline_page(item, page_index).map_or(OutlineDest::Unresolved, OutlineDest::Page)
}

/// Resolve an outline entry's target page index from `/Dest` or a `GoTo` `/A`
/// action. Named destinations (a `/Dest` name/string, or a `/D` name) require a
/// name-tree walk hayro does not provide, so those return `None`.
fn outline_page(item: &Dict<'_>, page_index: &HashMap<ObjectIdentifier, usize>) -> Option<usize> {
    // 1) Explicit destination array: /Dest [ pageRef /Fit ... ].
    if let Some(dest) = item.get::<Array>(keys::DEST)
        && let Some(page) = dest_array_page(&dest, page_index)
    {
        return Some(page);
    }
    // 2) GoTo action: /A << /S /GoTo /D [ pageRef ... ] >>.
    let action = item.get::<Dict>(keys::A)?;
    if action.get::<Name>(keys::S).as_deref() != Some(b"GoTo".as_slice()) {
        return None;
    }
    let dest = action.get::<Array>(keys::D)?;
    dest_array_page(&dest, page_index)
}

/// The first element of a destination array is the target page reference; map it
/// to a 0-based page index.
fn dest_array_page(
    dest: &Array<'_>,
    page_index: &HashMap<ObjectIdentifier, usize>,
) -> Option<usize> {
    let page_ref = dest.raw_iter().next()?.as_obj_ref()?;
    page_index.get(&ObjectIdentifier::from(page_ref)).copied()
}

/// Decode a PDF text string into a Rust `String` without external crates: UTF-16BE
/// when it opens with a `FE FF` byte-order mark, otherwise byte-for-byte as
/// Latin-1. That is only an approximation of PDFDocEncoding, which differs from
/// Latin-1 at `0x18..=0x1F` and `0x80..=0x9F`, and a UTF-8 (`EF BB BF`) BOM is not
/// recognised, so such a title decodes byte-by-byte as Latin-1 too.
fn decode_pdf_text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units = rest
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]));
        char::decode_utf16(units)
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

#[cfg(test)]
mod tests {
    use OutlineDest::Absent;
    use OutlineDest::Page;
    use OutlineDest::Unresolved;

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

    /// A minimal single-page PDF carrying an `/Outlines` dictionary with one
    /// bookmark ("Chapter 1") whose `/Dest` targets page object `3 0 R` (page 0).
    /// Like `RECT_PDF`, it has no xref table or `startxref`, so hayro parses it via
    /// its brute-force fallback and no byte-accurate offsets are needed.
    const OUTLINE_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R/Outlines 4 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>endobj\n\
4 0 obj<</Type/Outlines/First 5 0 R/Last 5 0 R/Count 1>>endobj\n\
5 0 obj<</Title(Chapter 1)/Parent 4 0 R/Dest[3 0 R/Fit]>>endobj\n\
trailer<</Size 6/Root 1 0 R>>\n%%EOF";

    #[test]
    fn outline_extracts_bookmark_with_page() {
        let items = Document::load(OUTLINE_PDF.to_vec())
            .map(|doc| doc.outline())
            .unwrap_or_default();
        assert_eq!(items.len(), 1);
        assert_eq!(items.first().map(|i| i.title.as_str()), Some("Chapter 1"));
        assert_eq!(items.first().map(|i| i.dest), Some(OutlineDest::Page(0)));
        assert!(items.first().is_some_and(|i| i.children.is_empty()));
    }

    /// Only a resolved destination has a page; the two unpaged kinds stay apart.
    #[test]
    fn outline_dest_reports_a_page_only_when_resolved() {
        assert_eq!(Page(3).page(), Some(3));
        assert_eq!(Unresolved.page(), None);
        assert_eq!(Absent.page(), None);
        let item = OutlineItem::new("Intro", Page(1), Vec::new());
        assert_eq!(
            (item.title.as_str(), item.dest, item.children.len()),
            ("Intro", Page(1), 0)
        );
    }

    #[test]
    fn outline_absent_returns_empty() {
        let items = Document::load(MINIMAL_PDF.to_vec())
            .map(|doc| doc.outline())
            .unwrap_or_default();
        assert!(items.is_empty());
    }

    /// Non-UTF-16 titles decode as Latin-1; a `FE FF` BOM decodes as UTF-16BE.
    #[test]
    fn decodes_pdf_text_strings() {
        assert_eq!(decode_pdf_text_string(b"Chapter 1"), "Chapter 1");
        assert_eq!(
            decode_pdf_text_string(&[0xFE, 0xFF, 0x00, 0x41, 0x00, 0x42]),
            "AB"
        );
    }

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
        let has_black = page.as_ref().is_some_and(|p| {
            p.rgba()
                .chunks_exact(4)
                .any(|px| px[0] < 16 && px[1] < 16 && px[2] < 16)
        });
        // …and the margin is still white.
        let has_white = page.as_ref().is_some_and(|p| {
            p.rgba()
                .chunks_exact(4)
                .any(|px| px == [255, 255, 255, 255])
        });
        assert!(
            has_black,
            "expected the filled rectangle to render as black pixels"
        );
        assert!(has_white, "expected the page margin to stay white");
    }

    /// Assemble a xref-less PDF (hayro's brute-force fallback locates the objects)
    /// from `(object number, body)` pairs, with `trailer` as the trailer dictionary.
    fn pdf_from_objects(objects: &[(usize, String)], trailer: &str) -> Vec<u8> {
        let mut pdf = String::from("%PDF-1.4\n");
        for (num, body) in objects {
            pdf.push_str(&format!("{num} 0 obj{body}endobj\n"));
        }
        pdf.push_str(&format!("trailer{trailer}\n%%EOF"));
        pdf.into_bytes()
    }

    /// A two-page document (pages `3 0 R` and `4 0 R`) whose `/Outlines` root
    /// (`5 0 R`) has `/First 6 0 R`; `entries` supplies objects 6 onward.
    fn outline_doc(entries: &[(usize, String)]) -> Vec<OutlineItem> {
        let mut objects = vec![
            (1, "<</Type/Catalog/Pages 2 0 R/Outlines 5 0 R>>".to_owned()),
            (2, "<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>".to_owned()),
            (
                3,
                "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>".to_owned(),
            ),
            (
                4,
                "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>".to_owned(),
            ),
            (5, "<</Type/Outlines/First 6 0 R>>".to_owned()),
        ];
        objects.extend(entries.iter().cloned());
        let size = objects.len() + 1;
        Document::load(pdf_from_objects(
            &objects,
            &format!("<</Size {size}/Root 1 0 R>>"),
        ))
        .map(|doc| doc.outline())
        .unwrap_or_default()
    }

    /// Flatten an outline into `(depth, title, dest)` rows in document order.
    fn flatten(items: &[OutlineItem], depth: usize, out: &mut Vec<(usize, String, OutlineDest)>) {
        for item in items {
            out.push((depth, item.title.clone(), item.dest));
            flatten(&item.children, depth + 1, out);
        }
    }

    fn rows(items: &[OutlineItem]) -> Vec<(usize, String, OutlineDest)> {
        let mut out = Vec::new();
        flatten(items, 0, &mut out);
        out
    }

    #[test]
    fn outline_walks_siblings_children_and_goto_actions() {
        let items = outline_doc(&[
            (
                6,
                "<</Title(One)/Dest[3 0 R/Fit]/First 8 0 R/Next 7 0 R>>".to_owned(),
            ),
            (
                7,
                "<</Title(Two)/A<</S/GoTo/D[4 0 R/XYZ 0 0 0]>>>>".to_owned(),
            ),
            (8, "<</Title(One.a)/Next 9 0 R>>".to_owned()),
            (9, "<</Title(One.b)/A<</S/GoTo/D[4 0 R/Fit]>>>>".to_owned()),
        ]);
        assert_eq!(
            rows(&items),
            vec![
                (0, "One".to_owned(), Page(0)),
                (1, "One.a".to_owned(), Absent),
                (1, "One.b".to_owned(), Page(1)),
                (0, "Two".to_owned(), Page(1)),
            ]
        );
    }

    /// Only `GoTo` actions resolve; a URI action, a named `/Dest`, and a `/Dest`
    /// whose page reference is not a page keep their title and report
    /// `Unresolved`, which stays distinct from an entry with no destination at all.
    #[test]
    fn outline_leaves_unresolvable_destinations_unpaged() {
        let items = outline_doc(&[
            (
                6,
                "<</Title(Uri)/A<</S/URI/URI(http://x)/D[3 0 R/Fit]>>/Next 7 0 R>>".to_owned(),
            ),
            (7, "<</Title(Named)/Dest/chap1/Next 8 0 R>>".to_owned()),
            (
                8,
                "<</Title(NotAPage)/Dest[5 0 R/Fit]/Next 9 0 R>>".to_owned(),
            ),
            (9, "<</Next 10 0 R>>".to_owned()),
            // An explicit /Dest that fails to resolve falls through to the action.
            (
                10,
                "<</Title(Fallback)/Dest[5 0 R/Fit]/A<</S/GoTo/D[4 0 R/Fit]>>>>".to_owned(),
            ),
        ]);
        assert_eq!(
            rows(&items),
            vec![
                (0, "Uri".to_owned(), Unresolved),
                (0, "Named".to_owned(), Unresolved),
                (0, "NotAPage".to_owned(), Unresolved),
                (0, String::new(), Absent),
                (0, "Fallback".to_owned(), Page(1)),
            ]
        );
    }

    /// A node whose `/Next` points at itself is emitted once, not forever.
    #[test]
    fn outline_self_next_cycle_terminates() {
        let items = outline_doc(&[(6, "<</Title(Loop)/Dest[3 0 R/Fit]/Next 6 0 R>>".to_owned())]);
        assert_eq!(rows(&items), vec![(0, "Loop".to_owned(), Page(0))]);
    }

    /// A `/Next` back to an earlier sibling and a `/First` back to an ancestor are
    /// both cut at the first revisit.
    #[test]
    fn outline_back_edges_to_seen_nodes_are_cut() {
        let items = outline_doc(&[
            (6, "<</Title(A)/First 8 0 R/Next 7 0 R>>".to_owned()),
            (7, "<</Title(B)/Next 6 0 R>>".to_owned()),
            (8, "<</Title(A.child)/First 6 0 R>>".to_owned()),
        ]);
        assert_eq!(
            rows(&items),
            vec![
                (0, "A".to_owned(), Absent),
                (1, "A.child".to_owned(), Absent),
                (0, "B".to_owned(), Absent),
            ]
        );
    }

    /// A `/First` chain deeper than `MAX_OUTLINE_DEPTH` is truncated: entries at
    /// depths `0..=MAX_OUTLINE_DEPTH` are kept and nothing below them.
    #[test]
    fn outline_depth_is_capped() {
        let total = MAX_OUTLINE_DEPTH + 10;
        let entries: Vec<(usize, String)> = (0..total)
            .map(|level| {
                let num = 6 + level;
                let child = if level + 1 < total {
                    format!("/First {} 0 R", num + 1)
                } else {
                    String::new()
                };
                (num, format!("<</Title(L{level}){child}>>"))
            })
            .collect();
        let flat = rows(&outline_doc(&entries));
        assert_eq!(flat.len(), MAX_OUTLINE_DEPTH + 1);
        assert_eq!(
            flat.last(),
            Some(&(MAX_OUTLINE_DEPTH, format!("L{MAX_OUTLINE_DEPTH}"), Absent))
        );
    }

    /// An `/Outlines` dictionary without `/First` yields no entries.
    #[test]
    fn outline_without_first_is_empty() {
        let objects = vec![
            (1, "<</Type/Catalog/Pages 2 0 R/Outlines 4 0 R>>".to_owned()),
            (2, "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_owned()),
            (
                3,
                "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>".to_owned(),
            ),
            (4, "<</Type/Outlines/Count 0>>".to_owned()),
        ];
        let items = Document::load(pdf_from_objects(&objects, "<</Size 5/Root 1 0 R>>"))
            .map(|doc| doc.outline());
        assert!(items.is_ok_and(|items| items.is_empty()));
    }

    /// Assemble a PDF with a byte-accurate xref table for objects `1..=n` given
    /// in order, appending `trailer_extra` to the trailer dictionary. hayro only
    /// reports encryption through a real xref (its brute-force fallback swallows
    /// a decryption failure as "invalid").
    fn pdf_with_xref(bodies: &[&str], trailer_extra: &str) -> Vec<u8> {
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, body) in bodies.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj{body}endobj\n", index + 1));
        }
        let xref_at = pdf.len();
        let size = bodies.len() + 1;
        pdf.push_str(&format!("xref\n0 {size}\n0000000000 65535 f \n"));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer<</Size {size}/Root 1 0 R{trailer_extra}>>\nstartxref\n{xref_at}\n%%EOF"
        ));
        pdf.into_bytes()
    }

    const PAGE_TREE: [&str; 3] = [
        "<</Type/Catalog/Pages 2 0 R>>",
        "<</Type/Pages/Kids[3 0 R]/Count 1>>",
        "<</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]>>",
    ];

    /// The xref helper itself yields a loadable document, so the encrypted case
    /// below fails because of `/Encrypt` and not because of the scaffolding.
    #[test]
    fn xref_built_document_loads() {
        let count = Document::load(pdf_with_xref(&PAGE_TREE, "")).map(|doc| doc.page_count());
        assert!(matches!(count, Ok(1)));
    }

    /// A trailer `/Encrypt` dictionary hayro cannot decrypt surfaces as
    /// `Encrypted`, not as a generic parse failure.
    #[test]
    fn encrypted_document_reports_encrypted() {
        let mut bodies = PAGE_TREE.to_vec();
        bodies.push("<</Filter/Unknown/V 1/R 2>>");
        let result = Document::load(pdf_with_xref(&bodies, "/Encrypt 4 0 R"));
        assert!(matches!(result, Err(PdfError::Encrypted)));
    }

    /// Zero, negative and NaN scales degrade to an empty raster rather than
    /// panicking; a fractional scale floors each dimension.
    #[test]
    fn degenerate_scales_render_empty_pages() -> Result<(), PdfError> {
        let doc = Document::load(RECT_PDF.to_vec())?;
        for scale in [0.0, -0.0, -1.0, f32::NAN, f32::NEG_INFINITY, 0.001] {
            let page = doc.render_page(0, scale);
            assert!(
                page.as_ref()
                    .is_ok_and(|p| (p.width(), p.height()) == (0, 0) && p.rgba().is_empty()),
                "scale {scale}: {page:?}"
            );
        }
        let dims = doc.render_page(0, 0.255).map(|p| (p.width(), p.height()));
        assert!(matches!(dims, Ok((25, 25))));
        Ok(())
    }

    /// A one-page document whose page box is `width`×`height` points.
    fn sized_page(width: u64, height: u64) -> Result<Document, PdfError> {
        let page = format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 {width} {height}]>>");
        let bodies = [PAGE_TREE[0], PAGE_TREE[1], page.as_str()];
        Document::load(pdf_with_xref(&bodies, ""))
    }

    /// An oversized MediaBox at the app's 2.0 scale, and the same page at an
    /// infinite scale, both narrow to `MAX_RENDER_SIDE` instead of asking hayro
    /// for a 65535-wide pixmap. The page is one point tall at the bound, so the
    /// raster stays cheap.
    #[test]
    fn oversized_page_and_infinite_scale_clamp_to_max_side() -> Result<(), PdfError> {
        let doc = sized_page(1_000_000, 100)?;
        for scale in [2.0, 1e9, f32::MAX, f32::INFINITY] {
            let page = doc.render_page(0, scale)?;
            assert_eq!(
                (page.width(), page.height()),
                (MAX_RENDER_SIDE, 1),
                "scale {scale}"
            );
            assert_eq!(page.rgba().len(), MAX_RENDER_SIDE as usize * 4);
        }
        Ok(())
    }

    /// A page that fits the budget renders at exactly the requested scale.
    #[test]
    fn in_budget_scale_is_untouched() {
        assert_eq!(bounded_scale((612.0, 792.0), 2.0), 2.0);
        assert_eq!(bounded_scale((100.0, 100.0), 0.255), 0.255);
    }

    /// Whichever bound binds, the clamped raster fits both the side and the
    /// pixel budget, computed the way hayro sizes its pixmap.
    #[test]
    fn clamped_scale_fits_side_and_pixel_budget() {
        let pages = [
            (100_000.0, 100_000.0), // pixel budget binds
            (1_000_000.0, 100.0),   // side bound binds
            (100.0, 1_000_000.0),
            (14_400.0, 14_400.0), // the PDF spec's largest page box
            (612.0, 792.0),
        ];
        for (w, h) in pages {
            for scale in [2.0, 1e6, f32::INFINITY] {
                let s = bounded_scale((w, h), scale);
                assert!(s.is_finite() && s > 0.0, "{w}x{h}@{scale}: {s}");
                let (pw, ph) = ((w * s).floor(), (h * s).floor());
                assert!(pw <= MAX_RENDER_SIDE as f32 && ph <= MAX_RENDER_SIDE as f32);
                let pixels = pw as u64 * ph as u64;
                assert!(pixels <= MAX_RENDER_PIXELS, "{w}x{h}@{scale}: {pixels}");
            }
        }
        // The pixel bound uses most of the budget rather than collapsing.
        let s = bounded_scale((100_000.0, 100_000.0), f32::INFINITY);
        assert_eq!((100_000.0 * s).floor(), 5792.0);
    }

    /// A page box with no finite positive size renders empty at any scale.
    #[test]
    fn non_finite_page_box_renders_empty() {
        for dims in [
            (f32::INFINITY, 10.0),
            (10.0, f32::INFINITY),
            (f32::NAN, 10.0),
            (0.0, 10.0),
        ] {
            assert_eq!(bounded_scale(dims, 2.0), 0.0, "{dims:?}");
        }
    }
}

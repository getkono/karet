//! Terminal image rendering: the Kitty graphics protocol with a truecolor
//! halfblock fallback (merged from the former `karet-image` crate).
//!
//! [`ImageWidget`] renders halfblocks straight into the ratatui buffer, which
//! works on any truecolor terminal. On a Kitty-graphics-capable terminal the
//! application instead reserves the area and flushes [`Image::kitty_escape`] to
//! the terminal after drawing, since the cell buffer cannot carry pixels. The
//! placement lifecycle across scroll/resize is intentionally minimal (active tab
//! only) for now; Sixel/iTerm2 protocols and PDF rasterization are out of scope.
//!
//! Pixel work sits behind two features so a lean build pulls no codec tree: the
//! shared primitives ([`Image`], [`ImageWidget`]) and their built-in bilinear
//! resampler require `raster` (enabled by both `images` and `pdf`), while the
//! image-file decoders ([`decode`], [`dimensions`]) require `images`. Gamut owns
//! every supported codec. Protocol detection ([`GraphicsProtocol`],
//! [`detect_protocol`], [`fit_rect`]) carries no codec dependency and is always
//! compiled.

#[cfg(feature = "raster")]
use base64::Engine as _;
#[cfg(feature = "images")]
use gamut::core::DecodeImage as _;
#[cfg(feature = "images")]
use gamut::core::Rgb8;
#[cfg(feature = "images")]
use gamut::core::Rgba8;
#[cfg(feature = "raster")]
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
#[cfg(feature = "raster")]
use ratatui::style::Color;
#[cfg(feature = "raster")]
use ratatui::widgets::Widget;

/// The maximum base64 payload per Kitty escape chunk.
#[cfg(feature = "raster")]
const KITTY_CHUNK: usize = 4096;

/// Errors decoding or rendering an image.
#[cfg(feature = "images")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ImageError {
    /// The image bytes could not be decoded.
    #[error("failed to decode image")]
    Decode,
}

/// The terminal graphics protocol to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsProtocol {
    /// The Kitty graphics protocol (also supported by ghostty, WezTerm, …).
    Kitty,
    /// Truecolor halfblocks — works on any 24-bit terminal.
    #[default]
    Halfblocks,
}

/// Detect the best-supported graphics protocol from the environment.
#[must_use]
pub fn detect_protocol() -> GraphicsProtocol {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return GraphicsProtocol::Kitty;
    }
    let env_contains = |key: &str, needles: &[&str]| {
        std::env::var(key)
            .map(|v| v.to_ascii_lowercase())
            .is_ok_and(|v| needles.iter().any(|n| v.contains(n)))
    };
    if env_contains("TERM", &["kitty", "ghostty"])
        || env_contains("TERM_PROGRAM", &["ghostty", "wezterm"])
    {
        return GraphicsProtocol::Kitty;
    }
    GraphicsProtocol::Halfblocks
}

/// The Kitty escape that deletes all displayed images (use when switching views).
#[must_use]
pub fn kitty_delete_all() -> String {
    "\x1b_Ga=d\x1b\\".to_string()
}

/// Approximate terminal cell aspect ratio (height ÷ width). A monospace cell is
/// roughly twice as tall as it is wide, so preserving a `w × h` pixel image's
/// aspect ratio means mapping it onto a cell box of `2w : h`.
const CELL_ASPECT: f64 = 2.0;

/// Fit a `px_w × px_h` pixel image into `area` (in cells), returning the largest
/// aspect-ratio-preserving sub-rect, centered. Used to reserve a Kitty placement
/// that does not stretch a page/image to the full pane. Falls back to `area` for
/// degenerate inputs.
#[must_use]
pub fn fit_rect(area: Rect, px_w: u32, px_h: u32) -> Rect {
    if px_w == 0 || px_h == 0 || area.width == 0 || area.height == 0 {
        return area;
    }
    // Target cell box that preserves the pixel aspect ratio (see `CELL_ASPECT`).
    let target_cols = f64::from(px_w) * CELL_ASPECT;
    let target_rows = f64::from(px_h);
    let scale = (f64::from(area.width) / target_cols).min(f64::from(area.height) / target_rows);
    let w = ((target_cols * scale).round() as u16).clamp(1, area.width);
    let h = ((target_rows * scale).round() as u16).clamp(1, area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect::new(x, y, w, h)
}

/// A decoded RGBA image.
#[cfg(feature = "raster")]
#[derive(Clone, Debug)]
pub struct Image {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

#[cfg(feature = "raster")]
impl Image {
    /// Build an image directly from raw 8-bit RGBA pixels (row-major, 4 bytes per
    /// pixel, `width * height * 4` bytes).
    ///
    /// This is the entry point for pixels produced by something other than an
    /// encoded image file — e.g. a rasterized PDF page — so they can reuse the
    /// Kitty escape / halfblock machinery. If `rgba` is not exactly
    /// `width * height * 4` bytes it is padded or truncated to fit, keeping the
    /// declared dimensions authoritative.
    #[must_use]
    pub fn from_rgba(mut rgba: Vec<u8>, width: u32, height: u32) -> Self {
        let expected = width as usize * height as usize * 4;
        rgba.resize(expected, 0);
        Self {
            rgba,
            width,
            height,
        }
    }

    /// The pixel width.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The pixel height.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Build the Kitty graphics escape that transmits and displays this image
    /// scaled into a `cols`×`rows` cell box. The application positions the cursor
    /// at the target cell and writes this sequence after drawing the frame.
    #[must_use]
    pub fn kitty_escape(&self, cols: u16, rows: u16) -> String {
        let payload = base64::engine::general_purpose::STANDARD.encode(&self.rgba);
        let chunks: Vec<&[u8]> = payload.as_bytes().chunks(KITTY_CHUNK).collect();
        let mut out = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let more = u8::from(i + 1 != chunks.len());
            let data = std::str::from_utf8(chunk).unwrap_or("");
            if i == 0 {
                out.push_str(&format!(
                    "\x1b_Ga=T,f=32,s={},v={},c={},r={},m={more};{data}\x1b\\",
                    self.width, self.height, cols, rows
                ));
            } else {
                out.push_str(&format!("\x1b_Gm={more};{data}\x1b\\"));
            }
        }
        out
    }

    /// Render the image as truecolor halfblocks into `area` (two vertically
    /// stacked pixels per cell), preserving aspect ratio.
    pub fn render_halfblocks(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.width == 0 || self.height == 0 {
            return;
        }
        // Fit within the available pixels: width columns × (height × 2) rows.
        let avail_w = f64::from(area.width);
        let avail_h = f64::from(area.height) * 2.0;
        let scale = (avail_w / f64::from(self.width)).min(avail_h / f64::from(self.height));
        let target_w = ((f64::from(self.width) * scale) as u32).clamp(1, u32::from(area.width));
        let target_h =
            ((f64::from(self.height) * scale) as u32).clamp(1, u32::from(area.height) * 2);
        self.paint_halfblocks(target_w, target_h, 0, area, buf);
    }

    /// Render a window of the image scaled into a `cols`×`rows` halfblock box: the
    /// box's cell rows from `first_row` on, as many as fit in `area`, clipped to its
    /// width.
    ///
    /// The box is taken as given — the caller chose its aspect — so a view scrolling
    /// past a tall image can paint just the rows on screen, each row identical to the
    /// one a whole-box render would paint there.
    pub fn render_halfblocks_rows(
        &self,
        cols: u16,
        rows: u16,
        first_row: u16,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if cols == 0 || rows == 0 || area.width == 0 || area.height == 0 {
            return;
        }
        if self.width == 0 || self.height == 0 {
            return;
        }
        self.paint_halfblocks(
            u32::from(cols),
            u32::from(rows) * 2,
            u32::from(first_row),
            area,
            buf,
        );
    }

    /// Paint this image resampled to `target_w`×`target_h` pixels, two pixels per
    /// cell, starting at cell row `first_row` of the result, into `area`.
    fn paint_halfblocks(
        &self,
        target_w: u32,
        target_h: u32,
        first_row: u32,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let last_row = target_h
            .div_ceil(2)
            .min(first_row.saturating_add(u32::from(area.height)));
        let cols = target_w.min(u32::from(area.width));
        for cy in first_row..last_row {
            for cx in 0..cols {
                let top = self.sample_resized(cx, (cy * 2).min(target_h - 1), target_w, target_h);
                let bottom_y = cy * 2 + 1;
                let bottom = if bottom_y < target_h {
                    self.sample_resized(cx, bottom_y, target_w, target_h)
                } else {
                    top
                };
                let x = area.x + cx as u16;
                let y = area.y + (cy - first_row) as u16;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char('▀');
                    cell.set_fg(Color::Rgb(top[0], top[1], top[2]));
                    cell.set_bg(Color::Rgb(bottom[0], bottom[1], bottom[2]));
                }
            }
        }
    }

    /// Bilinearly sample one destination pixel. Mapping pixel centers instead of
    /// corners avoids a half-pixel drift while scaling both up and down.
    fn sample_resized(&self, x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
        let source_x = ((x as f64 + 0.5) * f64::from(self.width) / f64::from(width) - 0.5)
            .clamp(0.0, f64::from(self.width - 1));
        let source_y = ((y as f64 + 0.5) * f64::from(self.height) / f64::from(height) - 0.5)
            .clamp(0.0, f64::from(self.height - 1));
        let x0 = source_x.floor() as u32;
        let y0 = source_y.floor() as u32;
        let x1 = x0.saturating_add(1).min(self.width - 1);
        let y1 = y0.saturating_add(1).min(self.height - 1);
        let x_weight = source_x - f64::from(x0);
        let y_weight = source_y - f64::from(y0);
        let top_left = self.pixel(x0, y0);
        let top_right = self.pixel(x1, y0);
        let bottom_left = self.pixel(x0, y1);
        let bottom_right = self.pixel(x1, y1);
        let mut result = [0_u8; 4];
        for channel in 0..4 {
            let top = f64::from(top_left[channel]) * (1.0 - x_weight)
                + f64::from(top_right[channel]) * x_weight;
            let bottom = f64::from(bottom_left[channel]) * (1.0 - x_weight)
                + f64::from(bottom_right[channel]) * x_weight;
            result[channel] = (top * (1.0 - y_weight) + bottom * y_weight).round() as u8;
        }
        result
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[offset],
            self.rgba[offset + 1],
            self.rgba[offset + 2],
            self.rgba[offset + 3],
        ]
    }
}

/// Decode image bytes into an [`Image`].
///
/// # Errors
/// Returns [`ImageError::Decode`] if the bytes are not a supported format.
#[cfg(feature = "images")]
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    if is_png(bytes) {
        return decode_gamut_rgba(gamut::png::PngDecoder::new(), bytes);
    }
    if is_jpeg(bytes) {
        return decode_gamut_jpeg(bytes);
    }
    if is_webp(bytes) {
        return decode_gamut_rgba(gamut::webp::WebpDecoder::new(), bytes);
    }
    if is_tiff(bytes) {
        return decode_gamut_rgba(gamut::tiff::TiffDecoder::new(), bytes);
    }
    Err(ImageError::Decode)
}

#[cfg(feature = "images")]
fn decode_gamut_rgba(
    decoder: impl gamut::core::DecodeImage<Rgba8>,
    bytes: &[u8],
) -> Result<Image, ImageError> {
    let decoded = decoder
        .decode_image(bytes)
        .map_err(|_| ImageError::Decode)?;
    Ok(from_gamut(decoded))
}

#[cfg(feature = "images")]
fn decode_gamut_jpeg(bytes: &[u8]) -> Result<Image, ImageError> {
    let decoded: gamut::core::ImageBuf<Rgb8> = gamut::jpeg::JpegDecoder::new()
        .decode_image(bytes)
        .map_err(|_| ImageError::Decode)?;
    let dimensions = decoded.dimensions();
    let rgb = decoded.into_samples();
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for pixel in rgb.chunks_exact(3) {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
    Ok(Image {
        rgba,
        width: dimensions.width,
        height: dimensions.height,
    })
}

#[cfg(feature = "images")]
fn from_gamut(decoded: gamut::core::ImageBuf<Rgba8>) -> Image {
    let dimensions = decoded.dimensions();
    Image {
        rgba: decoded.into_samples(),
        width: dimensions.width,
        height: dimensions.height,
    }
}

#[cfg(feature = "images")]
fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[cfg(feature = "images")]
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\xff\xd8\xff")
}

#[cfg(feature = "images")]
fn is_webp(bytes: &[u8]) -> bool {
    bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
}

#[cfg(feature = "images")]
fn is_tiff(bytes: &[u8]) -> bool {
    bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*")
}

/// Read just the pixel dimensions of `bytes` without fully decoding it (used for
/// placeholders), or `None` if the format cannot be determined.
#[cfg(feature = "images")]
#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    probe_dimensions(bytes).or_else(|| decode(bytes).ok().map(|image| (image.width, image.height)))
}

/// Read the pixel dimensions from the header at the start of an image file, never
/// decoding pixels — `head` may be just the file's first few kilobytes.
///
/// Knows PNG (`IHDR`), JPEG (the frame header, wherever the markers before it put
/// it) and WebP (`VP8 `, `VP8L` and `VP8X`). `None` for TIFF, whose header points
/// elsewhere in the file, for anything unrecognised, and for a header cut short.
#[cfg(feature = "images")]
#[must_use]
pub fn probe_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    let (width, height) = if is_png(head) {
        if head.get(12..16) != Some(b"IHDR") {
            return None;
        }
        (be_u32(head, 16)?, be_u32(head, 20)?)
    } else if is_jpeg(head) {
        let info = gamut::jpeg::info(head).ok()?;
        (info.width, info.height)
    } else if is_webp(head) {
        webp_dimensions(head)?
    } else {
        return None;
    };
    (width > 0 && height > 0).then_some((width, height))
}

/// The canvas size a WebP file's first chunk declares.
#[cfg(feature = "images")]
fn webp_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    let le24 = |at: usize| -> Option<u32> {
        let b = head.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    match head.get(12..16)? {
        // Extended: 24-bit canvas width and height, each stored minus one.
        b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
        // Lossless: after the 0x2f signature, 14-bit width and height, minus one.
        b"VP8L" => {
            if head.get(20) != Some(&0x2f) {
                return None;
            }
            let bits = u32::from_le_bytes(head.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        },
        // Lossy: a 3-byte frame tag and the 9d 01 2a start code, then 14-bit sizes.
        b"VP8 " => {
            if head.get(23..26) != Some(&[0x9d, 0x01, 0x2a]) {
                return None;
            }
            let le14 = |at: usize| -> Option<u32> {
                let b = head.get(at..at + 2)?;
                Some(u32::from(u16::from_le_bytes([b[0], b[1]]) & 0x3fff))
            };
            Some((le14(26)?, le14(28)?))
        },
        _ => None,
    }
}

/// The big-endian `u32` at byte `at`, if `bytes` reaches that far.
#[cfg(feature = "images")]
fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(all(test, feature = "images"))]
pub(crate) fn test_png() -> Vec<u8> {
    use gamut::core::Dimensions;
    use gamut::core::EncodeImage as _;
    use gamut::core::ImageRef;

    let rgba = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
    ];
    let Ok(dimensions) = Dimensions::new(2, 2) else {
        return Vec::new();
    };
    let Ok(image) = ImageRef::<Rgba8>::new(&rgba, dimensions) else {
        return Vec::new();
    };
    gamut::png::PngEncoder::new()
        .encode_to_vec(image)
        .unwrap_or_default()
}

/// A ratatui widget that renders an [`Image`] using truecolor halfblocks.
///
/// For the Kitty graphics path the application reserves the area and flushes
/// [`Image::kitty_escape`] itself; this widget is the universal fallback.
#[cfg(feature = "raster")]
pub struct ImageWidget<'a> {
    image: &'a Image,
}

#[cfg(feature = "raster")]
impl<'a> ImageWidget<'a> {
    /// Build a widget rendering `image`.
    #[must_use]
    pub fn new(image: &'a Image) -> Self {
        Self { image }
    }
}

#[cfg(feature = "raster")]
impl Widget for ImageWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.image.render_halfblocks(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "images")]
    fn rgba_fixture(encoder: impl gamut::core::EncodeImage<Rgba8>) -> Vec<u8> {
        use gamut::core::Dimensions;
        use gamut::core::ImageRef;

        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128,
        ];
        let Ok(dimensions) = Dimensions::new(2, 2) else {
            return Vec::new();
        };
        let Ok(image) = ImageRef::<Rgba8>::new(&rgba, dimensions) else {
            return Vec::new();
        };
        encoder.encode_to_vec(image).unwrap_or_default()
    }

    #[cfg(feature = "images")]
    fn jpeg_fixture() -> Vec<u8> {
        use gamut::core::Dimensions;
        use gamut::core::EncodeImage as _;
        use gamut::core::ImageRef;

        let rgb = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];
        let Ok(dimensions) = Dimensions::new(2, 2) else {
            return Vec::new();
        };
        let Ok(image) = ImageRef::<Rgb8>::new(&rgb, dimensions) else {
            return Vec::new();
        };
        gamut::jpeg::JpegEncoder::new()
            .encode_to_vec(image)
            .unwrap_or_default()
    }

    #[cfg(feature = "images")]
    fn empty() -> Image {
        Image {
            rgba: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    #[cfg(feature = "images")]
    #[test]
    fn decode_and_dimensions() {
        let png = test_png();
        assert_eq!(dimensions(&png), Some((2, 2)));
        assert_eq!(dimensions(&png[..24]), Some((2, 2)));
        assert!(decode(&png[..24]).is_err());
        let img = decode(&png);
        assert!(img.is_ok());
        let img = img.unwrap_or_else(|_| empty());
        assert_eq!((img.width(), img.height()), (2, 2));
    }

    #[cfg(feature = "images")]
    #[test]
    fn gamut_decodes_all_supported_formats_to_the_shared_rgba_model() {
        let png = test_png();
        let jpeg = jpeg_fixture();
        let webp = rgba_fixture(gamut::webp::WebpEncoder::lossless());
        let tiff = rgba_fixture(gamut::tiff::TiffEncoder::new());
        assert!(is_png(&png));
        assert!(is_jpeg(&jpeg));
        assert!(is_webp(&webp));
        assert!(is_tiff(&tiff));
        for encoded in [&png, &jpeg, &webp, &tiff] {
            assert_eq!(dimensions(encoded), Some((2, 2)));
            let decoded = decode(encoded);
            assert!(decoded.is_ok());
            let image = decoded.unwrap_or_else(|_| empty());
            assert_eq!((image.width(), image.height()), (2, 2));
            assert_eq!(image.rgba.len(), 16);
            if is_jpeg(encoded) {
                assert!(image.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
            }
        }
    }

    #[cfg(feature = "images")]
    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(decode(b"not an image"), Err(ImageError::Decode)));
    }

    #[cfg(feature = "raster")]
    #[test]
    fn from_rgba_keeps_dimensions_and_feeds_kitty() {
        // A 2×1 image supplied as raw RGBA reuses the Kitty escape path.
        let img = Image::from_rgba(vec![1, 2, 3, 4, 5, 6, 7, 8], 2, 1);
        assert_eq!((img.width(), img.height()), (2, 1));
        let esc = img.kitty_escape(2, 1);
        assert!(esc.contains("s=2"));
        assert!(esc.contains("v=1"));
    }

    #[cfg(feature = "raster")]
    #[test]
    fn from_rgba_pads_short_buffers_to_declared_size() {
        // Fewer bytes than width*height*4 are padded so the buffer stays valid.
        let img = Image::from_rgba(vec![255, 0, 0, 255], 2, 2);
        assert_eq!((img.width(), img.height()), (2, 2));
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        ImageWidget::new(&img).render(area, &mut buf);
        assert!(buf.content().iter().any(|c| c.symbol() == "▀"));
    }

    #[cfg(feature = "raster")]
    #[test]
    fn built_in_resampler_bilinearly_blends_pixel_centers() {
        let pixel = |value: u8| [value, value, value, 255];
        let rgba = [pixel(0), pixel(100), pixel(200), pixel(255)].concat();
        let image = Image::from_rgba(rgba, 2, 2);
        assert_eq!(image.sample_resized(1, 1, 3, 3), [139, 139, 139, 255]);
    }

    #[cfg(feature = "images")]
    #[test]
    fn halfblocks_fill_cells() {
        let img = decode(&test_png()).unwrap_or_else(|_| empty());
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        ImageWidget::new(&img).render(area, &mut buf);
        assert!(buf.content().iter().any(|c| c.symbol() == "▀"));
    }

    #[cfg(feature = "images")]
    #[test]
    fn kitty_escape_has_header_and_terminators() {
        let img = decode(&test_png()).unwrap_or_else(|_| empty());
        let esc = img.kitty_escape(4, 2);
        assert!(esc.starts_with("\x1b_G"));
        assert!(esc.ends_with("\x1b\\"));
        assert!(esc.contains("a=T"));
        assert!(esc.contains("f=32"));
        assert!(esc.contains("c=4"));
        assert!(esc.contains("r=2"));
    }

    #[cfg(feature = "images")]
    #[test]
    fn probe_reads_png_jpeg_and_webp_headers_from_a_prefix() {
        let png = test_png();
        assert_eq!(probe_dimensions(&png[..24]), Some((2, 2)));
        let webp = rgba_fixture(gamut::webp::WebpEncoder::lossless());
        assert_eq!(probe_dimensions(&webp[..25.min(webp.len())]), Some((2, 2)));
        let lossy = rgba_fixture(gamut::webp::WebpEncoder::lossy(80));
        assert_eq!(
            probe_dimensions(&lossy[..30.min(lossy.len())]),
            Some((2, 2))
        );
        // The JPEG frame header sits past the tables; the prefix need only reach the
        // end of its segment (marker, 2-byte length, then that many bytes less two).
        let jpeg = jpeg_fixture();
        let sof = jpeg
            .windows(2)
            .position(|w| w == [0xff, 0xc0] || w == [0xff, 0xc2])
            .unwrap_or(jpeg.len());
        let length = jpeg
            .get(sof + 2..sof + 4)
            .map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
        let end = (sof + 2 + length).min(jpeg.len());
        assert!(end < jpeg.len(), "the probe must not need the scan data");
        assert_eq!(probe_dimensions(&jpeg[..end]), Some((2, 2)));
    }

    #[cfg(feature = "images")]
    #[test]
    fn probe_reads_an_extended_webp_canvas() {
        // RIFF header, then a VP8X chunk: flags, reserved, canvas 300×200 minus one.
        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&[43, 1, 0, 199, 0, 0]);
        assert_eq!(probe_dimensions(&vp8x), Some((300, 200)));
    }

    #[cfg(feature = "images")]
    #[test]
    fn probe_refuses_tiff_garbage_and_cut_headers() {
        let tiff = rgba_fixture(gamut::tiff::TiffEncoder::new());
        assert_eq!(probe_dimensions(&tiff), None);
        assert_eq!(probe_dimensions(b"not an image"), None);
        assert_eq!(probe_dimensions(&test_png()[..20]), None);
        assert_eq!(probe_dimensions(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f"), None);
        assert_eq!(
            probe_dimensions(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\0\0\0"),
            None
        );
        // A zero-sized PNG is no image.
        let mut png = test_png();
        png[16..24].fill(0);
        assert_eq!(probe_dimensions(&png), None);
        // `dimensions` still gets TIFF right, by decoding.
        assert_eq!(dimensions(&tiff), Some((2, 2)));
    }

    /// A 3×4 image whose every pixel is distinct, so a misplaced row shows.
    #[cfg(feature = "raster")]
    fn gradient() -> Image {
        let rgba = (0..12u8)
            .flat_map(|i| [i * 20, 255 - i * 20, i, 255])
            .collect();
        Image::from_rgba(rgba, 3, 4)
    }

    #[cfg(feature = "raster")]
    #[test]
    fn a_row_window_paints_exactly_the_rows_a_whole_render_paints_there() {
        let image = gradient();
        let whole_area = Rect::new(0, 0, 6, 5);
        let mut whole = Buffer::empty(whole_area);
        image.render_halfblocks_rows(6, 5, 0, whole_area, &mut whole);
        for first in 0..5u16 {
            let window_area = Rect::new(10, 20, 6, 2);
            let mut window = Buffer::empty(Rect::new(10, 20, 6, 2));
            image.render_halfblocks_rows(6, 5, first, window_area, &mut window);
            for dy in 0..2u16 {
                for x in 0..6u16 {
                    let expected = whole.cell((x, first + dy)).cloned().unwrap_or_default();
                    let got = window.cell((10 + x, 20 + dy)).cloned().unwrap_or_default();
                    assert_eq!(got, expected, "row {first}+{dy}, column {x}");
                }
            }
        }
        // The whole box is painted, and nothing past it.
        assert!(whole.content().iter().all(|cell| cell.symbol() == "▀"));
    }

    #[cfg(feature = "raster")]
    #[test]
    fn a_row_window_is_clipped_to_its_area_and_the_box() {
        let image = gradient();
        // Narrower than the box: only the columns that fit.
        let area = Rect::new(0, 0, 2, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        image.render_halfblocks_rows(4, 2, 0, area, &mut buf);
        assert_eq!(
            buf.cell((1, 0)).map(|c| c.symbol().to_owned()).as_deref(),
            Some("▀")
        );
        assert_eq!(
            buf.cell((2, 0)).map(|c| c.symbol().to_owned()).as_deref(),
            Some(" ")
        );
        // A window starting past the box paints nothing.
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        image.render_halfblocks_rows(4, 2, 2, Rect::new(0, 0, 4, 2), &mut buf);
        assert!(buf.content().iter().all(|cell| cell.symbol() == " "));
        // A degenerate box paints nothing.
        image.render_halfblocks_rows(0, 2, 0, Rect::new(0, 0, 4, 2), &mut buf);
        assert!(buf.content().iter().all(|cell| cell.symbol() == " "));
    }

    #[test]
    fn fit_rect_preserves_aspect_and_centers() {
        // A tall page (612×792 px) into a wide area keeps its portrait aspect and
        // never exceeds the area.
        let area = Rect::new(0, 0, 80, 24);
        let fit = fit_rect(area, 612, 792);
        assert!(fit.width <= area.width && fit.height <= area.height);
        assert!(fit.width > 0 && fit.height > 0);
        // Portrait page → height should hit the limiting dimension.
        assert_eq!(fit.height, area.height);
        // Centered within the area (±1 cell from integer rounding on odd sizes).
        let fit_center = i32::from(fit.x) + i32::from(fit.width) / 2;
        let area_center = i32::from(area.x) + i32::from(area.width) / 2;
        assert!((fit_center - area_center).abs() <= 1);
        // Degenerate inputs fall back to the whole area.
        assert_eq!(fit_rect(area, 0, 10), area);
    }

    #[test]
    fn detect_protocol_returns_a_variant() {
        assert!(matches!(
            detect_protocol(),
            GraphicsProtocol::Kitty | GraphicsProtocol::Halfblocks
        ));
        assert_eq!(kitty_delete_all(), "\x1b_Ga=d\x1b\\");
    }
}

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
//! shared primitives ([`Image`], [`ImageWidget`]) and their built-in resampler
//! (area-averaging when shrinking, bilinear when enlarging) require `raster` (enabled by both `images` and `pdf`), while the
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

/// The most source pixels a side the per-frame halfblock painters average for one
/// destination pixel (see [`Image::sample_resized`]).
#[cfg(feature = "raster")]
const PAINT_TAPS: u32 = 4;

/// The source pixels, with their overlap, that destination pixel `at` of `dest`
/// covers along an axis `source` pixels long.
#[cfg(feature = "raster")]
fn covered(at: u32, dest: u32, source: u32) -> Vec<(u32, f64)> {
    let scale = f64::from(source) / f64::from(dest);
    let (start, end) = (f64::from(at) * scale, (f64::from(at) + 1.0) * scale);
    ((start.floor() as u32)..(end.ceil() as u32).min(source))
        .map(|pixel| {
            let overlap = (f64::from(pixel) + 1.0).min(end) - f64::from(pixel).max(start);
            (pixel, overlap)
        })
        .filter(|&(_, overlap)| overlap > 0.0)
        .collect()
}

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

/// The Kitty escape that deletes image `id` — its placements and its pixel data —
/// leaving every other image on screen alone.
#[must_use]
pub fn kitty_delete_image(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
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

    /// The raw RGBA pixels, row-major, 4 bytes per pixel.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Build the Kitty graphics escape that transmits and displays this image
    /// scaled into a `cols`×`rows` cell box. The application positions the cursor
    /// at the target cell and writes this sequence after drawing the frame.
    #[must_use]
    pub fn kitty_escape(&self, cols: u16, rows: u16) -> String {
        self.kitty_escape_keys(&format!(
            "a=T,f=32,s={},v={},c={cols},r={rows}",
            self.width, self.height
        ))
    }

    /// Like [`Image::kitty_escape`], but under image id `id` (and with the
    /// terminal's replies silenced), so [`kitty_delete_image`] can remove exactly
    /// this image without touching any other placement on screen.
    #[must_use]
    pub fn kitty_escape_with_id(&self, id: u32, cols: u16, rows: u16) -> String {
        self.kitty_escape_keys(&format!(
            "a=T,i={id},f=32,s={},v={},c={cols},r={rows},q=2",
            self.width, self.height
        ))
    }

    /// Chunk the base64 pixels into escapes, `keys` leading the first.
    fn kitty_escape_keys(&self, keys: &str) -> String {
        let payload = base64::engine::general_purpose::STANDARD.encode(&self.rgba);
        let chunks: Vec<&[u8]> = payload.as_bytes().chunks(KITTY_CHUNK).collect();
        let mut out = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let more = u8::from(i + 1 != chunks.len());
            let data = std::str::from_utf8(chunk).unwrap_or("");
            if i == 0 {
                out.push_str(&format!("\x1b_G{keys},m={more};{data}\x1b\\"));
            } else {
                out.push_str(&format!("\x1b_Gm={more};{data}\x1b\\"));
            }
        }
        out
    }

    /// This image resampled to `width`×`height` pixels: an exact area average when
    /// shrinking, so no source pixel is skipped, and bilinear when enlarging.
    ///
    /// The halfblock painters resample on every frame, so they bound the average's
    /// cost; a caller that paints the same box every frame can resample once here,
    /// exactly, and paint the result 1:1.
    #[must_use]
    pub fn resized(&self, width: u32, height: u32) -> Self {
        if self.width == 0 || self.height == 0 || width == 0 || height == 0 {
            return Self::from_rgba(Vec::new(), width, height);
        }
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&self.sample_resized(x, y, width, height, u32::MAX));
            }
        }
        Self {
            rgba,
            width,
            height,
        }
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
                let top = self.sample_resized(
                    cx,
                    (cy * 2).min(target_h - 1),
                    target_w,
                    target_h,
                    PAINT_TAPS,
                );
                let bottom_y = cy * 2 + 1;
                let bottom = if bottom_y < target_h {
                    self.sample_resized(cx, bottom_y, target_w, target_h, PAINT_TAPS)
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

    /// Sample one destination pixel of this image resampled to `width`×`height`,
    /// reading at most `taps` source pixels a side.
    ///
    /// Shrinking on both axes averages the source pixels the destination pixel
    /// covers, weighted by their overlap and by their alpha (a transparent pixel's
    /// colour must not bleed into its neighbours): a four-tap bilinear read would see
    /// four of the k² pixels a k-times shrink folds together and drop the rest, so
    /// thin lines and text would vanish or alias. Past `taps` pixels a side it reads
    /// `taps` evenly spread ones instead, bounding the cost of a painter that
    /// resamples on every frame. Anything else is bilinear.
    fn sample_resized(&self, x: u32, y: u32, width: u32, height: u32, taps: u32) -> [u8; 4] {
        if width > self.width
            || height > self.height
            || (width, height) == (self.width, self.height)
        {
            return self.sample_bilinear(x, y, width, height);
        }
        let scale_x = f64::from(self.width) / f64::from(width);
        let scale_y = f64::from(self.height) / f64::from(height);
        let averaged = if scale_x > f64::from(taps) || scale_y > f64::from(taps) {
            self.average(self.spread(x, y, (scale_x, scale_y), taps))
        } else {
            let columns = covered(x, width, self.width);
            let rows = covered(y, height, self.height);
            self.average(
                rows.iter()
                    .flat_map(|&(sy, wy)| columns.iter().map(move |&(sx, wx)| (sx, sy, wx * wy))),
            )
        };
        averaged.unwrap_or_else(|| self.sample_bilinear(x, y, width, height))
    }

    /// `taps`² source pixels spread over the area destination pixel `(x, y)` covers
    /// at `scale`, equally weighted: a grid whose every row is shifted by a further
    /// `1 / taps` of a column, so it reads every phase of a fine regular pattern — a
    /// one-pixel checkerboard averages to grey — instead of one.
    fn spread(
        &self,
        x: u32,
        y: u32,
        (scale_x, scale_y): (f64, f64),
        taps: u32,
    ) -> impl Iterator<Item = (u32, u32, f64)> {
        let (left, top) = (f64::from(x) * scale_x, f64::from(y) * scale_y);
        let (last_x, last_y) = (self.width.saturating_sub(1), self.height.saturating_sub(1));
        let n = f64::from(taps);
        (0..taps).flat_map(move |j| {
            let sy = ((top + (f64::from(j) + 0.5) * scale_y / n) as u32).min(last_y);
            (0..taps).map(move |k| {
                let offset = (f64::from(k) + (f64::from(j) + 0.5) / n) * scale_x / n;
                (((left + offset) as u32).min(last_x), sy, 1.0)
            })
        })
    }

    /// The alpha-weighted average of `samples`, each a source pixel and its weight.
    fn average(&self, samples: impl Iterator<Item = (u32, u32, f64)>) -> Option<[u8; 4]> {
        let (mut rgb, mut alpha, mut total) = ([0.0_f64; 3], 0.0_f64, 0.0_f64);
        for (sx, sy, weight) in samples {
            let pixel = self.pixel(sx, sy);
            let covered = weight * f64::from(pixel[3]);
            for (sum, &channel) in rgb.iter_mut().zip(&pixel[..3]) {
                *sum += f64::from(channel) * covered;
            }
            alpha += covered;
            total += weight;
        }
        if total <= 0.0 {
            return None;
        }
        let byte = |value: f64| value.round().clamp(0.0, 255.0) as u8;
        let colour = |sum: f64| if alpha > 0.0 { byte(sum / alpha) } else { 0 };
        Some([
            colour(rgb[0]),
            colour(rgb[1]),
            colour(rgb[2]),
            byte(alpha / total),
        ])
    }

    /// Bilinearly sample one destination pixel. Mapping pixel centers instead of
    /// corners avoids a half-pixel drift while scaling both up and down.
    fn sample_bilinear(&self, x: u32, y: u32, width: u32, height: u32) -> [u8; 4] {
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
/// placeholders), or `None` if the bytes do not decode.
///
/// Tries the header probe ([`probe_dimensions`]) first; when it has no answer —
/// TIFF, an extended WebP it will not vouch for, a header it cannot read — this
/// falls back to a full [`decode`], so the cost is unbounded for such input. A caller
/// enforcing a pixel budget before decoding should use [`probe_dimensions`] alone.
/// When that decode fails too, an extended WebP's declared canvas is still returned
/// (an animated WebP, say), since a placeholder only labels the size.
#[cfg(feature = "images")]
#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    probe_dimensions(bytes)
        .or_else(|| decode(bytes).ok().map(|image| (image.width, image.height)))
        .or_else(|| webp_canvas(bytes))
}

/// Read the pixel dimensions from the header at the start of an image file, never
/// decoding pixels — `head` may be just the file's first few kilobytes.
///
/// Knows PNG (`IHDR`), JPEG (the frame header, wherever the markers before it put
/// it) and WebP (`VP8 `, `VP8L` and `VP8X`). A size returned is the size [`decode`]
/// would produce, so it can gate a pixel budget. `None` for TIFF, whose header points
/// elsewhere in the file, for anything unrecognised, for a header cut short, and for
/// an extended WebP that is animated or whose frame is not within `head` or does not
/// match its canvas.
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

/// The first RIFF chunk of a WebP file, just past the 12-byte `RIFF`/size/`WEBP` header.
#[cfg(feature = "images")]
const WEBP_FIRST_CHUNK: usize = 12;

/// The size a WebP file decodes to.
///
/// A simple file's first chunk is its frame. An extended (`VP8X`) file declares a
/// canvas, but the decoder only checks that header and then decodes the first
/// `VP8 `/`VP8L` chunk at that frame's own size — so the canvas alone is no bound on
/// what decoding allocates. The chunks are walked to that frame, and the size is
/// trusted only when the frame is within `head` and agrees with the canvas.
///
/// An animated file (the `VP8X` animation flag) is refused outright: its frames sit in
/// `ANMF` chunks the still decoder skips, so it either fails to decode or decodes some
/// stray top-level frame the canvas says nothing about.
#[cfg(feature = "images")]
fn webp_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    if head.get(WEBP_FIRST_CHUNK..WEBP_FIRST_CHUNK + 4)? != b"VP8X" {
        return webp_frame_dimensions(head, WEBP_FIRST_CHUNK);
    }
    let payload = WEBP_FIRST_CHUNK + 8;
    if head.get(payload)? & 0x02 != 0 {
        return None;
    }
    let canvas = webp_canvas(head)?;
    // Every step passes at least a chunk header, and `head` bounds the walk.
    let mut at = WEBP_FIRST_CHUNK;
    loop {
        if matches!(head.get(at..at + 4)?, b"VP8 " | b"VP8L") {
            return (webp_frame_dimensions(head, at)? == canvas).then_some(canvas);
        }
        let size = usize::try_from(le_u32(head, at + 4)?).ok()?;
        at = at
            .checked_add(8)?
            .checked_add(size)?
            .checked_add(size & 1)?;
        // Past the bytes read, the frame is not within `head`; stopping here also keeps
        // `at` small enough that the reads above cannot overflow on 32-bit targets.
        if at > head.len() {
            return None;
        }
    }
}

/// The canvas an extended (`VP8X`) WebP file declares — not necessarily the size its
/// frame decodes to (see [`webp_dimensions`]).
#[cfg(feature = "images")]
fn webp_canvas(head: &[u8]) -> Option<(u32, u32)> {
    if !head.starts_with(b"RIFF")
        || head.get(8..12)? != b"WEBP"
        || head.get(WEBP_FIRST_CHUNK..WEBP_FIRST_CHUNK + 4)? != b"VP8X"
    {
        return None;
    }
    let payload = WEBP_FIRST_CHUNK + 8;
    Some((le24(head, payload + 4)? + 1, le24(head, payload + 7)? + 1))
}

/// The size a `VP8 ` or `VP8L` frame chunk starting at byte `at` declares.
#[cfg(feature = "images")]
fn webp_frame_dimensions(head: &[u8], at: usize) -> Option<(u32, u32)> {
    let payload = at + 8;
    match head.get(at..at + 4)? {
        // Lossless: after the 0x2f signature, 14-bit width and height, minus one.
        b"VP8L" => {
            if head.get(payload) != Some(&0x2f) {
                return None;
            }
            let bits = le_u32(head, payload + 1)?;
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        },
        // Lossy: a 3-byte frame tag and the 9d 01 2a start code, then 14-bit sizes.
        b"VP8 " => {
            if head.get(payload + 3..payload + 6) != Some(&[0x9d, 0x01, 0x2a]) {
                return None;
            }
            let le14 = |at: usize| -> Option<u32> {
                let b = head.get(at..at + 2)?;
                Some(u32::from(u16::from_le_bytes([b[0], b[1]]) & 0x3fff))
            };
            Some((le14(payload + 6)?, le14(payload + 8)?))
        },
        _ => None,
    }
}

/// The little-endian 24-bit integer at byte `at`, if `bytes` reaches that far.
#[cfg(feature = "images")]
fn le24(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}

/// The little-endian `u32` at byte `at`, if `bytes` reaches that far.
#[cfg(feature = "images")]
fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
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
mod tests;

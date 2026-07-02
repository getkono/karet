//! Terminal image rendering: the Kitty graphics protocol with a truecolor
//! halfblock fallback (merged from the former `karet-image` crate).
//!
//! [`ImageWidget`] renders halfblocks straight into the ratatui buffer, which
//! works on any truecolor terminal. On a Kitty-graphics-capable terminal the
//! application instead reserves the area and flushes [`Image::kitty_escape`] to
//! the terminal after drawing, since the cell buffer cannot carry pixels. The
//! placement lifecycle across scroll/resize is intentionally minimal (active tab
//! only) for now; Sixel/iTerm2 protocols and PDF rasterization are out of scope.

use base64::Engine as _;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

/// The maximum base64 payload per Kitty escape chunk.
const KITTY_CHUNK: usize = 4096;

/// Errors decoding or rendering an image.
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
#[derive(Clone, Debug)]
pub struct Image {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

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
        let Some(src) = ::image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
        else {
            return;
        };
        // Fit within the available pixels: width columns × (height × 2) rows.
        let avail_w = f64::from(area.width);
        let avail_h = f64::from(area.height) * 2.0;
        let scale = (avail_w / f64::from(self.width)).min(avail_h / f64::from(self.height));
        let target_w = ((f64::from(self.width) * scale) as u32).clamp(1, u32::from(area.width));
        let target_h =
            ((f64::from(self.height) * scale) as u32).clamp(1, u32::from(area.height) * 2);
        let resized = ::image::imageops::resize(
            &src,
            target_w,
            target_h,
            ::image::imageops::FilterType::Triangle,
        );

        for cy in 0..target_h.div_ceil(2) {
            for cx in 0..target_w {
                let top = *resized.get_pixel(cx, (cy * 2).min(target_h - 1));
                let bottom_y = cy * 2 + 1;
                let bottom = if bottom_y < target_h {
                    *resized.get_pixel(cx, bottom_y)
                } else {
                    top
                };
                let x = area.x + cx as u16;
                let y = area.y + cy as u16;
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char('▀');
                    cell.set_fg(Color::Rgb(top[0], top[1], top[2]));
                    cell.set_bg(Color::Rgb(bottom[0], bottom[1], bottom[2]));
                }
            }
        }
    }
}

/// Decode image bytes into an [`Image`].
///
/// # Errors
/// Returns [`ImageError::Decode`] if the bytes are not a supported format.
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let img = ::image::load_from_memory(bytes).map_err(|_| ImageError::Decode)?;
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Ok(Image {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

/// Read just the pixel dimensions of `bytes` without fully decoding it (used for
/// placeholders), or `None` if the format cannot be determined.
#[must_use]
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    ::image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// A ratatui widget that renders an [`Image`] using truecolor halfblocks.
///
/// For the Kitty graphics path the application reserves the area and flushes
/// [`Image::kitty_escape`] itself; this widget is the universal fallback.
pub struct ImageWidget<'a> {
    image: &'a Image,
}

impl<'a> ImageWidget<'a> {
    /// Build a widget rendering `image`.
    #[must_use]
    pub fn new(image: &'a Image) -> Self {
        Self { image }
    }
}

impl Widget for ImageWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.image.render_halfblocks(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 PNG built in-memory (no test fixtures on disk).
    fn tiny_png() -> Vec<u8> {
        let mut img = ::image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, ::image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 1, ::image::Rgba([0, 255, 0, 255]));
        let mut bytes = Vec::new();
        let _ = ::image::DynamicImage::ImageRgba8(img).write_to(
            &mut std::io::Cursor::new(&mut bytes),
            ::image::ImageFormat::Png,
        );
        bytes
    }

    fn empty() -> Image {
        Image {
            rgba: Vec::new(),
            width: 0,
            height: 0,
        }
    }

    #[test]
    fn decode_and_dimensions() {
        let png = tiny_png();
        assert_eq!(dimensions(&png), Some((2, 2)));
        let img = decode(&png);
        assert!(img.is_ok());
        let img = img.unwrap_or_else(|_| empty());
        assert_eq!((img.width(), img.height()), (2, 2));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(decode(b"not an image"), Err(ImageError::Decode)));
    }

    #[test]
    fn from_rgba_keeps_dimensions_and_feeds_kitty() {
        // A 2×1 image supplied as raw RGBA reuses the Kitty escape path.
        let img = Image::from_rgba(vec![1, 2, 3, 4, 5, 6, 7, 8], 2, 1);
        assert_eq!((img.width(), img.height()), (2, 1));
        let esc = img.kitty_escape(2, 1);
        assert!(esc.contains("s=2"));
        assert!(esc.contains("v=1"));
    }

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

    #[test]
    fn halfblocks_fill_cells() {
        let img = decode(&tiny_png()).unwrap_or_else(|_| empty());
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        ImageWidget::new(&img).render(area, &mut buf);
        assert!(buf.content().iter().any(|c| c.symbol() == "▀"));
    }

    #[test]
    fn kitty_escape_has_header_and_terminators() {
        let img = decode(&tiny_png()).unwrap_or_else(|_| empty());
        let esc = img.kitty_escape(4, 2);
        assert!(esc.starts_with("\x1b_G"));
        assert!(esc.ends_with("\x1b\\"));
        assert!(esc.contains("a=T"));
        assert!(esc.contains("f=32"));
        assert!(esc.contains("c=4"));
        assert!(esc.contains("r=2"));
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

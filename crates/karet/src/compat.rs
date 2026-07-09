//! Terminal compatibility checks and escape builders for app-owned rendering tiers.

use base64::Engine as _;
use crossterm::terminal;

/// Stable Kitty image id used only for the graphical editor caret.
const CARET_IMAGE_ID: u32 = 0x4b41_5201;

/// Pixel dimensions of one terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellPixels {
    /// Cell width in pixels.
    pub(crate) width: u16,
    /// Cell height in pixels.
    pub(crate) height: u16,
}

impl CellPixels {
    /// Read the current terminal's cell pixel dimensions.
    pub(crate) fn detect() -> Option<Self> {
        let size = terminal::window_size().ok()?;
        Self::from_window(size.columns, size.rows, size.width, size.height)
    }

    /// Convert terminal cell/pixel totals into per-cell pixels.
    fn from_window(columns: u16, rows: u16, width: u16, height: u16) -> Option<Self> {
        if columns == 0 || rows == 0 || width == 0 || height == 0 {
            return None;
        }
        let cell_width = width / columns;
        let cell_height = height / rows;
        if cell_width == 0 || cell_height == 0 {
            return None;
        }
        Some(Self {
            width: cell_width,
            height: cell_height,
        })
    }
}

/// One Kitty graphical caret placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphicsCaret {
    /// Cursor cell column, 0-based.
    pub(crate) x: u16,
    /// Cursor cell row, 0-based.
    pub(crate) y: u16,
    /// Pixel offset from the left edge of the cell.
    pub(crate) x_offset: u16,
    /// Pixel dimensions of one cell.
    pub(crate) cell: CellPixels,
}

impl GraphicsCaret {
    /// Build the escape sequence that deletes any previous caret placement and draws
    /// a semi-transparent 1-2px RGBA bar at this cell.
    pub(crate) fn escape(self) -> String {
        let bar_width = self.cell.width.clamp(1, 2);
        let x_offset = self.x_offset.min(self.cell.width.saturating_sub(1));
        let rgba = caret_rgba(bar_width, self.cell.height);
        let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
        format!(
            "{}\x1b[{};{}H\x1b_Ga=T,i={},f=32,s={},v={},c=1,r=1,X={},Y=0,z=1,C=1,q=2;{}\x1b\\",
            delete_graphics_caret(),
            self.y + 1,
            self.x + 1,
            CARET_IMAGE_ID,
            bar_width,
            self.cell.height,
            x_offset,
            payload
        )
    }
}

/// Delete the graphical editor caret image, leaving unrelated Kitty images alone.
pub(crate) fn delete_graphics_caret() -> String {
    format!("\x1b_Ga=d,d=I,i={CARET_IMAGE_ID}\x1b\\")
}

fn caret_rgba(width: u16, height: u16) -> Vec<u8> {
    let px = usize::from(width) * usize::from(height);
    let mut rgba = Vec::with_capacity(px * 4);
    for _ in 0..px {
        rgba.extend_from_slice(&[235, 239, 245, 192]);
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_pixels_require_reported_terminal_pixels() {
        assert_eq!(
            CellPixels::from_window(80, 24, 800, 480),
            Some(CellPixels {
                width: 10,
                height: 20
            })
        );
        assert_eq!(CellPixels::from_window(80, 24, 0, 480), None);
        assert_eq!(CellPixels::from_window(0, 24, 800, 480), None);
    }

    #[test]
    fn graphics_caret_escape_uses_offsets_and_specific_delete() {
        let esc = GraphicsCaret {
            x: 4,
            y: 2,
            x_offset: 99,
            cell: CellPixels {
                width: 9,
                height: 18,
            },
        }
        .escape();
        assert!(esc.contains("a=d,d=I"));
        assert!(esc.contains("\x1b[3;5H"));
        assert!(esc.contains("X=8"));
        assert!(esc.contains("z=1"));
    }
}

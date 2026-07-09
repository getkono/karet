//! Terminal compatibility checks and escape builders for app-owned rendering tiers.

use base64::Engine as _;

/// Stable Kitty image id used only for the graphical editor caret.
const CARET_IMAGE_ID: u32 = 0x4b41_5201;
const CARET_IMAGE_WIDTH: u16 = 32;
const CARET_IMAGE_HEIGHT: u16 = 64;
const CARET_BAR_WIDTH: u16 = 3;

/// One Kitty graphical caret placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphicsCaret {
    /// Cursor cell column, 0-based.
    pub(crate) x: u16,
    /// Cursor cell row, 0-based.
    pub(crate) y: u16,
}

impl GraphicsCaret {
    /// Build the escape sequence that deletes any previous caret placement and draws
    /// an RGBA bar scaled into this cell.
    pub(crate) fn escape(self) -> String {
        let rgba = caret_rgba();
        let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
        format!(
            "{}\x1b[{};{}H\x1b_Ga=T,i={},f=32,s={},v={},c=1,r=1,z=1,C=1,q=2;{}\x1b\\",
            delete_graphics_caret(),
            self.y + 1,
            self.x + 1,
            CARET_IMAGE_ID,
            CARET_IMAGE_WIDTH,
            CARET_IMAGE_HEIGHT,
            payload
        )
    }
}

/// Delete the graphical editor caret image, leaving unrelated Kitty images alone.
pub(crate) fn delete_graphics_caret() -> String {
    format!("\x1b_Ga=d,d=I,i={CARET_IMAGE_ID}\x1b\\")
}

fn caret_rgba() -> Vec<u8> {
    let px = usize::from(CARET_IMAGE_WIDTH) * usize::from(CARET_IMAGE_HEIGHT);
    let mut rgba = Vec::with_capacity(px * 4);
    for _y in 0..CARET_IMAGE_HEIGHT {
        for x in 0..CARET_IMAGE_WIDTH {
            if x < CARET_BAR_WIDTH {
                rgba.extend_from_slice(&[235, 239, 245, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_caret_escape_uses_a_cell_scaled_image_and_specific_delete() {
        let esc = GraphicsCaret { x: 4, y: 2 }.escape();
        assert!(esc.contains("a=d,d=I"));
        assert!(esc.contains("\x1b[3;5H"));
        assert!(esc.contains("s=32"));
        assert!(esc.contains("v=64"));
        assert!(esc.contains("c=1"));
        assert!(esc.contains("r=1"));
        assert!(esc.contains("z=1"));
    }

    #[test]
    fn graphics_caret_image_is_a_visible_bar_with_transparent_fill() {
        let rgba = caret_rgba();
        assert_eq!(
            rgba.len(),
            usize::from(CARET_IMAGE_WIDTH) * usize::from(CARET_IMAGE_HEIGHT) * 4
        );
        assert_eq!(&rgba[..4], &[235, 239, 245, 255]);
        let transparent = usize::from(CARET_BAR_WIDTH) * 4;
        assert_eq!(&rgba[transparent..transparent + 4], &[0, 0, 0, 0]);
    }
}

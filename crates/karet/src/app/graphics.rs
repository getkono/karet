//! Kitty image output and graphical caret state.

use base64::Engine as _;

use super::*;

/// The Kitty id of the image or PDF page an image tab shows: outside the 24 bits the
/// markdown preview's images use, so deleting one never deletes the other.
#[cfg(any(feature = "images", feature = "pdf"))]
const TAB_IMAGE_ID: u32 = 0x4b41_5202;

/// Stable Kitty image id used only for the graphical editor caret.
const CARET_IMAGE_ID: u32 = 0x4b41_5201;
const CARET_IMAGE_WIDTH: u16 = 32;
const CARET_IMAGE_HEIGHT: u16 = 64;
const CARET_BAR_WIDTH: u16 = 3;

/// One Kitty graphical caret placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GraphicsCaret {
    /// Cursor cell column, 0-based.
    pub(super) x: u16,
    /// Cursor cell row, 0-based.
    pub(super) y: u16,
}

impl GraphicsCaret {
    /// Build the escape sequences that delete any previous caret placement and draw
    /// an RGBA bar scaled into this cell, its payload chunked to the Kitty limit.
    fn escape(self) -> String {
        let payload = base64::engine::general_purpose::STANDARD.encode(caret_rgba());
        let keys = format!(
            "a=T,i={CARET_IMAGE_ID},f=32,s={CARET_IMAGE_WIDTH},v={CARET_IMAGE_HEIGHT},c=1,r=1,z=1,C=1,q=2"
        );
        format!(
            "{}\x1b[{};{}H{}",
            delete_graphics_caret(),
            self.y + 1,
            self.x + 1,
            image::kitty_chunked(&payload, &keys, ",q=2")
        )
    }
}

/// Delete the graphical editor caret image, leaving unrelated Kitty images alone.
fn delete_graphics_caret() -> String {
    image::kitty_delete_image(CARET_IMAGE_ID)
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

impl App {
    /// Tell the markdown preview how to paint images this frame: as Kitty unicode
    /// placeholders when the terminal draws them, on the cell size the terminal
    /// reports now (a font or zoom change re-sizes every image).
    pub(super) fn sync_preview_graphics(&mut self) {
        if let Ok(size) = crossterm::terminal::window_size()
            && size.width > 0
            && size.height > 0
            && size.columns > 0
            && size.rows > 0
        {
            self.caps.cell_px = (
                u32::from(size.width) / u32::from(size.columns),
                u32::from(size.height) / u32::from(size.rows),
            );
        }
        let kitty = self.caps.graphics == GraphicsProtocol::Kitty && self.caps.placeholders;
        self.preview_images.configure(kitty, self.caps.cell_px);
    }

    /// Delete every image the preview transmitted, before the editor exits.
    pub(super) fn release_preview_graphics(&self) {
        let output = self.preview_images.teardown();
        if !output.is_empty() {
            let mut stdout = io::stdout();
            let _ = write!(stdout, "{output}");
            let _ = stdout.flush();
        }
    }

    /// Transmit or clear the active tab's Kitty image after a frame is drawn.
    pub(super) fn flush_graphics(&mut self) {
        if self.caps.graphics != GraphicsProtocol::Kitty {
            return;
        }
        let mut stdout = io::stdout();
        // The preview's images: transmitted once, placed per cell box, deleted when
        // their pixels are dropped. Their placeholder cells were drawn with the frame.
        let preview = self.preview_images.take_output();
        if !preview.is_empty() {
            let _ = write!(stdout, "{preview}");
            let _ = stdout.flush();
        }
        // Transmitting a rasterized image/PDF page needs a raster branch compiled in
        // (`images`/`pdf`); the graphical text caret below is independent of it.
        #[cfg(any(feature = "images", feature = "pdf"))]
        {
            // The image, if any, belongs to the focused pane's active tab (keyed by
            // its stable ViewId so a focus switch re-transmits correctly). Documents
            // also key on the current page so paging re-transmits under an unchanged
            // ViewId.
            let current = self.tabs.get(self.active).map(|t| t.view);
            let current_page = match self.tabs.get(self.active).map(|t| &t.kind) {
                #[cfg(feature = "pdf")]
                Some(TabKind::Document { page, .. }) => *page,
                _ => 0,
            };
            // The pixels live directly on an image tab, or in a document's page cache.
            let image = match self.tabs.get(self.active).map(|t| &t.kind) {
                #[cfg(feature = "images")]
                Some(TabKind::Image { image, .. }) => Some(image),
                #[cfg(feature = "pdf")]
                Some(TabKind::Document {
                    rendered: Some((_, image)),
                    ..
                }) => Some(image),
                _ => None,
            };
            match self.last_frame.image_area() {
                Some(area) if self.shown_image != current || self.shown_page != current_page => {
                    let _ = write!(stdout, "{}", image::kitty_delete_image(TAB_IMAGE_ID));
                    let _ = write!(stdout, "\x1b[{};{}H", area.y + 1, area.x + 1);
                    if let Some(image) = image {
                        let _ = write!(
                            stdout,
                            "{}",
                            image.kitty_escape_with_id(TAB_IMAGE_ID, area.width, area.height)
                        );
                    }
                    let _ = stdout.flush();
                    self.shown_image = current;
                    self.shown_page = current_page;
                },
                None if self.shown_image.is_some() => {
                    let _ = write!(stdout, "{}", image::kitty_delete_image(TAB_IMAGE_ID));
                    let _ = stdout.flush();
                    self.shown_image = None;
                },
                _ => {},
            }
        }

        let caret = self.active_graphics_caret();
        match (caret, self.shown_graphics_caret) {
            (Some(next), shown) if shown != Some(next) => {
                let _ = write!(stdout, "{}", next.escape());
                let _ = stdout.flush();
                self.shown_graphics_caret = Some(next);
            },
            (None, Some(_)) => {
                let _ = write!(stdout, "{}", delete_graphics_caret());
                let _ = stdout.flush();
                self.shown_graphics_caret = None;
            },
            _ => {},
        }
    }

    pub(super) fn active_graphics_caret(&self) -> Option<GraphicsCaret> {
        if !self.graphics_caret_visible(Instant::now()) {
            return None;
        }
        self.active_graphics_caret_position()
    }

    pub(super) fn active_graphics_caret_position(&self) -> Option<GraphicsCaret> {
        if !self.graphical_cursor_enabled() || self.focus != Focus::Editor {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let fold_lines = resolve_folds(folds, folded);
        let (x, y) =
            tab.editor
                .primary_caret_cell(self.last_frame.editor_rect(), buffer, &fold_lines)?;
        Some(GraphicsCaret { x, y })
    }

    pub(super) fn graphics_caret_visible(&self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.graphics_caret_blink_epoch);
        let phase = elapsed.as_millis() / GRAPHICS_CARET_BLINK_INTERVAL.as_millis();
        phase.is_multiple_of(2)
    }

    pub(super) fn graphics_caret_next_wake(&self, now: Instant) -> Option<Duration> {
        self.active_graphics_caret_position()?;
        let elapsed = now.saturating_duration_since(self.graphics_caret_blink_epoch);
        let interval_ms = GRAPHICS_CARET_BLINK_INTERVAL.as_millis();
        let elapsed_ms = elapsed.as_millis();
        let remaining_ms = interval_ms - (elapsed_ms % interval_ms);
        Some(Duration::from_millis(remaining_ms as u64))
    }

    pub(super) fn reset_graphics_caret_blink(&mut self) {
        self.graphics_caret_blink_epoch = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphics_caret_escape_uses_a_cell_scaled_image_and_specific_delete() {
        let esc = GraphicsCaret { x: 4, y: 2 }.escape();
        // The previous caret goes first, by its id alone and with replies silenced.
        assert!(esc.starts_with(&image::kitty_delete_image(CARET_IMAGE_ID)));
        assert!(esc.contains(&format!("a=d,d=I,i={CARET_IMAGE_ID},q=2")));
        assert!(esc.contains("\x1b[3;5H"));
        assert!(esc.contains("s=32"));
        assert!(esc.contains("v=64"));
        assert!(esc.contains("c=1"));
        assert!(esc.contains("r=1"));
        assert!(esc.contains("z=1"));
    }

    #[test]
    fn graphics_caret_transmit_is_chunked_within_the_kitty_payload_limit() {
        let delete = image::kitty_delete_image(CARET_IMAGE_ID);
        let esc = GraphicsCaret { x: 0, y: 0 }.escape();
        let transmit = esc
            .strip_prefix(&delete)
            .and_then(|rest| rest.strip_prefix("\x1b[1;1H"))
            .unwrap_or_default();
        let mut keys = Vec::new();
        let mut payload = String::new();
        for escape in transmit.split_terminator("\x1b\\") {
            let body = escape.strip_prefix("\x1b_G").unwrap_or_default();
            let (head, data) = body.split_once(';').unwrap_or_default();
            assert!(data.len() <= 4096, "a chunk carries {} bytes", data.len());
            keys.push(head);
            payload.push_str(data);
        }
        // 8192 RGBA bytes are 10924 base64 bytes: three chunks.
        assert_eq!(keys.len(), 3);
        assert_eq!(
            keys[0],
            format!("a=T,i={CARET_IMAGE_ID},f=32,s=32,v=64,c=1,r=1,z=1,C=1,q=2,m=1")
        );
        assert_eq!(keys[1], "m=1,q=2");
        assert_eq!(keys[2], "m=0,q=2");
        let rgba = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap_or_default();
        assert_eq!(rgba, caret_rgba());
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

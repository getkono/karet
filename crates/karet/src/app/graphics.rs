//! Kitty image output and graphical caret state.

use super::*;

/// The Kitty id of the image or PDF page an image tab shows: outside the 24 bits the
/// markdown preview's images use, so deleting one never deletes the other.
#[cfg(any(feature = "images", feature = "pdf"))]
const TAB_IMAGE_ID: u32 = 0x4b41_5202;

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
            match self.image_area {
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
                let _ = write!(stdout, "{}", compat::delete_graphics_caret());
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
        let (x, y) = tab
            .editor
            .primary_caret_cell(self.editor_rect, buffer, &fold_lines)?;
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

//! Reserving rows for the images a paragraph holds on its own.

#[cfg(test)]
mod tests;

use super::ImageSlice;
use super::WrappedLine;
use super::image_chip;
use super::prefix_width;
use super::prefixed_line;
use super::space;
use super::wrap_runs;
use crate::ImageRef;
use crate::ImageSizer;
use crate::Inline;
use crate::TextSpan;

/// The pixel width of one terminal cell assumed when sizing an image. Cells are about
/// twice as tall as wide; the ratio is what matters, since a half-block row paints two
/// square pixels per cell.
const CELL_PX_WIDTH: u64 = 8;
/// The pixel height of one terminal cell assumed when sizing an image.
const CELL_PX_HEIGHT: u64 = 16;
/// The most lines one image may take, so a tall screenshot cannot swallow the view.
pub(crate) const MAX_IMAGE_ROWS: u16 = 20;

/// Wrap a paragraph made only of images (and the whitespace between them), giving each
/// image `sizer` sizes rows of its own and gathering the rest into lines of chips.
///
/// Returns `false`, writing nothing, for any other paragraph — and for one no image of
/// which is sized, whose chips then flow as ordinary text (a row of badges stays a row).
pub(super) fn wrap_image_paragraph(
    content: &[Inline],
    inner: usize,
    prefix: &[TextSpan],
    sizer: &dyn ImageSizer,
    out: &mut Vec<WrappedLine>,
) -> bool {
    let mut images = Vec::new();
    for inline in content {
        match inline {
            Inline::Image(image) => images.push(image),
            Inline::Text(text) if text.trim().is_empty() => {},
            _ => return false,
        }
    }
    let sized: Vec<(&ImageRef, Option<(u16, u16)>)> = images
        .into_iter()
        .map(|image| {
            let cells = sizer
                .dimensions(image)
                .and_then(|native| cell_box(native, (image.width, image.height), inner));
            (image, cells)
        })
        .collect();
    if sized.iter().all(|(_, cells)| cells.is_none()) {
        return false;
    }

    let col = u16::try_from(prefix_width(prefix)).unwrap_or(u16::MAX);
    let mut chips: Vec<TextSpan> = Vec::new();
    for (image, cells) in sized {
        let Some((cols, rows)) = cells else {
            if !chips.is_empty() {
                chips.push(space(1));
            }
            chips.push(image_chip(image));
            continue;
        };
        if !chips.is_empty() {
            wrap_runs(&std::mem::take(&mut chips), inner, prefix, out);
        }
        for row in 0..rows {
            let mut line = prefixed_line(prefix, Vec::new());
            line.image = Some(ImageSlice {
                src: image.src.clone(),
                link: image.link.clone(),
                alt: image.alt.clone(),
                row,
                rows,
                cols,
                col,
            });
            out.push(line);
        }
    }
    if !chips.is_empty() {
        wrap_runs(&chips, inner, prefix, out);
    }
    true
}

/// The `(columns, rows)` an image of `native` pixel size takes, given the author's
/// `hints` (HTML `width`/`height`, in CSS pixels) and at most `max_cols` columns.
///
/// Never larger than the image's native size, never wider than `max_cols`, never taller
/// than [`MAX_IMAGE_ROWS`], and aspect-preserving throughout except where both hints
/// are given (the author's call). `None` for an image with no area, or no room.
pub(crate) fn cell_box(
    native: (u32, u32),
    hints: (Option<u32>, Option<u32>),
    max_cols: usize,
) -> Option<(u16, u16)> {
    let (native_w, native_h) = (u64::from(native.0), u64::from(native.1));
    let max_cols = u64::try_from(max_cols)
        .unwrap_or(u64::MAX)
        .min(u64::from(u16::MAX));
    if native_w == 0 || native_h == 0 || max_cols == 0 {
        return None;
    }
    let (w, h) = match (hints.0.map(u64::from), hints.1.map(u64::from)) {
        (Some(w), Some(h)) => (w.min(native_w), h.min(native_h)),
        (Some(w), None) => {
            let w = w.min(native_w);
            (w, native_h * w / native_w)
        },
        (None, Some(h)) => {
            let h = h.min(native_h);
            (native_w * h / native_h, h)
        },
        (None, None) => (native_w, native_h),
    };
    let (w, h) = (w.max(1), h.max(1));
    let mut cols = w.div_ceil(CELL_PX_WIDTH);
    let mut rows = h.div_ceil(CELL_PX_HEIGHT);
    if cols > max_cols {
        cols = max_cols;
        rows = (cols * CELL_PX_WIDTH * h).div_ceil(w * CELL_PX_HEIGHT);
    }
    let max_rows = u64::from(MAX_IMAGE_ROWS);
    if rows > max_rows {
        rows = max_rows;
        cols = (rows * CELL_PX_HEIGHT * w / (h * CELL_PX_WIDTH)).clamp(1, max_cols);
    }
    Some((
        u16::try_from(cols.max(1)).unwrap_or(u16::MAX),
        u16::try_from(rows.max(1)).unwrap_or(u16::MAX),
    ))
}

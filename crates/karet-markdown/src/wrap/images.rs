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

/// The most rows or columns one image may take: the 297 row/column diacritics of the
/// Kitty unicode-placeholder protocol, so every row of a preview image stays
/// addressable. Only an image thousands of pixels tall at a small font reaches it.
pub(crate) const MAX_IMAGE_CELLS: u16 = 297;

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
            let cells = sizer.dimensions(image).and_then(|native| {
                cell_box(
                    native,
                    (image.width, image.height),
                    inner,
                    sizer.cell_pixels(),
                )
            });
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
            chips.push(image_chip(image, sizer.chip_glyph()));
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

/// The `(columns, rows)` an image of `native` pixel size takes on cells of `cell_px`
/// pixels, given the author's `hints` (HTML `width`/`height`, in CSS pixels) and at
/// most `max_cols` columns.
///
/// An image takes its native size — one image pixel per screen pixel — and shrinks
/// only to fit `max_cols` (or, past any real image, [`MAX_IMAGE_CELLS`]), keeping its
/// aspect throughout except where both hints are given (the author's call). `None`
/// for an image with no area, or no room.
pub(crate) fn cell_box(
    native: (u32, u32),
    hints: (Option<u32>, Option<u32>),
    max_cols: usize,
    cell_px: (u32, u32),
) -> Option<(u16, u16)> {
    let (cell_w, cell_h) = (u64::from(cell_px.0.max(1)), u64::from(cell_px.1.max(1)));
    let (native_w, native_h) = (u64::from(native.0), u64::from(native.1));
    let max_cols = u64::try_from(max_cols)
        .unwrap_or(u64::MAX)
        .min(u64::from(MAX_IMAGE_CELLS));
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
    let mut cols = w.div_ceil(cell_w);
    let mut rows = h.div_ceil(cell_h);
    if cols > max_cols {
        cols = max_cols;
        rows = (u128::from(cols * cell_w) * u128::from(h))
            .div_ceil(u128::from(w) * u128::from(cell_h))
            .try_into()
            .unwrap_or(u64::MAX);
    }
    let max_rows = u64::from(MAX_IMAGE_CELLS);
    if rows > max_rows {
        rows = max_rows;
        cols = u64::try_from(
            u128::from(rows * cell_h) * u128::from(w) / (u128::from(h) * u128::from(cell_w)),
        )
        .unwrap_or(u64::MAX)
        .clamp(1, max_cols);
    }
    Some((
        u16::try_from(cols.max(1)).unwrap_or(u16::MAX),
        u16::try_from(rows.max(1)).unwrap_or(u16::MAX),
    ))
}

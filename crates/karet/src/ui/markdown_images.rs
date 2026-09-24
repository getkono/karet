//! Painting the images a markdown preview reserved rows for.
//!
//! The wrapped document carries an [`ImageSlice`] on each row an image occupies; the
//! pixels come from the app's [`PreviewImages`] cache as truecolor halfblocks, so an
//! image scrolled half out of view paints just its visible rows. (Kitty graphics are
//! not used here: a placement cannot be clipped to a scrolling pane's rows.)

use std::path::Path;

use karet_markdown::ImageSlice;
use karet_markdown::WrappedDocument;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::preview_images::PreviewImages;

/// What a markdown preview renders beyond text.
#[derive(Clone, Copy)]
pub(super) struct PreviewEnv<'a> {
    /// Fence languages rendered as mermaid diagrams (`None` = disabled or compiled out).
    pub(super) mermaid: Option<&'a [String]>,
    /// The local images the preview may paint.
    pub(super) images: &'a PreviewImages,
}

/// Where a preview's images come from: the cache, and the document they are relative to.
// A lean build (no `images`) paints nothing, so it never reads the fields; the caller
// still builds the one value, keeping the draw path free of feature gates.
#[cfg_attr(not(feature = "images"), allow(dead_code))]
pub(super) struct Source<'a> {
    pub(super) images: &'a PreviewImages,
    pub(super) source: &'a Path,
    pub(super) root: &'a Path,
}

/// A run of consecutive visible rows showing one image.
struct Run<'a> {
    slice: &'a ImageSlice,
    /// The screen row of the run's first line.
    y: u16,
    /// How many rows are visible.
    height: u16,
}

/// The image runs visible in `area` with the document scrolled to `scroll`.
fn visible_runs(wrapped: &WrappedDocument, area: Rect, scroll: u16) -> Vec<Run<'_>> {
    let mut runs: Vec<Run<'_>> = Vec::new();
    let lines = wrapped
        .lines
        .iter()
        .skip(usize::from(scroll))
        .take(usize::from(area.height));
    for (offset, line) in (0u16..).zip(lines) {
        let Some(slice) = &line.image else {
            continue;
        };
        let y = area.y.saturating_add(offset);
        if let Some(run) = runs.last_mut()
            && same_image(run.slice, slice)
            && run.y.saturating_add(run.height) == y
            && run.slice.row.saturating_add(run.height) == slice.row
        {
            run.height += 1;
            continue;
        }
        runs.push(Run {
            slice,
            y,
            height: 1,
        });
    }
    runs
}

/// Whether two slices belong to one placed image.
fn same_image(a: &ImageSlice, b: &ImageSlice) -> bool {
    a.src == b.src && a.rows == b.rows && a.cols == b.cols && a.col == b.col
}

/// The screen rect a run paints into, clipped to `area`; `None` when nothing shows.
fn run_rect(run: &Run<'_>, area: Rect) -> Option<Rect> {
    let x = area.x.saturating_add(run.slice.col);
    let width = run.slice.cols.min(area.right().saturating_sub(x));
    (width > 0).then(|| Rect::new(x, run.y, width, run.height))
}

/// Paint every visible image: pixels once decoded, a muted placeholder once a decode
/// has been pending past the reveal delay, and nothing before that.
#[cfg(feature = "images")]
pub(super) fn paint(
    f: &mut Frame,
    theme: &karet_theme::Theme,
    wrapped: &WrappedDocument,
    area: Rect,
    scroll: u16,
    from: Source<'_>,
) {
    use karet_core::ThemeRole;
    use ratatui::text::Line;
    use ratatui::text::Span;

    use crate::preview_images::Lookup;

    for run in visible_runs(wrapped, area, scroll) {
        let Some(rect) = run_rect(&run, area) else {
            continue;
        };
        let slice = run.slice;
        match from.images.lookup(from.source, from.root, &slice.src) {
            Lookup::Ready(image) => image.render_halfblocks_rows(
                slice.cols,
                slice.rows,
                slice.row,
                rect,
                f.buffer_mut(),
            ),
            Lookup::Loading(pending) if pending.visible() => {
                let label = if slice.alt.is_empty() {
                    "image"
                } else {
                    slice.alt.as_str()
                };
                let line = Line::from(Span::styled(
                    format!("🖼 {label}"),
                    theme.style(ThemeRole::Muted),
                ));
                // The label may be wider than the image it stands in for.
                let width = area.right().saturating_sub(rect.x);
                f.buffer_mut().set_line(rect.x, rect.y, &line, width);
            },
            Lookup::Loading(_) | Lookup::Missing => {},
        }
    }
}

/// A lean build reserves no image rows, so there is nothing to paint.
#[cfg(not(feature = "images"))]
pub(super) fn paint(
    _f: &mut Frame,
    _theme: &karet_theme::Theme,
    _wrapped: &WrappedDocument,
    _area: Rect,
    _scroll: u16,
    _from: Source<'_>,
) {
}

/// The click targets of the visible images, one per visible row: an image activates
/// the link wrapping it, else the image itself.
pub(super) fn image_hits(
    wrapped: &WrappedDocument,
    area: Rect,
    scroll: u16,
) -> Vec<crate::app::MarkdownLinkHit> {
    let mut hits = Vec::new();
    for run in visible_runs(wrapped, area, scroll) {
        let Some(rect) = run_rect(&run, area) else {
            continue;
        };
        let target = run
            .slice
            .link
            .clone()
            .unwrap_or_else(|| run.slice.src.clone());
        for y in rect.y..rect.bottom() {
            hits.push(crate::app::MarkdownLinkHit {
                rect: Rect::new(rect.x, y, rect.width, 1),
                target: target.clone(),
            });
        }
    }
    hits
}

#[cfg(all(test, feature = "images"))]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::preview_images::tests::png;

    /// A tall image — one column by ten rows — below one line of text, laid out and
    /// decoded, in a scratch workspace.
    fn tall_image() -> (tempfile::TempDir, PreviewImages, WrappedDocument) {
        let dir = tempfile::tempdir().expect("a scratch workspace");
        let _ = std::fs::write(dir.path().join("tall.png"), png(8, 160, [90, 90, 90]));
        let images = PreviewImages::default();
        let source = dir.path().join("README.md");
        let wrapped = karet_markdown::parse("top\n\n[![tall](tall.png)](https://x)\n")
            .wrap_with(20, &images.sizer(&source, dir.path()));
        images.settle();
        (dir, images, wrapped)
    }

    /// Paint `wrapped` scrolled to `scroll` into a 20×6 area at (2, 1); count the
    /// halfblock cells.
    fn painted(
        dir: &Path,
        images: &PreviewImages,
        wrapped: &WrappedDocument,
        scroll: u16,
    ) -> usize {
        let Ok(mut terminal) = Terminal::new(TestBackend::new(30, 10));
        let theme = karet_theme::Theme::default();
        let source = dir.join("README.md");
        let area = Rect::new(2, 1, 20, 6);
        let drawn = terminal.draw(|f| {
            paint(
                f,
                &theme,
                wrapped,
                area,
                scroll,
                Source {
                    images,
                    source: &source,
                    root: dir,
                },
            );
        });
        drawn.map_or(0, |completed| {
            completed
                .buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "▀")
                .count()
        })
    }

    #[test]
    fn an_image_scrolled_half_out_of_view_paints_only_its_visible_rows() {
        let (dir, images, wrapped) = tall_image();
        // Lines: `top`, a blank, then the image's ten rows on lines 2..12.
        assert_eq!(wrapped.lines.len(), 12);
        assert_eq!(
            painted(dir.path(), &images, &wrapped, 0),
            4,
            "rows 0..4 fit"
        );
        assert_eq!(
            painted(dir.path(), &images, &wrapped, 7),
            5,
            "rows 5..10 remain"
        );
        assert_eq!(
            painted(dir.path(), &images, &wrapped, 12),
            0,
            "scrolled past"
        );
    }

    #[test]
    fn image_hits_follow_the_visible_rows_and_the_enclosing_link() {
        let (_dir, _images, wrapped) = tall_image();
        let area = Rect::new(2, 1, 20, 6);
        let hits = image_hits(&wrapped, area, 7);
        assert_eq!(hits.len(), 5);
        assert!(hits.iter().all(|hit| hit.target == "https://x"));
        assert_eq!(
            hits.first().map(|hit| hit.rect),
            Some(Rect::new(2, 1, 1, 1))
        );
        assert!(image_hits(&wrapped, area, 12).is_empty());
    }

    #[test]
    fn an_image_wider_than_the_area_is_clipped_to_it() {
        let dir = tempfile::tempdir().expect("a scratch workspace");
        let _ = std::fs::write(dir.path().join("wide.png"), png(160, 16, [1, 2, 3]));
        let images = PreviewImages::default();
        let source = dir.path().join("README.md");
        // Wrapped for 20 columns, painted into 5: the rest is cut, not spilled.
        let wrapped = karet_markdown::parse("![w](wide.png)\n")
            .wrap_with(20, &images.sizer(&source, dir.path()));
        images.settle();
        let hits = image_hits(&wrapped, Rect::new(0, 0, 5, 3), 0);
        assert!(hits.iter().all(|hit| hit.rect.right() <= 5));
    }
}

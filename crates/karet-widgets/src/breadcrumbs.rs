//! The pane breadcrumb: a file path's components joined by `›`, with clickable
//! per-segment hit regions.
//!
//! Painting and hit-region math live here; what a click on a segment *does*
//! (reveal in an explorer, open a picker, …) is the consumer's choice via the
//! returned [`BreadcrumbHit`]s.

use std::path::Path;
use std::path::PathBuf;

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::glyph::UiIcon;

/// The separator drawn between breadcrumb segments.
pub const BREADCRUMB_SEP: &str = "  \u{203a}  ";

/// A clickable breadcrumb segment recorded during the last frame: its column
/// span on the breadcrumb row and the path prefix it resolves to (always within
/// the consumer's `root` — segments above the root are never recorded).
#[derive(Clone)]
pub struct BreadcrumbHit {
    /// First column of the segment (inclusive), in screen coordinates.
    pub start: u16,
    /// One past the last column of the segment (exclusive).
    pub end: u16,
    /// The absolute path up to (and including) this segment's component.
    pub path: PathBuf,
}

/// The breadcrumb for one pane's active file.
pub struct Breadcrumbs<'a> {
    /// The file the breadcrumb describes.
    pub path: &'a Path,
    /// Segments resolving above this root are painted but not clickable.
    pub root: &'a Path,
    /// Whether the file is a symlink (appends the symlink glyph to the name).
    pub is_symlink: bool,
    /// The icon style resolving the symlink glyph.
    pub icon_style: IconStyle,
}

impl Breadcrumbs<'_> {
    /// Draw the breadcrumb into `area` and return the clickable segment
    /// regions. Segments past the pane's right edge are clipped.
    pub fn draw(&self, f: &mut Frame, theme: &Theme, area: Rect) -> Vec<BreadcrumbHit> {
        let mut components: Vec<String> = self
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if self.is_symlink
            && let Some(last) = components.last_mut()
        {
            last.push(' ');
            last.push(UiIcon::Symlink.glyph(self.icon_style));
        }
        let crumbs = components.join(BREADCRUMB_SEP);
        let style = theme.style(ThemeRole::LineNumberActive);
        f.render_widget(Paragraph::new(Line::styled(crumbs, style)), area);

        let mut hits = Vec::new();
        let mut prefix = PathBuf::new();
        for (comp, (start, end)) in self
            .path
            .components()
            .zip(breadcrumb_segment_spans(&components))
        {
            prefix.push(comp);
            if start >= area.width {
                break;
            }
            let end = end.min(area.width);
            // A segment resolving above the root cannot be revealed: skip it.
            if end > start && prefix.starts_with(self.root) {
                hits.push(BreadcrumbHit {
                    start: area.x.saturating_add(start),
                    end: area.x.saturating_add(end),
                    path: prefix.clone(),
                });
            }
        }
        hits
    }
}

/// The column span (start inclusive, end exclusive) of each of `components`
/// when joined by [`BREADCRUMB_SEP`], relative to the breadcrumb's left edge.
/// Uses terminal display width (wide-char aware), matching how the joined line
/// paints. Separator gaps belong to no segment. Pure, so it is unit-tested.
#[must_use]
pub fn breadcrumb_segment_spans(components: &[String]) -> Vec<(u16, u16)> {
    let width = |s: &str| u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX);
    let sep = width(BREADCRUMB_SEP);
    let mut spans = Vec::with_capacity(components.len());
    let mut x = 0u16;
    for (i, comp) in components.iter().enumerate() {
        if i > 0 {
            x = x.saturating_add(sep);
        }
        let end = x.saturating_add(width(comp));
        spans.push((x, end));
        x = end;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_spans_skip_the_separator_gaps() {
        let spans =
            breadcrumb_segment_spans(&["src".to_string(), "ui".to_string(), "mod.rs".to_string()]);
        let sep = u16::try_from(UnicodeWidthStr::width(BREADCRUMB_SEP)).unwrap_or(0);
        assert_eq!(spans[0], (0, 3));
        assert_eq!(spans[1].0, 3 + sep);
        assert_eq!(spans[1].1 - spans[1].0, 2);
        assert!(spans[2].0 > spans[1].1);
    }

    #[test]
    fn wide_characters_widen_their_segment() {
        let spans = breadcrumb_segment_spans(&["\u{65e5}\u{672c}".to_string()]);
        assert_eq!(spans[0], (0, 4));
    }
}

//! The pane breadcrumb: a file path's components, then the chain of symbols
//! enclosing the caret, joined by `›`, with clickable per-segment hit regions.
//!
//! Painting and hit-region math live here; what a click on a segment *does*
//! (reveal in an explorer, open a sibling picker, …) is the consumer's choice
//! via the returned [`BreadcrumbHit`]s. The symbol chain arrives as plain
//! labels, so this widget never learns where symbols come from.

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

/// What a breadcrumb segment stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BreadcrumbTarget {
    /// A path segment: the absolute path up to (and including) its component.
    Path(PathBuf),
    /// A symbol segment: its depth in the enclosing-symbol chain (0 = the
    /// outermost symbol), indexing [`Breadcrumbs::symbols`].
    Symbol(usize),
}

/// A clickable breadcrumb segment recorded during the last frame: its column
/// span on the breadcrumb row and what it stands for. A path segment always
/// resolves within the consumer's `root` — segments above the root are never
/// recorded.
#[derive(Clone, Debug)]
pub struct BreadcrumbHit {
    /// First column of the segment (inclusive), in screen coordinates.
    pub start: u16,
    /// One past the last column of the segment (exclusive).
    pub end: u16,
    /// The path prefix or symbol the segment stands for.
    pub target: BreadcrumbTarget,
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
    /// The names of the symbols enclosing the caret, outermost first, painted
    /// after the file name. Empty paints the path alone.
    pub symbols: &'a [String],
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
        let path_len = components.len();
        components.extend(self.symbols.iter().cloned());
        let crumbs = components.join(BREADCRUMB_SEP);
        let style = theme.style(ThemeRole::LineNumberActive);
        f.render_widget(Paragraph::new(Line::styled(crumbs, style)), area);

        let mut prefixes = self.path.components().scan(PathBuf::new(), |prefix, comp| {
            prefix.push(comp);
            Some(prefix.clone())
        });
        let mut hits = Vec::new();
        for (i, (start, end)) in breadcrumb_segment_spans(&components)
            .into_iter()
            .enumerate()
        {
            let target = if i < path_len {
                // A segment resolving above the root cannot be revealed: skip it.
                prefixes
                    .next()
                    .filter(|prefix| prefix.starts_with(self.root))
                    .map(BreadcrumbTarget::Path)
            } else {
                Some(BreadcrumbTarget::Symbol(i - path_len))
            };
            if start >= area.width {
                break;
            }
            let end = end.min(area.width);
            if let Some(target) = target.filter(|_| end > start) {
                hits.push(BreadcrumbHit {
                    start: area.x.saturating_add(start),
                    end: area.x.saturating_add(end),
                    target,
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

    /// Draw `crumbs` into a one-row buffer `width` wide: the recorded hits and
    /// the painted row.
    fn draw(crumbs: &Breadcrumbs<'_>, width: u16) -> (Vec<BreadcrumbHit>, String) {
        let mut hits = Vec::new();
        let theme = Theme::dark();
        let painted = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 1))
            .and_then(|mut terminal| {
                terminal.draw(|f| hits = crumbs.draw(f, &theme, f.area()))?;
                Ok(terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>())
            })
            .unwrap_or_default();
        (hits, painted)
    }

    fn crumbs<'a>(path: &'a Path, symbols: &'a [String]) -> Breadcrumbs<'a> {
        Breadcrumbs {
            path,
            root: Path::new("/w"),
            is_symlink: false,
            icon_style: IconStyle::Ascii,
            symbols,
        }
    }

    /// The display column `needle` starts at in `painted`.
    fn column_of(painted: &str, needle: &str) -> Option<u16> {
        let at = painted.find(needle)?;
        u16::try_from(UnicodeWidthStr::width(&painted[..at])).ok()
    }

    #[test]
    fn symbols_paint_after_the_file_name_and_record_their_depth() {
        let symbols = ["App".to_string(), "run".to_string()];
        let (hits, painted) = draw(&crumbs(Path::new("/w/src/app.rs"), &symbols), 80);
        let sep = BREADCRUMB_SEP;
        let expected = format!("/{sep}w{sep}src{sep}app.rs{sep}App{sep}run");
        assert_eq!(painted.trim_end(), expected);
        let targets: Vec<_> = hits.iter().map(|hit| hit.target.clone()).collect();
        assert_eq!(
            targets,
            [
                BreadcrumbTarget::Path(PathBuf::from("/w")),
                BreadcrumbTarget::Path(PathBuf::from("/w/src")),
                BreadcrumbTarget::Path(PathBuf::from("/w/src/app.rs")),
                BreadcrumbTarget::Symbol(0),
                BreadcrumbTarget::Symbol(1),
            ]
        );
        let run = column_of(&painted, "run");
        assert_eq!(
            hits.last().map(|hit| (hit.start, hit.end)),
            run.map(|start| (start, start + 3))
        );
    }

    #[test]
    fn a_symbol_reaching_past_the_right_edge_is_clipped_and_one_beyond_it_dropped() {
        let symbols = ["App".to_string(), "run".to_string()];
        let path = Path::new("/w/a.rs");
        let sep = BREADCRUMB_SEP;
        // Wide enough for the path and part of `App`, not for `run`.
        let shown = format!("/{sep}w{sep}a.rs{sep}Ap");
        let width = u16::try_from(UnicodeWidthStr::width(shown.as_str())).unwrap_or(u16::MAX);
        let (hits, _) = draw(&crumbs(path, &symbols), width);
        let last = hits.last();
        assert_eq!(
            last.map(|hit| &hit.target),
            Some(&BreadcrumbTarget::Symbol(0))
        );
        assert_eq!(last.map(|hit| hit.end), Some(width));
    }

    #[test]
    fn without_symbols_the_path_paints_alone() {
        let (hits, painted) = draw(&crumbs(Path::new("/w/a.rs"), &[]), 40);
        let sep = BREADCRUMB_SEP;
        assert_eq!(painted.trim_end(), format!("/{sep}w{sep}a.rs"));
        assert_eq!(
            hits.len(),
            2,
            "the root and the file; `/` is above the root"
        );
        assert!(
            hits.iter()
                .all(|hit| matches!(hit.target, BreadcrumbTarget::Path(_)))
        );
    }

    #[test]
    fn wide_characters_widen_their_segment() {
        let spans = breadcrumb_segment_spans(&["\u{65e5}\u{672c}".to_string()]);
        assert_eq!(spans[0], (0, 4));
    }
}

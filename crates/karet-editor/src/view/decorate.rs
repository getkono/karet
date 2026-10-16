//! Decoration lookup for the row painter: an interval index over the
//! decorations' line spans, so a frame costs its visible lines rather than
//! every decoration per cell, and the per-cell resolution of overlapping
//! decorations by [`priority`](Decoration::priority).

use std::cmp::Reverse;

use karet_core::DecorationColor;
use karet_core::TextModifiers;

use super::*;

/// An interval index over the line spans of a decoration slice.
///
/// The spans are sorted by first line and laid out as an implicit balanced
/// tree (each subrange's midpoint is its root), each node holding the
/// furthest last line beneath it. A query for one line visits only the
/// subtrees that can reach it: O(log n + k) for the k decorations it holds.
#[derive(Debug, Default)]
pub(crate) struct DecorationIndex {
    /// `(first line, last line, index into the slice)`, sorted.
    spans: Vec<(u32, u32, usize)>,
    /// The largest last line in the subtree rooted at each position.
    reach: Vec<u32>,
}

impl DecorationIndex {
    /// Index `decorations`. A decoration whose range ends before it starts
    /// covers no line, so it is left out.
    pub(crate) fn new(decorations: &[Decoration]) -> Self {
        let mut spans: Vec<(u32, u32, usize)> = decorations
            .iter()
            .enumerate()
            .filter(|(_, d)| d.range.start.line <= d.range.end.line)
            .map(|(i, d)| (d.range.start.line, d.range.end.line, i))
            .collect();
        spans.sort_unstable();
        let mut reach = vec![0; spans.len()];
        fill_reach(&spans, &mut reach, 0, spans.len());
        Self { spans, reach }
    }

    /// The indices of the decorations whose line span holds line `l`, in
    /// resolution order: highest priority first, ties in slice order.
    pub(crate) fn on_line(&self, decorations: &[Decoration], l: u32) -> Vec<usize> {
        let mut out = Vec::new();
        self.collect(l, 0, self.spans.len(), &mut out);
        out.sort_unstable_by_key(|&i| (Reverse(decorations.get(i).map_or(0, |d| d.priority)), i));
        out
    }

    fn collect(&self, l: u32, lo: usize, hi: usize, out: &mut Vec<usize>) {
        if lo >= hi {
            return;
        }
        let mid = lo + (hi - lo) / 2;
        let (Some(&(first, last, index)), Some(&reach)) =
            (self.spans.get(mid), self.reach.get(mid))
        else {
            return;
        };
        if reach < l {
            return;
        }
        self.collect(l, lo, mid, out);
        // Everything right of `mid` starts at or after it.
        if first > l {
            return;
        }
        if last >= l {
            out.push(index);
        }
        self.collect(l, mid + 1, hi, out);
    }
}

/// Fill `reach` for the subtree over `spans[lo..hi]`, returning its reach.
fn fill_reach(spans: &[(u32, u32, usize)], reach: &mut [u32], lo: usize, hi: usize) -> u32 {
    if lo >= hi {
        return 0;
    }
    let mid = lo + (hi - lo) / 2;
    let left = fill_reach(spans, reach, lo, mid);
    let right = fill_reach(spans, reach, mid + 1, hi);
    let own = spans.get(mid).map_or(0, |span| span.1);
    let furthest = own.max(left).max(right);
    if let Some(slot) = reach.get_mut(mid) {
        *slot = furthest;
    }
    furthest
}

/// Whether `d` reaches the text cell at column `col` of line `l`: the
/// text-level kinds by their range, a line background by its lines. Gutter
/// kinds never do.
fn covers_text(d: &Decoration, l: u32, col: u32) -> bool {
    match d.kind {
        DecorationKind::LineBackground => line_in_range(l, d.range),
        DecorationKind::TextBackground
        | DecorationKind::Underline
        | DecorationKind::ColorSwatch { .. }
        | DecorationKind::TextStyle => col_in_range(l, col, d.range),
        _ => false,
    }
}

/// The theme's color for a decoration foreground.
fn resolve_color(color: DecorationColor, theme: &Theme) -> Option<Rgba> {
    match color {
        DecorationColor::Role(role) => Some(theme.role(role)),
        DecorationColor::Token(token) => Some(theme.color(token)),
        _ => None,
    }
}

/// The ratatui modifiers for a decoration's text modifiers.
pub(super) fn to_modifier(modifiers: TextModifiers) -> Modifier {
    [
        (TextModifiers::BOLD, Modifier::BOLD),
        (TextModifiers::ITALIC, Modifier::ITALIC),
        (TextModifiers::STRIKETHROUGH, Modifier::CROSSED_OUT),
        (TextModifiers::DIM, Modifier::DIM),
    ]
    .into_iter()
    .filter(|(flag, _)| modifiers.contains(*flag))
    .fold(Modifier::empty(), |all, (_, modifier)| all | modifier)
}

/// The text styling the decorations of one line give one of its cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CellDecoration {
    /// The background, from the winning background or swatch.
    pub(super) bg: Option<Rgba>,
    /// The winning decoration foreground.
    pub(super) fg: Option<Rgba>,
    /// The contrasting foreground of a swatch that won the background.
    pub(super) swatch_fg: Option<Rgba>,
    /// The underline color.
    pub(super) underline: Option<Rgba>,
    /// The blended modifiers.
    pub(super) modifiers: Modifier,
}

impl Editor<'_> {
    /// The decorations on line `l`, in resolution order (see
    /// [`DecorationIndex::on_line`]).
    pub(super) fn line_decorations(&self, l: u32) -> Vec<&Decoration> {
        self.decoration_index
            .on_line(self.decorations, l)
            .into_iter()
            .filter_map(|i| self.decorations.get(i))
            .collect()
    }

    /// Resolve the styling `decorations` (line `l`'s, in resolution order)
    /// give column `col`.
    ///
    /// Each single-valued attribute comes from the first covering decoration
    /// that sets it, so the highest priority wins and ties go to the earlier
    /// one. The background is shared by swatches and background kinds, where a
    /// swatch wins a tie so its contrasting foreground stays with it.
    /// Modifiers are the union of every covering decoration's.
    pub(super) fn cell_decoration(
        decorations: &[&Decoration],
        (l, col): (u32, u32),
        theme: &Theme,
        default_fg: Rgba,
    ) -> CellDecoration {
        let mut cell = CellDecoration::default();
        let mut background: Option<(i32, bool, Rgba, Option<Rgba>)> = None;
        for d in decorations
            .iter()
            .copied()
            .filter(|d| covers_text(d, l, col))
        {
            cell.modifiers |= to_modifier(d.modifiers);
            if cell.fg.is_none() {
                cell.fg = d.fg.and_then(|color| resolve_color(color, theme));
            }
            let candidate = match (&d.kind, d.role) {
                (DecorationKind::ColorSwatch { rgba }, _) => {
                    let bg = Rgba::rgb(rgba[0], rgba[1], rgba[2]);
                    Some((true, bg, Some(contrasting(bg))))
                },
                (DecorationKind::LineBackground | DecorationKind::TextBackground, Some(role)) => {
                    Some((false, theme.role(role), None))
                },
                (DecorationKind::Underline, role) => {
                    if cell.underline.is_none() {
                        cell.underline = Some(role.map_or(default_fg, |role| theme.role(role)));
                    }
                    None
                },
                _ => None,
            };
            if let Some((swatch, bg, fg)) = candidate {
                let wins = match background {
                    None => true,
                    Some((priority, held_swatch, ..)) => {
                        priority == d.priority && swatch && !held_swatch
                    },
                };
                if wins {
                    background = Some((d.priority, swatch, bg, fg));
                }
            }
        }
        if let Some((_, _, bg, swatch_fg)) = background {
            cell.bg = Some(bg);
            cell.swatch_fg = swatch_fg;
        }
        cell
    }
}

/// Whichever of black or white contrasts better with `bg`, so a swatch stays
/// legible on any color; alpha is ignored (cells cannot blend).
fn contrasting(bg: Rgba) -> Rgba {
    let black = Rgba::rgb(0, 0, 0);
    let white = Rgba::rgb(255, 255, 255);
    if karet_theme::contrast_ratio(black, bg) >= karet_theme::contrast_ratio(white, bg) {
        black
    } else {
        white
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_lines(first: u32, last: u32) -> Decoration {
        Decoration::new(
            Range {
                start: LineCol::new(first, 0),
                end: LineCol::new(last, 1),
            },
            DecorationKind::LineBackground,
            None,
        )
    }

    /// The reference answer: every decoration whose line span holds `l`.
    fn scan(decorations: &[Decoration], l: u32) -> Vec<usize> {
        let mut out: Vec<usize> = (0..decorations.len())
            .filter(|&i| {
                decorations
                    .get(i)
                    .is_some_and(|d| line_in_range(l, d.range))
            })
            .collect();
        out.sort_by_key(|&i| (Reverse(decorations.get(i).map_or(0, |d| d.priority)), i));
        out
    }

    #[test]
    fn an_empty_index_holds_nothing() {
        let index = DecorationIndex::new(&[]);
        assert!(index.on_line(&[], 0).is_empty());
    }

    /// Nested, disjoint, touching and long-lived spans, in an order that is
    /// neither sorted by start nor by end, match a linear scan on every
    /// line, including lines past every span.
    #[test]
    fn a_query_matches_a_linear_scan_on_every_line() {
        let decorations = [
            on_lines(5, 9),
            on_lines(0, 100),
            on_lines(7, 7),
            on_lines(2, 3),
            on_lines(3, 4),
            on_lines(9, 9),
            on_lines(40, 60).with_priority(2),
            on_lines(1, 1).with_priority(-1),
            on_lines(8, 50),
            on_lines(60, 60),
        ];
        let index = DecorationIndex::new(&decorations);
        for l in 0..=110 {
            assert_eq!(
                index.on_line(&decorations, l),
                scan(&decorations, l),
                "line {l}"
            );
        }
    }

    /// A span whose last line is far beyond its neighbours sits in the left
    /// subtree; a subtree's reach must carry it up, or the query prunes it.
    #[test]
    fn a_long_span_early_in_the_order_is_still_found() {
        let decorations: Vec<Decoration> = std::iter::once(on_lines(0, 1_000))
            .chain((1..64).map(|l| on_lines(l, l)))
            .collect();
        let index = DecorationIndex::new(&decorations);
        assert_eq!(index.on_line(&decorations, 999), vec![0]);
        assert_eq!(index.on_line(&decorations, 30), vec![0, 30]);
        assert!(index.on_line(&decorations, 1_001).is_empty());
    }

    #[test]
    fn higher_priority_comes_first_and_ties_keep_slice_order() {
        let decorations = [
            on_lines(0, 0),
            on_lines(0, 0).with_priority(3),
            on_lines(0, 0),
            on_lines(0, 0).with_priority(3),
            on_lines(0, 0).with_priority(-2),
        ];
        let index = DecorationIndex::new(&decorations);
        assert_eq!(index.on_line(&decorations, 0), vec![1, 3, 0, 2, 4]);
    }

    /// A reversed range covers no line under the renderer's own test, so the
    /// index leaves it out rather than inventing a span for it.
    #[test]
    fn a_reversed_range_is_never_returned() {
        let reversed = Decoration::new(
            Range {
                start: LineCol::new(4, 0),
                end: LineCol::new(2, 0),
            },
            DecorationKind::LineBackground,
            None,
        );
        let decorations = [reversed, on_lines(3, 3)];
        let index = DecorationIndex::new(&decorations);
        assert_eq!(index.on_line(&decorations, 3), vec![1]);
    }

    #[test]
    fn every_modifier_maps_to_its_ratatui_flag() {
        assert_eq!(to_modifier(TextModifiers::NONE), Modifier::empty());
        assert_eq!(
            to_modifier(
                TextModifiers::BOLD
                    | TextModifiers::ITALIC
                    | TextModifiers::STRIKETHROUGH
                    | TextModifiers::DIM
            ),
            Modifier::BOLD | Modifier::ITALIC | Modifier::CROSSED_OUT | Modifier::DIM
        );
    }
}

//! The LSP completion popup.
//!
//! [`CompletionPopup`] renders `karet-core` [`CompletionItem`]s the backend
//! supplied over the event stream: one row per candidate with a kind glyph, the
//! full label, and a muted detail column. The popup fuzzy-filters through
//! [`karet_fuzzy::Matcher`] as the user keeps typing; with nothing typed the
//! candidates keep the server's intent (`sortText`, falling back to the label).
//!
//! Sizing policy ("the fields should be fully visible", issue #57): the popup
//! asks for enough width for its longest `label + detail` row via
//! [`CompletionPopup::desired_size`], clamped by the caller's available area.
//! When a row still cannot fit, the **label always wins** — it stays whole and
//! the detail is truncated with `…`. At most [`MAX_VISIBLE_ROWS`] rows are shown;
//! the selection scrolls the window.

use karet_core::CompletionItem;
use karet_core::CompletionKind;
use karet_core::ThemeRole;
use karet_core::TokenId;
use karet_fuzzy::Matcher;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::StatefulWidget;
use unicode_width::UnicodeWidthStr;

use crate::scroll::ScrollAxes;
use crate::scroll::ScrollExtent;
use crate::scroll::ScrollTrack;
use crate::scroll::ScrollbarStyles;
use crate::scroll::reserve_tracks;

/// The most candidate rows the popup shows at once; the rest scroll.
pub const MAX_VISIBLE_ROWS: u16 = 10;

/// Columns taken by the kind glyph and its trailing space.
const GLYPH_COLS: u16 = 2;

/// Columns of gap between the label and the detail column.
const DETAIL_GAP: u16 = 2;

/// A completion popup that fuzzy-filters [`CompletionItem`]s as you type.
pub struct CompletionPopup<'a> {
    /// The candidate items, supplied by the backend.
    pub items: &'a [CompletionItem],
    /// The matcher used for incremental filtering.
    pub matcher: &'a mut Matcher,
    /// The word prefix typed since the popup opened (empty = show everything).
    pub filter: &'a str,
    /// The theme resolving row colors and kind-glyph colors.
    pub theme: &'a Theme,
}

/// Selection and scroll state, kept by the caller across frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompletionState {
    /// The selected row, as an index into [`CompletionPopup::ranked`].
    pub selected: usize,
    /// The first visible row (maintained by the render).
    scroll: usize,
    /// The scrollbar track the last render painted, so the popup's bar can be
    /// grabbed like every other one. The popup lays itself out inside its own
    /// `render`, so this is the only way its caller learns where the bar ended up.
    track: Option<ScrollTrack>,
    /// Where the last render painted the popup, scrollbar included, so the caller
    /// can tell a pointer over the popup from one over the text beneath it.
    area: Rect,
    /// The rows the last render painted candidates into: [`area`](Self::area)
    /// less the scrollbar column.
    rows: Rect,
}

impl CompletionState {
    /// Where the last render painted the popup; empty once [`hide`](Self::hide)d.
    #[must_use]
    pub const fn area(&self) -> Rect {
        self.area
    }

    /// Forget where the popup was painted, for a frame that does not draw it.
    ///
    /// The render records its area and its scrollbar, but only a frame that
    /// renders can update them. Without this a popup that stopped showing (the
    /// caret scrolled out of view) would keep claiming the pointer at its last
    /// spot.
    pub fn hide(&mut self) {
        self.area = Rect::default();
        self.rows = Rect::default();
        self.track = None;
    }

    /// The candidate (an index into [`CompletionPopup::ranked`]) painted at the
    /// screen cell `(x, y)` by the last render, if a row is there. The scrollbar
    /// column is not a row.
    #[must_use]
    pub fn row_at(&self, x: u16, y: u16) -> Option<usize> {
        let rows = self.rows;
        let inside = x >= rows.x && x < rows.right() && y >= rows.y && y < rows.bottom();
        inside.then(|| self.scroll + usize::from(y - rows.y))
    }

    /// Scroll the window by `delta` rows over a `len`-row list, keeping it full
    /// and bringing the selection along when it would leave the window — a wheel
    /// notch over the popup.
    pub fn scroll_by(&mut self, delta: isize, len: usize) {
        let visible = usize::from(self.rows.height);
        if visible == 0 {
            return;
        }
        let max = len.saturating_sub(visible);
        let position = self.scroll.saturating_add_signed(delta).min(max);
        self.scroll_to(position, visible);
    }

    /// Move the selection down, wrapping past the end of a `len`-row list.
    pub fn select_next(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1) % len;
    }

    /// Move the selection up, wrapping past the start of a `len`-row list.
    pub fn select_prev(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(len - 1);
    }

    /// Reset selection and scroll (a fresh popup or a changed filter).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The scrollbar track the last render painted, if the list overflowed.
    #[must_use]
    pub const fn track(&self) -> Option<ScrollTrack> {
        self.track
    }

    /// Scroll so `position` is the first visible row of a `visible`-row window.
    ///
    /// The selection comes along when it would otherwise fall outside that window.
    /// The render pins the window to the selection, so a scroll written on its own
    /// would snap straight back on the next frame.
    pub fn scroll_to(&mut self, position: usize, visible: usize) {
        self.scroll = position;
        if visible == 0 {
            return;
        }
        let bottom = position.saturating_add(visible - 1);
        if self.selected < position {
            self.selected = position;
        } else if self.selected > bottom {
            self.selected = bottom;
        }
    }
}

/// A rank candidate: the text matched (filter text, else label) plus its index
/// back into the item slice, so duplicate labels stay distinguishable after
/// ranking.
struct Candidate<'a> {
    index: usize,
    label: &'a str,
}

impl AsRef<str> for Candidate<'_> {
    fn as_ref(&self) -> &str {
        self.label
    }
}

impl<'a> CompletionPopup<'a> {
    /// Build a popup over `items`, filtered by `filter`.
    #[must_use]
    pub fn new(
        items: &'a [CompletionItem],
        matcher: &'a mut Matcher,
        filter: &'a str,
        theme: &'a Theme,
    ) -> Self {
        Self {
            items,
            matcher,
            filter,
            theme,
        }
    }

    /// The candidate rows in display order, as indices into
    /// [`items`](Self::items): fuzzy-ranked by the filter when one is typed --
    /// matched against each item's filter text, falling back to its label --
    /// otherwise in the server's order (`sort_text`, falling back to the
    /// label). The caller uses the same list to resolve which item a selection
    /// accepts.
    #[must_use]
    pub fn ranked(&mut self) -> Vec<usize> {
        if self.filter.is_empty() {
            let mut order: Vec<usize> = (0..self.items.len()).collect();
            order.sort_by_key(|&i| {
                let item = &self.items[i];
                item.sort_text.as_deref().unwrap_or(&item.label)
            });
            return order;
        }
        let candidates: Vec<Candidate<'_>> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| Candidate {
                index,
                label: item.filter_key(),
            })
            .collect();
        self.matcher
            .rank(self.filter, &candidates)
            .into_iter()
            .map(|scored| scored.item.index)
            .collect()
    }

    /// The size this popup wants, clamped to `max` `(width, height)`: enough
    /// width for its widest `label + detail` row and one row per candidate up
    /// to [`MAX_VISIBLE_ROWS`]. `(0, 0)` when nothing matches the filter.
    #[must_use]
    pub fn desired_size(&mut self, max: (u16, u16)) -> (u16, u16) {
        let ranked = self.ranked();
        if ranked.is_empty() {
            return (0, 0);
        }
        let width = ranked
            .iter()
            .map(|&i| {
                let item = &self.items[i];
                let label = u16::try_from(item.label.width()).unwrap_or(u16::MAX);
                let detail = item.detail.as_deref().map_or(0, |d| {
                    DETAIL_GAP.saturating_add(u16::try_from(d.width()).unwrap_or(u16::MAX))
                });
                GLYPH_COLS.saturating_add(label).saturating_add(detail)
            })
            .max()
            .unwrap_or(0);
        let height = u16::try_from(ranked.len())
            .unwrap_or(u16::MAX)
            .min(MAX_VISIBLE_ROWS)
            .min(max.1);
        // Ask for the scrollbar's column too, or the track would eat the widest
        // label. This is the one place where a reservation may depend on the
        // content: the popup's rect is recomputed from this method every frame and
        // its width never feeds back into the ranking, so it cannot oscillate.
        let track = u16::from(shows_scrollbar(ranked.len(), height));
        (width.saturating_add(track).min(max.0), height)
    }
}

/// Whether a popup of `height` rows showing `count` candidates carries a
/// scrollbar. Shared by [`CompletionPopup::desired_size`] and the render pass so
/// the column asked for is exactly the column painted.
const fn shows_scrollbar(count: usize, height: u16) -> bool {
    count > height as usize && height > 2
}

/// The glyph and color marking a completion kind, VS Code-style: a single
/// lowercase letter colored like the token the item would become.
#[must_use]
fn kind_glyph(kind: CompletionKind) -> (char, Option<TokenId>) {
    match kind {
        CompletionKind::Method => ('m', Some(TokenId::FUNCTION)),
        CompletionKind::Function => ('f', Some(TokenId::FUNCTION)),
        CompletionKind::Field => ('F', Some(TokenId::VARIABLE)),
        CompletionKind::Variable => ('v', Some(TokenId::VARIABLE)),
        CompletionKind::Property => ('p', Some(TokenId::VARIABLE)),
        CompletionKind::Class => ('c', Some(TokenId::TYPE)),
        CompletionKind::Interface => ('i', Some(TokenId::TYPE)),
        CompletionKind::Struct => ('s', Some(TokenId::TYPE)),
        CompletionKind::Enum => ('e', Some(TokenId::TYPE)),
        CompletionKind::Module => ('M', Some(TokenId::TYPE)),
        CompletionKind::Keyword => ('k', Some(TokenId::KEYWORD)),
        CompletionKind::Snippet => ('S', Some(TokenId::STRING)),
        CompletionKind::Constant => ('C', Some(TokenId::CONSTANT)),
        // `Text` and any future kinds render as plain foreground text.
        _ => ('t', None),
    }
}

/// Fit `text` into `width` columns; when it does not fit, truncate and end
/// with `…` so the cut is visible.
fn truncate_to(text: &str, width: u16) -> String {
    crate::text::fit_end(text, usize::from(width))
}

impl StatefulWidget for CompletionPopup<'_> {
    type State = CompletionState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let ranked = self.ranked();
        if ranked.is_empty() {
            return;
        }
        state.area = area;
        let (area, tracks) = reserve_tracks(
            area,
            ScrollAxes {
                vertical: shows_scrollbar(ranked.len(), area.height),
                horizontal: false,
            },
        );
        state.rows = area;
        // Clamp the selection, then scroll the window to keep it visible.
        state.selected = state.selected.min(ranked.len() - 1);
        let visible = usize::from(area.height);
        if state.selected < state.scroll {
            state.scroll = state.selected;
        } else if state.selected >= state.scroll + visible {
            state.scroll = state.selected + 1 - visible;
        }
        state.scroll = state.scroll.min(ranked.len().saturating_sub(1));

        let background = self.theme.role(ThemeRole::Background).to_ratatui();
        let selection = self.theme.role(ThemeRole::Selection).to_ratatui();
        let foreground = self.theme.role(ThemeRole::Foreground).to_ratatui();
        let muted = self.theme.role(ThemeRole::Muted).to_ratatui();

        for (row, &item_index) in ranked.iter().skip(state.scroll).take(visible).enumerate() {
            let item = &self.items[item_index];
            let selected = state.scroll + row == state.selected;
            let row_bg = if selected { selection } else { background };
            let y = area
                .y
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));

            // Paint the whole row's background first.
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_style(Style::default().bg(row_bg));
                }
            }

            // Kind glyph column.
            let (glyph, token) = kind_glyph(item.kind);
            let glyph_fg = token.map_or(muted, |t| self.theme.color(t).to_ratatui());
            buf.set_string(
                area.x,
                y,
                glyph.to_string(),
                Style::default().fg(glyph_fg).bg(row_bg),
            );

            // The label: always whole (the popup was sized for it; if the
            // caller clamped harder, ratatui clips at the area edge).
            let mut label_style = Style::default().fg(foreground).bg(row_bg);
            if item.deprecated {
                label_style = label_style.add_modifier(Modifier::CROSSED_OUT);
            }
            if selected {
                label_style = label_style.add_modifier(Modifier::BOLD);
            }
            let label_x = area.x.saturating_add(GLYPH_COLS);
            let label_max = area.width.saturating_sub(GLYPH_COLS);
            buf.set_stringn(
                area.x + GLYPH_COLS,
                y,
                &item.label,
                label_max.into(),
                label_style,
            );

            // The detail, muted, truncated with `…` when the row is tight.
            if let Some(detail) = item.detail.as_deref() {
                let label_cols = u16::try_from(item.label.width()).unwrap_or(u16::MAX);
                let detail_x = label_x
                    .saturating_add(label_cols)
                    .saturating_add(DETAIL_GAP);
                let room = area.right().saturating_sub(detail_x);
                if room > 0 {
                    let text = truncate_to(detail, room);
                    buf.set_string(detail_x, y, text, Style::default().fg(muted).bg(row_bg));
                }
            }
        }

        state.track = tracks
            .paint(
                buf,
                ScrollbarStyles::from_theme(self.theme),
                ScrollExtent::new(ranked.len(), state.scroll, visible),
                ScrollExtent::default(),
            )
            .vertical;
    }
}

/// The narrowest a documentation pane beside the popup is drawn; with less
/// room on either side it is not drawn at all.
pub const MIN_DOCS_WIDTH: u16 = 24;

/// The widest a documentation pane beside the popup grows.
pub const MAX_DOCS_WIDTH: u16 = 60;

/// Where the selected candidate's documentation goes beside a popup painted at
/// `popup`, inside `bounds`: to its right when at least [`MIN_DOCS_WIDTH`]
/// columns are free there, else to its left, up to [`MAX_DOCS_WIDTH`] wide.
/// Top-aligned with the popup, and as tall as `rows_at(width)` -- the rows
/// the content wraps to at that width -- up to the bottom of `bounds`.
///
/// `None` when neither side has room, or the content is empty.
#[must_use]
pub fn docs_pane_rect(bounds: Rect, popup: Rect, rows_at: impl Fn(u16) -> u16) -> Option<Rect> {
    let right = bounds.right().saturating_sub(popup.right());
    let left = popup.left().saturating_sub(bounds.left());
    let (x, width) = if right >= MIN_DOCS_WIDTH {
        (popup.right(), right.min(MAX_DOCS_WIDTH))
    } else if left >= MIN_DOCS_WIDTH {
        let width = left.min(MAX_DOCS_WIDTH);
        (popup.left() - width, width)
    } else {
        return None;
    };
    let room = bounds.bottom().saturating_sub(popup.y);
    let height = rows_at(width).min(room);
    (height > 0).then_some(Rect::new(x, popup.y, width, height))
}

/// The span an accept replaces: from the popup's `anchor` to the current
/// `caret` (the typed prefix). `None` when the caret has wandered somewhere an
/// accept no longer makes sense (another line, or before the anchor).
#[must_use]
pub fn accept_range(
    anchor: karet_core::LineCol,
    caret: karet_core::LineCol,
) -> Option<karet_core::Range> {
    caret_still_anchored(anchor, caret).then_some(karet_core::Range {
        start: anchor,
        end: caret,
    })
}

/// Whether the popup (or its pending request) is still valid for `caret` —
/// the same line as the anchor and not before it. Any other movement dismisses.
#[must_use]
pub fn caret_still_anchored(anchor: karet_core::LineCol, caret: karet_core::LineCol) -> bool {
    caret.line == anchor.line && caret.col >= anchor.col
}

#[cfg(test)]
mod tests {
    use karet_core::Markup;
    use karet_core::MarkupKind;

    use super::*;

    fn item(label: &str, kind: CompletionKind, detail: Option<&str>) -> CompletionItem {
        CompletionItem {
            detail: detail.map(ToOwned::to_owned),
            ..CompletionItem::new(label, kind)
        }
    }

    /// Falsified by ranking on the label when the server gave filter text: a
    /// label decorated beyond what is typed (`HashMap (use std::…)`) would
    /// then rank by the decoration, and an item whose filter text matches
    /// but label does not would vanish.
    #[test]
    fn typing_matches_the_filter_text_when_there_is_one() {
        let items = [
            CompletionItem {
                filter_text: Some("deref".to_owned()),
                ..item("*x", CompletionKind::Method, None)
            },
            item("debug", CompletionKind::Function, None),
        ];
        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let ranked = CompletionPopup::new(&items, &mut matcher, "deref", &theme).ranked();
        assert_eq!(ranked, vec![0]);
    }

    /// Right of the popup when there is room, left when only that side has
    /// it, nowhere when neither does; never taller than the room below the
    /// popup's top.
    #[test]
    fn the_docs_pane_sits_beside_the_popup_where_it_fits() {
        let bounds = Rect::new(0, 0, 100, 20);
        let popup = Rect::new(10, 5, 20, 6);
        let rect = docs_pane_rect(bounds, popup, |_| 4);
        assert_eq!(rect, Some(Rect::new(30, 5, MAX_DOCS_WIDTH, 4)));
        // Wrapped taller than the room left: clipped at the bottom.
        let rect = docs_pane_rect(bounds, popup, |_| 40);
        assert_eq!(rect, Some(Rect::new(30, 5, MAX_DOCS_WIDTH, 15)));
        // Only the left side has room.
        let popup = Rect::new(50, 0, 40, 6);
        let rect = docs_pane_rect(bounds, popup, |width| {
            assert_eq!(width, 50.min(MAX_DOCS_WIDTH));
            2
        });
        assert_eq!(rect, Some(Rect::new(0, 0, 50, 2)));
        // No room either side, or nothing to show.
        let squeezed = Rect::new(20, 0, 70, 6);
        assert_eq!(docs_pane_rect(bounds, squeezed, |_| 3), None);
        assert_eq!(docs_pane_rect(bounds, Rect::new(10, 5, 20, 6), |_| 0), None);
    }

    fn render(
        items: &[CompletionItem],
        filter: &str,
        area: Rect,
        state: &mut CompletionState,
    ) -> Buffer {
        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let mut buf = Buffer::empty(area);
        CompletionPopup::new(items, &mut matcher, filter, &theme).render(area, &mut buf, state);
        buf
    }

    fn row_text(buf: &Buffer, y: u16) -> String {
        let area = buf.area;
        (area.left()..area.right())
            .map(|x| {
                buf.cell((x, y))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
            })
            .collect()
    }

    #[test]
    fn rows_carry_glyph_label_and_detail() {
        let items = [
            item("push", CompletionKind::Method, Some("fn(&mut self)")),
            item("pop", CompletionKind::Function, None),
        ];
        let mut state = CompletionState::default();
        let buf = render(&items, "", Rect::new(0, 0, 24, 2), &mut state);
        assert_eq!(row_text(&buf, 1).trim_end(), "m push  fn(&mut self)");
        assert_eq!(row_text(&buf, 0).trim_end(), "f pop");
    }

    #[test]
    fn empty_filter_orders_by_sort_text_then_label() {
        let mut a = item("zzz", CompletionKind::Text, None);
        a.sort_text = Some("0001".into());
        let b = item("aaa", CompletionKind::Text, None);
        let items = [b, a];
        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let ranked = CompletionPopup::new(&items, &mut matcher, "", &theme).ranked();
        // "0001" sorts before "aaa": the server's sortText wins.
        assert_eq!(ranked, vec![1, 0]);
    }

    #[test]
    fn typing_refilters_through_the_matcher() {
        let items = [
            item("println", CompletionKind::Function, None),
            item("size_hint", CompletionKind::Method, None),
            item("private", CompletionKind::Field, None),
        ];
        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let ranked = CompletionPopup::new(&items, &mut matcher, "pri", &theme).ranked();
        // Both "println" and "private" contain the subsequence; size_hint no.
        assert_eq!(ranked.len(), 2);
        assert!(!ranked.contains(&1));
        // And the rendered popup only shows the survivors.
        let mut state = CompletionState::default();
        let buf = render(&items, "pri", Rect::new(0, 0, 12, 3), &mut state);
        let all: Vec<String> = (0..3).map(|y| row_text(&buf, y)).collect();
        assert!(all.iter().all(|r| !r.contains("size_hint")));
    }

    #[test]
    fn selection_row_is_highlighted_and_bold() {
        let items = [
            item("first", CompletionKind::Text, None),
            item("second", CompletionKind::Text, None),
        ];
        let mut state = CompletionState::default();
        state.select_next(2); // selection on row 1 ("second")
        let buf = render(&items, "", Rect::new(0, 0, 10, 2), &mut state);
        let theme = Theme::dark();
        let selection = theme.role(ThemeRole::Selection).to_ratatui();
        let background = theme.role(ThemeRole::Background).to_ratatui();
        let sel_cell = buf.cell((2, 1)).cloned().unwrap_or_default();
        let plain_cell = buf.cell((2, 0)).cloned().unwrap_or_default();
        assert_eq!(sel_cell.bg, selection);
        assert!(sel_cell.modifier.contains(Modifier::BOLD));
        assert_eq!(plain_cell.bg, background);
        assert!(!plain_cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn desired_size_fits_the_widest_row_and_clamps() {
        let items = [
            item(
                "frobnicate_all",
                CompletionKind::Function,
                Some("fn() -> Frob"),
            ),
            item("id", CompletionKind::Field, None),
        ];
        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let mut popup = CompletionPopup::new(&items, &mut matcher, "", &theme);
        // 2 (glyph) + 14 (label) + 2 (gap) + 12 (detail) = 30.
        assert_eq!(popup.desired_size((80, 20)), (30, 2));
        // Clamped by the available area.
        assert_eq!(popup.desired_size((25, 1)), (25, 1));
        // Nothing matching: no popup.
        let mut popup = CompletionPopup::new(&items, &mut matcher, "zzz", &theme);
        assert_eq!(popup.desired_size((80, 20)), (0, 0));
    }

    #[test]
    fn tight_width_keeps_the_full_label_and_truncates_detail() {
        let items = [item(
            "push_str",
            CompletionKind::Method,
            Some("fn(&mut self, string: &str)"),
        )];
        let mut state = CompletionState::default();
        // 2 + 8 (label) + 2 (gap) leaves 4 columns for a 27-column detail.
        let buf = render(&items, "", Rect::new(0, 0, 16, 1), &mut state);
        let row = row_text(&buf, 0);
        assert!(row.starts_with("m push_str  "), "row: {row:?}");
        assert!(row.trim_end().ends_with('…'), "row: {row:?}");
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_selection_visible() {
        let items: Vec<CompletionItem> = (0..25)
            .map(|i| item(&format!("item_{i:02}"), CompletionKind::Text, None))
            .collect();
        let mut state = CompletionState::default();
        for _ in 0..12 {
            state.select_next(items.len());
        }
        let area = Rect::new(0, 0, 12, MAX_VISIBLE_ROWS);
        let buf = render(&items, "", area, &mut state);
        // Rows 3..=12 are visible; the selected item_12 is the bottom row.
        assert!(row_text(&buf, 0).contains("item_03"));
        assert!(row_text(&buf, 9).contains("item_12"));
        // Wrapping upward from the top scrolls back.
        let mut state = CompletionState::default();
        state.select_prev(items.len()); // wraps to the last item
        let buf = render(&items, "", area, &mut state);
        assert!(row_text(&buf, 9).contains("item_24"));
    }

    #[test]
    fn rows_resolve_to_the_candidates_painted_there() {
        let items: Vec<CompletionItem> = (0..25)
            .map(|i| item(&format!("item_{i:02}"), CompletionKind::Text, None))
            .collect();
        let mut state = CompletionState::default();
        for _ in 0..12 {
            state.select_next(items.len());
        }
        let area = Rect::new(4, 2, 12, MAX_VISIBLE_ROWS);
        let buf = render(&items, "", area, &mut state);
        assert_eq!(state.area(), area);
        // The window starts at item_03, so the top row is ranked index 3.
        assert!(row_text(&buf, 2).contains("item_03"));
        assert_eq!(state.row_at(4, 2), Some(3));
        assert_eq!(state.row_at(14, 11), Some(12));
        // The scrollbar column, and anything outside the popup, is no row.
        assert_eq!(state.row_at(15, 5), None, "the scrollbar column");
        assert_eq!(state.row_at(3, 2), None);
        assert_eq!(state.row_at(4, 12), None);
        state.hide();
        assert_eq!(state.area(), Rect::default());
        assert_eq!(state.row_at(4, 2), None);
        assert_eq!(state.track(), None);
    }

    #[test]
    fn a_wheel_scroll_moves_the_window_and_drags_the_selection_along() {
        let items: Vec<CompletionItem> = (0..25)
            .map(|i| item(&format!("item_{i:02}"), CompletionKind::Text, None))
            .collect();
        let mut state = CompletionState::default();
        let area = Rect::new(0, 0, 12, MAX_VISIBLE_ROWS);
        render(&items, "", area, &mut state);
        state.scroll_by(3, items.len());
        let buf = render(&items, "", area, &mut state);
        assert!(row_text(&buf, 0).contains("item_03"), "scrolled three rows");
        assert_eq!(state.selected, 3, "the selection stays in the window");
        // Clamped at the end, so the window stays full.
        state.scroll_by(100, items.len());
        let buf = render(&items, "", area, &mut state);
        assert!(row_text(&buf, 9).contains("item_24"));
        assert_eq!(state.row_at(0, 0), Some(15));
        // And at the top.
        state.scroll_by(-100, items.len());
        let buf = render(&items, "", area, &mut state);
        assert!(row_text(&buf, 0).contains("item_00"));
        // A popup that was never painted has no window to scroll.
        let mut unpainted = CompletionState::default();
        unpainted.scroll_by(3, items.len());
        assert_eq!(unpainted, CompletionState::default());
    }

    #[test]
    fn a_long_list_paints_a_scrollbar_and_asks_for_its_column() {
        let items: Vec<CompletionItem> = (0..25)
            .map(|i| item(&format!("item_{i:02}"), CompletionKind::Text, None))
            .collect();
        let mut state = CompletionState::default();
        let area = Rect::new(0, 0, 12, MAX_VISIBLE_ROWS);
        let buf = render(&items, "", area, &mut state);

        let track: String = (0..area.height)
            .map(|y| buf[(area.right() - 1, y)].symbol().to_owned())
            .collect();
        assert!(track.contains('\u{2588}'), "track was {track:?}");
        // The label keeps its full width because `desired_size` asked for the
        // extra column; both sides must agree or the track eats a character.
        assert!(row_text(&buf, 0).contains("item_00"));

        let theme = Theme::dark();
        let mut matcher = Matcher::new();
        let wide = CompletionPopup::new(&items, &mut matcher, "", &theme).desired_size((80, 80));
        let narrow: Vec<CompletionItem> = items.iter().take(3).cloned().collect();
        let thin = CompletionPopup::new(&narrow, &mut matcher, "", &theme).desired_size((80, 80));
        assert_eq!(
            wide.0,
            thin.0 + 1,
            "only the popup that scrolls should reserve a column"
        );
    }

    #[test]
    fn deprecated_items_are_struck_through() {
        let mut deprecated = item("old_api", CompletionKind::Function, None);
        deprecated.deprecated = true;
        let items = [deprecated];
        let mut state = CompletionState::default();
        let buf = render(&items, "", Rect::new(0, 0, 12, 1), &mut state);
        let cell = buf.cell((2, 0)).cloned().unwrap_or_default();
        assert!(cell.modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn selection_navigation_wraps_and_resets() {
        let mut state = CompletionState::default();
        state.select_prev(3);
        assert_eq!(state.selected, 2);
        state.select_next(3);
        assert_eq!(state.selected, 0);
        state.select_next(0); // empty list is safe
        assert_eq!(state.selected, 0);
        state.selected = 2;
        state.reset();
        assert_eq!(state, CompletionState::default());
    }

    #[test]
    fn empty_area_or_no_items_renders_nothing() {
        let items = [item("x", CompletionKind::Text, None)];
        let mut state = CompletionState::default();
        let buf = render(&items, "", Rect::new(0, 0, 0, 0), &mut state);
        assert_eq!(buf.area.width, 0);
        let buf = render(&[], "", Rect::new(0, 0, 8, 2), &mut state);
        assert_eq!(row_text(&buf, 0).trim(), "");
    }

    #[test]
    fn glyphs_cover_every_kind_distinctly_enough() {
        // Every mapped kind yields a printable single-column glyph.
        for kind in [
            CompletionKind::Text,
            CompletionKind::Method,
            CompletionKind::Function,
            CompletionKind::Field,
            CompletionKind::Variable,
            CompletionKind::Class,
            CompletionKind::Interface,
            CompletionKind::Module,
            CompletionKind::Property,
            CompletionKind::Keyword,
            CompletionKind::Snippet,
            CompletionKind::Constant,
            CompletionKind::Struct,
            CompletionKind::Enum,
        ] {
            let (glyph, _) = kind_glyph(kind);
            assert!(glyph.is_ascii_graphic(), "glyph for {kind:?}");
        }
    }

    #[test]
    fn documentation_field_is_tolerated_but_unrendered() {
        // v1 keeps rows to label/kind/detail; docs on the item must not panic.
        let mut with_docs = item("doc_bearer", CompletionKind::Function, None);
        with_docs.documentation = Some(Markup {
            kind: MarkupKind::Markdown,
            value: "# docs".into(),
        });
        let items = [with_docs];
        let mut state = CompletionState::default();
        let buf = render(&items, "", Rect::new(0, 0, 14, 1), &mut state);
        assert!(row_text(&buf, 0).contains("doc_bearer"));
    }

    #[test]
    fn accept_range_spans_anchor_to_caret_only_while_anchored() {
        use karet_core::LineCol;
        let anchor = LineCol::new(3, 4);
        let span = |caret| {
            accept_range(anchor, caret).map(|range: karet_core::Range| (range.start, range.end))
        };
        assert_eq!(span(LineCol::new(3, 7)), Some((anchor, LineCol::new(3, 7))));
        assert_eq!(span(anchor), Some((anchor, anchor)), "an empty prefix");
        assert_eq!(span(LineCol::new(3, 3)), None, "before the anchor");
        assert_eq!(span(LineCol::new(4, 7)), None, "another line");
        assert_eq!(span(LineCol::new(2, 9)), None, "another line");
    }
}

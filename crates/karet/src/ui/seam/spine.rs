//! The spine: cascading columns when the terminal can carry them, an indented tree when
//! it cannot.
//!
//! The fallback is not a lesser mode. Eighty columns is the common terminal, and four
//! columns of twenty cells each shows nothing but truncated stems. Both renderings read
//! the same state and offer the same operations; only the shape differs.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use karet_widgets::Columns;
use karet_widgets::UiIcon;
use karet_widgets::glyph::glyph_slot;
use karet_widgets::glyph::slot;
use karet_widgets::glyph::slots;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::column_style;
use super::columns_for;
use super::row_for;
use super::wide_enough;
use crate::app::seam::SeamViewState;

/// Draw the spine into `area`.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    if wide_enough(area.width) {
        draw_columns(f, theme, area, state, icons);
    } else {
        draw_tree(f, theme, area, state, icons);
    }
}

/// The cascading rendering.
fn draw_columns(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    let all = columns_for(state, icons, area.height);
    // Remember where each column scrolled to, so returning to one keeps its place.
    state.offsets = all.iter().map(|column| column.offset).collect();

    let capacity = Columns::fits(area.width);
    let window = Columns::window(all.len(), state.focused_column, capacity);
    let visible: Vec<_> = all
        .get(window.clone())
        .map(<[_]>::to_vec)
        .unwrap_or_default();
    let focused = state.focused_column.saturating_sub(window.start);

    // Recorded from the same loop that paints, and keyed on identity: the mouse never has
    // to re-derive which node a row was, so the window offset cannot drift between them.
    let ids = state.columns();
    let rects = Columns::layout(area, visible.len());
    let mut rows = Vec::new();
    for (index, (column, rect)) in visible.iter().zip(&rects).enumerate() {
        let Some(ids) = ids.get(window.start + index) else {
            break;
        };
        for offset in 0..rect.height {
            let Some(id) = ids.get(column.offset + usize::from(offset)) else {
                break;
            };
            rows.push((
                Rect::new(rect.x, rect.y.saturating_add(offset), rect.width, 1),
                id.clone(),
            ));
        }
    }
    state.hits.rows = rows;

    let widget = Columns::new(&visible, focused, column_style(theme))
        .child_marker(UiIcon::SeamHasChildren.glyph(icons))
        .marker_slot(u16::try_from(glyph_slot(icons)).unwrap_or(1));
    widget.render(area, f.buffer_mut());
}

/// The indented rendering, for terminals too narrow to cascade.
fn draw_tree(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    let selected_style = Style::default()
        .fg(theme.role(ThemeRole::Foreground).to_ratatui())
        .bg(theme.role(ThemeRole::Selection).to_ratatui());
    let muted = theme.style(ThemeRole::Muted);
    let normal = theme.style(ThemeRole::Foreground);

    let mut lines = Vec::new();
    let mut ids_by_line = Vec::new();
    // The path the reader has open, one level per line — the same information the
    // columns show side by side, stacked instead.
    for (depth, ids) in state.columns().into_iter().enumerate() {
        for id in ids {
            ids_by_line.push(id.clone());
            let row = row_for(state, &id, icons);
            let chosen = state.selection.get(depth).is_some_and(|sel| *sel == id);
            let style = if chosen {
                selected_style
            } else if row.emphasis == karet_widgets::RowEmphasis::Dimmed {
                muted
            } else {
                normal
            };
            let mut spans = vec![Span::styled("  ".repeat(depth), muted)];
            spans.push(Span::styled(row.label.clone(), style));
            if !row.markers.is_empty() {
                spans.push(Span::styled(
                    format!(" {}", slots(row.markers.iter().copied(), icons)),
                    theme.style(ThemeRole::DiagnosticInfo),
                ));
            }
            if let Some(trailing) = &row.trailing {
                spans.push(Span::styled(
                    format!(" {trailing}"),
                    theme.style(ThemeRole::LineNumber),
                ));
            }
            if row.has_children {
                spans.push(Span::styled(
                    format!(" {}", slot(UiIcon::SeamHasChildren.glyph(icons), icons)),
                    muted,
                ));
            }
            lines.push(Line::from(spans));
        }
        // Only the open path is expanded, so the tree never grows past the screen.
        if state.selection.len() <= depth {
            break;
        }
    }

    let skip = lines
        .len()
        .saturating_sub(usize::from(area.height))
        .min(usize::from(area.height));
    // The tree shows its tail, so the rows it records start from the same skip.
    state.hits.rows = ids_by_line
        .into_iter()
        .skip(skip)
        .take(usize::from(area.height))
        .enumerate()
        .map(|(row, id)| {
            (
                Rect::new(
                    area.x,
                    area.y
                        .saturating_add(u16::try_from(row).unwrap_or(u16::MAX)),
                    area.width,
                    1,
                ),
                id,
            )
        })
        .collect();
    let visible: Vec<Line> = lines.into_iter().skip(skip).collect();
    f.render_widget(
        Paragraph::new(visible).style(Style::default().add_modifier(Modifier::empty())),
        area,
    );
}

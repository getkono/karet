//! Painting the Seam view: header, spine, facet pane, query line.
//!
//! The layout answers three questions at once and never makes the reader choose between
//! them: *where am I* (the header's breadcrumb and configuration), *what is here* (the
//! spine), and *what is true of this one thing* (the facet pane). The spine is the
//! primary surface; the facet pane exists so a glyph never has to carry meaning on its own.
//!
//! Below the width a cascading spine needs, this falls back to an indented tree rather
//! than squeezing. That is not a degraded mode grudgingly supported — an 80-column
//! terminal is the common case, and four columns of twenty cells is unreadable.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use karet_widgets::Column;
use karet_widgets::ColumnRow;
use karet_widgets::ColumnStyle;
use karet_widgets::Columns;
use karet_widgets::UiIcon;
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

pub(super) use crate::app::seam::LENS_NAMES;
use crate::app::seam::LensFilter;
use crate::app::seam::SeamFocus;
use crate::app::seam::SeamViewState;

mod facets;
mod header;
mod spine;

#[cfg(test)]
mod tests;

/// How many rows the facet pane takes when there is a selection.
const FACET_HEIGHT: u16 = 9;

/// The glyph for a lens, resolved to the reader's icon style.
#[must_use]
pub(crate) fn lens_glyph(lens: &str, icons: IconStyle) -> char {
    let icon = match lens {
        "api" => UiIcon::SeamApi,
        "substitution" => UiIcon::SeamSubstitution,
        "variation" => UiIcon::SeamVariation,
        "boundary" => UiIcon::SeamBoundary,
        _ => UiIcon::SeamHazard,
    };
    icon.glyph(icons)
}

/// Draw the whole view.
pub(super) fn draw_seam(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    if area.height < 3 || area.width < 8 {
        return;
    }
    let facet_height = if state.selected().is_some() {
        FACET_HEIGHT.min(area.height.saturating_sub(4))
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // header
            Constraint::Min(3),               // spine
            Constraint::Length(facet_height), // facet pane
            Constraint::Length(1),            // query line
        ])
        .split(area);

    header::draw(f, theme, chunks[0], state, icons);

    if let Some(message) = placeholder(state) {
        let muted = theme.style(ThemeRole::Muted);
        f.render_widget(Paragraph::new(Line::from(message).style(muted)), chunks[1]);
    } else {
        spine::draw(f, theme, chunks[1], state, icons);
    }

    if facet_height > 0 {
        facets::draw(f, theme, chunks[2], state, icons);
    }
    draw_query_line(f, theme, chunks[3], state);
}

/// The message to show instead of a spine, when there is nothing to show.
///
/// Each state gets its own words. "Indexing", "no seams here", and "that is not a
/// package" are three different facts, and a shared empty state would flatten them into
/// one wrong impression.
fn placeholder(state: &SeamViewState) -> Option<String> {
    if let Some(error) = &state.error {
        // "Nothing here", not "this package": the root may be a workspace, a directory of
        // crates, or somewhere with no package at all — and saying which of those it was
        // is the error's job, not this sentence's.
        return Some(format!(
            "Nothing here could be indexed: {error} — press r to choose another start point."
        ));
    }
    if state.is_loading() {
        // Nothing at all until the shared reveal delay, so a fast index never flashes.
        return state
            .loading_since
            .filter(|pending| pending.visible())
            .map(|_| "Indexing…".to_owned())
            .or(Some(String::new()));
    }
    let columns = state.columns();
    if columns.first().is_some_and(|rows| rows.is_empty()) {
        if state.query_error.is_some() {
            return Some("The query could not be read — see below.".to_owned());
        }
        if !state.lenses.is_empty() || state.query_matches.is_some() {
            return Some("Nothing here matches the current filters.".to_owned());
        }
        return Some("No seams here under the active configuration.".to_owned());
    }
    None
}

/// The query line, with its parse error underneath when it has one.
fn draw_query_line(f: &mut Frame, theme: &Theme, area: Rect, state: &SeamViewState) {
    let muted = theme.style(ThemeRole::Muted);
    let focused = state.focus == SeamFocus::Query;
    let mut spans = vec![Span::styled(
        if focused { "/ " } else { "  " },
        if focused {
            theme
                .style(ThemeRole::Foreground)
                .add_modifier(Modifier::BOLD)
        } else {
            muted
        },
    )];
    if state.query.is_empty() && !focused {
        spans.push(Span::styled("press / to filter", muted));
    } else {
        spans.push(Span::styled(
            state.query.clone(),
            theme.style(ThemeRole::Foreground),
        ));
    }
    if let Some(error) = &state.query_error {
        spans.push(Span::styled("  ", muted));
        spans.push(Span::styled(
            error.message.clone(),
            theme.style(ThemeRole::DiagnosticError),
        ));
        if !error.suggestions.is_empty() {
            spans.push(Span::styled(
                format!("  did you mean {}?", error.suggestions.join(", ")),
                muted,
            ));
        }
    } else {
        // The reversal path has to be visible, not merely available.
        let depth = state.narrow.len();
        if depth > 0 {
            spans.push(Span::styled(format!("   ⌫ widen ({depth})"), muted));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The styles the spine paints with.
#[must_use]
fn column_style(theme: &Theme) -> ColumnStyle {
    ColumnStyle {
        normal: theme.style(ThemeRole::Foreground),
        selected: Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .bg(theme.role(ThemeRole::Selection).to_ratatui()),
        // The trail the reader followed here stays legible without competing with the
        // column they are actually in.
        trail: Style::default()
            .fg(theme.role(ThemeRole::Foreground).to_ratatui())
            .bg(theme.role(ThemeRole::HoverHighlight).to_ratatui()),
        dimmed: theme.style(ThemeRole::Muted),
        marker: theme.style(ThemeRole::DiagnosticInfo),
        trailing: theme.style(ThemeRole::LineNumber),
        divider: theme.style(ThemeRole::IndentGuide),
    }
}

/// Compose one node into a spine row.
#[must_use]
fn row_for(state: &SeamViewState, id: &str, icons: IconStyle) -> ColumnRow {
    let Some(node) = state.nodes.get(id) else {
        return ColumnRow::new(id);
    };
    let mut markers = String::new();
    for (index, lens) in LENS_NAMES.iter().enumerate() {
        let carries = node.facets.iter().any(|facet| facet.lens == *lens);
        let under = node.rollups.get(index).is_some_and(|count| *count > 0);
        // A glyph the node earns itself, or one it inherits from its subtree; the
        // renderer distinguishes them by intensity, not by presence.
        if carries || under {
            markers.push(lens_glyph(lens, icons));
        }
    }
    if node.membership != "active" {
        markers.push(UiIcon::SeamInactive.glyph(icons));
    }

    let total: u32 = node.rollups.iter().copied().fold(0, u32::saturating_add);
    let count = active_lens_count(state, node).unwrap_or(total);
    let mut row = ColumnRow::new(node.name.clone())
        .with_markers(markers)
        .with_children(!node.children.is_empty());
    if count > 0 {
        row = row.with_trailing(count.to_string());
    }
    // Demoted rather than removed: the tree keeps its shape as filters change.
    if node.membership == "inactive"
        || (state.lens_filter == LensFilter::Demote && !state.matches(id))
    {
        row = row.dimmed();
    }
    row
}

/// The rollup count for the single active lens, when exactly one is active.
///
/// With several on, a per-lens number would have to pick one arbitrarily, so the total
/// is shown instead and the facet pane carries the breakdown.
fn active_lens_count(
    state: &SeamViewState,
    node: &karet_session::api::SeamNodeView,
) -> Option<u32> {
    if state.lenses.len() != 1 {
        return None;
    }
    let lens = state.lenses.iter().next()?;
    let index = LENS_NAMES.iter().position(|name| name == lens)?;
    node.rollups.get(index).copied()
}

/// Build the widget columns from the view state.
#[must_use]
fn columns_for(state: &SeamViewState, icons: IconStyle, height: u16) -> Vec<Column> {
    state
        .columns()
        .into_iter()
        .enumerate()
        .map(|(depth, ids)| {
            let selected = state
                .selection
                .get(depth)
                .and_then(|chosen| ids.iter().position(|id| id == chosen));
            let mut column = Column::new(
                ids.iter()
                    .map(|id| row_for(state, id, icons))
                    .collect::<Vec<_>>(),
            );
            column.selected = selected;
            column.offset = state.offsets.get(depth).copied().unwrap_or(0);
            column.scroll_into_view(height);
            column
        })
        .collect()
}

/// Whether the terminal is wide enough for a cascading spine.
#[must_use]
pub(crate) fn wide_enough(width: u16) -> bool {
    Columns::fits(width) >= 2
}

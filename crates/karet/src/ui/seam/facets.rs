//! The facet pane: everything true of the current selection, grouped by lens.
//!
//! This is what keeps the glyphs honest. A marker in the spine summarizes; it must never
//! be the only place a fact appears, or the reader is left decoding pictograms. Every
//! glyph on a row corresponds to a line here, spelled out.
//!

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use karet_widgets::glyph::slot;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::ABSENT;
use super::LENS_NAMES;
use super::PENDING;
use super::UNRESOLVABLE;
use super::lens_glyph;
use crate::app::seam::SeamFocus;
use crate::app::seam::SeamViewState;

/// Draw the facet pane.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &mut SeamViewState,
    icons: IconStyle,
) {
    let Some(node) = state.selected() else {
        return;
    };
    let muted = theme.style(ThemeRole::Muted);
    let strong = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);

    // Beside a source preview the pane is half a terminal wide, so the fixed label
    // columns shrink rather than eating the room an edge needs to name its target.
    let compact = area.width < super::FACET_COMPACT_WIDTH;
    let lens_column = if compact { 4 } else { 13 };
    let kind_column = if compact { 10 } else { 18 };

    let mut lines = Vec::new();

    // The identity, which is also what an agent would cite and what `y` yanks.
    let mut title = vec![
        Span::styled(node.id.clone(), strong),
        Span::styled(format!("   {}", node.kind), muted),
    ];
    if let Some(visibility) = &node.visibility {
        title.push(Span::styled(format!(" · {visibility}"), muted));
    }
    title.push(Span::styled(
        format!(
            "   {}:{}",
            node.file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?"),
            node.range.start.line.saturating_add(1)
        ),
        muted,
    ));
    if node.provisional {
        // The subtree did not parse cleanly, so what follows is incomplete rather
        // than merely sparse.
        title.push(Span::styled(
            "   (partial — this file did not parse cleanly)",
            theme.style(ThemeRole::DiagnosticWarning),
        ));
    }
    lines.push(Line::from(title));

    for lens in LENS_NAMES {
        let facets: Vec<_> = node.facets.iter().filter(|f| f.lens == lens).collect();
        let mut spans = vec![
            Span::styled(format!("{} ", slot(lens_glyph(lens, icons), icons)), muted),
            Span::styled(
                format!("{:<lens_column$} ", lens_label(lens, compact)),
                muted,
            ),
        ];
        if facets.is_empty() {
            spans.push(Span::styled(ABSENT, muted));
        } else {
            let described: Vec<String> = facets
                .iter()
                .map(|facet| {
                    let mut text = facet.subtype.clone();
                    if facet.sites.len() > 1 {
                        text.push_str(&format!(" ×{}", facet.sites.len()));
                    }
                    if let Some(detail) = &facet.detail {
                        text.push_str(&format!(" ({detail})"));
                    }
                    text
                })
                .collect();
            spans.push(Span::styled(
                described.join(" · "),
                theme.style(ThemeRole::Foreground),
            ));
            // Effective reach is a modifier on the declared visibility, not a fact of
            // its own — so it reads beside it rather than as a separate lens line.
            if let Some(effective) = facets.iter().find_map(|f| f.effective.as_ref()) {
                spans.push(Span::styled(
                    format!("   effective: {effective}"),
                    theme.style(ThemeRole::DiagnosticInfo),
                ));
            }
        }
        lines.push(Line::from(spans));
    }

    lines.push(edges_line(theme, state, lens_column));

    // Measured from what has been pushed so far rather than from a constant, so adding a
    // line above the edges can never quietly move what the mouse thinks it is aiming at.
    let first_edge = area
        .y
        .saturating_add(u16::try_from(lines.len()).unwrap_or(u16::MAX));
    let mut hits = Vec::new();
    let focused = state.focus == SeamFocus::Facets;
    for (index, edge) in state.edges.iter().enumerate() {
        let arrow = if edge.outgoing { "→" } else { "←" };
        let target = edge
            .target
            .clone()
            .or_else(|| edge.display.clone())
            .unwrap_or_else(|| match (edge.state.as_str(), edge.resolvable) {
                ("unresolved", true) => PENDING.to_owned(),
                ("unresolved", false) => UNRESOLVABLE.to_owned(),
                _ => ABSENT.to_owned(),
            });
        let style = if focused && index == state.facet_row {
            theme
                .style(ThemeRole::Foreground)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.style(ThemeRole::Foreground)
        };
        let y = first_edge.saturating_add(u16::try_from(index).unwrap_or(u16::MAX));
        if y < area.y.saturating_add(area.height) {
            hits.push((Rect::new(area.x, y, area.width, 1), index));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "  {} {arrow} ",
                    karet_widgets::text::fit_end(&edge.kind, kind_column)
                        + &" ".repeat(kind_column.saturating_sub(edge.kind.chars().count()))
                ),
                muted,
            ),
            Span::styled(target, style),
            Span::styled(format!("   ({})", edge.state), muted),
        ]));
    }

    state.hits.edges = hits;
    f.render_widget(Paragraph::new(lines), area);
}

/// The lens's name, abbreviated where the pane is too narrow for it.
fn lens_label(lens: &str, compact: bool) -> &str {
    if !compact {
        return lens;
    }
    match lens {
        "api" => "api",
        "substitution" => "sub",
        "variation" => "var",
        "boundary" => "bnd",
        _ => "haz",
    }
}

/// The edges header, which distinguishes "none" from "not yet resolved".
fn edges_line<'a>(theme: &Theme, state: &SeamViewState, lens_column: usize) -> Line<'a> {
    let width = lens_column + 2;
    let muted = theme.style(ThemeRole::Muted);
    if state.edges.is_empty() {
        return Line::from(vec![
            Span::styled(format!("{:<width$}", "  edges"), muted),
            // Without a semantic tier nothing has looked, so saying "none" would assert
            // an absence the index has not established.
            Span::styled(
                format!("{PENDING} not resolved — structural relations only"),
                muted,
            ),
        ]);
    }
    Line::from(vec![
        Span::styled(format!("{:<width$}", "  edges"), muted),
        Span::styled(
            format!("{} — Enter to pivot", state.edges.len()),
            theme.style(ThemeRole::DiagnosticInfo),
        ),
    ])
}

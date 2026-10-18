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
        if facets.is_empty() && node.unread_lenses.iter().any(|unread| unread == lens) {
            // Not looked at, which is a different answer from looked and found nothing.
            spans.push(Span::styled(
                format!("{UNRESOLVABLE} not read for this language"),
                muted,
            ));
        } else if facets.is_empty() {
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

    if let Some(metrics) = &state.metrics {
        // The full line when it fits, the numbers alone when it would be cut — a cut
        // line loses whatever came last, and that is never the least important part.
        let room = usize::from(area.width).saturating_sub(lens_column + 3);
        let full = calls_summary(metrics, compact);
        let summary = if full.chars().count() > room {
            calls_summary(metrics, true)
        } else {
            full
        };
        // Sized to the lens labels, so the numbers start where every lens's facets do.
        let label = if compact { "call" } else { "calls" };
        lines.push(Line::from(vec![
            Span::styled(format!("  {label:<lens_column$} "), muted),
            Span::styled(summary, theme.style(ThemeRole::Foreground)),
        ]));
    }
    lines.push(edges_line(theme, state, lens_column));

    // Measured from what has been pushed so far rather than from a constant, so adding a
    // line above the edges can never quietly move what the mouse thinks it is aiming at.
    let first_edge = area
        .y
        .saturating_add(u16::try_from(lines.len()).unwrap_or(u16::MAX));
    let mut hits = Vec::new();
    let focused = state.focus == SeamFocus::Facets;
    // A function with a hundred callers has more edges than the pane has rows. The list
    // scrolls with the selection, and a last row says how many are out of sight, so the
    // selected edge is never one the reader cannot see.
    let room = usize::from(area.height).saturating_sub(lines.len());
    let total = state.edges.len();
    let (start, shown) = if total <= room {
        (0, total)
    } else {
        let shown = room.saturating_sub(1).max(1);
        let start = state.facet_row.saturating_sub(shown - 1).min(total - shown);
        (start, shown)
    };
    for (index, edge) in state.edges.iter().enumerate().skip(start).take(shown) {
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
        let y = first_edge.saturating_add(u16::try_from(index - start).unwrap_or(u16::MAX));
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
            Span::styled(format!("   ({})", edge_note(edge)), muted),
        ]));
    }
    if shown < total {
        lines.push(Line::from(Span::styled(
            format!("  … {} more", total - shown),
            muted,
        )));
    }

    state.hits.edges = hits;
    f.render_widget(Paragraph::new(lines), area);
}

/// What follows an edge's target: how a call was resolved and how many sites it stands
/// for, or the endpoint's state for every other relation.
fn edge_note(edge: &karet_session::api::SeamEdgeView) -> String {
    match &edge.resolution {
        Some(resolution) if edge.count > 1 => format!("{resolution} ×{}", edge.count),
        Some(resolution) => resolution.clone(),
        None => edge.state.clone(),
    }
}

/// A function's call measures on one line: its own size, what it reaches, and its shape.
///
/// The upper bound appears only where it differs, since an equal one says nothing a
/// reader needs; the compact form drops the words and keeps the numbers.
fn calls_summary(m: &karet_session::api::SeamCallMetricsView, compact: bool) -> String {
    let reach = if m.inclusive_upper > m.inclusive {
        format!("{} (≤{})", m.inclusive, m.inclusive_upper)
    } else {
        m.inclusive.to_string()
    };
    let tree = format!(
        "{}{}",
        if m.saturated { "≥" } else { "" },
        abbreviate(m.tree_weighted)
    );
    let mut parts = if compact {
        vec![
            format!("{} own", m.code_lines),
            format!("{reach} reach"),
            format!("in {} out {}", m.fan_in, m.fan_out),
        ]
    } else {
        vec![
            format!("{} own", m.code_lines),
            format!("{reach} reach"),
            format!("tree {tree}"),
            format!("in {}", m.fan_in),
            format!("out {}", m.fan_out),
            format!("depth {}", m.depth),
        ]
    };
    if m.external_apis > 0 && !compact {
        parts.push(format!("ext {}", m.external_apis));
    }
    if m.scc_size > 1 {
        parts.push(format!("cycle of {}", m.scc_size));
    } else if m.recursive {
        parts.push("recursive".to_owned());
    }
    parts.join(" · ")
}

/// A count in at most four characters: `950`, `1.2k`, `34k`, `5.6M`.
fn abbreviate(value: u64) -> String {
    // The boundaries are where rounding would carry into the next unit, so 9 960 reads
    // `10k` rather than `10.0k`.
    match value {
        0..1_000 => value.to_string(),
        1_000..9_950 => format!("{:.1}k", value as f64 / 1_000.0),
        9_950..999_500 => format!("{}k", (value + 500) / 1_000),
        999_500..9_950_000 => format!("{:.1}M", value as f64 / 1_000_000.0),
        // Saturating: a tree weight that hit the ceiling is `u64::MAX`.
        _ => format!("{}M", value.saturating_add(500_000) / 1_000_000),
    }
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
        // A function's calls have been looked for, so for one, empty means none. Its other
        // relations, like every node's, still wait on a semantic tier — saying "none"
        // there would assert an absence the index has not established.
        let note = if state.metrics.is_some() {
            format!("{ABSENT} no calls or callers · {PENDING} other relations not resolved")
        } else {
            format!("{PENDING} not resolved — structural relations only")
        };
        return Line::from(vec![
            Span::styled(format!("{:<width$}", "  edges"), muted),
            Span::styled(note, muted),
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

#[cfg(test)]
mod tests {
    use super::abbreviate;

    #[test]
    fn a_count_fits_four_characters_across_every_boundary() {
        for (value, shown) in [
            (999, "999"),
            (1_000, "1.0k"),
            (9_949, "9.9k"),
            (9_950, "10k"),
            (999_499, "999k"),
            (999_500, "1.0M"),
            (9_949_999, "9.9M"),
            (9_950_000, "10M"),
        ] {
            assert_eq!(abbreviate(value), shown, "{value}");
            assert!(abbreviate(value).chars().count() <= 4, "{value}");
        }
        // A saturated tree weight is the ceiling itself, and must not overflow.
        assert_eq!(abbreviate(u64::MAX), "18446744073709M");
    }
}

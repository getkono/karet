//! The facet pane: everything true of the current selection, grouped by lens.
//!
//! This is what keeps the glyphs honest. A marker in the spine summarizes; it must never
//! be the only place a fact appears, or the reader is left decoding pictograms. Every
//! glyph on a row corresponds to a line here, spelled out.
//!
//! Three states are kept apart deliberately. A lens with nothing is `—`, meaning the
//! index looked and found nothing. A relation still being resolved is `…`, meaning the
//! answer has not arrived. One that can never resolve is `?`. Collapsing these into a
//! blank would make "there is no unsafe code here" and "nobody has checked" look identical.

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::LENS_NAMES;
use super::lens_glyph;
use crate::app::seam::SeamFocus;
use crate::app::seam::SeamViewState;

/// Shown when a lens carries nothing: the index looked, and there was nothing.
const ABSENT: &str = "—";
/// Shown while an answer is still being resolved.
const PENDING: &str = "…";
/// Shown when an answer can never be resolved.
const UNRESOLVABLE: &str = "?";

/// Draw the facet pane.
pub(super) fn draw(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    state: &SeamViewState,
    icons: IconStyle,
) {
    let Some(node) = state.selected() else {
        return;
    };
    let muted = theme.style(ThemeRole::Muted);
    let strong = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);

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
            Span::styled(format!("{} ", lens_glyph(lens, icons)), muted),
            Span::styled(format!("{lens:<13} "), muted),
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

    lines.push(edges_line(theme, state));

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
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<18} {arrow} ", edge.kind), muted),
            Span::styled(target, style),
            Span::styled(format!("   ({})", edge.state), muted),
        ]));
    }

    f.render_widget(Paragraph::new(lines), area);
}

/// The edges header, which distinguishes "none" from "not yet resolved".
fn edges_line<'a>(theme: &Theme, state: &SeamViewState) -> Line<'a> {
    let muted = theme.style(ThemeRole::Muted);
    if state.edges.is_empty() {
        return Line::from(vec![
            Span::styled(format!("{:<15}", "  edges"), muted),
            // Without a semantic tier nothing has looked, so saying "none" would assert
            // an absence the index has not established.
            Span::styled(
                format!("{PENDING} not resolved — structural relations only"),
                muted,
            ),
        ]);
    }
    Line::from(vec![
        Span::styled(format!("{:<15}", "  edges"), muted),
        Span::styled(
            format!("{} — Enter to pivot", state.edges.len()),
            theme.style(ThemeRole::DiagnosticInfo),
        ),
    ])
}

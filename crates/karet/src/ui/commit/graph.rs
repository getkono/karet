//! The full-screen commit-graph view: the history DAG across the whole pane.
//!
//! The graph gets the entire width on purpose — a branchy history with many lanes and
//! long summaries is exactly what an embedded side pane would squeeze. Selecting a
//! commit opens it as its own tab instead.

use std::collections::HashMap;
use std::path::Path;

use karet_graph::RailRow;

use super::list;
use super::*;

/// Everything the graph view reads to paint one frame. Grouped so the painter keeps a
/// short signature as the header grows.
pub(in crate::ui) struct CommitGraphInput<'a> {
    /// Set when the view is scoped to one file's history.
    pub(in crate::ui) history_path: Option<&'a Path>,
    /// The loaded commits, newest first.
    pub(in crate::ui) commits: &'a [karet_vcs::Commit],
    /// The cached lane layout, parallel to `commits`.
    pub(in crate::ui) rails: &'a [RailRow],
    /// Refs decorating each commit hash.
    pub(in crate::ui) labels: &'a HashMap<String, Vec<karet_vcs::RefLabel>>,
    /// Branch, upstream and divergence for the header.
    pub(in crate::ui) repo_state: Option<&'a karet_vcs::RepositoryState>,
    /// Whether older history remains to be paged in.
    pub(in crate::ui) has_more: bool,
    /// Whether a page is in flight.
    pub(in crate::ui) loading: bool,
    /// The in-flight page request, for the delayed-loading policy.
    pub(in crate::ui) loading_since: Option<Pending>,
    /// The selected row.
    pub(in crate::ui) selected: usize,
}

/// The view's two-axis scroll state, written back as the frame is painted.
pub(in crate::ui) struct CommitGraphScroll<'a> {
    /// First visible row.
    pub(in crate::ui) list_offset: &'a mut u16,
    /// Horizontal offset.
    pub(in crate::ui) column: &'a mut u16,
    /// The painted rect of the commit rows: it drives how far ahead history is
    /// prefetched, and maps a click back to the commit under it.
    pub(in crate::ui) list_rect: &'a mut Rect,
}

/// Rows the header occupies: the branch line, the HEAD line, and a separating rule.
const HEADER_ROWS: u16 = 3;

pub(in crate::ui) fn draw_commit_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    input: &CommitGraphInput<'_>,
    scroll: CommitGraphScroll<'_>,
    hits: &mut ScrollHits,
) {
    let rows = Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).split(area);
    draw_header(f, theme, rows[0], input);
    draw_rows(f, theme, rows[1], input, scroll, hits);
}

/// The pinned header: which branch this is, where its tip is, and how much history is
/// loaded behind it.
fn draw_header(f: &mut Frame, theme: &Theme, area: Rect, input: &CommitGraphInput<'_>) {
    if area.height < HEADER_ROWS {
        return;
    }
    let strong = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);

    // Line 1 — the branch (or the file, when this view is scoped to one).
    let mut first = vec![Span::styled(" \u{2387} ", strong)];
    match input.history_path {
        Some(path) => {
            first.push(Span::styled(path.display().to_string(), strong));
            first.push(Span::styled("  file history", dim));
        },
        None => {
            let branch = input
                .repo_state
                .and_then(|state| state.branch.as_deref())
                .unwrap_or("detached HEAD");
            first.push(Span::styled(branch.to_string(), strong));
            if let Some(state) = input.repo_state {
                if let Some(upstream) = state.upstream.as_deref() {
                    first.push(Span::styled(format!("  \u{2192} {upstream}"), dim));
                }
                let mut parts = Vec::new();
                if state.ahead > 0 {
                    parts.push(format!("\u{2191}{}", state.ahead));
                }
                if state.behind > 0 {
                    parts.push(format!("\u{2193}{}", state.behind));
                }
                if let Some(operation) = state.operation {
                    parts.push(format!("{operation:?}"));
                }
                if !parts.is_empty() {
                    first.push(Span::styled(format!("  {}", parts.join(" ")), dim));
                }
            }
        },
    }

    // Line 2 — the tip commit, and how much history is behind it.
    let second = match input.commits.first() {
        Some(tip) => {
            let mut spans = vec![
                Span::styled(" \u{25C9} ", theme.style(ThemeRole::DiagnosticInfo)),
                Span::styled(
                    tip.short_hash.clone(),
                    theme.style(ThemeRole::DiagnosticWarning),
                ),
                Span::styled(format!("  {}", tip.author), muted),
                Span::styled(
                    format!("  committed {}", list::relative_time(tip.time)),
                    dim,
                ),
            ];
            let loaded = input.commits.len();
            let more = if input.has_more { "+" } else { "" };
            spans.push(Span::styled(
                format!("  \u{00b7}  {loaded}{more} commits loaded"),
                dim,
            ));
            Line::from(spans)
        },
        // Keep the row reserved rather than letting the header change height, and stay
        // blank on the fast path so nothing flashes.
        None if input.loading && !input.loading_since.is_some_and(Pending::visible) => {
            Line::raw("")
        },
        None if input.loading => Line::styled(" loading history\u{2026}", dim),
        None => Line::styled(" no commits yet", dim),
    };

    f.render_widget(
        Paragraph::new(vec![Line::from(first), second]),
        Rect { height: 2, ..area },
    );
    let rule = "\u{2500}".repeat(area.width as usize);
    f.render_widget(
        Paragraph::new(Line::styled(rule, theme.style(ThemeRole::IndentGuide))),
        Rect {
            y: area.y + 2,
            height: 1,
            ..area
        },
    );
}

/// The graph itself: a window of rows, panned in both axes.
fn draw_rows(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    input: &CommitGraphInput<'_>,
    scroll: CommitGraphScroll<'_>,
    hits: &mut ScrollHits,
) {
    let (area, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
    *scroll.list_rect = area;
    let height = usize::from(area.height);
    let dim = theme.style(ThemeRole::LineNumber);

    if input.commits.is_empty() {
        *scroll.list_offset = 0;
        return;
    }

    // One trailing affordance row exists only while more history remains; prefetching
    // normally keeps it out of reach, so seeing it means the fetch was outrun.
    let trailing = usize::from(input.has_more);
    let total = input.commits.len() + trailing;
    let max_offset = total.saturating_sub(height.max(1));
    let offset = usize::from(*scroll.list_offset).min(max_offset);
    *scroll.list_offset = u16::try_from(offset).unwrap_or(u16::MAX);

    let last = (offset + height).min(input.commits.len());
    let first = offset.min(last);
    let window = &input.commits[first..last];
    let entries = list::entries_from_commits(
        window,
        input.labels,
        // The tip carries the HEAD glyph, and only when it is actually on screen.
        (offset == 0).then_some(0),
    );
    let mut lines = list::commit_list_lines(
        theme,
        &entries,
        // The cache is kept parallel to `commits`, but slice defensively: a torn cache
        // should paint a bare row, never take the editor down.
        input.rails.get(first..last).unwrap_or(&[]),
        input.selected.checked_sub(offset),
    );
    if trailing == 1 && last == input.commits.len() && lines.len() < height {
        let label = if input.loading {
            " loading more\u{2026}"
        } else {
            " \u{22ef} more"
        };
        lines.push(Line::styled(label, dim));
    }

    // The horizontal extent is measured over the visible rows: it is what the viewer can
    // actually pan across right now, and measuring every loaded row each frame is the
    // cost this windowed render exists to avoid.
    let mut content_width = lines.iter().map(line_width).max().unwrap_or_default();
    // Pad the selected row so its highlight reads as a full bar rather than stopping at
    // the end of the summary — a `Paragraph` line only paints as wide as its content.
    if let Some(row) = input.selected.checked_sub(offset)
        && let Some(line) = lines.get_mut(row)
    {
        let pad = content_width
            .max(usize::from(area.width))
            .saturating_sub(line_width(line));
        if pad > 0 {
            line.spans.push(Span::styled(
                " ".repeat(pad),
                Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()),
            ));
        }
        content_width = content_width.max(line_width(line));
    }
    let max_column = content_width.saturating_sub(usize::from(area.width));
    *scroll.column = (*scroll.column).min(u16::try_from(max_column).unwrap_or(u16::MAX));

    f.render_widget(Paragraph::new(lines).scroll((0, *scroll.column)), area);
    hits.record_both(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, offset, height),
            ScrollExtent::new(
                content_width,
                usize::from(*scroll.column),
                usize::from(area.width),
            ),
        ),
        ScrollSurface::TabRows,
        ScrollSurface::TabColumns,
    );
}

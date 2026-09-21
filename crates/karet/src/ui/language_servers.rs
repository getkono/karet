//! The Language Servers manager tab: a toolbar, the provider table, and a
//! detail pane for the selected provider.
//!
//! The table lives here; the buttons, the detail pane and the status vocabulary
//! are modules of their own, so each stays well inside the file-size ceiling
//! rather than one file creeping up on it.

mod actions;
mod detail;
mod labels;

use actions::*;
use detail::*;
use labels::*;

use super::*;
use crate::tab::LanguageServerAction;
use crate::tab::LanguageServerActionHit;
use crate::tab::LanguageServerPendingKind;
use crate::tab::LanguageServersViewState;

pub(super) fn draw_language_servers(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut LanguageServersViewState,
    hits: &mut ScrollHits,
) {
    let detail_height = if area.height >= 16 { 7 } else { 4 };
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(detail_height),
    ])
    .split(area);
    draw_actions(f, theme, sections[0], view);
    draw_inventory(f, theme, sections[1], view, hits);
    draw_detail(f, theme, sections[2], view);
}

fn draw_actions(f: &mut Frame, theme: &Theme, area: Rect, view: &mut LanguageServersViewState) {
    view.action_hits.clear();
    let mut x = area.x;
    let y = area.y;
    let refreshing = view.inventory_request.is_some();
    let check_all_pending = view
        .pending
        .iter()
        .any(|pending| pending.kind == LanguageServerPendingKind::CheckAll);
    let has_installed = view
        .servers
        .iter()
        .any(|status| status.managed && status.installed.is_some());
    let mut buttons = vec![(
        if refreshing {
            "Refreshing…"
        } else {
            "↻ Refresh"
        },
        (!refreshing).then_some(LanguageServerAction::Refresh),
    )];
    if check_all_pending {
        buttons.push(("Checking all…", None));
    } else if has_installed {
        buttons.push(("U Check all", Some(LanguageServerAction::CheckAll)));
    }
    buttons.push(("/ Filter", Some(LanguageServerAction::Filter)));
    let mut spans = Vec::new();
    for (label, action) in buttons {
        let width = u16::try_from(label.width() + 2).unwrap_or(u16::MAX);
        if x.saturating_add(width) > area.right() {
            break;
        }
        let rect = Rect::new(x, y, width, 1);
        let hovered =
            action.is_some_and(|_| view.action_hover.is_some_and(|point| contains(rect, point)));
        spans.push(Span::styled(
            format!(" {label} "),
            action_style(theme, hovered, action.is_none(), action),
        ));
        if let Some(action) = action {
            view.action_hits.push(LanguageServerActionHit {
                rect,
                action,
                server: None,
            });
        }
        x = x.saturating_add(width + 1);
        spans.push(Span::raw(" "));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { height: 1, ..area },
    );
    let (filter, filter_role) = view.error.clone().map_or_else(
        || {
            (
                if view.filter.is_empty() {
                    "Filter: all".to_string()
                } else {
                    format!("Filter: {}", view.filter)
                },
                if view.filter.is_empty() {
                    ThemeRole::Muted
                } else {
                    ThemeRole::DiagnosticInfo
                },
            )
        },
        |error| (error, ThemeRole::DiagnosticError),
    );
    f.render_widget(
        Paragraph::new(filter).style(theme.style(filter_role)),
        Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        },
    );
}

fn draw_inventory(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    view: &mut LanguageServersViewState,
    hits: &mut ScrollHits,
) {
    view.table_rect = area;
    view.row_hits.clear();
    let border_style = theme.style(ThemeRole::IndentGuide);
    let table_block = Block::default()
        .title(" Language servers ")
        .borders(Borders::ALL)
        .border_style(border_style);
    let content = table_block.inner(area);
    f.render_widget(table_block, area);
    // Reserved inside the border so the box outline stays whole.
    let (content, tracks) = reserve_tracks(content, ScrollAxes::VERTICAL);

    let visible = view.visible_indices();
    if visible.is_empty() {
        let message = if view.servers.is_empty() {
            view.error.as_deref().or_else(|| {
                view.loading_since
                    .filter(|since| since.visible())
                    .map(|_| "Loading language servers…")
            })
        } else {
            Some("No language servers match the filter")
        };
        if let Some(message) = message {
            f.render_widget(
                Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .style(theme.style(ThemeRole::Muted)),
                content,
            );
        }
        return;
    }

    view.selected = view.selected.min(visible.len().saturating_sub(1));
    if content.height == 0 || content.width == 0 {
        return;
    }
    let stacked = content.width < 60;
    let action_column_width = if stacked {
        content.width
    } else {
        (content.width / 3).clamp(20, 38)
    };
    let meta_width = if stacked {
        content.width
    } else {
        content.width.saturating_sub(action_column_width)
    };
    let action_width = if stacked {
        action_column_width
    } else {
        action_column_width.saturating_sub(1)
    };
    let header = if stacked {
        "Server · runtime"
    } else if meta_width >= 72 {
        "Server               Languages        Source       Installed      Runtime"
    } else if meta_width >= 42 {
        "Server               Installed      Runtime"
    } else {
        "Server · runtime"
    };
    f.render_widget(
        Paragraph::new(header).style(Style::default().add_modifier(Modifier::BOLD)),
        Rect::new(content.x, content.y, meta_width, 1),
    );
    if !stacked {
        let action_header = Rect::new(
            content.x.saturating_add(meta_width),
            content.y,
            action_column_width,
            1,
        );
        let divider = Block::default()
            .borders(Borders::LEFT)
            .border_style(border_style);
        let action_header_inner = divider.inner(action_header);
        f.render_widget(divider, action_header);
        f.render_widget(
            Paragraph::new("Actions").style(Style::default().add_modifier(Modifier::BOLD)),
            action_header_inner,
        );
    }

    let content_height = content.height.saturating_sub(1);
    view.offset = view.offset.min(view.selected);
    while row_heights_through_selection(view, &visible, view.offset, action_width, stacked)
        > content_height
        && view.offset < view.selected
    {
        view.offset += 1;
    }

    let mut y = content.y.saturating_add(1);
    // Rows are two or more terminal rows tall depending on how their actions wrap,
    // so the extent is measured in servers, not rows — and the viewport is however
    // many the loop actually managed to paint.
    let mut painted = 0_usize;
    for (visible_index, &server_index) in visible.iter().enumerate().skip(view.offset) {
        let Some(status) = view.servers.get(server_index).cloned() else {
            continue;
        };
        let actions = server_actions(view, &status);
        let action_lines = action_line_count(&actions, action_width);
        let row_content_height = if stacked {
            1_u16.saturating_add(action_lines)
        } else {
            action_lines.max(1)
        };
        let wanted_height = row_content_height.saturating_add(1);
        if y >= content.bottom() {
            break;
        }
        let height = wanted_height.min(content.bottom().saturating_sub(y));
        let row_rect = Rect::new(content.x, y, content.width, height);
        let selected = visible_index == view.selected;
        let row_style = if selected {
            Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui())
        } else {
            Style::default()
        };
        let row_block = Block::default()
            .borders(Borders::TOP)
            .border_style(border_style)
            .style(row_style);
        let row_inner = row_block.inner(row_rect);
        f.render_widget(row_block, row_rect);
        f.render_widget(
            Paragraph::new(inventory_summary(theme, &status, meta_width)).style(row_style),
            Rect::new(row_inner.x, row_inner.y, meta_width, 1),
        );

        let (action_x, action_y) = if stacked {
            (row_inner.x, row_inner.y.saturating_add(1))
        } else {
            (row_inner.x.saturating_add(meta_width), row_inner.y)
        };
        let mut action_area = Rect::new(
            action_x,
            action_y,
            if stacked {
                action_width
            } else {
                action_column_width
            },
            row_inner.bottom().saturating_sub(action_y),
        );
        if !stacked {
            let divider = Block::default()
                .borders(Borders::LEFT)
                .border_style(border_style)
                .style(row_style);
            let inner = divider.inner(action_area);
            f.render_widget(divider, action_area);
            action_area = inner;
        }
        render_server_actions(f, theme, view, &status, &actions, action_area);
        view.row_hits.push((row_rect, status.server.clone()));
        painted += 1;
        y = y.saturating_add(wanted_height);
    }
    // The extent counts servers, not rows: the cards are variable height, so
    // `painted` is the viewport in the same unit `offset` is stored in.
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(visible.len(), view.offset, painted),
            ScrollExtent::default(),
        ),
        ScrollSurface::TabRows,
    );
}

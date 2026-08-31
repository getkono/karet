mod aicommit;

use karet_session::ChangeSummary;
use karet_widgets::textarea::TextArea;
use karet_widgets::textarea::TextAreaStyle;

use super::*;

pub(super) fn draw_scm(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    let header_rows = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    draw_repository_header(f, app, theme, header_rows[0]);
    let area = header_rows[1];
    // The commit editor is a permanent part of Source Control. Keep it layout-stable
    // while focus moves between the draft and the file lists.
    let input_height = area.height.min(5);
    let rows = Layout::vertical([Constraint::Length(input_height), Constraint::Min(0)]).split(area);
    draw_commit_input(f, app, theme, rows[0]);
    let list_area = rows[1];

    // A scrollable changes region on top; when there is commit history and room for
    // it, a resizable commit-log region pinned to the bottom with a drag divider.
    let has_log = !app.scm.log.is_empty() || app.scm.log_has_more;
    let (changes_area, commits_area) = if has_log && list_area.height > MIN_SCM_REGION * 2 + 1 {
        let commits_h = app.scm_ui.commits_h.clamp(
            MIN_SCM_REGION,
            list_area.height.saturating_sub(MIN_SCM_REGION + 1),
        );
        let parts = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(commits_h),
        ])
        .split(list_area);
        app.scm_ui.divider_y = parts[1].y;
        draw_scm_divider(f, theme, parts[1], app.scm_ui.resizing);
        (parts[0], Some(parts[2]))
    } else {
        app.scm_ui.divider_y = 0;
        (list_area, None)
    };

    draw_scm_changes(f, app, theme, changes_area, hits);
    if let Some(commits_area) = commits_area {
        draw_scm_commits(f, app, theme, commits_area, hits);
    } else {
        // No pinned region this frame: clear its state so stale hit-testing can't fire.
        app.scm_ui.commits_rect = Rect::default();
        app.scm_ui.commits_title_rect = Rect::default();
        app.scm_ui.commits_total = 0;
        app.scm_ui.more_row = None;
    }
}

/// Draw current branch/divergence plus direct Sync, Commit, and overflow actions.
pub(super) fn draw_repository_header(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.scm_ui.header_hits.clear();
    if area.height < 2 {
        return;
    }
    let branch = match app.scm.repository.as_ref() {
        Some(snapshot) => snapshot.state.branch.as_deref().unwrap_or("detached HEAD"),
        None if app
            .scm
            .repository_loading_since
            .is_some_and(Pending::visible) =>
        {
            "Loading repository…"
        },
        None => "Repository",
    };
    let state = app.scm.repository.as_ref().map(|snapshot| &snapshot.state);
    let divergence = state.map_or(String::new(), |state| {
        let mut parts = Vec::new();
        if state.ahead > 0 {
            parts.push(format!("↑{}", state.ahead));
        }
        if state.behind > 0 {
            parts.push(format!("↓{}", state.behind));
        }
        if let Some(operation) = state.operation {
            parts.push(format!("{operation:?}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("  {}", parts.join(" "))
        }
    });
    let branch_style = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ⎇ ", branch_style),
            Span::styled(branch.to_string(), branch_style),
            Span::styled(divergence, theme.style(ThemeRole::LineNumber)),
        ])),
        Rect { height: 1, ..area },
    );
    let action_row = area.y + 1;
    let labels = [
        (" Sync ", Command::ScmSync),
        (" Commit ", Command::ScmCommit),
        (" Branch ", Command::ScmSwitchBranch),
        (" ⋯ ", Command::ScmMenu),
    ];
    let mut x = area.x;
    let mut spans = Vec::new();
    for (label, command) in labels {
        let width = label.chars().count() as u16;
        if x.saturating_add(width) > area.right() {
            break;
        }
        app.scm_ui
            .header_hits
            .push((x, x + width, action_row, command));
        spans.push(Span::styled(
            label,
            theme
                .style(ThemeRole::Foreground)
                .bg(theme.role(ThemeRole::HoverHighlight).to_ratatui()),
        ));
        spans.push(Span::raw(" "));
        x = x.saturating_add(width + 1);
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            y: action_row,
            height: 1,
            ..area
        },
    );
}
/// Draw the horizontal drag divider between the changes and commit-log regions. It
/// brightens while a resize is active (mirrors the sidebar-width divider).
pub(super) fn draw_scm_divider(f: &mut Frame, theme: &Theme, area: Rect, active: bool) {
    let role = if active {
        ThemeRole::LineNumberActive
    } else {
        ThemeRole::IndentGuide
    };
    let style = theme.style(role);
    let rule = "\u{2500}".repeat(area.width as usize); // ─
    f.render_widget(Paragraph::new(Line::styled(rule, style)), area);
}

/// Draw the changes region. Both the staged and working sections are always shown;
/// an empty section renders a greyed placeholder line rather than collapsing, so the
/// layout stays stable as files move between them.
pub(super) fn draw_scm_changes(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    let selection_bg = theme.role(ThemeRole::Selection).to_ratatui();
    let hover_bg = theme.role(ThemeRole::HoverHighlight).to_ratatui();
    let hovered = app.hovered_scm_change();
    let cursor = app.scm.selection.cursor();
    let header_style = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    let placeholder_style = theme.style(ThemeRole::Muted);
    let mut items: Vec<ListItem> = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();

    // Both sections are always drawn, in order. Each reserves at least one line — a
    // greyed placeholder when empty — so staging a single file (moving it between the
    // two sections) never makes a header appear or disappear and shift the layout.
    let staged = app.scm.staged_count;
    let total_changes = app.scm.changes.len();
    let sections = [
        ("STAGED CHANGES", "No staged changes", 0..staged),
        ("CHANGES", "No changes", staged..total_changes),
    ];
    for (label, empty_hint, range) in sections {
        items.push(ListItem::new(Line::styled(
            format!(" {label}"),
            header_style,
        )));
        row_map.push(None);
        if range.is_empty() {
            items.push(ListItem::new(Line::styled(
                format!("   {empty_hint}"),
                placeholder_style,
            )));
            row_map.push(None);
            continue;
        }
        for i in range {
            let change = &app.scm.changes[i];
            let item = ListItem::new(change_line(theme, change, (change.added, change.removed)));
            // Every selected row (a contiguous range or a scattered toggle-set) gets
            // the selection background; the cursor row additionally gets a bold
            // highlight. A hovered-but-unselected row gets the secondary hover accent.
            let mut style = Style::default();
            if app.scm.selection.is_selected(i) {
                style = style.bg(selection_bg);
            } else if hovered == Some(i) {
                style = style.bg(hover_bg);
            }
            if i == cursor {
                style = style.add_modifier(Modifier::BOLD);
            }
            items.push(item.style(style));
            row_map.push(Some(i));
        }
    }

    let (area, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
    app.scm_ui.changes_rect = area;
    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_ui.offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_ui.row_map = row_map;
    app.scm_ui.offset = state.offset();
    app.scm_ui.total_rows = total;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, app.scm_ui.offset, height),
            ScrollExtent::default(),
        ),
        ScrollSurface::ScmChanges,
    );
}

pub(super) fn change_line(
    theme: &Theme,
    change: &ChangeSummary,
    (added, removed): (usize, usize),
) -> Line<'static> {
    let (glyph, role) = status_glyph(change.status);
    // Filename front and centre; the parent directory trails in dim grey and
    // is omitted entirely for files at the repo root.
    let name = change.path.file_name().map_or_else(
        || change.path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let parent = change
        .path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty());
    let mut spans = vec![
        Span::styled(format!(" {glyph} "), theme.style(role)),
        Span::raw(name),
    ];
    if let Some(parent) = parent {
        spans.push(Span::styled(
            format!("  {parent}"),
            theme.style(ThemeRole::LineNumber),
        ));
    }
    spans.extend([
        Span::styled(
            format!("   +{added}"),
            theme.style(ThemeRole::DiagnosticHint),
        ),
        Span::styled(
            format!(" \u{2212}{removed}"),
            theme.style(ThemeRole::DiagnosticError),
        ),
    ]);
    Line::from(spans)
}

/// Draw the `COMMITS` title as a button onto the full commit-graph view: it brightens
/// under the pointer and carries a hint once there is somewhere to go.
fn draw_commits_title(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let hovered = app.hovered_scm_commits_title();
    let mut style = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    if hovered {
        style = style.bg(theme.role(ThemeRole::HoverHighlight).to_ratatui());
    }
    let mut spans = vec![Span::styled(" COMMITS", style)];
    if hovered {
        spans.push(Span::styled(
            "  open graph \u{2192}",
            theme
                .style(ThemeRole::Muted)
                .bg(theme.role(ThemeRole::HoverHighlight).to_ratatui()),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Draw the pinned commit-log region: a clickable `COMMITS` title, the lazily-loaded
/// commits, and the "load more" affordance. The commit rows aren't selectable; clicking
/// one opens that commit, and clicking the title opens the full commit-graph view.
pub(super) fn draw_scm_commits(
    f: &mut Frame,
    app: &mut App,
    theme: &Theme,
    area: Rect,
    hits: &mut ScrollHits,
) {
    app.scm_ui.more_row = None;
    let dim = theme.style(ThemeRole::LineNumber);
    // The title is pinned above the list rather than scrolling with it: it is an
    // affordance, so it has to stay put and stay hit-testable.
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    let (title_area, area) = (rows[0], rows[1]);
    app.scm_ui.commits_title_rect = title_area;
    draw_commits_title(f, app, theme, title_area);
    let head = (app.scm_ui.commits_offset == 0).then_some(0);
    let entries =
        crate::ui::commit::list::entries_from_commits(&app.scm.log, &app.scm.ref_labels, head);
    let mut items = commit_list_items(theme, &entries, None);
    if app.scm.log_has_more {
        // The "load more" display row is relative to the commit region's top.
        app.scm_ui.more_row = Some(items.len());
        let label = if app.scm.log_loading_since.is_some_and(Pending::visible) {
            " loading…"
        } else {
            " ⋯ load more"
        };
        items.push(ListItem::new(Line::styled(label, dim)));
    }

    let (area, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_ui.commits_offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_ui.commits_offset = state.offset();
    app.scm_ui.commits_total = total;
    app.scm_ui.commits_rect = area;
    hits.record(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total, app.scm_ui.commits_offset, height),
            ScrollExtent::default(),
        ),
        ScrollSurface::ScmCommits,
    );
}

/// Draw the permanent multiline commit-message editor above the change list.
pub(super) fn draw_commit_input(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    if area.width == 0 || area.height == 0 {
        app.scm_ui.commit_rect = Rect::default();
        return;
    }
    let accent = theme.role(ThemeRole::LineNumberActive).to_ratatui();
    let muted = theme.role(ThemeRole::LineNumber).to_ratatui();
    // Short on purpose. The sidebar defaults to 30 columns, and the old
    // "· Ctrl+Enter commit" tail was wider than the whole panel — ratatui
    // truncated it away unseen, while still costing the border room the AI chip
    // needs. The commit chord lives in the status hints bar, which is where the
    // rest of the context-sensitive keys are advertised.
    let title = if app.commit_input.pending.is_some() {
        " Commit message · committing… "
    } else {
        " Commit message "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if app.commit_input.focused {
            accent
        } else {
            muted
        }));
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.scm_ui.commit_rect = inner;
    draw_ai_chip(f, app, theme, area, title);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    app.commit_input
        .edit
        .ensure_cursor_visible(&app.commit_input.text, inner.width, inner.height);
    let foreground = theme.role(ThemeRole::Foreground).to_ratatui();
    let selection = theme.role(ThemeRole::Selection).to_ratatui();
    f.render_widget(
        TextArea::new(&app.commit_input.text, &app.commit_input.edit)
            .focused(app.commit_input.focused)
            .style(TextAreaStyle::new(
                Style::default().fg(foreground),
                Style::default().fg(foreground).bg(selection),
                Style::default().fg(accent),
            ))
            .placeholder("Type a commit message", Style::default().fg(muted)),
        inner,
    );
}

/// Paint the AI affordance into the commit box's top border, and record where it
/// landed so a click can reach it.
///
/// The rect is cleared every frame before it is recomputed: a chip that fails to
/// fit, or a state with nothing to say, must not leave a stale click target
/// behind where the user would hit something they can no longer see.
fn draw_ai_chip(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect, title: &str) {
    app.scm_ui.ai_chip_rect = Rect::default();
    let now = std::time::Instant::now();
    let Some(chip) = aicommit::chip(app, now, app.icon_style) else {
        return;
    };
    let title_width = karet_widgets::text::width(title) as u16;
    // Longest phrasing that fits; a squeezed sidebar still gets the mark.
    let Some((label, rect)) = chip.fit(area, title_width) else {
        return;
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(label.to_string(), aicommit::chip_style(theme, chip.role)),
            Span::raw(" "),
        ])),
        rect,
    );
    app.scm_ui.ai_chip_rect = rect;
}

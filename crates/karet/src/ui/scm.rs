use super::*;

pub(super) fn draw_scm(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let header_rows = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    draw_repository_header(f, app, theme, header_rows[0]);
    let area = header_rows[1];
    // Reserve a top row for the commit-message input while it is open.
    let list_area = if app.commit_input.is_some() {
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
        draw_commit_input(f, app, theme, rows[0]);
        rows[1]
    } else {
        area
    };

    // A scrollable changes region on top; when there is commit history and room for
    // it, a resizable commit-log region pinned to the bottom with a drag divider.
    let has_log = !app.scm.log.is_empty() || app.scm.log_has_more;
    let (changes_area, commits_area) = if has_log && list_area.height > MIN_SCM_REGION * 2 + 1 {
        let commits_h = app.scm_commits_h.clamp(
            MIN_SCM_REGION,
            list_area.height.saturating_sub(MIN_SCM_REGION + 1),
        );
        let parts = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(commits_h),
        ])
        .split(list_area);
        app.scm_divider_y = parts[1].y;
        draw_scm_divider(f, theme, parts[1], app.scm_resizing);
        (parts[0], Some(parts[2]))
    } else {
        app.scm_divider_y = 0;
        (list_area, None)
    };

    draw_scm_changes(f, app, theme, changes_area);
    if let Some(commits_area) = commits_area {
        draw_scm_commits(f, app, theme, commits_area);
    } else {
        // No pinned region this frame: clear its state so stale hit-testing can't fire.
        app.scm_commits_rect = Rect::default();
        app.scm_commits_total = 0;
        app.scm_more_row = None;
    }
}

/// Draw current branch/divergence plus direct Sync, Commit, and overflow actions.
pub(super) fn draw_repository_header(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.scm_header_hits.clear();
    if area.height < 2 {
        return;
    }
    let branch = match app.scm.repository.as_ref() {
        Some(snapshot) => snapshot.state.branch.as_deref().unwrap_or("detached HEAD"),
        None if app
            .scm
            .repository_loading_since
            .is_some_and(loading_visible) =>
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
    let branch_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ⎇ ", branch_style),
            Span::styled(branch.to_string(), branch_style),
            Span::styled(
                divergence,
                Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
            ),
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
        app.scm_header_hits
            .push((x, x + width, action_row, command));
        spans.push(Span::styled(
            label,
            Style::default()
                .fg(theme.role(ThemeRole::Foreground).to_ratatui())
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
    let style = Style::default().fg(theme.role(role).to_ratatui());
    let rule = "\u{2500}".repeat(area.width as usize); // ─
    f.render_widget(Paragraph::new(Line::styled(rule, style)), area);
}

/// Draw the changes region. Both the staged and working sections are always shown;
/// an empty section renders a greyed placeholder line rather than collapsing, so the
/// layout stays stable as files move between them.
pub(super) fn draw_scm_changes(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let selection_bg = theme.role(ThemeRole::Selection).to_ratatui();
    let hover_bg = theme.role(ThemeRole::HoverHighlight).to_ratatui();
    let hovered = app.hovered_scm_change();
    let cursor = app.scm.selection.cursor();
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let placeholder_style = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
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
                Span::styled(
                    format!(" {glyph} "),
                    Style::default().fg(theme.role(role).to_ratatui()),
                ),
                Span::raw(name),
            ];
            if let Some(parent) = parent {
                spans.push(Span::styled(
                    format!("  {parent}"),
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ));
            }
            let item = ListItem::new(Line::from(spans));
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

    app.scm_changes_rect = area;
    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_row_map = row_map;
    app.scm_offset = state.offset();
    app.scm_total_rows = total;
}

/// Draw the pinned commit-log region (header, lazily-loaded commits, "load more").
/// Its rows aren't selectable; only the "load more" affordance is clickable.
pub(super) fn draw_scm_commits(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    app.scm_more_row = None;
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let header_style = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let hash_style = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    // A small cycle of distinct colours so adjacent branch lanes read apart. Like
    // other git tools, lane colour is decorative, so it uses fixed terminal colours
    // rather than theme tokens.
    const LANE_COLORS: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
        Color::Red,
    ];
    let lane_style = |lane: u8| Style::default().fg(LANE_COLORS[lane as usize % LANE_COLORS.len()]);
    let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(" COMMITS", header_style))];

    // Lay the loaded commits out as a DAG: one rail gutter per row, drawn to the left
    // of the hash/summary/age columns. The newest loaded commit (row 0, page 0) is the
    // current tip. Parents beyond the loaded window simply leave their lane open.
    let inputs: Vec<LaneInput> = app
        .scm
        .log
        .iter()
        .enumerate()
        .map(|(i, c)| LaneInput {
            id: c.hash.clone(),
            parents: c.parents.clone(),
            head: i == 0 && app.scm_commits_offset == 0,
        })
        .collect();
    let rails = assign_lanes(&inputs);
    for (commit, rail) in app.scm.log.iter().zip(rails.iter()) {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(render_rail(rail, lane_style).spans);
        spans.push(Span::styled(format!(" {} ", commit.short_hash), hash_style));
        spans.push(Span::raw(commit.summary.clone()));
        spans.push(Span::styled(
            format!("  {}", relative_time(commit.time)),
            dim,
        ));
        items.push(ListItem::new(Line::from(spans)));
    }
    if app.scm.log_has_more {
        // The "load more" display row is relative to the commit region's top.
        app.scm_more_row = Some(items.len());
        let label = if app.scm.log_loading_since.is_some_and(loading_visible) {
            " loading…"
        } else {
            " ⋯ load more"
        };
        items.push(ListItem::new(Line::styled(label, dim)));
    }

    let total = items.len();
    let height = area.height as usize;
    let offset = app.scm_commits_offset.min(total.saturating_sub(height));
    let mut state = ListState::default();
    *state.offset_mut() = offset;
    f.render_stateful_widget(List::new(items), area, &mut state);
    app.scm_commits_offset = state.offset();
    app.scm_commits_total = total;
    app.scm_commits_rect = area;
}

/// A terse `git log`-style relative time (e.g. `3d ago`) for a Unix timestamp.
pub(crate) fn relative_time(secs: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    relative_time_at(secs, now)
}

pub(super) fn relative_time_at(secs: i64, now: i64) -> String {
    let delta = now - secs;
    if delta < 0 {
        return "just now".to_string();
    }
    let (n, unit) = if delta < 60 {
        (delta, "s")
    } else if delta < 3600 {
        (delta / 60, "m")
    } else if delta < 86_400 {
        (delta / 3600, "h")
    } else if delta < 86_400 * 7 {
        (delta / 86_400, "d")
    } else if delta < 86_400 * 30 {
        (delta / (86_400 * 7), "w")
    } else if delta < 86_400 * 365 {
        (delta / (86_400 * 30), "mo")
    } else {
        (delta / (86_400 * 365), "y")
    };
    format!("{n}{unit} ago")
}

/// Draw the one-line commit-message input shown above the change list.
pub(super) fn draw_commit_input(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let message = app.commit_input.as_deref().unwrap_or("");
    let line = Line::from(vec![
        Span::styled(
            " commit ",
            Style::default()
                .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(message.to_string()),
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

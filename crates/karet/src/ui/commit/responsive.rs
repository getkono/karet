use super::*;
use crate::app::CommitFileHit;
use crate::tab::CommitLayoutMode;
use crate::tab::CommitViewState;

/// Minimum pane-content width for the pinned file rail beside commit diffs.
pub(in crate::ui) const WIDE_COMMIT_WIDTH: u16 = 104;

/// Geometry a responsive commit-like view reports to the pane coordinator.
#[derive(Default)]
pub(in crate::ui) struct CommitPaint {
    /// Visible signature badge, for the commit view's explanatory double-click.
    pub(in crate::ui) badge_rect: Option<Rect>,
    /// Visible file-index rows and their jump destinations.
    pub(in crate::ui) file_hits: Vec<CommitFileHit>,
}

#[allow(clippy::too_many_arguments)] // metadata, progressive file state, and view state are independent
pub(in crate::ui) fn draw_commit(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    detail: &karet_vcs::CommitDetail,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    verification: Option<&karet_session::GithubVerification>,
    explain_since: Option<Instant>,
    view: &mut CommitViewState,
) -> CommitPaint {
    let reveal = explain_since.is_some_and(|t| t.elapsed() < crate::app::COMMIT_REVEAL);
    let (header, badge) = commit_metadata_lines(theme, detail, verification, reveal);
    draw_responsive(f, theme, area, header, badge, files, file_status, view)
}

#[allow(clippy::too_many_arguments)] // range labels and layout state are independent
pub(in crate::ui) fn draw_compare(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
    files: &[render::FileView],
    view: &mut CommitViewState,
) -> CommitPaint {
    let header = compare_header_lines(theme, base_label, head_label, merge_base);
    draw_responsive(
        f,
        theme,
        area,
        header,
        None,
        files,
        CommitFileStatus::Ready,
        view,
    )
}

#[allow(clippy::too_many_arguments)] // shared renderer receives all model and transient state explicitly
fn draw_responsive(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    header: Vec<Line<'static>>,
    badge: Option<BadgeHit>,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    view: &mut CommitViewState,
) -> CommitPaint {
    let mode = if area.width >= WIDE_COMMIT_WIDTH {
        CommitLayoutMode::Wide
    } else {
        CommitLayoutMode::Stacked
    };
    match mode {
        CommitLayoutMode::Wide => {
            draw_wide(f, theme, area, header, badge, files, file_status, view)
        },
        CommitLayoutMode::Stacked => {
            draw_stacked(f, theme, area, header, badge, files, file_status, view)
        },
    }
}

#[derive(Default)]
struct FileDocument {
    prefix: Vec<Line<'static>>,
    anchors: Vec<u16>,
    toc_rows: Vec<u16>,
    rows: u16,
}

fn build_files(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
    stacked: bool,
    file_status: CommitFileStatus<'_>,
) -> FileDocument {
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let mut doc = FileDocument::default();
    match file_status {
        CommitFileStatus::Loading(since) => {
            if loading_visible(since) {
                doc.prefix
                    .push(Line::styled(" loading changed files\u{2026}", muted));
            }
            doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
            return doc;
        },
        CommitFileStatus::Failed(error) => {
            doc.prefix.push(Line::from(vec![
                Span::styled(" changed files unavailable", label),
                Span::raw("   "),
                Span::styled(error.to_string(), muted),
            ]));
            doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
            return doc;
        },
        CommitFileStatus::Ready => {},
    }

    if stacked {
        doc.prefix.push(Line::raw(""));
        doc.prefix.push(file_summary_line(theme, files));
        for file in files {
            doc.toc_rows
                .push(u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX));
            doc.prefix.push(file_index_line(theme, file, width, false));
        }
    }

    if files.is_empty() {
        if !stacked {
            doc.prefix.push(Line::styled(" No file changes", muted));
        }
        doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
        return doc;
    }
    let mut rows = doc.prefix.len();
    for file in files {
        rows = rows.saturating_add(1);
        doc.anchors.push(u16::try_from(rows).unwrap_or(u16::MAX));
        let card_rows = if width < 11 {
            1
        } else {
            render::unified_line_count(file, theme).saturating_add(2)
        };
        rows = rows.saturating_add(card_rows);
    }
    doc.rows = u16::try_from(rows).unwrap_or(u16::MAX);
    doc
}

fn visible_file_lines(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
    doc: &FileDocument,
    start: u16,
    height: u16,
) -> Vec<Line<'static>> {
    let start = usize::from(start);
    let end = start.saturating_add(usize::from(height));
    let mut lines = Vec::with_capacity(usize::from(height));
    let prefix_end = doc.prefix.len().min(end);
    if start < prefix_end {
        lines.extend(doc.prefix[start..prefix_end].iter().cloned());
    }
    let mut row = doc.prefix.len();
    for file in files.iter().take(doc.anchors.len()) {
        if row >= end {
            break;
        }
        if row >= start {
            lines.push(Line::raw(""));
        }
        row = row.saturating_add(1);
        let body_rows = if width < 11 {
            0
        } else {
            render::unified_line_count(file, theme)
        };
        let card_rows = body_rows.saturating_add(if width < 11 { 1 } else { 2 });
        let card_end = row.saturating_add(card_rows);
        if row < end && card_end > start {
            let local_start = start.saturating_sub(row);
            let local_end = end.min(card_end).saturating_sub(row);
            for local in local_start..local_end {
                if local == 0 {
                    lines.push(file_card_header(theme, file, width));
                } else if local <= body_rows {
                    lines.extend(file_card_body(theme, file, local - 1, 1));
                } else {
                    lines.push(file_card_footer(theme, width));
                }
            }
        }
        row = card_end;
    }
    lines
}

#[allow(clippy::too_many_arguments)]
fn draw_stacked(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    header: Vec<Line<'static>>,
    badge: Option<BadgeHit>,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    view: &mut CommitViewState,
) -> CommitPaint {
    let header_len = u16::try_from(header.len()).unwrap_or(u16::MAX);
    let file_doc = build_files(theme, files, area.width, true, file_status);
    let file_start = header_len;
    let anchors = offset_rows(&file_doc.anchors, file_start);
    remap_layout(view, CommitLayoutMode::Stacked, &anchors, header_len);
    let total = header_len.saturating_add(file_doc.rows);

    let normal_height = area.height.max(1);
    let normal_max = total.saturating_sub(normal_height);
    let sticky_max = total.saturating_sub(area.height.saturating_sub(1).max(1));
    view.scroll = view.scroll.min(sticky_max);
    let mut active = active_file(&anchors, view.scroll);
    let mut sticky = active.filter(|i| view.scroll > anchors[*i]);
    if sticky.is_some() && area.height > 1 {
        active = active_file(&anchors, view.scroll);
        sticky = active.filter(|i| view.scroll > anchors[*i]);
    } else {
        sticky = None;
        view.scroll = view.scroll.min(normal_max);
    }

    let body = if let Some(file) = sticky {
        let top = Rect { height: 1, ..area };
        f.render_widget(
            Paragraph::new(file_card_header(theme, &files[file], area.width)),
            top,
        );
        Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        }
    } else {
        area
    };
    let mut visible = header
        .iter()
        .skip(usize::from(view.scroll))
        .take(usize::from(body.height))
        .cloned()
        .collect::<Vec<_>>();
    let remaining = body
        .height
        .saturating_sub(u16::try_from(visible.len()).unwrap_or(u16::MAX));
    if remaining > 0 {
        let files_scroll = view.scroll.saturating_sub(header_len);
        visible.extend(visible_file_lines(
            theme,
            files,
            area.width,
            &file_doc,
            files_scroll,
            remaining,
        ));
    }
    f.render_widget(Paragraph::new(visible), body);

    let row_shift = u16::from(sticky.is_some());
    let toc_rows = offset_rows(&file_doc.toc_rows, file_start);
    let file_hits = toc_rows
        .iter()
        .enumerate()
        .filter_map(|(file, row)| {
            let screen = row.checked_sub(view.scroll)?.saturating_add(row_shift);
            (screen < area.height).then_some(CommitFileHit {
                rect: Rect {
                    y: area.y.saturating_add(screen),
                    height: 1,
                    ..area
                },
                file,
                scroll: anchors[file],
            })
        })
        .collect();
    let badge_rect = visible_badge(area, badge, view.scroll, 0);
    view.file_anchors = anchors;
    view.layout = Some(CommitLayoutMode::Stacked);
    CommitPaint {
        badge_rect,
        file_hits,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_wide(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    header: Vec<Line<'static>>,
    badge: Option<BadgeHit>,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    view: &mut CommitViewState,
) -> CommitPaint {
    let header_len = u16::try_from(header.len()).unwrap_or(u16::MAX);
    let rail_width = ((u32::from(area.width) * 30) / 100).clamp(31, 40) as u16;
    let diff_width = area.width.saturating_sub(rail_width.saturating_add(1));
    let file_doc = build_files(theme, files, diff_width, false, file_status);
    let anchors = offset_rows(&file_doc.anchors, header_len);
    remap_layout(view, CommitLayoutMode::Wide, &anchors, header_len);
    let total = header_len.saturating_add(file_doc.rows);
    view.scroll = view.scroll.min(total.saturating_sub(area.height));

    let header_visible = header_len.saturating_sub(view.scroll).min(area.height);
    if header_visible > 0 {
        let rect = Rect {
            height: header_visible,
            ..area
        };
        f.render_widget(Paragraph::new(header).scroll((view.scroll, 0)), rect);
    }
    let lower = Rect {
        y: area.y.saturating_add(header_visible),
        height: area.height.saturating_sub(header_visible),
        ..area
    };
    let cols = Layout::horizontal([
        Constraint::Length(rail_width),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(lower);
    if lower.height > 0 {
        let local_scroll = view.scroll.saturating_sub(header_len);
        f.render_widget(
            Paragraph::new(visible_file_lines(
                theme,
                files,
                diff_width,
                &file_doc,
                local_scroll,
                lower.height,
            )),
            cols[2],
        );
        f.render_widget(Block::new().borders(Borders::LEFT), cols[1]);
    }

    let active = active_file(&anchors, view.scroll)
        .unwrap_or(0)
        .min(files.len().saturating_sub(1));
    let mut file_hits = Vec::new();
    if lower.height > 0 && matches!(file_status, CommitFileStatus::Ready) {
        let summary = Rect {
            height: 1,
            ..cols[0]
        };
        f.render_widget(Paragraph::new(file_summary_line(theme, files)), summary);
        let list_height = lower.height.saturating_sub(1) as usize;
        keep_rail_visible(&mut view.rail_offset, active, files.len(), list_height);
        for (row, file) in files
            .iter()
            .enumerate()
            .skip(view.rail_offset)
            .take(list_height)
        {
            let y = lower
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row - view.rail_offset).unwrap_or(u16::MAX));
            let rect = Rect {
                y,
                height: 1,
                ..cols[0]
            };
            f.render_widget(
                Paragraph::new(file_index_line(theme, file, rail_width, row == active)),
                rect,
            );
            file_hits.push(CommitFileHit {
                rect,
                file: row,
                scroll: anchors.get(row).copied().unwrap_or(header_len),
            });
        }
    }

    let badge_rect = visible_badge(area, badge, view.scroll, 0)
        .filter(|rect| rect.y < lower.y || header_visible == area.height);
    view.file_anchors = anchors;
    view.layout = Some(CommitLayoutMode::Wide);
    CommitPaint {
        badge_rect,
        file_hits,
    }
}

fn remap_layout(
    view: &mut CommitViewState,
    next: CommitLayoutMode,
    next_anchors: &[u16],
    header_len: u16,
) {
    let Some(previous) = view.layout else {
        return;
    };
    if previous == next {
        return;
    }
    if let Some(file) = active_file(&view.file_anchors, view.scroll) {
        let within = view.scroll.saturating_sub(view.file_anchors[file]);
        if let Some(anchor) = next_anchors.get(file) {
            view.scroll = anchor.saturating_add(within);
        }
    } else {
        view.scroll = view.scroll.min(header_len);
    }
}

fn active_file(anchors: &[u16], scroll: u16) -> Option<usize> {
    anchors.iter().rposition(|anchor| *anchor <= scroll)
}

fn offset_rows(rows: &[u16], offset: u16) -> Vec<u16> {
    rows.iter().map(|row| row.saturating_add(offset)).collect()
}

fn keep_rail_visible(offset: &mut usize, active: usize, len: usize, height: usize) {
    if height == 0 || len == 0 {
        *offset = 0;
        return;
    }
    if active < *offset {
        *offset = active;
    } else if active >= offset.saturating_add(height) {
        *offset = active + 1 - height;
    }
    *offset = (*offset).min(len.saturating_sub(height));
}

fn visible_badge(area: Rect, badge: Option<BadgeHit>, scroll: u16, shift: u16) -> Option<Rect> {
    badge.and_then(|hit| {
        let row = hit.line.checked_sub(scroll)?.saturating_add(shift);
        (row < area.height).then_some(Rect {
            x: area.x.saturating_add(hit.col),
            y: area.y.saturating_add(row),
            width: hit.width.min(area.width.saturating_sub(hit.col)),
            height: 1,
        })
    })
}

fn file_summary_line(theme: &Theme, files: &[render::FileView]) -> Line<'static> {
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let add = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let remove = Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui());
    let (added, removed) = files.iter().fold((0usize, 0usize), |(a, r), file| {
        let (next_a, next_r) = file.line_stats();
        (a + next_a, r + next_r)
    });
    Line::from(vec![
        Span::styled(
            format!(
                " {} file{} changed",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            label,
        ),
        Span::raw("   "),
        Span::styled(format!("+{added}"), add),
        Span::raw(" "),
        Span::styled(format!("\u{2212}{removed}"), remove),
    ])
}

fn file_index_line(
    theme: &Theme,
    file: &render::FileView,
    width: u16,
    selected: bool,
) -> Line<'static> {
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let add = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let remove = Style::default().fg(theme.role(ThemeRole::DiagnosticError).to_ratatui());
    let (glyph, role) = status_glyph(file.change.status);
    let (added, removed) = file.line_stats();
    let stats = format!("+{added} \u{2212}{removed}");
    let stats_width = UnicodeWidthStr::width(stats.as_str());
    let show_stats = usize::from(width) >= 4 + stats_width + 4;
    let fixed = if show_stats { 4 + stats_width } else { 3 };
    let path_width = usize::from(width).saturating_sub(fixed).max(1);
    let path = truncate_start(&file.change.path.to_string_lossy(), path_width);
    let padding = if show_stats {
        usize::from(width).saturating_sub(3 + UnicodeWidthStr::width(path.as_str()) + stats_width)
    } else {
        0
    };
    let mut spans = vec![
        Span::styled(
            format!(" {glyph} "),
            Style::default().fg(theme.role(role).to_ratatui()),
        ),
        Span::styled(
            path,
            if selected {
                fg.add_modifier(Modifier::BOLD)
            } else {
                fg
            },
        ),
    ];
    if show_stats {
        spans.push(Span::raw(" ".repeat(padding)));
        spans.push(Span::styled(format!("+{added}"), add));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("\u{2212}{removed}"), remove));
    }
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()));
    }
    line
}

fn compare_header_lines(
    theme: &Theme,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
) -> Vec<Line<'static>> {
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let label = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());
    let hash = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    vec![
        Line::from(vec![
            Span::styled(" Comparing ", fg.add_modifier(Modifier::BOLD)),
            Span::styled(base_label.to_string(), hash),
            Span::styled(if merge_base { " \u{2026} " } else { " .. " }, muted),
            Span::styled(head_label.to_string(), hash),
        ]),
        Line::styled(
            format!(
                "  {}",
                if merge_base {
                    "changes since the two diverged (merge base)"
                } else {
                    "changes from the first to the second"
                }
            ),
            label,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_vcs::FileChange;
    use karet_vcs::StatusKind;

    use super::*;

    fn file(path: &str, old: &str, new: &str) -> render::FileView {
        render::FileView::new(
            FileChange {
                path: PathBuf::from(path),
                old_path: None,
                status: StatusKind::Modified,
                is_binary: false,
                old: old.to_owned(),
                new: new.to_owned(),
            },
            render::Section::Staged,
            false,
        )
    }

    #[test]
    fn file_document_windows_match_the_complete_document() {
        let theme = Theme::dark();
        let files = vec![
            file("src/a.rs", "one\ntwo\n", "one\nchanged\n"),
            file("src/b.rs", "old\n", "new\nmore\n"),
        ];
        let width = 72;
        let doc = build_files(&theme, &files, width, true, CommitFileStatus::Ready);
        let mut complete = doc.prefix.clone();
        for file in &files {
            complete.push(Line::raw(""));
            complete.extend(file_card(&theme, file, width));
        }
        assert_eq!(usize::from(doc.rows), complete.len());
        for start in 0..complete.len() {
            let actual = visible_file_lines(
                &theme,
                &files,
                width,
                &doc,
                u16::try_from(start).unwrap_or(u16::MAX),
                4,
            );
            let expected = complete
                .iter()
                .skip(start)
                .take(4)
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "window starting at row {start}");
        }
    }
}

//! The wide two-column commit layout, split from the responsive module to
//! respect the per-file code-line ceiling.

use super::*;

// A render helper threads every precomputed section through one call; a
// one-use bundling struct would only add indirection.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_wide(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    header: Vec<Line<'static>>,
    badge: Option<BadgeHit>,
    files: &[render::FileView],
    file_status: CommitFileStatus<'_>,
    view: &mut CommitViewState,
    hits: &mut ScrollHits,
) -> CommitPaint {
    // Reserved before the rail/diff split, so both columns are laid out at the width
    // they are painted at. The vertical bar reports the whole view's scroll (header
    // and diff move together) rather than hugging the diff column alone.
    let (area, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
    let header_len = u16::try_from(header.len()).unwrap_or(u16::MAX);
    let rail_width = ((u32::from(area.width) * 30) / 100).clamp(31, 40) as u16;
    let diff_width = area.width.saturating_sub(rail_width.saturating_add(1));
    let file_doc = build_files(
        theme,
        files,
        diff_width,
        false,
        file_status,
        &view.toggled_files,
    );
    let max_column = file_doc.columns.saturating_sub(usize::from(diff_width));
    view.column = view
        .column
        .min(u16::try_from(max_column).unwrap_or(u16::MAX));
    let anchors = offset_rows(&file_doc.anchors, header_len);
    remap_layout(view, CommitLayoutMode::Wide, &anchors, header_len);
    let total = header_len.saturating_add(file_doc.rows);
    let normal_max = total.saturating_sub(area.height.max(1));
    let sticky_max = total.saturating_sub(area.height.saturating_sub(1).max(1));
    view.scroll = view.scroll.min(sticky_max);
    let mut sticky = active_file(&anchors, view.scroll).filter(|i| view.scroll > anchors[*i]);
    if sticky.is_none() || area.height <= 1 {
        sticky = None;
        view.scroll = view.scroll.min(normal_max);
    }

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
    let diff_body = if let Some(file) = sticky {
        let top = Rect {
            height: 1,
            ..cols[2]
        };
        f.render_widget(
            Paragraph::new(file_card_header(
                theme,
                &files[file],
                diff_width,
                card_collapsed(&files[file], file, &view.toggled_files),
            ))
            .scroll((0, view.column)),
            top,
        );
        Rect {
            y: cols[2].y.saturating_add(1),
            height: cols[2].height.saturating_sub(1),
            ..cols[2]
        }
    } else {
        cols[2]
    };
    if lower.height > 0 {
        let local_scroll = view.scroll.saturating_sub(header_len);
        f.render_widget(
            Paragraph::new(visible_file_lines(
                theme,
                files,
                diff_width,
                &file_doc,
                local_scroll,
                diff_body.height,
                &view.toggled_files,
            ))
            .scroll((0, view.column)),
            diff_body,
        );
        f.render_widget(Block::new().borders(Borders::LEFT), cols[1]);
    }
    hits.record_both(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total.into(), view.scroll.into(), area.height.into()),
            ScrollExtent::new(file_doc.columns, view.column.into(), diff_width.into()),
        ),
        ScrollSurface::TabRows,
        ScrollSurface::TabColumns,
    );

    let row_shift = u16::from(sticky.is_some());
    let mut collapse_hits = visible_collapse_hits(
        cols[2],
        &anchors,
        view.scroll.max(header_len),
        row_shift,
        view.column,
    );
    if let Some(hit) = sticky.and_then(|file| collapse_hit(cols[2], file, cols[2].y, view.column)) {
        collapse_hits.push(hit);
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
        let rail_offset = rail_offset(active, files.len(), list_height);
        for (row, file) in files.iter().enumerate().skip(rail_offset).take(list_height) {
            let y = lower
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row - rail_offset).unwrap_or(u16::MAX));
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

    let badge_rect = visible_badge(area, badge, view.scroll, 0, 0)
        .filter(|rect| rect.y < lower.y || header_visible == area.height);
    view.file_anchors = anchors;
    view.layout = Some(CommitLayoutMode::Wide);
    CommitPaint {
        badge_rect,
        file_hits,
        collapse_hits,
    }
}

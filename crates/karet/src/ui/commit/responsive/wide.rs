//! The wide two-column commit layout, split from the responsive module to
//! respect the per-file code-line ceiling.

use super::super::list::keep_visible;
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
    tree: &[ChangedFileRow],
    file_status: CommitFileStatus<'_>,
    view: &mut CommitViewState,
    icons: IconStyle,
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
        tree,
        diff_width,
        false,
        file_status,
        &view.toggled_files,
        icons,
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
    let mut select_regions = Vec::new();
    if lower.height > 0 {
        let local_scroll = view.scroll.saturating_sub(header_len);
        select_regions.push(crate::app::SelectRegion {
            surface: crate::app::SelectSurface::CommitCards {
                prefix_rows: u16::try_from(file_doc.prefix.len()).unwrap_or(u16::MAX),
            },
            area: diff_body,
            first_row: usize::from(local_scroll),
            hscroll: usize::from(view.column),
        });
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
    let mut dir_hits = Vec::new();
    let mut rail_rect = None;
    if lower.height > 0 && matches!(file_status, CommitFileStatus::Ready) {
        let summary = Rect {
            height: 1,
            ..cols[0]
        };
        f.render_widget(Paragraph::new(file_summary_line(theme, files)), summary);
        let list = Rect {
            y: lower.y.saturating_add(1),
            height: lower.height.saturating_sub(1),
            ..cols[0]
        };
        // The rail gets its own track: it scrolls independently of the diff, so it
        // needs its own thumb to say where in the file list the reader is.
        let (list, list_tracks) = reserve_tracks(list, ScrollAxes::VERTICAL);
        rail_rect = Some(list);
        let selected = selected_row(tree, files, active);
        view.rail_scroll = reveal(view, tree.len(), list.height, selected, active);
        for (offset, row) in tree
            .iter()
            .enumerate()
            .skip(usize::from(view.rail_scroll))
            .take(usize::from(list.height))
        {
            let rect = Rect {
                y: list.y.saturating_add(
                    u16::try_from(offset)
                        .unwrap_or(u16::MAX)
                        .saturating_sub(view.rail_scroll),
                ),
                height: 1,
                ..list
            };
            f.render_widget(
                Paragraph::new(tree_line(
                    theme,
                    files,
                    row,
                    list.width,
                    icons,
                    Some(offset) == selected,
                )),
                rect,
            );
            match row {
                ChangedFileRow::File { file, .. } => file_hits.push(CommitFileHit {
                    rect,
                    file: *file,
                    scroll: anchors.get(*file).copied().unwrap_or(header_len),
                }),
                ChangedFileRow::Dir { path, .. } => dir_hits.push(CommitDirHit {
                    rect,
                    path: path.clone(),
                }),
            }
        }
        hits.record(
            list_tracks.paint(
                f.buffer_mut(),
                ScrollbarStyles::from_theme(theme),
                ScrollExtent::new(tree.len(), view.rail_scroll.into(), list.height.into()),
                ScrollExtent::default(),
            ),
            ScrollSurface::CommitFileRail,
        );
    }

    let badge_rect = visible_badge(area, badge, view.scroll, 0, 0)
        .filter(|rect| rect.y < lower.y || header_visible == area.height);
    view.file_anchors = anchors;
    view.layout = Some(CommitLayoutMode::Wide);
    CommitPaint {
        badge_rect,
        file_hits,
        dir_hits,
        collapse_hits,
        select_regions,
        rail_rect,
    }
}

/// The tree row the diff's active file is shown by: its own row, or — when it sits
/// inside a fold — the collapsed directory standing in for it.
///
/// A fold is a claim that what is inside does not need listing, not that the reader
/// has lost their place; the row that hides the active file is the row that marks it.
fn selected_row(
    tree: &[ChangedFileRow],
    files: &[render::FileView],
    active: usize,
) -> Option<usize> {
    if let Some(row) = tree.iter().position(|row| row.file() == Some(active)) {
        return Some(row);
    }
    let path = files.get(active)?.change.path.as_path();
    // Searched from the back, so the deepest ancestor still on screen wins over the
    // expanded directories above it.
    tree.iter()
        .rposition(|row| row.dir().is_some_and(|dir| path.starts_with(dir)))
}

/// The rail offset for this frame: clamped to the list, and pulled to the selected
/// row when — and only when — the diff has moved on to a different file.
///
/// Revealing on every frame would undo the manual scroll the rail now has, which is
/// the whole point of giving it one; revealing on a change is what keeps it an index
/// of where the diff actually is.
fn reveal(
    view: &mut CommitViewState,
    rows: usize,
    height: u16,
    selected: Option<usize>,
    active: usize,
) -> u16 {
    let max = u16::try_from(rows.saturating_sub(usize::from(height))).unwrap_or(u16::MAX);
    let offset = view.rail_scroll.min(max);
    if view.rail_revealed == Some(active) {
        return offset;
    }
    view.rail_revealed = Some(active);
    let Some(selected) = selected else {
        return offset;
    };
    u16::try_from(keep_visible(
        selected,
        usize::from(offset),
        usize::from(height),
    ))
    .unwrap_or(u16::MAX)
    .min(max)
}

use std::collections::BTreeSet;

use karet_filetype::IconStyle;

use super::rail::file_summary_line;
use super::rail::tree_line;
use super::*;
use crate::app::CommitCollapseHit;
use crate::app::CommitDirHit;
use crate::app::CommitFileHit;
use crate::tab::ChangedFileRow;
use crate::tab::CommitLayoutMode;
use crate::tab::CommitViewState;
use crate::tab::changed_file_rows;

mod wide;
use wide::draw_wide;

/// Minimum pane-content width for the pinned file rail beside commit diffs.
pub(in crate::ui) const WIDE_COMMIT_WIDTH: u16 = 104;

/// Geometry a responsive commit-like view reports to the pane coordinator.
#[derive(Default)]
pub(in crate::ui) struct CommitPaint {
    /// Visible signature badge, for the commit view's explanatory double-click.
    pub(in crate::ui) badge_rect: Option<Rect>,
    /// Visible file-index rows and their jump destinations.
    pub(in crate::ui) file_hits: Vec<CommitFileHit>,
    /// Visible directory rows of the file index, and the fold each one toggles.
    pub(in crate::ui) dir_hits: Vec<CommitDirHit>,
    /// Visible file-card disclosure controls.
    pub(in crate::ui) collapse_hits: Vec<CommitCollapseHit>,
    /// Where the file cards painted their rows, for pointer selection.
    pub(in crate::ui) select_regions: Vec<crate::app::SelectRegion>,
    /// The wide layout's file rail, for routing a wheel notch over it to the rail's
    /// own offset rather than the document's. `None` in the stacked layout, whose
    /// index scrolls with the document by construction.
    pub(in crate::ui) rail_rect: Option<Rect>,
}

#[allow(clippy::too_many_arguments)] // metadata, progressive file state, and view state are independent
pub(in crate::ui) fn draw_commit(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    detail: &karet_vcs::CommitDetail,
    files: &CommitFiles,
    explain_since: Option<Instant>,
    view: &mut CommitViewState,
    icons: IconStyle,
    hits: &mut ScrollHits,
) -> CommitPaint {
    let reveal = explain_since.is_some_and(|t| t.elapsed() < crate::app::COMMIT_REVEAL);
    let (header, badge) = commit_metadata_lines(theme, detail, files.verification.as_ref(), reveal);
    draw_responsive(
        f,
        theme,
        area,
        header,
        badge,
        &files.files,
        file_load_status(files),
        view,
        icons,
        hits,
    )
}

#[allow(clippy::too_many_arguments)] // range labels and layout state are independent
pub(in crate::ui) fn draw_compare(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
    files: &CommitFiles,
    view: &mut CommitViewState,
    icons: IconStyle,
    hits: &mut ScrollHits,
) -> CommitPaint {
    let header = compare_header_lines(theme, base_label, head_label, merge_base);
    draw_responsive(
        f,
        theme,
        area,
        header,
        None,
        &files.files,
        file_load_status(files),
        view,
        icons,
        hits,
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
    icons: IconStyle,
    hits: &mut ScrollHits,
) -> CommitPaint {
    let mode = if area.width >= WIDE_COMMIT_WIDTH {
        CommitLayoutMode::Wide
    } else {
        CommitLayoutMode::Stacked
    };
    // Built once here rather than per layout: both paint the same index, and a
    // second construction site is a second place for the two to disagree.
    let tree = changed_file_rows(files, &view.collapsed_dirs);
    match mode {
        CommitLayoutMode::Wide => draw_wide(
            f,
            theme,
            area,
            header,
            badge,
            files,
            &tree,
            file_status,
            view,
            icons,
            hits,
        ),
        CommitLayoutMode::Stacked => draw_stacked(
            f,
            theme,
            area,
            header,
            badge,
            files,
            &tree,
            file_status,
            view,
            icons,
            hits,
        ),
    }
}

#[derive(Default)]
struct FileDocument {
    prefix: Vec<Line<'static>>,
    anchors: Vec<u16>,
    toc: Vec<TocEntry>,
    rows: u16,
    columns: usize,
}

/// A table-of-contents row in the stacked layout's document, and what clicking it
/// does. Directory rows fold; file rows jump to that file's card.
struct TocEntry {
    /// The row's index within the *file* document, i.e. before the metadata header
    /// above it is added — the same frame [`FileDocument::anchors`] uses.
    row: u16,
    /// What the row addresses.
    target: TocTarget,
}

/// What a table-of-contents row addresses.
enum TocTarget {
    /// A changed file, by its index into the view's `files`.
    File(usize),
    /// A directory to fold, by the path its row reports.
    Dir(PathBuf),
}

/// Whether file `index` renders collapsed: its machine-maintained default,
/// flipped when the user has toggled that card.
///
/// The renderers take the override set rather than the whole view state so they
/// stay directly testable.
fn card_collapsed(file: &render::FileView, index: usize, toggled_files: &BTreeSet<usize>) -> bool {
    auto_collapse_reason(file).is_some() ^ toggled_files.contains(&index)
}

#[allow(clippy::too_many_arguments)] // the tree, the card state and the load state are independent
fn build_files(
    theme: &Theme,
    files: &[render::FileView],
    tree: &[ChangedFileRow],
    width: u16,
    stacked: bool,
    file_status: CommitFileStatus<'_>,
    toggled_files: &BTreeSet<usize>,
    icons: IconStyle,
) -> FileDocument {
    let muted = theme.style(ThemeRole::Muted);
    let label = theme.style(ThemeRole::LineNumberActive);
    let mut doc = FileDocument::default();
    match file_status {
        CommitFileStatus::Loading(since) => {
            if since.visible() {
                doc.prefix
                    .push(Line::styled(" loading changed files\u{2026}", muted));
            }
            doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
            doc.columns = doc.prefix.iter().map(line_width).max().unwrap_or_default();
            return doc;
        },
        CommitFileStatus::Failed(error) => {
            doc.prefix.push(Line::from(vec![
                Span::styled(" changed files unavailable", label),
                Span::raw("   "),
                Span::styled(error.to_string(), muted),
            ]));
            doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
            doc.columns = doc.prefix.iter().map(line_width).max().unwrap_or_default();
            return doc;
        },
        CommitFileStatus::Ready => {},
    }

    if stacked {
        doc.prefix.push(Line::raw(""));
        doc.prefix.push(file_summary_line(theme, files));
        for row in tree {
            let target = match row {
                ChangedFileRow::Dir { path, .. } => TocTarget::Dir(path.clone()),
                ChangedFileRow::File { file, .. } => TocTarget::File(*file),
            };
            doc.toc.push(TocEntry {
                row: u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX),
                target,
            });
            doc.prefix
                .push(tree_line(theme, files, row, width, icons, false));
        }
    }

    if files.is_empty() {
        if !stacked {
            doc.prefix.push(Line::styled(" No file changes", muted));
        }
        doc.rows = u16::try_from(doc.prefix.len()).unwrap_or(u16::MAX);
        doc.columns = doc.prefix.iter().map(line_width).max().unwrap_or_default();
        return doc;
    }
    doc.columns = doc
        .prefix
        .iter()
        .map(line_width)
        .max()
        .unwrap_or_default()
        .max(usize::from(width));
    let mut rows = doc.prefix.len();
    for (index, file) in files.iter().enumerate() {
        let collapsed = card_collapsed(file, index, toggled_files);
        if !collapsed {
            doc.columns = doc
                .columns
                .max(render::unified_max_width(file, theme).saturating_add(2));
        }
        rows = rows.saturating_add(1);
        doc.anchors.push(u16::try_from(rows).unwrap_or(u16::MAX));
        let card_rows = if width < FILE_CARD_MIN_WIDTH || collapsed {
            1
        } else {
            render::unified_line_count(file, theme).saturating_add(2)
        };
        rows = rows.saturating_add(card_rows);
    }
    doc.rows = u16::try_from(rows).unwrap_or(u16::MAX);
    doc
}

/// Columns the file card's `\u{2502} ` rail occupies before a diff row's own gutter.
pub(crate) const CARD_RAIL_WIDTH: u16 = 2;

/// The copyable content of the file-card document's row `row`.
///
/// Mirrors [`visible_file_lines`]'s walk — `prefix_rows` of index and summary,
/// then per file a blank separator, a card header, its diff body and a footer —
/// so the row a selection names is the row that was painted there. Only body
/// rows carry content; the chrome around them reports `None` and is therefore
/// neither selectable nor copied.
pub(crate) fn document_row(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
    prefix_rows: usize,
    toggled_files: &BTreeSet<usize>,
    row: usize,
) -> Option<crate::app::SurfaceRow> {
    if row < prefix_rows {
        return None;
    }
    let mut next = prefix_rows;
    for (index, file) in files.iter().enumerate() {
        // Each card opens with a blank separator row.
        if row == next {
            return None;
        }
        next = next.saturating_add(1);
        let collapsed = card_collapsed(file, index, toggled_files);
        let body_rows = if width < FILE_CARD_MIN_WIDTH || collapsed {
            0
        } else {
            render::unified_line_count(file, theme)
        };
        let card_rows = if body_rows == 0 {
            1
        } else {
            body_rows.saturating_add(2)
        };
        if row < next.saturating_add(card_rows) {
            let local = row.saturating_sub(next);
            // Row 0 is the card header and the last is its footer.
            if local == 0 || local > body_rows {
                return None;
            }
            let content = karet_diff::unified_row(&file.change.diff, local - 1)?;
            return Some(crate::app::SurfaceRow {
                text: content.text,
                content_x: content.gutter_width.saturating_add(CARD_RAIL_WIDTH),
            });
        }
        next = next.saturating_add(card_rows);
    }
    None
}

fn visible_file_lines(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
    doc: &FileDocument,
    start: u16,
    height: u16,
    toggled_files: &BTreeSet<usize>,
) -> Vec<Line<'static>> {
    let start = usize::from(start);
    let end = start.saturating_add(usize::from(height));
    let mut lines = Vec::with_capacity(usize::from(height));
    let prefix_end = doc.prefix.len().min(end);
    if start < prefix_end {
        lines.extend(doc.prefix[start..prefix_end].iter().cloned());
    }
    let mut row = doc.prefix.len();
    for (index, file) in files.iter().take(doc.anchors.len()).enumerate() {
        if row >= end {
            break;
        }
        if row >= start {
            lines.push(Line::raw(""));
        }
        row = row.saturating_add(1);
        let collapsed = card_collapsed(file, index, toggled_files);
        let body_rows = if width < FILE_CARD_MIN_WIDTH || collapsed {
            0
        } else {
            render::unified_line_count(file, theme)
        };
        let card_rows = if width < FILE_CARD_MIN_WIDTH || collapsed {
            1
        } else {
            body_rows.saturating_add(2)
        };
        let card_end = row.saturating_add(card_rows);
        if row < end && card_end > start {
            let local_start = start.saturating_sub(row);
            let local_end = end.min(card_end).saturating_sub(row);
            for local in local_start..local_end {
                if local == 0 {
                    lines.push(file_card_header(theme, file, width, collapsed));
                } else if local <= body_rows {
                    lines.extend(file_card_body(theme, file, local - 1, 1, width));
                } else {
                    lines.push(file_card_footer(theme, width));
                }
            }
        }
        row = card_end;
    }
    lines
}

// A render helper threads every precomputed section through one call; a
// one-use bundling struct would only add indirection.
#[allow(clippy::too_many_arguments)]
fn draw_stacked(
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
    // Reserve the tracks first so the file cards below are built to the width they
    // will actually be painted at, and every hit rect derived from `area` lands
    // inside the content rather than under a bar.
    let (area, tracks) = reserve_tracks(area, ScrollAxes::BOTH);
    let header_len = u16::try_from(header.len()).unwrap_or(u16::MAX);
    let file_doc = build_files(
        theme,
        files,
        tree,
        area.width,
        true,
        file_status,
        &view.toggled_files,
        icons,
    );
    let content_width = file_doc
        .columns
        .max(header.iter().map(line_width).max().unwrap_or_default());
    let max_column = content_width.saturating_sub(usize::from(area.width));
    view.column = view
        .column
        .min(u16::try_from(max_column).unwrap_or(u16::MAX));
    let file_start = header_len;
    let anchors = offset_rows(&file_doc.anchors, file_start);
    // Before the layout remap, which owns the other reason the anchors can move.
    remap_prefix(
        view,
        file_start.saturating_add(u16::try_from(file_doc.prefix.len()).unwrap_or(u16::MAX)),
    );
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
            Paragraph::new(file_card_header(
                theme,
                &files[file],
                area.width,
                card_collapsed(&files[file], file, &view.toggled_files),
            ))
            .scroll((0, view.column)),
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
    let header_shown = u16::try_from(visible.len()).unwrap_or(u16::MAX);
    let remaining = body.height.saturating_sub(header_shown);
    let mut select_regions = Vec::new();
    if remaining > 0 {
        let files_scroll = view.scroll.saturating_sub(header_len);
        // The header and the cards share one paragraph, so the cards begin
        // wherever the visible header ran out.
        select_regions.push(crate::app::SelectRegion {
            surface: crate::app::SelectSurface::CommitCards {
                prefix_rows: u16::try_from(file_doc.prefix.len()).unwrap_or(u16::MAX),
            },
            area: Rect {
                y: body.y.saturating_add(header_shown),
                height: remaining,
                ..body
            },
            first_row: usize::from(files_scroll),
            hscroll: usize::from(view.column),
        });
        visible.extend(visible_file_lines(
            theme,
            files,
            area.width,
            &file_doc,
            files_scroll,
            remaining,
            &view.toggled_files,
        ));
    }
    f.render_widget(Paragraph::new(visible).scroll((0, view.column)), body);
    hits.record_both(
        tracks.paint(
            f.buffer_mut(),
            ScrollbarStyles::from_theme(theme),
            ScrollExtent::new(total.into(), view.scroll.into(), area.height.into()),
            ScrollExtent::new(content_width, view.column.into(), area.width.into()),
        ),
        ScrollSurface::TabRows,
        ScrollSurface::TabColumns,
    );

    let row_shift = u16::from(sticky.is_some());
    let mut file_hits = Vec::new();
    let mut dir_hits = Vec::new();
    for entry in &file_doc.toc {
        let Some(screen) = entry
            .row
            .saturating_add(file_start)
            .checked_sub(view.scroll)
            .map(|screen| screen.saturating_add(row_shift))
            .filter(|screen| *screen < area.height)
        else {
            continue;
        };
        let rect = Rect {
            y: area.y.saturating_add(screen),
            height: 1,
            ..area
        };
        match &entry.target {
            TocTarget::File(file) => file_hits.push(CommitFileHit {
                rect,
                file: *file,
                scroll: anchors.get(*file).copied().unwrap_or(file_start),
            }),
            TocTarget::Dir(path) => dir_hits.push(CommitDirHit {
                rect,
                path: path.clone(),
            }),
        }
    }
    let mut collapse_hits =
        visible_collapse_hits(area, &anchors, view.scroll, row_shift, view.column);
    if let Some(hit) = sticky.and_then(|file| collapse_hit(area, file, area.y, view.column)) {
        collapse_hits.push(hit);
    }
    let badge_rect = visible_badge(area, badge, view.scroll, 0, view.column);
    view.file_anchors = anchors;
    view.layout = Some(CommitLayoutMode::Stacked);
    CommitPaint {
        badge_rect,
        file_hits,
        dir_hits,
        collapse_hits,
        select_regions,
        rail_rect: None,
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
    // The rail is not on screen in the stacked layout and its offset means nothing
    // there, so the next wide frame must reveal the active file afresh.
    view.rail_revealed = None;
    if let Some(file) = active_file(&view.file_anchors, view.scroll) {
        let within = view.scroll.saturating_sub(view.file_anchors[file]);
        if let Some(anchor) = next_anchors.get(file) {
            view.scroll = anchor.saturating_add(within);
        }
    } else {
        view.scroll = view.scroll.min(header_len);
    }
}

/// Absorb a change in how many rows precede the first file card.
///
/// Folding a directory in the stacked layout shortens the table of contents, which
/// moves every card anchor up by the same amount. Without this the offset stays
/// where it was and the diff slides out from under the reader; with it, whatever
/// was on screen stays there. Rows still inside the prefix are left alone — the
/// index is what the reader is looking at, and it does not move.
fn remap_prefix(view: &mut CommitViewState, prefix_rows: u16) {
    let previous = std::mem::replace(&mut view.prefix_rows, prefix_rows);
    // A *layout* change moves the prefix too, but `remap_layout` already remaps
    // that by card anchor; correcting it twice would double the shift.
    // `<`, not `<=`: `previous` is the first row *after* the index (the blank
    // separator above card 0), so a reader parked exactly there is looking at the
    // cards and has to be carried along with them.
    if view.layout != Some(CommitLayoutMode::Stacked)
        || previous == prefix_rows
        || view.scroll < previous
    {
        return;
    }
    view.scroll = if prefix_rows > previous {
        view.scroll.saturating_add(prefix_rows - previous)
    } else {
        view.scroll.saturating_sub(previous - prefix_rows)
    };
}

fn active_file(anchors: &[u16], scroll: u16) -> Option<usize> {
    anchors.iter().rposition(|anchor| *anchor <= scroll)
}

fn offset_rows(rows: &[u16], offset: u16) -> Vec<u16> {
    rows.iter().map(|row| row.saturating_add(offset)).collect()
}

fn collapse_hit(area: Rect, file: usize, y: u16, scroll_column: u16) -> Option<CommitCollapseHit> {
    let content_column: u16 = if area.width < FILE_CARD_MIN_WIDTH {
        0
    } else {
        3
    };
    let column = content_column.checked_sub(scroll_column)?;
    (column < area.width).then_some(CommitCollapseHit {
        rect: Rect::new(area.x.saturating_add(column), y, 1, 1),
        file,
    })
}

fn visible_collapse_hits(
    area: Rect,
    anchors: &[u16],
    scroll: u16,
    row_shift: u16,
    column: u16,
) -> Vec<CommitCollapseHit> {
    anchors
        .iter()
        .enumerate()
        .filter_map(|(file, anchor)| {
            let screen = anchor.checked_sub(scroll)?.saturating_add(row_shift);
            (screen < area.height)
                .then(|| collapse_hit(area, file, area.y.saturating_add(screen), column))
                .flatten()
        })
        .collect()
}

fn visible_badge(
    area: Rect,
    badge: Option<BadgeHit>,
    scroll: u16,
    shift: u16,
    column: u16,
) -> Option<Rect> {
    badge.and_then(|hit| {
        let row = hit.line.checked_sub(scroll)?.saturating_add(shift);
        let col = hit.col.checked_sub(column)?;
        (row < area.height && col < area.width).then_some(Rect {
            x: area.x.saturating_add(col),
            y: area.y.saturating_add(row),
            width: hit.width.min(area.width.saturating_sub(col)),
            height: 1,
        })
    })
}

fn compare_header_lines(
    theme: &Theme,
    base_label: &str,
    head_label: &str,
    merge_base: bool,
) -> Vec<Line<'static>> {
    let fg = theme.style(ThemeRole::Foreground);
    let label = theme.style(ThemeRole::LineNumberActive);
    let hash = theme.style(ThemeRole::DiagnosticWarning);
    let muted = theme.style(ThemeRole::Muted);
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
#[path = "responsive/tests.rs"]
mod tests;

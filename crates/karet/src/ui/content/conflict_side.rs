//! One read-only side (ours / theirs) of the three-way merge-conflict view.
//!
//! Split out of [`super`] because the pane painter sits at the repo's per-file
//! code-line ceiling.

use super::*;

#[allow(clippy::too_many_arguments)] // presentation state and delayed-load state are independent
pub(super) fn draw_conflict_side(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    buffer: Option<&TextBuffer>,
    editor: &mut EditorState,
    loading_since: Pending,
    error: Option<&str>,
    word_wrap: bool,
    ctx: &PaneCtx,
    hits: &mut ScrollHits,
) {
    if let Some(buffer) = buffer {
        let (text_rect, tracks) = reserve_tracks(area, ScrollAxes::VERTICAL);
        f.render_stateful_widget(
            ctx.display
                .apply_read_only(Editor::new(buffer))
                .theme(theme)
                .focused(false)
                .read_only(true)
                .word_wrap(word_wrap)
                .tab_width(ctx.tab_width),
            text_rect,
            editor,
        );
        // The side panes copy the merged editor's offset every frame, so their bars
        // grab the same thing the middle one does.
        hits.record(
            tracks.paint(
                f.buffer_mut(),
                ScrollbarStyles::from_theme(theme),
                ScrollExtent::new(
                    buffer.line_count(),
                    editor.scroll_line as usize,
                    editor.visible_lines() as usize,
                ),
                ScrollExtent::default(),
            ),
            ScrollSurface::TabRows,
        );
    } else if let Some(error) = error {
        f.render_widget(
            Paragraph::new(error).style(theme.style(ThemeRole::DiagnosticError)),
            area,
        );
    } else if loading_since.visible() {
        f.render_widget(
            Paragraph::new("Loading conflict side…").style(theme.style(ThemeRole::Muted)),
            area,
        );
    }
}

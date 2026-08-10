use super::*;

impl App {
    /// Scroll the active non-wrapping view horizontally by `delta` columns.
    pub(super) fn scroll_columns(&mut self, delta: i32) {
        let word_wrap = self.tabs.get(self.active).is_some_and(|tab| {
            effective_word_wrap(
                tab,
                self.settings
                    .editor
                    .for_language(tab_language(tab))
                    .word_wrap(),
            )
        });
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        match &mut tab.kind {
            TabKind::Code { buffer, .. } if !word_wrap => {
                tab.editor.scroll_columns(buffer, delta);
            },
            TabKind::Diff { pager, .. }
            | TabKind::StashPreview { pager, .. }
            | TabKind::Graph { pager, .. }
            | TabKind::LoadedConfig { pager, .. }
            | TabKind::CommitLoading { pager, .. } => adjust(&mut pager.column, delta),
            TabKind::CommitGraph {
                detail_column: column,
                ..
            } => adjust(column, delta),
            TabKind::Commit { view, .. } | TabKind::Compare { view, .. } => {
                adjust(&mut view.column, delta);
            },
            _ => {},
        }
    }
}

fn adjust(offset: &mut u16, delta: i32) {
    let next = (i64::from(*offset) + i64::from(delta)).clamp(0, i64::from(u16::MAX));
    *offset = next as u16;
}

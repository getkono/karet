//! Application wiring for Expand and Shrink Selection (Shift+Alt+Right/Left):
//! parse the active document — injected languages included, so a selection in a
//! Markdown code fence grows through the fence's own language first — and hand
//! the enclosing node ranges to `karet_editor`, which picks each caret's step
//! and keeps the history Shrink walks back.

use karet_core::Span;
use karet_treesitter::LayeredTree;

use super::*;

impl App {
    /// Grow every selection in the active code tab to the next enclosing syntax
    /// node; a bare caret on a word takes the word first. A file with no grammar
    /// still gets the word step.
    pub(super) fn expand_selection(&mut self) {
        let Some(Tab {
            kind: TabKind::Code { path, buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        let tree = karet_treesitter::language_id_from_path(path).and_then(|lang| {
            karet_treesitter::LayeredParser::new()
                .parse(lang, &buffer.text())
                .ok()
        });
        editor.expand_selection(buffer, |range| {
            tree.as_ref()
                .map(|tree| enclosing_ranges(tree, buffer, range))
                .unwrap_or_default()
        });
    }

    /// Restore the selections the last Expand Selection grew from, in the active
    /// code tab.
    pub(super) fn shrink_selection(&mut self) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.shrink_selection(buffer);
        }
    }
}

/// The ranges of every node in `tree` that encloses `range`, smallest first.
fn enclosing_ranges(tree: &LayeredTree, buffer: &TextBuffer, range: Range) -> Vec<Range> {
    let (Ok(start), Ok(end)) = (
        buffer.line_col_to_byte(range.start),
        buffer.line_col_to_byte(range.end),
    ) else {
        return Vec::new();
    };
    tree.enclosing_spans(Span { start, end })
        .into_iter()
        .map(|span| Range {
            start: buffer.byte_to_line_col(span.start),
            end: buffer.byte_to_line_col(span.end),
        })
        .collect()
}

//! The editor's pointer selection drag: its granularity, and autoscroll.
//!
//! A press in the editor arms a drag that owns the pointer until release. Two
//! things make it feel like a GUI rather than a hit test repeated every event:
//! the drag remembers *how* the opening click selected — by character, by word,
//! or by whole line — and extends in those units, and it keeps scrolling while
//! the pointer sits outside the viewport instead of stopping at the edge.

use std::time::Duration;

use karet_core::LineCol;
use karet_editor::line_span;
use karet_editor::word_bounds as word_at;
use karet_text::TextBuffer;
use ratatui::layout::Rect;

use super::App;
use crate::tab::Tab;
use crate::tab::TabKind;

/// How far apart autoscroll steps are while a drag rests outside the viewport.
///
/// Slow enough to stay controllable, quick enough not to feel stuck: about
/// twenty rows a second.
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// The unit an editor drag extends by, set by the click that opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DragGranularity {
    /// A single click: extend to the exact character under the pointer.
    Character,
    /// A double click: extend to whole words.
    Word,
    /// A triple click: extend to whole lines.
    Line,
}

/// An in-progress editor text-selection drag.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EditorDrag {
    /// The span the opening click selected. A word or line drag never shrinks
    /// inside it, so double-clicking a word and dragging keeps that whole word
    /// however far back the pointer travels.
    pub(crate) anchor: (LineCol, LineCol),
    /// The unit this drag extends by.
    pub(crate) granularity: DragGranularity,
    /// Where the pointer was last seen, which is what autoscroll steers by.
    pub(crate) pointer: (u16, u16),
    /// The editor viewport the drag began in.
    ///
    /// Carried rather than read back off the app: the opening click may have
    /// moved the focus to another pane, whose rect is only recorded on the next
    /// frame, so the focused-pane rect is stale for the rest of this one.
    pub(crate) area: Rect,
}

impl App {
    /// The span `pos` belongs to at `granularity`.
    pub(super) fn drag_span(
        buffer: &TextBuffer,
        pos: LineCol,
        granularity: DragGranularity,
    ) -> (LineCol, LineCol) {
        match granularity {
            DragGranularity::Character => (pos, pos),
            DragGranularity::Word => word_at(buffer, pos),
            DragGranularity::Line => line_span(buffer, pos.line),
        }
    }

    /// Extend the editor selection to the cell under `(col, row)` while dragging.
    pub(super) fn drag_select_to(&mut self, col: u16, row: u16) {
        let Some(drag) = self.editor_drag.as_mut() else {
            return;
        };
        drag.pointer = (col, row);
        let drag = *drag;
        let area = drag.area;
        let Some(Tab {
            kind:
                TabKind::Code {
                    buffer,
                    folds,
                    folded,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        let fold_lines = super::resolve_folds(folds, folded);
        let pos = editor.pos_at(area, buffer, &fold_lines, col, row);
        if drag.granularity == DragGranularity::Character {
            editor.extend_to(buffer, pos);
            return;
        }
        // Word and line drags select the union of the opening span and the span
        // under the pointer, oriented so the caret leads in the direction of travel.
        let (start, end) = Self::drag_span(buffer, pos, drag.granularity);
        if pos < drag.anchor.0 {
            editor.set_selection(buffer, drag.anchor.1, start);
        } else {
            editor.set_selection(buffer, drag.anchor.0, end);
        }
    }

    /// Which way the viewport should creep while the drag rests outside it, or
    /// `None` when the pointer is over the editor (or no drag is live).
    fn drag_autoscroll_delta(&self) -> Option<i32> {
        let drag = self.editor_drag?;
        let area = drag.area;
        if area.height == 0 {
            return None;
        }
        let (_, row) = drag.pointer;
        if row < area.y {
            Some(-1)
        } else if row >= area.bottom() {
            Some(1)
        } else {
            None
        }
    }

    /// How long until the next autoscroll step, for the event loop's wake-up.
    pub(super) fn drag_autoscroll_wake(&self) -> Option<Duration> {
        self.drag_autoscroll_delta().map(|_| AUTOSCROLL_INTERVAL)
    }

    /// Creep the viewport one row and re-extend the selection, when a drag is
    /// resting outside it.
    pub(super) fn tick_drag_autoscroll(&mut self) {
        let Some(delta) = self.drag_autoscroll_delta() else {
            return;
        };
        let Some(drag) = self.editor_drag else {
            return;
        };
        self.scroll_lines(delta);
        let (col, row) = drag.pointer;
        self.drag_select_to(col, row);
    }
}

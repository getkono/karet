//! The editor's pointer selection drag: its granularity, its lifecycle, and
//! autoscroll.
//!
//! A press in the editor arms a drag that owns the pointer until release. Two
//! things make it feel like a GUI rather than a hit test repeated every event:
//! the drag remembers *how* the opening click selected — by character, by word,
//! or by whole line — and extends in those units, and it keeps scrolling while
//! the pointer sits outside the viewport instead of stopping at the edge.
//!
//! A drag is bound to the pane and tab it began in. It ends on a release
//! anywhere, on any press (the release before it was lost), on buttonless
//! motion (the button is up, though its release never arrived), and as soon as
//! it no longer addresses what is on screen: an overlay opened, the focus or the
//! active tab moved, or the editor view was put away. The event loop also ends
//! it on a key press and when the terminal loses focus.

use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use karet_core::LineCol;
use karet_editor::line_span;
use karet_editor::word_bounds as word_at;
use karet_session::ViewId;
use karet_text::TextBuffer;
use karet_widgets::PaneId;
use ratatui::layout::Rect;

use super::App;
use super::effective_word_wrap;
use super::tab_language;
use crate::keymap::Focus;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::view::View;

/// How far apart autoscroll steps are while a drag rests outside the viewport.
///
/// The steps run on this clock and nothing else, so the speed never depends on
/// how much other traffic wakes the event loop.
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// The most rows (or columns) one autoscroll step moves.
///
/// A step moves one unit per cell the pointer sits past the edge, so the
/// further out it is the faster the view creeps; this caps it at about 160
/// rows a second, still slow enough to stop on what scrolls by.
const AUTOSCROLL_MAX_STEP: i32 = 8;

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
    /// The editor viewport the drag maps the pointer against.
    ///
    /// Seeded from the opening click's pane frame rather than read off the
    /// app — the click may have moved the focus to another pane, whose rect is
    /// only recorded on the next frame — and refreshed from that pane's frame
    /// on every later event, so a resize mid-drag is followed.
    pub(crate) area: Rect,
    /// The pane the drag began in.
    pub(crate) pane: PaneId,
    /// The tab the drag began in.
    pub(crate) view: ViewId,
    /// When autoscroll last stepped, or `None` while the pointer is over the
    /// text, so the first step past an edge is immediate.
    pub(crate) last_step: Option<Instant>,
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

    /// Whether `drag` still addresses what is on screen: no overlay over it,
    /// the keyboard in the editor view, and the pane and tab it began in still
    /// the focused ones.
    fn drag_is_live(&self, drag: &EditorDrag) -> bool {
        self.overlay.is_none()
            && self.focus == Focus::Editor
            && self.view == View::Editor
            && self.focus_pane() == drag.pane
            && self
                .tabs
                .get(self.active)
                .is_some_and(|tab| tab.view == drag.view)
    }

    /// The viewport `drag`'s pane was last painted at, or the one it began in
    /// when no frame has recorded that pane.
    fn drag_area(&self, drag: &EditorDrag) -> Rect {
        self.pane_frames
            .iter()
            .find(|frame| frame.pane == drag.pane)
            .map(|frame| frame.editor_rect)
            .filter(|rect| !rect.is_empty())
            .unwrap_or(drag.area)
    }

    /// The live editor drag with its viewport refreshed, ending (and returning
    /// `None` for) one that no longer addresses what is on screen.
    fn live_editor_drag(&mut self) -> Option<EditorDrag> {
        let drag = self.editor_drag?;
        if !self.drag_is_live(&drag) {
            self.editor_drag = None;
            return None;
        }
        let area = self.drag_area(&drag);
        let drag = self.editor_drag.as_mut()?;
        drag.area = area;
        Some(*drag)
    }

    /// The first screen row of `area` that shows the document itself, below
    /// the sticky-scroll headers the last frame pinned above it.
    ///
    /// The sticky band is what the last render left of `area` once its live
    /// viewport was taken out, which the editor reports as its viewport height.
    fn drag_text_top(&self, area: Rect) -> u16 {
        let live = self.tabs.get(self.active).map_or(area.height, |tab| {
            let viewport = tab.editor.viewport();
            let rows = viewport.end.line.saturating_sub(viewport.start.line);
            u16::try_from(rows).unwrap_or(u16::MAX)
        });
        let sticky = area.height.saturating_sub(live);
        area.y
            .saturating_add(sticky)
            .min(area.bottom().saturating_sub(1))
    }

    /// The first screen column of `area` that shows text, right of the gutter.
    fn drag_text_left(&self, area: Rect) -> u16 {
        let width = self
            .tabs
            .get(self.active)
            .map_or(area.width, |tab| tab.editor.content_width());
        area.right().saturating_sub(width).max(area.x)
    }

    /// End the editor drag on the mouse events that end one wherever they land;
    /// `true` when the event is spent doing so.
    ///
    /// A left release ends it and is consumed: it answers the drag's own press.
    /// Any press ends it too — a press with no release before it means the
    /// release was lost (outside the window, or under an overlay) — and then
    /// goes on to do what a press does. Buttonless motion ends it the same way:
    /// with mouse capture on, the terminal reports motion under a held button
    /// as a drag, so a plain move proves the button is already up; the move is
    /// not consumed, so it still drives hover. A drag that no longer addresses
    /// what is on screen ends on any event at all, which consumes nothing.
    pub(super) fn end_editor_drag_on(&mut self, mouse: &MouseEvent) -> bool {
        if self.live_editor_drag().is_none() {
            return false;
        }
        match mouse.kind {
            MouseEventKind::Up(MouseButton::Left) => {
                self.editor_drag = None;
                true
            },
            MouseEventKind::Down(_) | MouseEventKind::Moved => {
                self.editor_drag = None;
                false
            },
            _ => false,
        }
    }

    /// Extend the editor selection to the cell under `(col, row)` while dragging.
    ///
    /// The pointer is held to the drag's own text area: past an edge, the
    /// selection stops at the edge and autoscroll brings the rest in, rather
    /// than mapping a cell in another pane or a sticky header to whatever
    /// position its offset happens to name.
    pub(super) fn drag_select_to(&mut self, col: u16, row: u16) {
        let Some(drag) = self.live_editor_drag() else {
            return;
        };
        if let Some(live) = self.editor_drag.as_mut() {
            live.pointer = (col, row);
        }
        let area = drag.area;
        if area.is_empty() {
            return;
        }
        let row = row.clamp(self.drag_text_top(area), area.bottom() - 1);
        let col = col.min(area.right() - 1);
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

    /// Route a mouse event to the live drag: motion extends the selection, and
    /// a wheel notch scrolls three rows (or columns) and extends. Shift turns
    /// the vertical wheel sideways, as it does outside a drag.
    pub(super) fn editor_drag_mouse(&mut self, mouse: &MouseEvent) {
        let (col, row) = (mouse.column, mouse.row);
        let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => self.drag_select_to(col, row),
            MouseEventKind::ScrollDown if shift => self.drag_wheel(0, 3, col, row),
            MouseEventKind::ScrollUp if shift => self.drag_wheel(0, -3, col, row),
            MouseEventKind::ScrollDown => self.drag_wheel(3, 0, col, row),
            MouseEventKind::ScrollUp => self.drag_wheel(-3, 0, col, row),
            MouseEventKind::ScrollRight => self.drag_wheel(0, 3, col, row),
            MouseEventKind::ScrollLeft => self.drag_wheel(0, -3, col, row),
            _ => {},
        }
    }

    /// Scroll the drag's view by a wheel notch and keep extending to the
    /// pointer, so a wheel under a held button moves the selection with the
    /// text the way a GUI editor's does.
    fn drag_wheel(&mut self, rows: i32, cols: i32, col: u16, row: u16) {
        if self.live_editor_drag().is_none() {
            return;
        }
        if rows != 0 {
            self.scroll_lines(rows);
        }
        if cols != 0 {
            self.scroll_columns(cols);
        }
        self.drag_select_to(col, row);
    }

    /// How far, in rows and columns, the viewport should creep in one step
    /// while `drag` rests past an edge of its text, or `None` while the pointer
    /// is over the text.
    ///
    /// Each axis moves one unit per cell the pointer sits past the edge, up to
    /// [`AUTOSCROLL_MAX_STEP`]. The sticky-scroll headers count as above the
    /// text and the gutter as left of it. Horizontal creep only runs where
    /// there is somewhere to go: never left of column zero, and never in a
    /// wrapped view, which does not scroll sideways.
    fn drag_autoscroll_delta(&self, drag: &EditorDrag) -> Option<(i32, i32)> {
        let area = self.drag_area(drag);
        if area.is_empty() {
            return None;
        }
        let (col, row) = drag.pointer;
        let top = self.drag_text_top(area);
        let rows = if row < top {
            -i32::from(top - row)
        } else if row >= area.bottom() {
            i32::from(row - area.bottom()) + 1
        } else {
            0
        };
        let tab = self.tabs.get(self.active)?;
        let word_wrap = effective_word_wrap(
            tab,
            self.settings
                .editor
                .for_language(tab_language(tab))
                .word_wrap(),
        );
        let left = self.drag_text_left(area);
        let cols = if word_wrap {
            0
        } else if col < left && tab.editor.scroll_col > 0 {
            -i32::from(left - col)
        } else if col >= area.right() {
            i32::from(col - area.right()) + 1
        } else {
            0
        };
        let cap = |delta: i32| delta.clamp(-AUTOSCROLL_MAX_STEP, AUTOSCROLL_MAX_STEP);
        (rows != 0 || cols != 0).then(|| (cap(rows), cap(cols)))
    }

    /// How long until the next autoscroll step, for the event loop's wake-up.
    pub(super) fn drag_autoscroll_wake(&self) -> Option<Duration> {
        let drag = self.editor_drag.filter(|drag| self.drag_is_live(drag))?;
        self.drag_autoscroll_delta(&drag)?;
        Some(drag.last_step.map_or(Duration::ZERO, |last| {
            AUTOSCROLL_INTERVAL.saturating_sub(last.elapsed())
        }))
    }

    /// Step the viewport and re-extend the selection when a drag rests past an
    /// edge and the autoscroll clock is due at `now`.
    ///
    /// The event loop calls this on every wake-up, whatever woke it; only the
    /// clock decides whether a step happens, so the speed is the same whether
    /// the terminal is quiet or busy.
    pub(super) fn tick_drag_autoscroll(&mut self, now: Instant) {
        let Some(drag) = self.live_editor_drag() else {
            return;
        };
        let Some((rows, cols)) = self.drag_autoscroll_delta(&drag) else {
            if let Some(live) = self.editor_drag.as_mut() {
                live.last_step = None;
            }
            return;
        };
        if drag
            .last_step
            .is_some_and(|last| now.saturating_duration_since(last) < AUTOSCROLL_INTERVAL)
        {
            return;
        }
        if rows != 0 {
            self.scroll_lines(rows);
        }
        if cols != 0 {
            self.scroll_columns(cols);
        }
        if let Some(live) = self.editor_drag.as_mut() {
            live.last_step = Some(now);
        }
        let (col, row) = drag.pointer;
        self.drag_select_to(col, row);
    }
}

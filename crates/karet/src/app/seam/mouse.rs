//! Answering the pointer across the Seam view's whole surface.
//!
//! Every affordance the key map offers has a place on screen, and each of those places
//! answers the mouse: a crumb steps back to itself, a legend entry toggles its lens, the
//! configuration marker cycles, a row selects and a second click steps into it.
//!
//! A press anywhere on the view is *consumed*, even where the gesture does nothing.
//! Falling through would hand it to the editor behind this view, which reads a press as
//! the start of a text selection and then waits for a drag that is never coming.

use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;

use super::Reroot;
use super::geometry::SeamTarget;
use crate::app::App;
use crate::app::Focus;
use crate::app::seam::SeamFocus;

impl App {
    /// Route a mouse event that landed on the Seam view, reporting whether it did.
    pub(crate) fn seam_mouse(&mut self, mouse: MouseEvent) -> bool {
        // The pane context menu is the shell's, not this view's.
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
            return false;
        }
        let Some(state) = self.active_seam() else {
            return false;
        };
        let Some(target) = state.hits.at(mouse.column, mouse.row) else {
            return false;
        };
        match mouse.kind {
            // Handled, but not consumed: the shell's own hover bookkeeping still runs.
            MouseEventKind::Moved => {
                state.hover = Some((mouse.column, mouse.row));
                return false;
            },
            MouseEventKind::ScrollUp => self.seam_wheel(&target, -1),
            MouseEventKind::ScrollDown => self.seam_wheel(&target, 1),
            MouseEventKind::ScrollLeft => self.seam_wheel_across(&target, -1),
            MouseEventKind::ScrollRight => self.seam_wheel_across(&target, 1),
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = Focus::Editor;
                let repeat = self.click_streak(mouse.column, mouse.row);
                self.seam_click(&target, repeat >= 2);
            },
            _ => {},
        }
        true
    }

    /// Act on a left click, or on the second of a pair.
    fn seam_click(&mut self, target: &SeamTarget, again: bool) {
        match target {
            SeamTarget::Crumb(depth) => self.seam_widen_to(*depth),
            SeamTarget::Configuration => self.seam_configuration(),
            // A second click on a lens turns it back off, which is what the digit does.
            SeamTarget::Lens(index) => self.seam_toggle_lens(*index),
            SeamTarget::Row(id) => self.seam_select_row(id, again),
            SeamTarget::Spine => {
                if let Some(state) = self.active_seam() {
                    state.focus = SeamFocus::Spine;
                }
            },
            SeamTarget::Edge(index) => {
                if let Some(state) = self.active_seam() {
                    state.focus = SeamFocus::Facets;
                    state.facet_row = *index;
                }
                if again {
                    self.pivot_seam_edge();
                }
            },
            SeamTarget::Facets => {
                if let Some(state) = self.active_seam() {
                    state.focus = SeamFocus::Facets;
                }
            },
            SeamTarget::Query => self.seam_focus_query(),
            SeamTarget::Widen => self.seam_widen(),
        }
    }

    /// Select the row a click landed on, stepping into it on a second click.
    fn seam_select_row(&mut self, id: &str, again: bool) {
        let Some(state) = self.active_seam() else {
            return;
        };
        state.focus = SeamFocus::Spine;
        if state.selected_id() != Some(id) {
            state.select_path(id);
            self.request_seam_node();
            return;
        }
        if !again {
            return;
        }
        // A second click on the row already under the pointer is Enter: narrow into it,
        // or cross back to its source when there is nothing to narrow to.
        match state.reroot() {
            Reroot::Narrowed | Reroot::Descended => self.request_seam_node(),
            Reroot::Refused => self.open_seam_selection(),
        }
    }

    /// A vertical wheel notch.
    ///
    /// One row per notch, not three: the spine's scroll offset is pinned to the selection
    /// by the render, so a notch moves the selection — the same bargain the commit browser
    /// already makes.
    fn seam_wheel(&mut self, target: &SeamTarget, delta: isize) {
        match target {
            SeamTarget::Row(_) | SeamTarget::Spine => {
                if let Some(state) = self.active_seam() {
                    state.focus = SeamFocus::Spine;
                }
                self.seam_move_row(delta);
            },
            SeamTarget::Edge(_) | SeamTarget::Facets => {
                if let Some(state) = self.active_seam() {
                    state.move_facet_row(delta);
                }
            },
            _ => {},
        }
    }

    /// A horizontal wheel notch: the natural gesture for a cascading spine.
    fn seam_wheel_across(&mut self, target: &SeamTarget, delta: isize) {
        if matches!(target, SeamTarget::Row(_) | SeamTarget::Spine) {
            if let Some(state) = self.active_seam() {
                state.focus = SeamFocus::Spine;
            }
            self.seam_move_column(delta);
        }
    }

    /// Step back out to `depth` narrows, asking once for what that lands on.
    fn seam_widen_to(&mut self, depth: usize) {
        let Some(state) = self.active_seam() else {
            return;
        };
        if state.widen_to(depth) {
            self.request_seam_node();
        }
    }

    /// Whether the pointer is over something a click would act on.
    ///
    /// Drives the pointer-shape hint, from the same resolution the click uses, so the two
    /// can never disagree about what looks clickable.
    pub(crate) fn seam_affordance_at(&mut self, x: u16, y: u16) -> bool {
        self.active_seam().is_some_and(|state| {
            matches!(
                state.hits.at(x, y),
                Some(
                    SeamTarget::Crumb(_)
                        | SeamTarget::Configuration
                        | SeamTarget::Lens(_)
                        | SeamTarget::Row(_)
                        | SeamTarget::Edge(_)
                        | SeamTarget::Widen
                )
            )
        })
    }
}

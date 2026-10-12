//! What the pointer and the toggle key do with inlay hints: a tooltip while
//! the pointer rests on a hint, Ctrl+click on a label part to go where it
//! leads, resolving on demand what the server deferred for either, and
//! turning the hints off and on.
//!
//! Hit-testing reads back what the editor painted: each label part is its own
//! [`karet_core::VirtualText`], so the cells under the pointer name the part,
//! not just the hint.

use karet_core::Markup;
use karet_session::api::Command as SessionCommand;
use karet_session::api::DocumentId;
use karet_session::api::RequestId;

use super::super::App;
use super::super::KeyModifiers;
use super::super::MouseEvent;
use super::super::MouseEventKind;
use super::super::Rect;
use super::super::util::rect_contains;

/// Where the last frame painted one label part of one of the focused pane's
/// hints: the part `part` of hint `hint` in `doc`'s held set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InlayHintHit {
    /// The cells it was painted in.
    pub(crate) rect: Rect,
    /// The document whose hint it is.
    pub(crate) doc: DocumentId,
    /// Its index in the document's held hints.
    pub(crate) hint: usize,
    /// Its index in the hint's label parts (0 for a hint without parts).
    pub(crate) part: usize,
}

/// The tooltip shown for the hint under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InlayTooltip {
    /// What it says.
    pub(crate) markup: Markup,
    /// The cells of the part it describes, which it is drawn against.
    pub(crate) anchor: Rect,
}

/// A resolve in flight: which hint, and what to do with the answer.
#[derive(Clone, Debug)]
pub(crate) struct PendingResolve {
    /// The request to match the answer against.
    pub(crate) id: RequestId,
    /// The document whose hint it is.
    pub(crate) doc: DocumentId,
    /// The hint's payload when asked, which is how the answer finds it again:
    /// indices move whenever a new set is adopted.
    pub(crate) payload: String,
    /// The part a Ctrl+click asked to follow, if the answer should be followed.
    pub(crate) follow: Option<usize>,
}

/// The pointer's and the toggle key's state for inlay hints.
#[derive(Debug, Default)]
pub(crate) struct InlayUi {
    /// Whether the toggle key has flipped the configured visibility.
    pub(crate) flipped: bool,
    /// Whether the key being handled toggled them, so a momentary flip
    /// survives the key that made it.
    pub(crate) toggled_this_key: bool,
    /// The focused pane's painted label parts, from the last frame.
    pub(crate) hits: Vec<InlayHintHit>,
    /// The part the pointer rests on, if any.
    pub(crate) hovered: Option<InlayHintHit>,
    /// The tooltip on screen, if any.
    pub(crate) tooltip: Option<InlayTooltip>,
    /// The resolve in flight, if any.
    pub(crate) resolving: Option<PendingResolve>,
}

impl App {
    /// Whether inlay hints are drawn right now: the setting's own state,
    /// flipped while the toggle key has flipped it.
    pub(crate) fn inlay_hints_visible(&self) -> bool {
        self.settings.editor.inlay_hints.enabled.shown() != self.inlay_ui.flipped
    }

    /// Toggle Inlay Hints: flip whether they are drawn. Under an
    /// `…UnlessPressed` setting the flip lasts until the next key; otherwise
    /// it lasts the session, or until the setting changes.
    pub(crate) fn toggle_inlay_hints(&mut self) {
        self.inlay_ui.flipped = !self.inlay_ui.flipped;
        self.inlay_ui.toggled_this_key = true;
        self.apply_inlay_visibility();
    }

    /// Note that a key is about to be handled.
    pub(crate) fn begin_inlay_key(&mut self) {
        self.inlay_ui.toggled_this_key = false;
        // Typing means the pointer is no longer the subject.
        self.inlay_ui.tooltip = None;
        self.inlay_ui.hovered = None;
    }

    /// Note that a key has been handled: a momentary flip that this key did
    /// not make ends here.
    pub(crate) fn end_inlay_key(&mut self) {
        if self.inlay_ui.flipped
            && !self.inlay_ui.toggled_this_key
            && self.settings.editor.inlay_hints.enabled.momentary()
        {
            self.inlay_ui.flipped = false;
            self.apply_inlay_visibility();
        }
    }

    /// Drop what is shown when hints have just been hidden. Showing needs
    /// nothing: the next frame asks for what it paints.
    pub(crate) fn apply_inlay_visibility(&mut self) {
        if !self.inlay_hints_visible() {
            self.forget_all_inlay_hints();
            self.inlay_ui.tooltip = None;
            self.inlay_ui.hovered = None;
            self.inlay_ui.resolving = None;
        }
    }

    /// The painted part under `point`, if any.
    fn inlay_hit_at(&self, point: (u16, u16)) -> Option<InlayHintHit> {
        self.inlay_ui
            .hits
            .iter()
            .copied()
            .find(|hit| rect_contains(hit.rect, point))
    }

    /// Re-evaluate the tooltip for every mouse event: shown while the pointer
    /// rests on a hint that has one, asked for when the server deferred it,
    /// and gone as soon as the pointer moves off, presses, or scrolls.
    pub(crate) fn update_inlay_hover(&mut self, mouse: &MouseEvent) {
        let hit = matches!(mouse.kind, MouseEventKind::Moved)
            .then(|| self.inlay_hit_at((mouse.column, mouse.row)))
            .flatten();
        if hit == self.inlay_ui.hovered {
            return;
        }
        self.inlay_ui.hovered = hit;
        self.inlay_ui.tooltip = hit.and_then(|hit| self.tooltip_for(hit));
        if let Some(hit) = hit
            && self.inlay_ui.tooltip.is_none()
        {
            self.resolve_inlay_hint(hit, None);
        }
    }

    /// The tooltip `hit` shows from what is held, if it says anything.
    fn tooltip_for(&self, hit: InlayHintHit) -> Option<InlayTooltip> {
        let hint = self.docs.inlay_hints.get(&hit.doc)?.get(hit.hint)?;
        let markup = hint.tooltip_for(hit.part)?;
        (!markup.value.trim().is_empty()).then(|| InlayTooltip {
            markup: markup.clone(),
            anchor: hit.rect,
        })
    }

    /// Ctrl+click on a label part that leads somewhere goes there. Returns
    /// `true` when the press was the hint's: it led somewhere, or is being
    /// resolved to find out where. A plain click, or a part that can lead
    /// nowhere, is left to the editor, which puts the caret where the hint is.
    pub(crate) fn handle_inlay_hint_mouse(&mut self, mouse: &MouseEvent) -> bool {
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(crossterm::event::MouseButton::Left)
        ) || !mouse
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return false;
        }
        let Some(hit) = self.inlay_hit_at((mouse.column, mouse.row)) else {
            return false;
        };
        let Some(hint) = self
            .docs
            .inlay_hints
            .get(&hit.doc)
            .and_then(|hints| hints.get(hit.hint))
        else {
            return false;
        };
        if let Some(location) = hint.location_for(hit.part).cloned() {
            self.jump_to_location(&location.path, location.range.start);
            return true;
        }
        if hint.payload.is_none() {
            return false;
        }
        self.resolve_inlay_hint(hit, Some(hit.part));
        true
    }

    /// Ask for what the server deferred for `hit`'s hint, unless that is
    /// already being asked; `follow` says to go where `follow`'s part leads
    /// once the answer is in.
    fn resolve_inlay_hint(&mut self, hit: InlayHintHit, follow: Option<usize>) {
        let Some(hint) = self
            .docs
            .inlay_hints
            .get(&hit.doc)
            .and_then(|hints| hints.get(hit.hint))
        else {
            return;
        };
        let Some(payload) = hint.payload.clone() else {
            return; // nothing deferred: what is held is all there is
        };
        if let Some(pending) = self.inlay_ui.resolving.as_mut()
            && pending.doc == hit.doc
            && pending.payload == payload
        {
            // Already asked; a Ctrl+click while it is in flight still counts.
            pending.follow = follow.or(pending.follow);
            return;
        }
        let command = SessionCommand::ResolveInlayHint {
            doc: hit.doc,
            hint: hint.clone(),
        };
        self.inlay_ui.resolving = self.send(command).map(|id| PendingResolve {
            id,
            doc: hit.doc,
            payload,
            follow,
        });
    }

    /// Adopt a resolved hint into the held set, then do what it was asked for:
    /// show the tooltip if the pointer still rests on it, or follow the part
    /// a Ctrl+click asked for.
    pub(crate) fn on_inlay_hint_resolved(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        resolved: karet_core::InlayHint,
    ) {
        let Some(pending) = self.inlay_ui.resolving.take_if(|p| Some(p.id) == id) else {
            return; // superseded: a newer hover asked about another hint
        };
        // Found by payload, not index: a new set may have been adopted since.
        let held = self.docs.inlay_hints.get_mut(&doc).and_then(|hints| {
            hints
                .iter_mut()
                .enumerate()
                .find(|(_, hint)| hint.payload.as_deref() == Some(pending.payload.as_str()))
        });
        let Some((index, hint)) = held else {
            return; // the hint is gone: edited away, or hints were hidden
        };
        hint.adopt_resolved(resolved);
        if let Some(part) = pending.follow {
            if let Some(location) = hint.location_for(part).cloned() {
                self.jump_to_location(&location.path, location.range.start);
            }
            return;
        }
        if let Some(hovered) = self.inlay_ui.hovered
            && hovered.doc == doc
            && hovered.hint == index
        {
            self.inlay_ui.tooltip = self.tooltip_for(hovered);
        }
    }
}

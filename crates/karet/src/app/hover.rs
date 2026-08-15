//! Hover request/response handling on the [`App`] (model in [`crate::hover`]).

use karet_core::Hover;
use karet_session::Command as SessionCommand;
use karet_session::RequestId;

use super::*;
use crate::hover::HoverUi;
use crate::hover::PendingHover;

impl App {
    /// Request hover information at the caret (Ctrl+K Ctrl+I). The answer is
    /// composed with the diagnostics under the caret when it arrives.
    pub(super) fn request_hover(&mut self) {
        if !self.settings.editor.hover.enabled {
            self.status = Some("hover is disabled (editor.hover.enabled)".to_owned());
            return;
        }
        let Some((doc, at)) = self.completion_target() else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        if backend
            .send(id, SessionCommand::Hover { doc, position: at })
            .is_ok()
        {
            self.pending_hover = Some(PendingHover { id, doc, at });
        }
    }

    /// Handle the `HoverResult` answering a pending request. Stale answers — a
    /// superseded request id, or a caret that has moved on — are dropped, per
    /// the loading-state policy.
    pub(super) fn on_hover_result(&mut self, id: Option<RequestId>, hover: Option<Hover>) {
        let Some(pending) = self.pending_hover else {
            return;
        };
        if id != Some(pending.id) {
            return;
        }
        self.pending_hover = None;
        if self.completion_target() != Some((pending.doc, pending.at)) {
            return;
        }
        let empty = Vec::new();
        let diagnostics = self.docs.diagnostics.get(&pending.doc).unwrap_or(&empty);
        let at_caret = crate::hover::diagnostics_at(diagnostics, pending.at);
        let hint = self.manifest_hint_markdown(pending.doc, pending.at.line);
        match crate::hover::hover_markup(&at_caret, hover.as_ref(), hint.as_deref()) {
            Some(markup) => {
                self.hover_ui = Some(HoverUi {
                    markup,
                    doc: pending.doc,
                    at: pending.at,
                });
            },
            None => self.status = Some("no hover information".to_owned()),
        }
    }

    /// Dismiss the popup and any pending request once the caret leaves the
    /// position they were anchored to.
    pub(super) fn reconcile_hover(&mut self) {
        let target = self.completion_target();
        if self
            .hover_ui
            .as_ref()
            .is_some_and(|h| target != Some((h.doc, h.at)))
        {
            self.hover_ui = None;
        }
        if self
            .pending_hover
            .is_some_and(|p| target != Some((p.doc, p.at)))
        {
            self.pending_hover = None;
        }
    }

    /// Close an open hover popup; `true` when one was open (so the caller
    /// consumes the dismissing key).
    pub(super) fn dismiss_hover(&mut self) -> bool {
        self.hover_ui.take().is_some()
    }
}

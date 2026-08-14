//! Go to Definition: request, correlate, and land the caret.
//!
//! The backend seam already existed — `Command::Definition` in, `Event::Definitions`
//! out — and this module is the client half. Ctrl+click and F12 funnel through the
//! same [`App::request_definition`], so a mouse jump and a keyboard jump behave
//! identically and there is one place where staleness is decided.

use karet_core::Location;

use super::*;

/// A definition request awaiting its answer.
///
/// `Event::Definitions` carries no document or version, so the request id is the
/// *only* correlation key: a late answer to a superseded request must not evict the
/// record for the live one. The view is remembered so a jump is dropped when the
/// user has moved on, and it survives tab reordering (which an index would not).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingDefinition {
    pub(crate) id: RequestId,
    pub(crate) doc: DocumentId,
    pub(crate) view: ViewId,
}

impl App {
    /// Ask the language server for the definition of the symbol at the caret.
    ///
    /// A new request replaces any in flight, so the last gesture wins. There is no
    /// "resolving…" status: nothing on screen changes until the answer lands, and a
    /// spinner for a sub-second round trip is the flicker the UI guidelines warn off.
    pub(crate) fn request_definition(&mut self) {
        let Some((doc, position)) = self.completion_target() else {
            self.status = Some("go to definition: open a code file first".to_string());
            return;
        };
        let Some(view) = self.tabs.get(self.active).map(|tab| tab.view) else {
            return;
        };
        self.pending_definition = self
            .send(SessionCommand::Definition { doc, position })
            .map(|id| PendingDefinition { id, doc, view });
    }

    /// Adopt a definition answer: jump to it, or say why there is nothing to jump to.
    pub(crate) fn on_definitions(&mut self, id: Option<RequestId>, locations: Vec<Location>) {
        let Some(pending) = self.pending_definition else {
            return; // unsolicited
        };
        if id != Some(pending.id) {
            return; // a superseded request answering late; the live one still stands
        }
        self.pending_definition = None;
        // The user asked explicitly, so a moved *caret* is no reason to drop the
        // jump — but a different view or document means they have moved on.
        let current = self.tabs.get(self.active);
        let same_view = current.is_some_and(|tab| tab.view == pending.view);
        let same_doc = current.is_some_and(
            |tab| matches!(tab.kind, TabKind::Code { doc: Some(doc), .. } if doc == pending.doc),
        );
        if !same_view || !same_doc {
            return;
        }
        match locations.first() {
            None => self.status = Some(self.no_definition_reason().to_string()),
            Some(first) => {
                let (path, position) = (first.path.clone(), first.range.start);
                self.jump_to_location(&path, position);
            },
        }
    }

    /// Why an empty answer came back, phrased for the status line.
    ///
    /// A server that is still starting answers instantly with nothing, which would
    /// otherwise read as "this symbol has no definition".
    fn no_definition_reason(&self) -> &'static str {
        match self.active_language_server_badge() {
            None | Some(LanguageServerBadge::Unavailable) => "no language server for this file",
            Some(LanguageServerBadge::Starting | LanguageServerBadge::Retrying) => {
                "language server is still starting"
            },
            _ => "no definition found",
        }
    }

    /// Move to `position` in `path`, recording where the jump started.
    ///
    /// The caret lands on the definition's start, collapsed to a bare caret with no
    /// selection. For a server that answers with a `LocationLink` that is the first
    /// character of the definition's *name*; a plain `Location` is whatever the
    /// server calls the start, often the `pub`/`fn` before it.
    pub(crate) fn jump_to_location(&mut self, path: &Path, position: LineCol) {
        self.focus_by_file_line(path, position);
    }
}

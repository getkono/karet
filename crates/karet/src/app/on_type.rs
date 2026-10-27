//! On-type formatting on the [`App`]: after a character is typed, the
//! language server may rewrite the text around it -- reindent a line on `}`,
//! close a statement on `;`, continue a comment on Enter.
//!
//! Off unless `editor.formatOnType` says otherwise for the language, and
//! Toggle Format on Type flips that for the session. While on, a typed
//! character goes to the session as `LspCommand::FormatOnType`; the answering
//! `Event::OnTypeFormatting` names the server's trigger characters, after
//! which only those are sent. Its edits are applied here, by the app, as an
//! edit of its own -- and only when the text is still the text the server was
//! asked about, so typing on while the server works never loses a keystroke
//! nor lands an edit in the wrong place.

use std::collections::HashMap;

use karet_core::Change;
use karet_core::EditCause;
use karet_core::NotificationKind;
use karet_core::TextEdit;
use karet_editor::editing;
use karet_session::DocumentId;
use karet_session::LspCommand;
use karet_session::RequestId;

use crate::app::App;
use crate::app::notifications::Report;
use crate::tab::Tab;
use crate::tab::TabKind;

/// The on-type formatting state of the app.
#[derive(Debug, Default)]
pub(crate) struct OnTypeState {
    /// The request in flight, if any; a newer one supersedes it.
    pending: Option<PendingOnType>,
    /// The trigger characters each document's server answered with, empty
    /// for a server that formats on none; dropped when the document closes,
    /// is moved, or may have changed servers.
    /// A document without an entry is offered every typed character until
    /// its server says which it formats on.
    pub(crate) triggers: HashMap<DocumentId, Vec<char>>,
    /// Whether Toggle Format on Type has flipped `editor.formatOnType`.
    flipped: bool,
}

/// An on-type formatting request awaiting its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingOnType {
    /// The request, matched against the answer's id.
    id: RequestId,
    /// The document typed in.
    doc: DocumentId,
    /// The buffer version just after the typed character.
    version: u64,
}

impl App {
    /// Whether on-type formatting is on for the active tab's language.
    fn format_on_type_enabled(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let setting = self
            .settings
            .editor
            .for_language(tab.language_id())
            .format_on_type();
        setting != self.on_type.flipped
    }

    /// Toggle Format on Type: flip `editor.formatOnType` for this session,
    /// and say where that leaves the active file.
    pub(super) fn toggle_format_on_type(&mut self) {
        self.on_type.flipped = !self.on_type.flipped;
        self.on_type.pending = None;
        let state = if self.format_on_type_enabled() {
            "on"
        } else {
            "off"
        };
        self.notify(
            Report::Outcome,
            NotificationKind::Lsp,
            format!("format on type: {state}"),
        );
    }

    /// Forget every document's remembered trigger characters, so each is
    /// offered typed characters again until its server answers anew. Called
    /// when the `lsp` settings change and when a managed server is installed,
    /// updated, or removed: a document may now be served by a server that
    /// formats on characters the old one did not.
    pub(super) fn forget_on_type_triggers(&mut self) {
        self.on_type.triggers.clear();
    }

    /// After `ch` was typed (`\n` for Enter): offer it to the language
    /// server, when on-type formatting is on, a single bare caret typed it,
    /// and the server formats on it or has yet to say what it formats on.
    pub(super) fn maybe_format_on_type(&mut self, ch: char) {
        if !self.format_on_type_enabled() {
            return;
        }
        let Some(Tab {
            kind:
                TabKind::Code {
                    doc: Some(doc),
                    buffer,
                    ..
                },
            editor,
            ..
        }) = self.tabs.get(self.active)
        else {
            return;
        };
        let cursors = editor.cursors();
        let [only] = cursors.selections.as_slice() else {
            return; // the server formats around one position, not many
        };
        if !only.is_empty() {
            return;
        }
        if self
            .on_type
            .triggers
            .get(doc)
            .is_some_and(|triggers| !triggers.contains(&ch))
        {
            return;
        }
        let (doc, version, position) = (*doc, buffer.version(), only.head);
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        let command = LspCommand::FormatOnType { doc, position, ch };
        if backend.send(id, command.into()).is_ok() {
            self.on_type.pending = Some(PendingOnType { id, doc, version });
        }
    }

    /// Adopt an answering `Event::OnTypeFormatting`: remember the server's
    /// triggers, and apply its edits when they answer the latest request and
    /// the active tab still holds the text they were made for.
    pub(super) fn on_type_formatting(
        &mut self,
        id: Option<RequestId>,
        (doc, version): (DocumentId, u64),
        edits: Vec<TextEdit>,
        triggers: Option<Vec<char>>,
    ) {
        // An empty list is remembered too: a server that formats on nothing
        // is offered nothing more, rather than every keystroke (each of which
        // would flush the debounced `didChange` ahead of its request).
        if let Some(triggers) = triggers {
            self.on_type.triggers.insert(doc, triggers);
        }
        let Some(pending) = self
            .on_type
            .pending
            .take_if(|pending| Some(pending.id) == id)
        else {
            return;
        };
        if edits.is_empty() || (pending.doc, pending.version) != (doc, version) {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let current = matches!(
            &tab.kind,
            TabKind::Code { doc: Some(open), next_version, .. }
                if *open == doc && *next_version == version
        );
        if !current {
            return; // typed on, or moved away: the edits describe other text
        }
        let mut cursors = tab.editor.cursors().clone();
        for selection in &mut cursors.selections {
            *selection = editing::map_selection(*selection, &edits);
        }
        let change = Change::new(version, edits);
        self.submit_multi_edit(EditCause::Replace, editing::MultiEdit { change, cursors });
    }
}

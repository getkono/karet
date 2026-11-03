//! Signature help -- the parameter hints of the call around the caret -- on
//! the [`App`].
//!
//! The app talks to the language server only through the session seam:
//! `Command::SignatureHelp` goes out, the answering `Event::SignatureHelp`
//! (correlated by request id) opens, refreshes or closes the popup, which
//! [`karet_widgets::SignaturePopup`] paints above the caret.
//!
//! It opens on the server's trigger characters (before the server named
//! any, `(` and `,`) and by hand (Ctrl+Shift+Space). While it is open, every
//! caret move asks again, so the highlighted parameter follows the caret and
//! a typed retrigger character (`,`) moves it on; the server's answer is the
//! authority on whether the caret is still in a call. The client closes it
//! itself only where no answer is needed: on Esc, when the caret leaves every
//! open `(` behind it (a typed `)` or a move out), and when the editor gives
//! up the keyboard.

use std::collections::HashMap;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use karet_core::LineCol;
use karet_core::SignatureHelp;
use karet_core::SignatureHelpOptions;
use karet_session::DocumentId;
use karet_session::LspCommand;
use karet_session::RequestId;
use karet_text::TextBuffer;

use crate::app::App;
use crate::tab::TabKind;

/// The trigger characters assumed before a server names its own: what
/// rust-analyzer, typescript-language-server, pyright and clangd all use.
const DEFAULT_TRIGGERS: [char; 2] = ['(', ','];

/// How many lines above the caret [`call_open`] looks for the call's `(`: a
/// call whose arguments are formatted one per line still finds it.
const CALL_SCAN_LINES: usize = 40;

/// The signature-help state of the app: the request in flight, the open
/// popup, and what each document's server said about its characters.
#[derive(Debug, Default)]
pub(crate) struct SignatureState {
    /// The request in flight, if any.
    pub(crate) pending: Option<PendingSignature>,
    /// The open popup, if any.
    pub(crate) ui: Option<SignatureUi>,
    /// The trigger and retrigger characters each document's server answered
    /// with; dropped when the document closes.
    pub(crate) chars: HashMap<DocumentId, SignatureHelpOptions>,
}

/// A signature-help request awaiting its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingSignature {
    /// The request, matched against the answer's id.
    pub(crate) id: RequestId,
    /// The document asked about.
    pub(crate) doc: DocumentId,
    /// The caret it was asked at.
    pub(crate) at: LineCol,
}

/// The open signature popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SignatureUi {
    /// The document it belongs to.
    pub(crate) doc: DocumentId,
    /// The server's answer.
    pub(crate) help: SignatureHelp,
    /// The caret the answer was asked at; a caret anywhere else asks again.
    pub(crate) at: LineCol,
    /// The overload shown, when the user picked one with Up/Down; `None`
    /// shows the one the server marked active.
    pub(crate) chosen: Option<usize>,
}

impl SignatureUi {
    /// The overload painted, as an index into the answer's signatures.
    pub(crate) fn shown(&self) -> usize {
        self.chosen.unwrap_or(self.help.active_signature)
    }
}

/// Where the call enclosing `caret` opens: the position just after the
/// nearest `(` before it that no `)` between them closes, looking back at
/// most [`CALL_SCAN_LINES`] lines. `None` when the caret is in no call.
///
/// Brackets inside strings and comments are counted like any other: this
/// only decides when the popup may close without asking, and the server's
/// answer still decides everything else.
pub(crate) fn call_open(buffer: &TextBuffer, caret: LineCol) -> Option<LineCol> {
    let mut depth = 0_usize;
    let first = (caret.line as usize).saturating_sub(CALL_SCAN_LINES);
    for line_index in (first..=caret.line as usize).rev() {
        let line: Vec<char> = buffer.line(line_index)?.chars().collect();
        let end = if line_index == caret.line as usize {
            (caret.col as usize).min(line.len())
        } else {
            line.len()
        };
        for col in (0..end).rev() {
            match line[col] {
                ')' => depth += 1,
                '(' if depth == 0 => {
                    let col = u32::try_from(col + 1).unwrap_or(u32::MAX);
                    return Some(LineCol::new(u32::try_from(line_index).ok()?, col));
                },
                '(' => depth -= 1,
                _ => {},
            }
        }
    }
    None
}

impl App {
    /// The active code tab's buffer and caret, with its document.
    fn signature_target(&self) -> Option<(DocumentId, LineCol, &TextBuffer)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        Some((*doc, tab.editor.cursor(), buffer))
    }

    /// Ask for signature help at the caret. `manual` (Ctrl+Shift+Space)
    /// also hears why when the server offers none.
    pub(crate) fn request_signature_help(&mut self, manual: bool) {
        let Some((doc, at, _)) = self.signature_target() else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let id = backend.next_id();
        let command = LspCommand::SignatureHelp {
            doc,
            position: at,
            manual,
        };
        if backend.send(id, command.into()).is_ok() {
            self.signature.pending = Some(PendingSignature { id, doc, at });
        }
    }

    /// After typing `c`: a trigger character asks for signature help. A
    /// typed retrigger character, and anything else typed while the popup
    /// is open, is served by [`Self::reconcile_signature`], which asks again
    /// whenever the caret moved.
    pub(super) fn maybe_signature_help(&mut self, c: char) {
        if self.signature.ui.is_some() {
            return;
        }
        let Some((doc, _, _)) = self.signature_target() else {
            return;
        };
        let triggers = self
            .signature
            .chars
            .get(&doc)
            .map(|options| options.trigger_characters.clone())
            .filter(|triggers| !triggers.is_empty())
            .unwrap_or_else(|| DEFAULT_TRIGGERS.to_vec());
        if triggers.contains(&c) {
            self.request_signature_help(false);
        }
    }

    /// Adopt (or drop as stale) an answering `Event::SignatureHelp`: open
    /// the popup or refresh it, or close it when the caret is in no call
    /// the server recognises.
    pub(super) fn on_signature_help(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        help: Option<SignatureHelp>,
        options: SignatureHelpOptions,
    ) {
        let Some(pending) = self.signature.pending else {
            return;
        };
        if id != Some(pending.id) {
            return; // an answer to a superseded request
        }
        self.signature.pending = None;
        if !options.trigger_characters.is_empty() || !options.retrigger_characters.is_empty() {
            self.signature.chars.insert(doc, options);
        }
        let in_call = self
            .signature_target()
            .is_some_and(|(d, caret, buffer)| d == doc && call_open(buffer, caret).is_some());
        let help = help.filter(|help| !help.signatures.is_empty());
        let (Some(help), true) = (help, in_call && pending.doc == doc) else {
            self.signature.ui = None;
            return;
        };
        // An overload the user picked survives a refresh that offers the
        // same overloads; the server's pick wins otherwise.
        let chosen = self
            .signature
            .ui
            .as_ref()
            .filter(|ui| ui.doc == doc && ui.help.signatures.len() == help.signatures.len())
            .and_then(|ui| ui.chosen);
        self.signature.ui = Some(SignatureUi {
            doc,
            help,
            at: pending.at,
            chosen,
        });
        // Typed on while it was asked: ask again for where the caret is now.
        self.reconcile_signature();
    }

    /// Close the popup and forget any request in flight.
    pub(crate) fn dismiss_signature_help(&mut self) {
        self.signature.ui = None;
        self.signature.pending = None;
    }

    /// Keep the popup with the caret: close it when the editor gave up the
    /// keyboard, the document changed, or the caret left every open call;
    /// otherwise ask again wherever the caret moved, so the highlighted
    /// parameter follows it.
    pub(crate) fn reconcile_signature(&mut self) {
        if self.signature.ui.is_none() && self.signature.pending.is_none() {
            return;
        }
        let target = self
            .signature_target()
            .map(|(doc, caret, buffer)| (doc, caret, call_open(buffer, caret)));
        let doc = self
            .signature
            .ui
            .as_ref()
            .map(|ui| ui.doc)
            .or(self.signature.pending.map(|pending| pending.doc));
        let Some((current, caret, Some(_))) = target else {
            self.dismiss_signature_help();
            return;
        };
        if !self.editor_owns_keyboard() || doc != Some(current) {
            self.dismiss_signature_help();
            return;
        }
        let asked = self.signature.pending.map(|pending| pending.at).or(self
            .signature
            .ui
            .as_ref()
            .map(|ui| ui.at));
        if self.signature.ui.is_some() && asked != Some(caret) {
            self.request_signature_help(false);
        }
    }

    /// Handle a key while the popup is open; returns whether it was consumed.
    /// Esc closes it, and with overloads to pick from Up/Down cycle them.
    /// Only while the completion popup is closed: its keys come first. Nor
    /// while the hover popup is open: the signature popup is hidden under it,
    /// so the arrows move the caret instead of cycling an unseen overload.
    pub(super) fn signature_key(&mut self, key: KeyEvent) -> bool {
        if self.completion.popup.is_some()
            || self.hover_ui.is_some()
            || !self.editor_owns_keyboard()
            || !key.modifiers.is_empty()
        {
            return false;
        }
        let Some(ui) = self.signature.ui.as_mut() else {
            return false;
        };
        let count = ui.help.signatures.len();
        match key.code {
            KeyCode::Esc => {
                self.dismiss_signature_help();
                true
            },
            KeyCode::Down if count > 1 => {
                ui.chosen = Some((ui.shown() + 1) % count);
                true
            },
            KeyCode::Up if count > 1 => {
                ui.chosen = Some((ui.shown() + count - 1) % count);
                true
            },
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_text(text)
    }

    /// Falsified by stopping at the first `(` to the left: in `f(g(x), y)`
    /// the caret after `y` is in `f`'s call, not `g`'s.
    #[test]
    fn the_enclosing_call_skips_calls_closed_before_the_caret() {
        let text = buffer("f(g(x), y)\n");
        assert_eq!(
            call_open(&text, LineCol::new(0, 9)),
            Some(LineCol::new(0, 2))
        );
        assert_eq!(
            call_open(&text, LineCol::new(0, 5)),
            Some(LineCol::new(0, 4))
        );
    }

    /// Before the `(`, or after the `)` that closes it, the caret is in no
    /// call.
    #[test]
    fn outside_every_call_there_is_none() {
        let text = buffer("f(x) + y\n");
        assert_eq!(call_open(&text, LineCol::new(0, 1)), None);
        assert_eq!(call_open(&text, LineCol::new(0, 4)), None);
        assert_eq!(call_open(&text, LineCol::new(0, 8)), None);
    }

    /// Arguments formatted one per line still find their call's `(`.
    #[test]
    fn a_call_spanning_lines_is_found() {
        let text = buffer("call(\n    a,\n    b\n");
        assert_eq!(
            call_open(&text, LineCol::new(2, 5)),
            Some(LineCol::new(0, 5))
        );
    }
}

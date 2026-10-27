//! Ghost text: what a language server (Copilot, say) proposes at the caret,
//! drawn dimmed after it -- its first line inline, the rest as rows below --
//! for Tab to accept whole, Ctrl+Right to accept a word at a time, Esc to
//! dismiss, and Alt+] / Alt+[ to cycle (or, with nothing shown, to ask).
//!
//! Asking is debounced on edits, not on caret moves: an edit to the active
//! document schedules an ask [`LSP_CHANGE_DEBOUNCE`] later, and any further
//! edit or caret move before then reschedules or cancels it, so typing costs
//! the server one ask per pause. Typing on cancels what is shown without
//! waiting for anything: every frame re-reads the shown item against the line
//! as it is now ([`InlineCompletionItem::ghost`]), which keeps it while what
//! was typed is a prefix of what it proposes and drops it the moment it is
//! not. Only the newest ask's answers are taken, so a slow answer to an ask
//! the user has typed past never replaces a newer one.

use std::time::Duration;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use karet_core::Change;
use karet_core::InlineAsk;
use karet_core::InlineCompletionItem;
use karet_core::InlineCompletionTrigger;
use karet_core::InlineGhost;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::ThemeRole;
use karet_core::VirtualLine;
use karet_core::VirtualText;
use karet_editor::editing;
use karet_session::DocumentId;
use karet_session::LSP_CHANGE_DEBOUNCE;
use karet_session::LanguageServerCommand;
use karet_session::LanguageServerId;
use karet_session::LspCommand;
use karet_session::RequestId;
use karet_session::api::AccountOutcome;
use karet_text::EditCause;

use crate::app::App;
use crate::app::notifications::Report;
use crate::keymap::Editing;
use crate::tab::TabKind;

/// Where an ask is aimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InlineKey {
    doc: DocumentId,
    version: u64,
    caret: LineCol,
    /// The caret line's length in characters, which the edits made after an
    /// ask are measured against.
    line_len: u32,
}

impl InlineKey {
    /// The ask made here, as [`InlineCompletionItem::ghost`] takes it.
    fn ask(self) -> InlineAsk {
        InlineAsk {
            line: self.caret.line,
            len: self.line_len,
        }
    }
}

/// The items one ask brought back, each with the server that proposed it.
#[derive(Debug)]
pub(crate) struct Shown {
    /// The ask they answer.
    request: RequestId,
    /// Where it was made.
    asked: InlineKey,
    items: Vec<(LanguageServerId, InlineCompletionItem)>,
    /// The one on show.
    index: usize,
}

/// The ghost-text state of the active editor.
#[derive(Debug, Default)]
pub(crate) struct InlineCompletions {
    /// The document version the active editor was last seen at: a newer
    /// version of the same document is an edit, which is what asks.
    seen: Option<(DocumentId, u64)>,
    /// The ask waiting for typing to pause, and when it is due.
    probe: Option<(InlineKey, Instant, InlineCompletionTrigger)>,
    /// The newest ask sent; only its answers are taken.
    sent: Option<(RequestId, InlineKey)>,
    /// What is on offer.
    shown: Option<Shown>,
    /// A version reached by accepting part of the ghost, which must not ask
    /// again: the answer would replace the rest the user is taking.
    quiet: Option<(DocumentId, u64)>,
}

impl App {
    /// Where an ask would go now, and the caret line's text: the focused
    /// editor's document, with one empty caret, typing into it (not a modal
    /// preset's Normal mode), no completion popup open, and
    /// `editor.inlineCompletion` on for its language.
    fn inline_target(&self) -> Option<(InlineKey, String)> {
        if !self.editor_owns_keyboard()
            || self.completion.popup.is_some()
            || !matches!(self.vi_editing(), Editing::Modeless | Editing::Insert)
        {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let enabled = self
            .settings
            .editor
            .for_language(tab.language_id())
            .inline_completion();
        let cursors = tab.editor.cursors();
        let [selection] = cursors.selections.as_slice() else {
            return None;
        };
        if !enabled || !selection.range().is_empty() {
            return None;
        }
        let caret = selection.head;
        let line = buffer.line(caret.line as usize).unwrap_or_default();
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        let key = InlineKey {
            doc: *doc,
            version: buffer.version(),
            caret,
            line_len: InlineAsk::new(caret.line, &line).len,
        };
        Some((key, line))
    }

    /// Keep the ask in step with the active editor, once per frame: schedule
    /// one after an edit, cancel it when the caret moves or the editor stops
    /// being typed into, and send it once typing has paused. Drops what is
    /// shown once it no longer applies.
    pub(crate) fn probe_inline_completion(&mut self, now: Instant) {
        let Some((key, _)) = self.inline_target() else {
            let state = &mut self.inline_completion;
            state.probe = None;
            state.shown = None;
            return;
        };
        if self.inline_ghost().is_none() {
            self.inline_completion.shown = None;
        }
        let state = &mut self.inline_completion;
        let edited =
            matches!(state.seen, Some((doc, version)) if doc == key.doc && version < key.version);
        state.seen = Some((key.doc, key.version));
        if edited && state.quiet != Some((key.doc, key.version)) {
            let due = now + LSP_CHANGE_DEBOUNCE;
            state.probe = Some((key, due, InlineCompletionTrigger::Automatic));
        }
        let Some((probe, due, trigger)) = state.probe else {
            return;
        };
        if probe != key {
            // The caret moved, or the document changed in a way that asked
            // nothing (another view's edit): nothing is asked for here.
            state.probe = None;
            return;
        }
        if now >= due {
            self.ask_inline_completion(key, trigger);
        }
    }

    /// Send an ask for `key`, replacing any older one.
    fn ask_inline_completion(&mut self, key: InlineKey, trigger: InlineCompletionTrigger) {
        self.inline_completion.probe = None;
        let command = LspCommand::InlineCompletion {
            doc: key.doc,
            position: key.caret,
            trigger,
        };
        self.inline_completion.sent = self.send(command).map(|id| (id, key));
    }

    /// How long until the pending ask is due, so the loop wakes to send it
    /// without another keystroke.
    pub(crate) fn inline_completion_next_wake(&self, now: Instant) -> Option<Duration> {
        let (_, due, _) = self.inline_completion.probe?;
        Some(due.saturating_duration_since(now))
    }

    /// Take one server's answer to the newest ask, keeping the items that
    /// still apply to the line as it is now. A second server's answer to the
    /// same ask joins the first; an answer to an older ask is dropped.
    pub(super) fn on_inline_completions(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        server: LanguageServerId,
        items: Vec<InlineCompletionItem>,
    ) {
        let Some((request, asked)) = self.inline_completion.sent else {
            return;
        };
        if id != Some(request) || doc != asked.doc {
            return;
        }
        let Some((key, line)) = self.inline_target() else {
            return;
        };
        if key.doc != doc || key.caret.line != asked.caret.line {
            return;
        }
        let fresh = items
            .into_iter()
            .filter(|item| item.ghost(&line, asked.ask(), key.caret).is_some())
            .map(|item| (server.clone(), item));
        let state = &mut self.inline_completion;
        match &mut state.shown {
            Some(shown) if shown.request == request => shown.items.extend(fresh),
            shown => {
                let items: Vec<_> = fresh.collect();
                if !items.is_empty() {
                    *shown = Some(Shown {
                        request,
                        asked,
                        items,
                        index: 0,
                    });
                }
            },
        }
    }

    /// The ghost on show, with the item it comes from and its server: `None`
    /// when nothing is shown or what is shown no longer applies.
    pub(crate) fn inline_ghost(
        &self,
    ) -> Option<(InlineGhost, &LanguageServerId, &InlineCompletionItem)> {
        let shown = self.inline_completion.shown.as_ref()?;
        let (key, line) = self.inline_target()?;
        if key.doc != shown.asked.doc || key.caret.line != shown.asked.caret.line {
            return None;
        }
        let (server, item) = shown.items.get(shown.index)?;
        Some((
            item.ghost(&line, shown.asked.ask(), key.caret)?,
            server,
            item,
        ))
    }

    /// The ghost as the editor draws it: its first line after the caret and
    /// its further lines as rows below.
    pub(crate) fn inline_ghost_render(&self) -> Option<(Option<VirtualText>, Vec<VirtualLine>)> {
        let (ghost, ..) = self.inline_ghost()?;
        let (_, line) = self.inline_target()?;
        let tab_width = self
            .tabs
            .get(self.active)
            .map_or(4, |tab| self.tab_width_for(tab));
        let tab_width = u8::try_from(tab_width).unwrap_or(u8::MAX);
        let role = ThemeRole::Muted;
        Some((
            ghost.virtual_text(&line, role, tab_width),
            ghost.virtual_lines(role, tab_width),
        ))
    }

    /// Tab accepts the ghost, Ctrl+Right its next word, Esc dismisses it;
    /// returns whether the key was taken. Nothing is taken while no ghost is
    /// on show, so each key keeps its ordinary meaning then.
    pub(super) fn inline_completion_key(&mut self, key: KeyEvent) -> bool {
        if self.inline_ghost().is_none() {
            return false;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Tab, KeyModifiers::NONE) => self.accept_inline_completion(false),
            (KeyCode::Right, KeyModifiers::CONTROL) => self.accept_inline_completion(true),
            (KeyCode::Esc, KeyModifiers::NONE) => self.dismiss_inline_completion(),
            _ => return false,
        }
        true
    }

    /// Show the next (or previous) proposal; with none on show, ask for some
    /// at once -- an ask the user made, so a server may offer several.
    pub(crate) fn cycle_inline_completion(&mut self, forward: bool) {
        if self.inline_ghost().is_some()
            && let Some((key, line)) = self.inline_target()
            && let Some(shown) = self.inline_completion.shown.as_mut()
        {
            let len = shown.items.len();
            for step in 1..len {
                let index = if forward {
                    (shown.index + step) % len
                } else {
                    (shown.index + len - step) % len
                };
                let asked = shown.asked.ask();
                if shown.items[index]
                    .1
                    .ghost(&line, asked, key.caret)
                    .is_some()
                {
                    shown.index = index;
                    return;
                }
            }
            return;
        }
        if let Some((key, _)) = self.inline_target() {
            self.ask_inline_completion(key, InlineCompletionTrigger::Invoked);
        }
    }

    /// Drop what is shown, and stop waiting for answers to the ask behind it.
    pub(crate) fn dismiss_inline_completion(&mut self) {
        let state = &mut self.inline_completion;
        state.shown = None;
        state.probe = None;
        state.sent = None;
    }

    /// Insert the ghost -- all of it, or with `word` its next word -- as one
    /// edit at the caret. A whole accept tells the proposing server; a word
    /// keeps the rest on show and asks nothing, so it is not replaced while
    /// the user takes it.
    fn accept_inline_completion(&mut self, word: bool) {
        let Some((ghost, server, item)) = self.inline_ghost() else {
            return;
        };
        let (server, item) = (server.clone(), item.clone());
        let edit = if word {
            ghost.accept_word()
        } else {
            ghost.accept()
        };
        let whole = edit == ghost.accept();
        let Some(doc) = self.inline_target().map(|(key, _)| key.doc) else {
            return;
        };
        let at = ghost.at;
        let replaced = ghost.replace_end.saturating_sub(at.col);
        let landed = edit.clone();
        self.submit_edit_with_cause(EditCause::Replace, move |caret, _, _, base| {
            (caret == at).then(|| editing::Edit {
                change: Change::new(base, vec![landed.clone()]),
                caret: editing::caret_after_insert(landed.range.start, &landed.new_text),
            })
        });
        // Modal editing's `.` and macros replay the text, not the key.
        let ahead = if whole { replaced } else { 0 };
        self.vi_note_text(0, ahead, &edit.new_text);
        if whole {
            self.inline_completion.shown = None;
            self.send_command(LspCommand::AcceptInlineCompletion { doc, server, item });
        } else if let Some((key, _)) = self.inline_target() {
            self.inline_completion.quiet = Some((key.doc, key.version));
        }
    }

    /// Sign in to (`sign_in`) or out of GitHub through Copilot's server.
    pub(crate) fn copilot_account(&mut self, sign_in: bool) {
        let server = LanguageServerId::new(COPILOT);
        self.send_command(if sign_in {
            LanguageServerCommand::SignIn { server }
        } else {
            LanguageServerCommand::SignOut { server }
        });
    }

    /// Tell the user what came of a sign-in or sign-out. A device code goes
    /// on the clipboard and stays on screen until dismissed, with the page to
    /// enter it at: the user opens that page and types it in, which takes a
    /// while. Whether the sign-in then landed arrives as a second outcome,
    /// except for a sign-in a later sign-in or sign-out superseded, or whose
    /// server stopped first, which gets none.
    pub(super) fn on_language_server_account(
        &mut self,
        server: &LanguageServerId,
        outcome: AccountOutcome,
    ) {
        let name = if server.key() == COPILOT {
            "Copilot"
        } else {
            server.key()
        }
        .to_owned();
        let (tier, message) = match outcome {
            AccountOutcome::DeviceCode {
                user_code,
                verification_uri,
            } => {
                let copied = self.clipboard.set(&user_code).is_ok();
                let at = verification_uri.map_or_else(String::new, |uri| format!(" at {uri}"));
                let copied = if copied { " (copied)" } else { "" };
                let message = format!(
                    "{name}: enter code {user_code}{copied}{at} in the browser to finish signing in"
                );
                (Report::Alert, message)
            },
            AccountOutcome::SignedIn { user } => {
                let who = user.map_or_else(String::new, |user| format!(" as {user}"));
                (Report::Outcome, format!("{name}: signed in{who}"))
            },
            AccountOutcome::SignedOut => (Report::Outcome, format!("{name}: signed out")),
            AccountOutcome::Failed { message } => (
                Report::Failure,
                format!("{name}: sign-in failed: {message}"),
            ),
            _ => return,
        };
        self.notify(tier, NotificationKind::Lsp, message);
    }
}

/// Copilot's language server, which proposes the ghost text and owns the
/// GitHub sign-in.
const COPILOT: &str = "copilot-language-server";

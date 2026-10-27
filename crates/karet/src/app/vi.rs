//! Modal editing in the app: routes keys to the `editor.keymap` preset's
//! layers and grammar ([`crate::keymap::vi`]) and applies what the grammar
//! returns to the active code tab.
//!
//! In Normal mode a key goes, in order, to the mode's layer (its commands and
//! the user's rebindings, sequences included), then to the grammar, then —
//! for a modified chord only — to the editor's and global layers under it. A
//! plain key nothing takes is dropped: Normal mode never types. Visual mode
//! asks the grammar first, since its `u`, `U` and `~` are case operators.

use std::sync::Arc;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Selection;
use karet_editor::editing;
use karet_session::config::schema::KeymapPreset;

use crate::app::App;
use crate::app::notifications::Report;
use crate::command::Command;
use crate::keymap;
use crate::keymap::Context;
use crate::keymap::Editing;
use crate::keymap::Focus;
use crate::keymap::KeyChord;
use crate::keymap::Resolved;
use crate::keymap::vi::Clipboard as ViClipboard;
use crate::keymap::vi::Effect;
use crate::keymap::vi::Engine;
use crate::keymap::vi::Mode;
use crate::keymap::vi::Outcome;
use crate::keymap::vi::Scroll;
use crate::keymap::vi::Snapshot;
use crate::keymap::vi::Typed;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::view::View;

/// How deeply macros and `.` may replay into each other before the replay is
/// abandoned (a macro that runs itself).
const MAX_REPLAY_DEPTH: u8 = 16;

/// The app's modal editing state.
#[derive(Debug, Default)]
pub(crate) struct ViState {
    /// The grammar: mode, registers, macros, the last change.
    pub(crate) engine: Engine,
    /// The keys behind [`App::pending`] while it holds a Normal-layer
    /// sequence, replayed into the grammar when the sequence breaks.
    keys: Vec<KeyEvent>,
    /// Whether [`App::pending`] holds a sequence for the layers under the
    /// Normal layer (`Ctrl+K …`).
    below: bool,
    /// How many macro or `.` replays are running inside each other.
    depth: u8,
    /// The find-bar query last handed to the grammar for `n`/`N`.
    find_query: String,
    /// Set while a commit character accepts a completion: the character is
    /// typed after the completion's text, though it was noted first.
    pub(super) committing: bool,
}

/// The system clipboard as the `+`/`*` registers see it.
struct Registers<'a>(&'a mut crate::clipboard::Clipboard);

impl ViClipboard for Registers<'_> {
    fn get(&mut self) -> Option<String> {
        self.0.get().ok()
    }

    fn set(&mut self, text: &str) {
        let _ = self.0.set(text);
    }
}

impl App {
    /// The `editor.keymap` preset in effect.
    pub(crate) fn keymap_preset(&self) -> KeymapPreset {
        self.settings.editor.keymap
    }

    /// The modal editing state of the focused editor: [`Editing::Modeless`]
    /// unless a modal preset is on and an editable code tab has the keys.
    /// Read by [`App::input_context`], so it must not consult it.
    pub(crate) fn vi_editing(&self) -> Editing {
        let preset = self.keymap_preset();
        if !preset.is_modal()
            || self.focus != Focus::Editor
            || self.view != View::Editor
            || self.active_code_doc().is_none()
        {
            return Editing::Modeless;
        }
        match (self.vi.engine.mode(), preset) {
            (Mode::Insert, _) => Editing::Insert,
            (_, KeymapPreset::Helix) => Editing::HelixNormal,
            _ => Editing::VimNormal,
        }
    }

    /// The mode the status bar names, when modal editing has the editor.
    pub(crate) fn vi_status(&self) -> Option<(Mode, Option<char>, String)> {
        (self.vi_editing() != Editing::Modeless).then(|| {
            let engine = &self.vi.engine;
            (engine.mode(), engine.recording(), engine.showcmd())
        })
    }

    /// Note a key the user typed, for the macro being recorded. Replayed
    /// keys are not recorded: the macro holds the `@a` that replayed them.
    pub(super) fn vi_record(&mut self, key: KeyEvent) {
        if self.vi.depth == 0 {
            self.vi.engine.record(key);
        }
    }

    /// Route `key` through modal editing. Returns whether it was taken; a key
    /// left untaken goes through the default path (Insert-mode typing, or a
    /// Normal-mode `Esc` nothing else wanted).
    pub(super) fn vi_key(&mut self, key: KeyEvent) -> bool {
        let editing = self.vi_editing();
        let ctx = Context::focus(self.focus_target()).with_editing(editing);
        match editing {
            Editing::Modeless => false,
            Editing::Insert => {
                self.vi.engine.observe_insert(key);
                // `Esc` leaves Insert mode even with a popup open: the popup
                // closes on the way out, as vim's does.
                let leaves = self.pending.is_empty()
                    && keymap::resolve(ctx, &[KeyChord::from_event(key)])
                        == Resolved::Command(Command::EnterNormalMode);
                if leaves {
                    self.dispatch(Command::EnterNormalMode);
                }
                leaves
            },
            Editing::VimNormal | Editing::HelixNormal => self.vi_normal_key(key, ctx),
        }
    }

    fn vi_normal_key(&mut self, key: KeyEvent, ctx: Context) -> bool {
        let chord = KeyChord::from_event(key);
        // Something else (a click, a focus change) dropped the sequence being
        // typed: forget the keys and the stage it was for.
        if self.pending.is_empty() {
            self.vi.keys.clear();
            self.vi.below = false;
        }
        if self.vi.below {
            self.pending.push(chord);
            match keymap::resolve_below_normal(ctx, &self.pending) {
                Resolved::Command(command) => {
                    self.pending.clear();
                    self.vi.below = false;
                    self.vi_run(command, 1);
                },
                Resolved::Pending => {},
                Resolved::None => {
                    self.pending.clear();
                    self.vi.below = false;
                },
            }
            return true;
        }
        let visual = matches!(self.vi.engine.mode(), Mode::Visual | Mode::VisualLine);
        if visual || self.vi.engine.busy() {
            return match self.vi_grammar(key) {
                Outcome::Unhandled if visual => match keymap::resolve_normal(ctx, &[chord]) {
                    Resolved::Command(command) => {
                        self.vi_run(command, 1);
                        true
                    },
                    _ => self.vi_below(key, ctx),
                },
                _ => true,
            };
        }
        self.pending.push(chord);
        self.vi.keys.push(key);
        match keymap::resolve_normal(ctx, &self.pending) {
            Resolved::Command(command) => {
                self.pending.clear();
                self.vi.keys.clear();
                let count = self.vi.engine.take_count();
                self.vi_run(command, count);
                true
            },
            Resolved::Pending => true,
            Resolved::None => {
                self.pending.clear();
                let keys = std::mem::take(&mut self.vi.keys);
                if let [only] = keys.as_slice() {
                    return match self.vi_grammar(*only) {
                        Outcome::Unhandled => self.vi_below(*only, ctx),
                        _ => true,
                    };
                }
                // A broken Normal-layer sequence (`g` then a key no `g …`
                // binding names) is the grammar's, key by key.
                for key in keys {
                    let _ = self.vi_grammar(key);
                }
                true
            },
        }
    }

    /// A key neither the Normal layer nor the grammar took: a modified chord
    /// may still be the editor's or a global binding; a plain key is dropped,
    /// except `Esc`, which the default path still handles.
    fn vi_below(&mut self, key: KeyEvent, ctx: Context) -> bool {
        let chord = KeyChord::from_event(key);
        if chord.is_plain() {
            return !(key.code == KeyCode::Esc && key.modifiers.is_empty());
        }
        self.pending.clear();
        self.pending.push(chord);
        match keymap::resolve_below_normal(ctx, &self.pending) {
            Resolved::Command(command) => {
                self.pending.clear();
                self.vi_run(command, 1);
            },
            Resolved::Pending => self.vi.below = true,
            Resolved::None => self.pending.clear(),
        }
        true
    }

    /// Run a layer command `count` times.
    fn vi_run(&mut self, command: Command, count: u32) {
        let command = self.keyboard_command(command);
        for _ in 0..count.clamp(1, 1000) {
            self.dispatch(command);
        }
    }

    /// The grammar's view of the active code tab, handed to `f`.
    fn vi_with_snapshot<R>(
        &mut self,
        f: impl FnOnce(&mut Engine, &Snapshot, &mut crate::clipboard::Clipboard) -> R,
    ) -> Option<R> {
        let indent = self.active_indentation();
        let tab_width = self
            .tabs
            .get(self.active)
            .map_or(4, |tab| u32::from(self.tab_width_for(tab)));
        let (buffer, selection, selections, top, height) = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { buffer, .. },
                editor,
                ..
            }) => {
                let cursors = editor.cursors();
                let primary = cursors.selections.get(cursors.primary).copied()?;
                let viewport = editor.viewport();
                (
                    Arc::clone(buffer),
                    primary,
                    cursors.selections.len(),
                    viewport.start.line,
                    viewport.end.line.saturating_sub(viewport.start.line).max(1),
                )
            },
            _ => return None,
        };
        let snap = Snapshot {
            buffer: &buffer,
            selection,
            selections,
            indent: &indent,
            tab_width,
            top,
            height,
        };
        Some(f(&mut self.vi.engine, &snap, &mut self.clipboard))
    }

    /// Feed one key to the grammar and apply what it returns.
    fn vi_grammar(&mut self, key: KeyEvent) -> Outcome {
        if let Some(query) = self.active_find().map(|find| find.query.clone())
            && !query.is_empty()
            && query != self.vi.find_query
        {
            self.vi.engine.search_for(&query);
            self.vi.find_query = query;
        }
        let preset = self.keymap_preset();
        let Some((outcome, effects)) = self.vi_with_snapshot(|engine, snap, clipboard| {
            engine.key(preset, key, snap, &mut Registers(clipboard))
        }) else {
            return Outcome::Unhandled;
        };
        self.vi_apply(effects);
        outcome
    }

    /// Leave Insert mode (`Esc`): close the popups typing opened and put the
    /// caret where Normal mode keeps it.
    pub(super) fn vi_enter_normal(&mut self) {
        let preset = self.keymap_preset();
        if !preset.is_modal() {
            return;
        }
        self.dismiss_completion();
        self.dismiss_signature_help();
        self.completion.snippet = None;
        let _ = self.dismiss_hover();
        if let Some(effects) =
            self.vi_with_snapshot(|engine, snap, _| engine.enter_normal(preset, snap))
        {
            self.vi_apply(effects);
        }
    }

    /// Apply the grammar's effects to the active code tab, in order.
    fn vi_apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Edit {
                    edits,
                    cause,
                    anchor,
                    head,
                } => {
                    let Some(base) = (match self.tabs.get(self.active) {
                        Some(Tab {
                            kind: TabKind::Code { next_version, .. },
                            ..
                        }) => Some(*next_version),
                        _ => None,
                    }) else {
                        continue;
                    };
                    self.submit_multi_edit(
                        cause,
                        editing::MultiEdit {
                            change: karet_core::Change::new(base, edits),
                            cursors: CursorState::single(Selection { anchor, head }),
                        },
                    );
                },
                Effect::Select { anchor, head } => {
                    if let Some(Tab {
                        kind: TabKind::Code { buffer, .. },
                        editor,
                        ..
                    }) = self.tabs.get_mut(self.active)
                    {
                        editor.set_cursor_state(
                            buffer,
                            CursorState::single(Selection { anchor, head }),
                        );
                        editor.reveal(buffer, head);
                    }
                },
                Effect::Scroll(scroll) => self.vi_scroll(scroll),
                Effect::Command(command) => self.dispatch(command),
                Effect::Replay { keys, dot } => self.vi_replay(keys, dot),
            }
        }
    }

    fn vi_scroll(&mut self, scroll: Scroll) {
        if let Scroll::Lines(delta) = scroll {
            self.scroll_lines(delta);
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let line = tab.editor.cursor().line;
        let height = tab
            .editor
            .viewport()
            .end
            .line
            .saturating_sub(tab.editor.scroll_line());
        match scroll {
            Scroll::Center => tab.editor.center_on(line),
            Scroll::Top => tab.editor.scroll_to_line(line),
            Scroll::Bottom => tab
                .editor
                .scroll_to_line(line.saturating_sub(height.saturating_sub(1))),
            Scroll::Lines(_) => {},
        }
    }

    /// Replay keys through the whole input path (a macro, or `.`).
    fn vi_replay(&mut self, keys: Vec<Typed>, dot: bool) {
        if self.vi.depth >= MAX_REPLAY_DEPTH {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "macro: stopped a replay that runs itself",
            );
            return;
        }
        self.vi.depth += 1;
        if dot {
            self.vi.engine.set_replaying_dot(true);
        }
        for step in keys {
            match step {
                Typed::Key(key) => self.handle_key(key),
                Typed::Text { back, ahead, text } => self.vi_type(back, ahead, &text),
            }
        }
        if dot {
            self.vi.engine.set_replaying_dot(false);
        }
        self.vi.depth -= 1;
    }

    /// Replay text a completion inserted: replace `back` characters before
    /// each caret and `ahead` after it with `text`. Noted as part of the
    /// change it belongs to, as the completion was.
    pub(super) fn vi_type(&mut self, back: u32, ahead: u32, text: &str) {
        let Some(multi) = self.compose_edit(|caret, _, buffer, base| {
            let len = buffer
                .line(caret.line as usize)
                .map_or(0, |line| line.chars().count());
            let len = u32::try_from(len).unwrap_or(u32::MAX);
            let range = karet_core::Range {
                start: LineCol::new(caret.line, caret.col.saturating_sub(back)),
                end: LineCol::new(caret.line, caret.col.saturating_add(ahead).min(len)),
            };
            Some(editing::Edit {
                change: karet_core::Change::new(
                    base,
                    vec![karet_core::TextEdit {
                        range,
                        new_text: text.to_string(),
                    }],
                ),
                caret: crate::completion::caret_after_insert(range.start, text),
            })
        }) else {
            return;
        };
        self.submit_multi_edit(karet_core::EditCause::Replace, multi);
        self.vi_note_text(back, ahead, text);
    }

    /// Note text a completion inserted, for `.` and the macro being recorded.
    pub(super) fn vi_note_text(&mut self, back: u32, ahead: u32, text: &str) {
        let typed = Typed::Text {
            back,
            ahead,
            text: text.to_string(),
        };
        let recorded = self.vi.depth == 0;
        self.vi
            .engine
            .observe_text(typed, self.vi.committing, recorded);
    }

    /// A popup (completion, signature help, a snippet) took `key`: it is not
    /// part of what `.` or the macro being recorded replays.
    pub(super) fn vi_popup_took(&mut self, key: KeyEvent) {
        let recorded = self.vi.depth == 0;
        self.vi.engine.forget(key, recorded);
    }
}

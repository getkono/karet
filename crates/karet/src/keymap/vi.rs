//! Modal editing — the `editor.keymap` `"vim"` and `"helix"` presets.
//!
//! A modal preset adds two layers to the code editor's [stack](super::active_layers):
//! a Normal layer ([`Layer::VimNormal`](super::Layer) or `HelixNormal`), which
//! holds the preset's plain-key commands (undo, search, go-to-definition, …)
//! and is rebindable like any other layer, and the shared Insert layer, whose
//! `Esc` returns to Normal. Everything those layers cannot express as a fixed
//! binding — counts, operators over motions and text objects, registers,
//! character arguments (`f`, `r`, `q`), Visual and Select modes, macros and
//! `.` repeat — is the grammar in this module: a pure state machine over the
//! document ([`Engine::key`]) that returns [`Effect`]s for the app to apply.
//!
//! The engine keeps the selection inclusive, as vim's Visual mode and helix
//! select characters, while the editor's selection is exclusive with a block
//! caret on its head. It therefore writes the selection *for display* (the
//! caret cell completes the inclusive range) and remembers what it wrote: a
//! selection it finds changed was made by something else — a click, a drag,
//! a search — and is adopted as exclusive.

mod helix;
mod motion;
mod object;
mod ops;
mod register;
mod text;
mod vim;
mod vim_motion;
mod visual;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use karet_core::EditCause;
use karet_core::LineCol;
use karet_core::Selection;
use karet_core::TextEdit;
use karet_session::config::schema::KeymapPreset;
use karet_text::TextBuffer;
use motion::CharSearch;
pub(crate) use register::Clipboard;
use register::Register;
use register::Registers;
use register::Write;
use text::Doc;

use crate::command::Command;

/// The editing mode of a modal preset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Keys are commands.
    #[default]
    Normal,
    /// Keys type, as in the default keymap.
    Insert,
    /// Vim's characterwise Visual mode.
    Visual,
    /// Vim's linewise Visual mode.
    VisualLine,
    /// Helix's Select (extend) mode: motions grow the selection.
    Select,
}

impl Mode {
    /// The mode's name as the status bar shows it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::Select => "SELECT",
        }
    }
}

/// What the engine reads of the focused code tab for one key.
pub(crate) struct Snapshot<'a> {
    /// The document.
    pub(crate) buffer: &'a TextBuffer,
    /// The editor's primary selection.
    pub(crate) selection: Selection,
    /// How many selections the editor holds.
    pub(crate) selections: usize,
    /// One level of indentation (`>`/`<`).
    pub(crate) indent: &'a str,
    /// How many columns a tab spans.
    pub(crate) tab_width: u32,
    /// The first visible line.
    pub(crate) top: u32,
    /// How many lines the viewport shows.
    pub(crate) height: u32,
}

impl Snapshot<'_> {
    /// How many columns one level of indentation spans.
    fn indent_width(&self) -> u32 {
        self.indent
            .chars()
            .map(|c| if c == '\t' { self.tab_width.max(1) } else { 1 })
            .sum()
    }
}

/// How to scroll the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scroll {
    /// Put the cursor's line in the middle (`zz`).
    Center,
    /// Put the cursor's line at the top (`zt`).
    Top,
    /// Put the cursor's line at the bottom (`zb`).
    Bottom,
    /// Scroll by this many lines (`Ctrl+E`, `Ctrl+D`, …).
    Lines(i32),
}

/// Something the app does for the engine, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Apply these non-overlapping edits (against the document the key saw)
    /// as one change, then put the selection at `anchor`/`head`.
    Edit {
        /// The edits.
        edits: Vec<TextEdit>,
        /// Why, for undo grouping.
        cause: EditCause,
        /// The selection's anchor once applied.
        anchor: LineCol,
        /// The selection's head (the caret) once applied.
        head: LineCol,
    },
    /// Put a single selection here.
    Select {
        /// The fixed end.
        anchor: LineCol,
        /// The moving end, where the caret is drawn.
        head: LineCol,
    },
    /// Scroll the viewport.
    Scroll(Scroll),
    /// Run an app command (after the effects before it).
    Command(Command),
    /// Feed these keys back through the input path (a macro, or `.`).
    Replay {
        /// The keys, and the text completions inserted between them.
        keys: Vec<Typed>,
        /// A `.` repeat, which must not record itself as the last change.
        dot: bool,
    },
}

/// One step of a recorded macro or change: a key, or text a completion put
/// in. A completion arrives asynchronously and is accepted by a key whose
/// meaning depends on the popup being open (Enter, Tab), so the key alone
/// cannot replay it; the text it inserted can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Typed {
    /// A key, fed back through the input path.
    Key(KeyEvent),
    /// Replace the `back` characters before the caret and the `ahead` after
    /// it (on its line) with `text`, leaving the caret after it.
    Text {
        /// Characters before the caret the text replaces.
        back: u32,
        /// Characters after the caret the text replaces.
        ahead: u32,
        /// The text.
        text: String,
    },
}

/// How a key went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The key did something (or deliberately nothing).
    Handled,
    /// The key started or continued a command that needs more keys.
    Pending,
    /// The grammar has no meaning for the key; the caller may resolve it.
    Unhandled,
}

/// The engine's selection: inclusive at both ends, `cursor` being the end
/// that moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sel {
    anchor: LineCol,
    cursor: LineCol,
}

impl Sel {
    fn at(p: LineCol) -> Self {
        Self {
            anchor: p,
            cursor: p,
        }
    }

    fn start(self) -> LineCol {
        self.anchor.min(self.cursor)
    }

    fn end(self) -> LineCol {
        self.anchor.max(self.cursor)
    }
}

/// A key reduced to what the grammars match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    Esc,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Other,
}

impl Key {
    fn of(event: KeyEvent) -> Self {
        let mods = event.modifiers;
        if mods.contains(KeyModifiers::SUPER) {
            return Key::Other;
        }
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        let shift = mods.contains(KeyModifiers::SHIFT);
        match event.code {
            // `Ctrl+Shift+…` is the default keymap's (palette, search, …),
            // never the grammar's.
            KeyCode::Char(_) if ctrl && shift => Key::Other,
            KeyCode::Char(c) if ctrl && !alt => Key::Ctrl(c.to_ascii_lowercase()),
            KeyCode::Char(c) if alt && !ctrl => Key::Alt(c),
            KeyCode::Char(c) if !ctrl && !alt => Key::Char(c),
            _ if ctrl || alt => Key::Other,
            KeyCode::Esc => Key::Esc,
            KeyCode::Enter => Key::Enter,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Tab => Key::Tab,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            _ => Key::Other,
        }
    }
}

/// How a grammar step ended, which decides what `.` repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Done {
    /// Moved, yanked, scrolled — nothing `.` repeats.
    Motion,
    /// Changed the text; the command's keys become the last change.
    Change,
    /// Changed the text and entered Insert mode; the typing that follows is
    /// part of the change, up to the return to Normal.
    Insert,
    /// Needs more keys.
    Pending,
    /// Meant nothing.
    Unhandled,
}

/// An operator awaiting its motion or text object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Case(ops::Case),
}

/// What the next key completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    /// `"`: a register name.
    Register,
    /// `g`, `z`, `m` (helix): the second key of a two-key command.
    Prefix(char),
    /// `f`, `t`, `F`, `T`: the character to find.
    Find { forward: bool, till: bool },
    /// `r`: the replacement character.
    Replace,
    /// `i`/`a` after an operator or in Visual mode (helix: `mi`/`ma`).
    Object { inner: bool },
    /// `q` (vim): the register to record into.
    Record,
    /// `@` (vim): the register to replay.
    Play,
}

/// The command being typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pending {
    register: Option<char>,
    count: Option<u32>,
    op: Option<Op>,
    op_count: Option<u32>,
    wait: Option<Wait>,
}

impl Pending {
    /// The count the command runs with: the counts before and after the
    /// operator multiply.
    fn total(&self) -> Option<u32> {
        match (self.count, self.op_count) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(1).saturating_mul(b.unwrap_or(1)).min(MAX_COUNT)),
        }
    }
}

/// The largest count honoured, so `99999999dd` cannot stall a keystroke.
const MAX_COUNT: u32 = 100_000;

/// The last pattern `n`/`N` look for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Search {
    text: String,
    forward: bool,
    word: bool,
}

/// The modal editing state: mode, the command being typed, registers,
/// macros and the last change. One per app, like vim's.
#[derive(Debug, Default)]
pub(crate) struct Engine {
    mode: Mode,
    sel: Sel,
    displayed: Option<Selection>,
    /// The editor selection an edit this key made leaves, in post-edit
    /// coordinates, where the edit worked it out itself.
    after_edit: Option<Selection>,
    goal: Option<u32>,
    pending: Pending,
    registers: Registers,
    last_find: Option<CharSearch>,
    search: Option<Search>,
    macros: HashMap<char, Vec<Typed>>,
    recording: Option<(char, Vec<Typed>)>,
    last_macro: Option<char>,
    /// The keys of the last change, for `.`.
    dot: Option<Vec<Typed>>,
    /// The keys of the command being typed.
    keys: Vec<KeyEvent>,
    /// The keys of a change still in Insert mode.
    change: Option<Vec<Typed>>,
    /// Set while a `.` replays, so the replay does not become the last change.
    replaying_dot: bool,
}

impl Engine {
    /// The current mode.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The register a macro is being recorded into.
    pub(crate) fn recording(&self) -> Option<char> {
        self.recording.as_ref().map(|(name, _)| *name)
    }

    /// Whether the grammar is mid-command beyond a bare count, so the next
    /// key is the grammar's whatever the layers bind.
    pub(crate) fn busy(&self) -> bool {
        self.pending.op.is_some() || self.pending.wait.is_some() || self.pending.register.is_some()
    }

    /// Take the bare count typed so far (a layer command runs that many
    /// times), clearing the command.
    pub(crate) fn take_count(&mut self) -> u32 {
        let count = self.pending.total().unwrap_or(1);
        self.pending = Pending::default();
        self.keys.clear();
        count
    }

    /// What has been typed of the current command, as vim's `showcmd` shows.
    pub(crate) fn showcmd(&self) -> String {
        let mut out = String::new();
        for key in &self.keys {
            match key.code {
                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    out.push('^');
                    out.push(c.to_ascii_uppercase());
                },
                KeyCode::Char(c) => out.push(c),
                _ => {},
            }
        }
        out
    }

    /// Append `key` to the macro being recorded. The caller records every
    /// key the user types, before handling it.
    pub(crate) fn record(&mut self, key: KeyEvent) {
        if let Some((_, keys)) = self.recording.as_mut() {
            keys.push(Typed::Key(key));
        }
    }

    /// Note a key typed in Insert mode, which belongs to the change that
    /// entered it.
    pub(crate) fn observe_insert(&mut self, key: KeyEvent) {
        if let Some(change) = self.change.as_mut() {
            change.push(Typed::Key(key));
        }
    }

    /// Drop the last `key` from the change in Insert mode (and, with
    /// `recorded`, from the macro being recorded): a popup took it (Enter
    /// accepting a completion, Up choosing one), so replayed without the
    /// popup it would mean something else.
    pub(crate) fn forget(&mut self, key: KeyEvent, recorded: bool) {
        let drop_last = |steps: &mut Vec<Typed>| {
            if let Some(at) = steps.iter().rposition(|step| *step == Typed::Key(key)) {
                steps.remove(at);
            }
        };
        if let Some(change) = self.change.as_mut() {
            drop_last(change);
        }
        if recorded && let Some((_, keys)) = self.recording.as_mut() {
            drop_last(keys);
        }
    }

    /// Note text a completion inserted in Insert mode, for the change (and,
    /// with `recorded`, the macro being recorded). `before_last` puts it
    /// ahead of the key just noted: the commit character that accepted the
    /// completion is typed after its text.
    pub(crate) fn observe_text(&mut self, text: Typed, before_last: bool, recorded: bool) {
        let push = |steps: &mut Vec<Typed>| {
            let at = if before_last {
                steps.len().saturating_sub(1)
            } else {
                steps.len()
            };
            steps.insert(at, text.clone());
        };
        if let Some(change) = self.change.as_mut() {
            push(change);
        }
        if recorded && let Some((_, keys)) = self.recording.as_mut() {
            push(keys);
        }
    }

    /// Make `query` (typed in the find bar) the pattern `n` and `N` look for.
    pub(crate) fn search_for(&mut self, query: &str) {
        self.search = Some(Search {
            text: query.to_string(),
            forward: true,
            word: false,
        });
    }

    /// Mark whether a `.` replay is running.
    pub(crate) fn set_replaying_dot(&mut self, replaying: bool) {
        self.replaying_dot = replaying;
    }

    /// Leave Insert mode for Normal (`Esc`): vim steps the caret back onto
    /// the character it typed last; helix keeps it. Finishes the change the
    /// insert belonged to, for `.`.
    pub(crate) fn enter_normal(&mut self, preset: KeymapPreset, snap: &Snapshot) -> Vec<Effect> {
        let doc = Doc::new(snap.buffer, snap.selection.head.line);
        let was_insert = self.mode == Mode::Insert;
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        self.keys.clear();
        if let Some(change) = self.change.take()
            && !self.replaying_dot
        {
            self.dot = Some(change);
        }
        let head = doc.clamp(snap.selection.head);
        let cursor = if was_insert && preset == KeymapPreset::Vim && head.col > 0 {
            LineCol::new(head.line, head.col - 1)
        } else {
            head
        };
        self.sel = Sel::at(doc.clamp_char(cursor));
        self.goal = None;
        let display = self.display(&doc);
        self.displayed = Some(display);
        vec![Effect::Select {
            anchor: display.anchor,
            head: display.head,
        }]
    }

    /// Handle one key in a non-Insert mode for `preset`, reading `snap` and
    /// the clipboard behind the `+`/`*` registers.
    pub(crate) fn key(
        &mut self,
        preset: KeymapPreset,
        event: KeyEvent,
        snap: &Snapshot,
        clipboard: &mut dyn Clipboard,
    ) -> (Outcome, Vec<Effect>) {
        let doc = Doc::new(snap.buffer, snap.selection.head.line);
        let forced = self.sync(preset, &doc, snap);
        let mut fx = Vec::new();
        self.keys.push(event);
        let key = Key::of(event);
        let done = match preset {
            KeymapPreset::Helix => self.helix_key(key, &doc, snap, clipboard, &mut fx),
            _ => self.vim_key(key, &doc, snap, clipboard, &mut fx),
        };
        let outcome = match done {
            Done::Pending => Outcome::Pending,
            Done::Unhandled => {
                self.pending = Pending::default();
                self.keys.clear();
                Outcome::Unhandled
            },
            Done::Change => {
                let keys = std::mem::take(&mut self.keys);
                if !self.replaying_dot {
                    self.dot = Some(keys.into_iter().map(Typed::Key).collect());
                }
                Outcome::Handled
            },
            Done::Insert => {
                let keys = std::mem::take(&mut self.keys);
                self.change = Some(keys.into_iter().map(Typed::Key).collect());
                Outcome::Handled
            },
            Done::Motion => {
                self.keys.clear();
                Outcome::Handled
            },
        };
        if outcome != Outcome::Pending {
            self.pending = Pending::default();
        }
        self.finish(&doc, snap, forced, &mut fx);
        (outcome, fx)
    }

    /// Read the editor's selection back. Unchanged since the engine last
    /// wrote it, the engine's own (inclusive) selection stands; otherwise
    /// something else moved it, and it is adopted. Returns whether the
    /// selection must be written back even if unchanged (several carets).
    fn sync(&mut self, preset: KeymapPreset, doc: &Doc, snap: &Snapshot) -> bool {
        let current = Selection {
            anchor: doc.clamp(snap.selection.anchor),
            head: doc.clamp(snap.selection.head),
        };
        if self.displayed != Some(current) {
            let (anchor, head) = (current.anchor, current.head);
            self.sel = if anchor == head {
                Sel::at(head)
            } else if anchor < head {
                Sel {
                    anchor,
                    cursor: doc.prev(head).unwrap_or(head),
                }
            } else {
                Sel {
                    anchor: doc.prev(anchor).unwrap_or(anchor),
                    cursor: head,
                }
            };
            self.goal = None;
            if preset == KeymapPreset::Vim {
                match (self.mode, anchor == head) {
                    (Mode::Normal, false) => self.mode = Mode::Visual,
                    (Mode::Visual | Mode::VisualLine, true) => self.mode = Mode::Normal,
                    _ => {},
                }
            }
        }
        if self.mode == Mode::Normal && preset == KeymapPreset::Vim {
            self.sel = Sel::at(doc.clamp_char(self.sel.cursor));
        }
        snap.selections > 1
    }

    /// Write the selection back for display unless an edit already places it.
    fn finish(&mut self, doc: &Doc, snap: &Snapshot, forced: bool, fx: &mut Vec<Effect>) {
        let edited = fx
            .iter()
            .any(|effect| matches!(effect, Effect::Edit { .. }));
        let after_edit = self.after_edit.take();
        let display = if edited {
            // An edit moves the cursor sideways: the next `j` steers from
            // where it is, not from a column remembered before the edit.
            self.goal = None;
            after_edit.unwrap_or_else(|| self.display_after_edit())
        } else {
            self.display(doc)
        };
        self.displayed = Some(display);
        if let Some(Effect::Edit { anchor, head, .. }) = fx
            .iter_mut()
            .rev()
            .find(|effect| matches!(effect, Effect::Edit { .. }))
        {
            *anchor = display.anchor;
            *head = display.head;
            return;
        }
        let commands = fx.iter().any(|effect| matches!(effect, Effect::Command(_)));
        if forced || display != snap.selection || commands {
            let select = Effect::Select {
                anchor: display.anchor,
                head: display.head,
            };
            // A command (opening a line, say) and a replay (`@q`, `.`) act
            // from the selection, so the selection goes first.
            let at = fx
                .iter()
                .position(|effect| matches!(effect, Effect::Command(_) | Effect::Replay { .. }))
                .unwrap_or(fx.len());
            fx.insert(at, select);
        }
    }

    /// The editor selection that shows the engine's: the caret cell is the
    /// inclusive end, so a forward selection ends *at* the cursor and a
    /// backward one starts after the anchor. Linewise Visual covers the lines.
    fn display(&self, doc: &Doc) -> Selection {
        let Sel { anchor, cursor } = self.sel;
        match self.mode {
            Mode::Normal | Mode::Insert if anchor == cursor => Selection::caret(cursor),
            Mode::VisualLine => {
                let (first, last) = (anchor.line.min(cursor.line), anchor.line.max(cursor.line));
                if cursor.line >= anchor.line {
                    Selection {
                        anchor: LineCol::new(first, 0),
                        head: doc.line_end(last),
                    }
                } else {
                    Selection {
                        anchor: doc.line_end(last),
                        head: LineCol::new(first, 0),
                    }
                }
            },
            _ if cursor >= anchor => Selection {
                anchor,
                head: cursor,
            },
            _ => Selection {
                anchor: doc.after(anchor),
                head: cursor,
            },
        }
    }

    /// [`Self::display`] for a selection in post-edit coordinates, where the
    /// document the key saw no longer applies. An edit that leaves a backward
    /// selection sets [`Engine::after_edit`] instead, since showing one needs
    /// the position after its anchor in the edited document.
    fn display_after_edit(&self) -> Selection {
        let Sel { anchor, cursor } = self.sel;
        Selection {
            anchor,
            head: cursor,
        }
    }

    /// Store `reg` in the pending register (or the unnamed one).
    fn store(&mut self, reg: Register, kind: Write, clipboard: &mut dyn Clipboard) {
        self.registers
            .write(self.pending.register, reg, kind, clipboard);
    }

    /// The pending register's contents (or the unnamed one's).
    fn fetch(&self, clipboard: &mut dyn Clipboard) -> Option<Register> {
        self.registers.read(self.pending.register, clipboard)
    }

    /// Start recording a macro into `name`, or stop the one recording.
    fn toggle_recording(&mut self, name: Option<char>) {
        if let Some((name, mut keys)) = self.recording.take() {
            // The key that stopped the recording was recorded before it was
            // handled; it is not part of the macro.
            keys.pop();
            self.macros.insert(name, keys);
        } else if let Some(name) = name {
            // An uppercase register (`qA`) appends to the macro already in
            // its lowercase one.
            let lower = name.to_ascii_lowercase();
            let keys = if name.is_ascii_uppercase() {
                self.macros.get(&lower).cloned().unwrap_or_default()
            } else {
                Vec::new()
            };
            self.recording = Some((lower, keys));
        }
    }

    /// Replay the macro in `name` (`@@`/helix default: the last one) `count`
    /// times.
    fn play(&mut self, name: char, count: u32, fx: &mut Vec<Effect>) -> Done {
        let name = if name == '@' && self.last_macro.is_some() && !self.macros.contains_key(&'@') {
            self.last_macro.unwrap_or('@')
        } else {
            name.to_ascii_lowercase()
        };
        let Some(keys) = self.macros.get(&name) else {
            return Done::Motion;
        };
        self.last_macro = Some(name);
        let keys = repeated(keys, count);
        fx.push(Effect::Replay { keys, dot: false });
        Done::Motion
    }

    /// Repeat the last change `count` times (`.`).
    fn repeat_change(&mut self, count: u32, fx: &mut Vec<Effect>) -> Done {
        if let Some(keys) = &self.dot {
            fx.push(Effect::Replay {
                keys: repeated(keys, count),
                dot: true,
            });
        }
        Done::Motion
    }

    /// Push the edit `planned` makes (cursor in post-edit coordinates).
    fn apply(&mut self, planned: ops::Planned, cause: EditCause, fx: &mut Vec<Effect>) {
        self.sel = Sel::at(planned.cursor);
        if !planned.edits.is_empty() {
            fx.push(Effect::Edit {
                edits: planned.edits,
                cause,
                anchor: planned.cursor,
                head: planned.cursor,
            });
        }
    }
}

/// `steps` repeated `count` times (at least once, at most 1000).
fn repeated(steps: &[Typed], count: u32) -> Vec<Typed> {
    let times = count.clamp(1, 1000) as usize;
    steps
        .iter()
        .cycle()
        .take(steps.len().saturating_mul(times))
        .cloned()
        .collect()
}
